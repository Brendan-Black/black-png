//! Writing PNGs.

use black_zlib::checksum::crc32_update;

use crate::{ColorType, Compression, Error, Result, SIGNATURE, filter};

/// The largest width or height PNG allows.
const MAX_SIDE: u32 = (1 << 31) - 1;

/// How much compressed data goes in each IDAT chunk.
const IDAT_SIZE: usize = 1 << 20;

/// Encode `width * height` pixels, top row first, as an 8-bit PNG of `color` type. `pixels`
/// must hold exactly `width * height * color.channels()` bytes.
pub fn encode(width: u32, height: u32, color: ColorType, pixels: &[u8], level: Compression) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(Error::invalid(format!("a PNG can't be {width}x{height}")));
    }
    let bpp = color.channels();
    let stride = (width as usize).checked_mul(bpp).ok_or_else(|| Error::invalid("the image is too big"))?;
    let want = stride.checked_mul(height as usize).ok_or_else(|| Error::invalid("the image is too big"))?;
    if pixels.len() != want {
        return Err(Error::invalid(format!(
            "a {width}x{height} {color:?} image needs {want} bytes of pixels, not {}",
            pixels.len()
        )));
    }

    // Each row becomes its filter type and the filtered bytes.
    let mut filtered = Vec::with_capacity((stride + 1) * height as usize);
    let mut scratch = vec![0u8; stride];
    let zeros = vec![0u8; stride];
    let mut prev: &[u8] = &zeros;
    for row in pixels.chunks_exact(stride) {
        filter::best(row, prev, bpp, &mut filtered, &mut scratch);
        prev = row;
    }
    let compressed = black_zlib::zlib::compress(&filtered, level);

    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend(SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    // Bit depth 8, the color type, deflate, adaptive filtering, not interlaced.
    ihdr.extend([8, color.code(), 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    for part in compressed.chunks(IDAT_SIZE) {
        chunk(&mut out, b"IDAT", part);
    }
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// Append a chunk: its length, type, data, and the CRC-32 of the type and data.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(data);
    out.extend(crc32_update(crc32_update(0, kind), data).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use black_zlib::checksum::crc32;

    /// Split a PNG into its chunks, checking each CRC.
    fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(png[..8], SIGNATURE);
        let mut out = Vec::new();
        let mut rest = &png[8..];
        while !rest.is_empty() {
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = rest[4..8].try_into().unwrap();
            let data = rest[8..8 + len].to_vec();
            let crc = u32::from_be_bytes(rest[8 + len..12 + len].try_into().unwrap());
            assert_eq!(crc, crc32(&rest[4..8 + len]), "CRC of {}", String::from_utf8_lossy(&kind));
            out.push((kind, data));
            rest = &rest[12 + len..];
        }
        out
    }

    /// Undo the filters, independently of the encoder's own code paths.
    fn unfilter(data: &[u8], stride: usize, bpp: usize) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for (y, row) in data.chunks(stride + 1).enumerate() {
            let start = out.len();
            for i in 0..stride {
                let a = if i >= bpp { out[start + i - bpp] } else { 0 };
                let b = if y > 0 { out[start - stride + i] } else { 0 };
                let c = if y > 0 && i >= bpp { out[start - stride + i - bpp] } else { 0 };
                let p = match row[0] {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((a as u16 + b as u16) / 2) as u8,
                    4 => filter::paeth(a, b, c),
                    f => panic!("filter type {f}"),
                };
                out.push(row[1 + i].wrapping_add(p));
            }
        }
        out
    }

    fn picture(width: usize, height: usize, channels: usize) -> Vec<u8> {
        let mut seed = 99u32;
        let mut out = Vec::new();
        for y in 0..height {
            for x in 0..width {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                for ch in 0..channels {
                    out.push(match (y * 4 / height, ch) {
                        (0, _) => 200,                              // flat
                        (1, _) => (x * 255 / width) as u8,          // gradient
                        (2, _) => (seed >> (8 + ch * 3)) as u8,     // noise
                        _ => ((x / 3 + y / 3) % 2 * 255) as u8,     // checks
                    });
                }
            }
        }
        out
    }

    #[test]
    fn writes_a_valid_png() {
        for color in [ColorType::Gray, ColorType::GrayAlpha, ColorType::Rgb, ColorType::Rgba] {
            let (w, h) = (37, 24);
            let px = picture(w, h, color.channels());
            let png = encode(w as u32, h as u32, color, &px, Compression::Default).unwrap();
            let chunks = chunks(&png);
            let kinds: Vec<&[u8]> = chunks.iter().map(|c| &c.0[..]).collect();
            assert_eq!(kinds, [&b"IHDR"[..], b"IDAT", b"IEND"]);
            let ihdr = &chunks[0].1;
            assert_eq!(ihdr[..8], [0, 0, 0, 37, 0, 0, 0, 24]);
            assert_eq!(ihdr[8..], [8, color.code(), 0, 0, 0]);
            let stride = w * color.channels();
            let raw = black_zlib::zlib::decompress(&chunks[1].1, (stride + 1) * h).unwrap();
            assert_eq!(unfilter(&raw, stride, color.channels()), px, "{color:?}");
            // The varied picture should use more than one filter.
            let used: std::collections::HashSet<u8> = raw.chunks(stride + 1).map(|r| r[0]).collect();
            assert!(used.len() > 1, "{color:?} used only {used:?}");
        }
    }

    #[test]
    fn splits_big_data_over_several_idat_chunks() {
        // Noise doesn't compress, so 1.5 MB of it needs two IDAT chunks.
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let px: Vec<u8> = (0..4_000_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 32) as u8
            })
            .collect();
        let png = encode(1000, 4000, ColorType::Gray, &px, Compression::Fast).unwrap();
        let chunks = chunks(&png);
        let idat: Vec<u8> = chunks.iter().filter(|c| &c.0 == b"IDAT").flat_map(|c| c.1.clone()).collect();
        assert!(chunks.iter().filter(|c| &c.0 == b"IDAT").count() >= 2);
        let raw = black_zlib::zlib::decompress(&idat, 1001 * 4000).unwrap();
        assert_eq!(unfilter(&raw, 1000, 1), px);
    }

    #[test]
    fn compresses_flat_images() {
        let png = encode(1920, 1080, ColorType::Rgb, &vec![40; 1920 * 1080 * 3], Compression::Default).unwrap();
        assert!(png.len() < 20_000, "{} bytes", png.len());
    }

    #[test]
    fn rejects_bad_sizes() {
        let bad = |w, h, color, len: usize| matches!(encode(w, h, color, &vec![0; len], Compression::Fast), Err(Error::Invalid(_)));
        assert!(bad(0, 5, ColorType::Gray, 0));
        assert!(bad(5, 0, ColorType::Gray, 0));
        assert!(bad(2, 2, ColorType::Rgb, 11));
        assert!(bad(2, 2, ColorType::Rgb, 13));
        assert!(bad(1 << 31, 1, ColorType::Gray, 0));
        assert!(bad(MAX_SIDE, MAX_SIDE, ColorType::Rgba, 0));
        assert!(encode(1, 1, ColorType::Gray, &[0], Compression::Fast).is_ok());
    }
}
