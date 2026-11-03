//! Reading PNGs.

use black_zlib::checksum::crc32;

use crate::{ColorType, Error, Result, SIGNATURE, filter};

/// A decoded image: `width * height` pixels, top row first, `color.channels()` bytes each.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub color: ColorType,
    pub pixels: Vec<u8>,
}

impl Image {
    /// The pixels as RGBA, whatever the color type: gray becomes equal red, green and blue,
    /// and a missing alpha becomes 255.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        for p in self.pixels.chunks_exact(self.color.channels()) {
            out.extend(match self.color {
                ColorType::Gray => [p[0], p[0], p[0], 255],
                ColorType::GrayAlpha => [p[0], p[0], p[0], p[1]],
                ColorType::Rgb => [p[0], p[1], p[2], 255],
                ColorType::Rgba => [p[0], p[1], p[2], p[3]],
            });
        }
        out
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{} {:?})", self.width, self.height, self.color)
    }
}

/// The header: what IHDR says about the image.
struct Header {
    width: u32,
    height: u32,
    color: ColorType,
}

/// Decode a PNG, refusing images whose pixels would take more than `max_bytes`. Checks
/// every chunk's CRC. Ancillary chunks (text, gamma, timestamps and the like) are skipped.
pub fn decode(png: &[u8], max_bytes: usize) -> Result<Image> {
    let Some(rest) = png.strip_prefix(&SIGNATURE) else {
        return Err(Error::corrupt("not a PNG (wrong signature)"));
    };
    let mut chunks = Chunks { rest };
    let (kind, data) = chunks.next().ok_or_else(|| Error::corrupt("no chunks"))??;
    if &kind != b"IHDR" {
        return Err(Error::corrupt("the first chunk isn't IHDR"));
    }
    let h = header(data)?;

    let bpp = h.color.channels();
    let stride = h.width as usize * bpp;
    let size = stride.checked_mul(h.height as usize).filter(|&n| n <= max_bytes);
    let Some(size) = size else {
        return Err(Error::TooLarge { limit: max_bytes });
    };

    // The compressed pixels are split over consecutive IDAT chunks.
    let mut idat = Vec::new();
    let mut idat_done = false;
    for chunk in chunks {
        let (kind, data) = chunk?;
        match &kind {
            b"IDAT" if idat_done => return Err(Error::corrupt("IDAT chunks aren't consecutive")),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            b"IHDR" => return Err(Error::corrupt("a second IHDR")),
            // PLTE is only a suggested palette for truecolor images; nothing needs it yet.
            b"PLTE" => {}
            _ if kind[0].is_ascii_uppercase() => {
                return Err(Error::Unsupported(format!("the critical chunk {}", String::from_utf8_lossy(&kind))));
            }
            _ => {}
        }
        if !idat.is_empty() && &kind != b"IDAT" {
            idat_done = true;
        }
    }
    if idat.is_empty() {
        return Err(Error::corrupt("no IDAT chunk"));
    }

    let filtered_len = (stride + 1) * h.height as usize;
    let filtered = black_zlib::zlib::decompress(&idat, filtered_len).map_err(|e| match e {
        black_zlib::Error::TooLarge { .. } => Error::corrupt("more pixel data than the image holds"),
        e => e.into(),
    })?;
    if filtered.len() != filtered_len {
        return Err(Error::corrupt(format!("{} bytes of pixel data where {filtered_len} were expected", filtered.len())));
    }

    let mut pixels = vec![0u8; size];
    let zeros = vec![0u8; stride];
    for y in 0..h.height as usize {
        let src = &filtered[y * (stride + 1)..(y + 1) * (stride + 1)];
        let (done, row) = pixels.split_at_mut(y * stride);
        let prev = if y == 0 { &zeros[..] } else { &done[(y - 1) * stride..] };
        let row = &mut row[..stride];
        row.copy_from_slice(&src[1..]);
        if !filter::undo(src[0], row, prev, bpp) {
            return Err(Error::corrupt(format!("unknown filter type {} on row {y}", src[0])));
        }
    }
    Ok(Image { width: h.width, height: h.height, color: h.color, pixels })
}

fn header(d: &[u8]) -> Result<Header> {
    let Ok(d) = <&[u8; 13]>::try_from(d) else {
        return Err(Error::corrupt("IHDR isn't 13 bytes"));
    };
    let width = u32::from_be_bytes(d[0..4].try_into().unwrap());
    let height = u32::from_be_bytes(d[4..8].try_into().unwrap());
    let [depth, color, compression, filter, interlace] = d[8..13].try_into().unwrap();
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(Error::corrupt(format!("a PNG can't be {width}x{height}")));
    }
    if compression != 0 || filter != 0 || interlace > 1 {
        return Err(Error::corrupt("unknown compression, filter or interlace method"));
    }
    let color = match color {
        0 => ColorType::Gray,
        2 => ColorType::Rgb,
        4 => ColorType::GrayAlpha,
        6 => ColorType::Rgba,
        3 => return Err(Error::Unsupported("palette images".into())),
        c => return Err(Error::corrupt(format!("unknown color type {c}"))),
    };
    if depth != 8 {
        return Err(Error::Unsupported(format!("{depth}-bit channels")));
    }
    if interlace == 1 {
        return Err(Error::Unsupported("interlaced images".into()));
    }
    Ok(Header { width, height, color })
}

/// Walks the chunks after the signature, checking each CRC.
struct Chunks<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Result<([u8; 4], &'a [u8])>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let r = self.rest;
        let cut = || Some(Err(Error::corrupt("the PNG is cut short")));
        let Some(head) = r.first_chunk::<8>() else {
            return cut();
        };
        let len = u32::from_be_bytes(head[..4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = head[4..].try_into().unwrap();
        if len > i32::MAX as usize {
            return Some(Err(Error::corrupt("a chunk claims to be over 2 GiB")));
        }
        let Some(crc) = r.get(8 + len..12 + len) else {
            return cut();
        };
        if u32::from_be_bytes(crc.try_into().unwrap()) != crc32(&r[4..8 + len]) {
            return Some(Err(Error::corrupt(format!("the {} chunk's CRC doesn't match", String::from_utf8_lossy(&kind)))));
        }
        self.rest = &r[12 + len..];
        Some(Ok((kind, &r[8..8 + len])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Compression, encode};

    fn picture(len: usize) -> Vec<u8> {
        let mut seed = 5u32;
        (0..len)
            .map(|i| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                if i % 3 == 0 { (seed >> 16) as u8 } else { (i / 11) as u8 }
            })
            .collect()
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(data);
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(crc32(&body).to_be_bytes());
        out
    }

    /// A PNG built by hand from an IHDR and the filtered rows.
    fn hand_made(width: u32, height: u32, color: u8, rows: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
        let mut ihdr = width.to_be_bytes().to_vec();
        ihdr.extend(height.to_be_bytes());
        ihdr.extend([8, color, 0, 0, 0]);
        let mut png = SIGNATURE.to_vec();
        png.extend(chunk(b"IHDR", &ihdr));
        for e in extra {
            png.extend(e);
        }
        png.extend(chunk(b"IDAT", &black_zlib::zlib::compress(rows, Compression::Fast)));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn reads_what_encode_writes() {
        for color in [ColorType::Gray, ColorType::GrayAlpha, ColorType::Rgb, ColorType::Rgba] {
            let (w, h) = (41, 13);
            let px = picture(w * h * color.channels());
            let png = encode(w as u32, h as u32, color, &px, Compression::Default).unwrap();
            let img = decode(&png, usize::MAX).unwrap();
            assert_eq!((img.width, img.height, img.color), (w as u32, h as u32, color));
            assert!(img.pixels == px, "{color:?}");
        }
    }

    #[test]
    fn converts_to_rgba() {
        let img = Image { width: 2, height: 1, color: ColorType::GrayAlpha, pixels: vec![10, 20, 30, 40] };
        assert_eq!(img.to_rgba(), [10, 10, 10, 20, 30, 30, 30, 40]);
        let img = Image { width: 1, height: 1, color: ColorType::Rgb, pixels: vec![1, 2, 3] };
        assert_eq!(img.to_rgba(), [1, 2, 3, 255]);
        assert_eq!(format!("{img:?}"), "Image(1x1 Rgb)");
    }

    #[test]
    fn skips_ancillary_chunks_and_splits_idat() {
        let mut png = SIGNATURE.to_vec();
        let mut ihdr = 2u32.to_be_bytes().to_vec();
        ihdr.extend(1u32.to_be_bytes());
        ihdr.extend([8, 0, 0, 0, 0]);
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"tEXt", b"Comment\0hello"));
        png.extend(chunk(b"gAMA", &45455u32.to_be_bytes()));
        let z = black_zlib::zlib::compress(&[0, 7, 9], Compression::Fast);
        let (a, b) = z.split_at(3);
        png.extend(chunk(b"IDAT", a));
        png.extend(chunk(b"IDAT", b));
        png.extend(chunk(b"IEND", &[]));
        assert_eq!(decode(&png, 100).unwrap().pixels, [7, 9]);
    }

    #[test]
    fn rejects_bad_pngs() {
        let good = encode(3, 2, ColorType::Rgb, &picture(18), Compression::Fast).unwrap();
        assert!(decode(&good, 100).is_ok());
        let corrupt = |png: &[u8]| matches!(decode(png, 1000), Err(Error::Corrupt(_)));
        let unsupported = |png: &[u8]| matches!(decode(png, 1000), Err(Error::Unsupported(_)));

        assert!(corrupt(b"GIF89a"), "signature");
        assert!(corrupt(&SIGNATURE), "no chunks");
        assert!(corrupt(&good[..good.len() - 20]), "cut short");
        let mut bad_crc = good.clone();
        bad_crc[20] ^= 1;
        assert!(corrupt(&bad_crc), "CRC");
        // Filter type 5 doesn't exist.
        assert!(corrupt(&hand_made(1, 1, 0, &[5, 0], &[])), "filter type");
        assert!(corrupt(&hand_made(2, 2, 0, &[0, 1, 2], &[])), "too little pixel data");
        assert!(corrupt(&hand_made(1, 1, 0, &[0, 1, 0, 2], &[])), "too much pixel data");
        assert!(corrupt(&hand_made(0, 1, 0, &[], &[])), "zero width");
        assert!(corrupt(&hand_made(1, 1, 1, &[0, 1], &[])), "color type 1");
        let mut no_idat = SIGNATURE.to_vec();
        no_idat.extend(&good[8..33]);
        no_idat.extend(chunk(b"IEND", &[]));
        assert!(corrupt(&no_idat), "no IDAT");
        let mut ihdr_second = SIGNATURE.to_vec();
        ihdr_second.extend(chunk(b"tEXt", b"a\0b"));
        ihdr_second.extend(&good[8..]);
        assert!(corrupt(&ihdr_second), "IHDR not first");

        assert!(unsupported(&hand_made(1, 1, 0, &[0, 1], &[chunk(b"ZZZZ", &[])])), "unknown critical chunk");
        assert!(unsupported(&hand_made(1, 1, 3, &[0, 0], &[])), "palette, for now");
    }

    #[test]
    fn stops_at_the_limit() {
        let png = encode(100, 100, ColorType::Rgba, &vec![0; 40_000], Compression::Fast).unwrap();
        assert_eq!(decode(&png, 39_999), Err(Error::TooLarge { limit: 39_999 }));
        assert!(decode(&png, 40_000).is_ok());
        // A header claiming a huge image is refused before anything is decompressed.
        let mut huge = SIGNATURE.to_vec();
        let mut ihdr = 100_000u32.to_be_bytes().to_vec();
        ihdr.extend(100_000u32.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        huge.extend(chunk(b"IHDR", &ihdr));
        assert_eq!(decode(&huge, 1 << 30), Err(Error::TooLarge { limit: 1 << 30 }));
    }
}
