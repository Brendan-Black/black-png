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

/// How the file stores each pixel, which can differ from the [`ColorType`] it decodes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stored {
	Gray,
	Rgb,
	Palette,
	GrayAlpha,
	Rgba,
}

impl Stored {
	fn samples(self) -> usize {
		match self {
			Stored::Gray | Stored::Palette => 1,
			Stored::GrayAlpha => 2,
			Stored::Rgb => 3,
			Stored::Rgba => 4,
		}
	}
}

/// The header: what IHDR says about the image.
struct Header {
	width: u32,
	height: u32,
	depth: u8,
	stored: Stored,
	interlaced: bool,
}

impl Header {
	fn bits_per_pixel(&self) -> usize {
		self.stored.samples() * self.depth as usize
	}

	/// Bytes in a row `width` pixels wide, before its filter byte.
	fn row_bytes(&self, width: usize) -> usize {
		(width * self.bits_per_pixel()).div_ceil(8)
	}
}

/// What tRNS says is transparent: one exact color (as stored, before scaling to 8 bits),
/// or an alpha for each palette entry.
enum Transparency {
	None,
	Gray(u16),
	Rgb([u16; 3]),
	Palette(Vec<u8>),
}

/// The passes an image's pixels come in: (first x, first y, x step, y step). Interlaced
/// (Adam7) images send every 8th pixel of every 8th row first, then fill in.
const ONE_PASS: [(usize, usize, usize, usize); 1] = [(0, 0, 1, 1)];
const ADAM7: [(usize, usize, usize, usize); 7] = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

/// Decode a PNG, refusing images whose pixels would take more than `max_bytes`.
///
/// Reads every standard PNG: grayscale (1, 2, 4, 8 or 16 bits), RGB, palette, gray + alpha
/// and RGBA, interlaced or not. Pixels always come out as 8 bits per channel: 16-bit
/// channels are rounded, low-bit gray is scaled up (a 1-bit image gives 0 and 255),
/// palette images become RGB, and a tRNS chunk adds an alpha channel. Checks every chunk's
/// CRC; ancillary chunks (text, gamma, color profiles and the like) are skipped.
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
	// Refuse a too-big image straight away, at the smallest it could decode to (tRNS can
	// add an alpha channel, checked again below).
	let least_channels = if h.stored == Stored::Palette { 3 } else { h.stored.samples() };
	if (h.width as u64 * h.height as u64).saturating_mul(least_channels as u64) > max_bytes as u64 {
		return Err(Error::TooLarge { limit: max_bytes });
	}

	let mut palette: Option<&[u8]> = None;
	let mut trns = Transparency::None;
	// The compressed pixels are split over consecutive IDAT chunks.
	let mut idat = Vec::new();
	let mut idat_done = false;
	for chunk in chunks {
		let (kind, data) = chunk?;
		if !idat.is_empty() && &kind != b"IDAT" {
			idat_done = true;
		}
		let before_idat = |name: &str| {
			if idat.is_empty() { Ok(()) } else { Err(Error::corrupt(format!("{name} comes after the pixel data"))) }
		};
		match &kind {
			b"IDAT" if idat_done => return Err(Error::corrupt("IDAT chunks aren't consecutive")),
			b"IDAT" => idat.extend_from_slice(data),
			b"IEND" => break,
			b"IHDR" => return Err(Error::corrupt("a second IHDR")),
			b"PLTE" => {
				before_idat("PLTE")?;
				if matches!(h.stored, Stored::Gray | Stored::GrayAlpha) {
					return Err(Error::corrupt("a palette in a grayscale image"));
				}
				if data.is_empty() || data.len() % 3 != 0 || data.len() > 256 * 3 {
					return Err(Error::corrupt(format!("a {}-byte palette", data.len())));
				}
				palette = Some(data);
			}
			b"tRNS" => {
				before_idat("tRNS")?;
				let u16_at = |i: usize| u16::from_be_bytes([data[i], data[i + 1]]);
				trns = match h.stored {
					Stored::Gray if data.len() == 2 => Transparency::Gray(u16_at(0)),
					Stored::Rgb if data.len() == 6 => Transparency::Rgb([u16_at(0), u16_at(2), u16_at(4)]),
					Stored::Palette if palette.is_some_and(|p| data.len() <= p.len() / 3) => Transparency::Palette(data.to_vec()),
					Stored::Palette if palette.is_none() => return Err(Error::corrupt("tRNS before PLTE")),
					// Images with an alpha channel can't have tRNS; ignore it, as libpng does.
					Stored::GrayAlpha | Stored::Rgba => Transparency::None,
					_ => return Err(Error::corrupt(format!("a {}-byte tRNS for a {:?} image", data.len(), h.stored))),
				};
			}
			_ if kind[0].is_ascii_uppercase() => {
				return Err(Error::Unsupported(format!("the critical chunk {}", String::from_utf8_lossy(&kind))));
			}
			_ => {}
		}
	}
	if idat.is_empty() {
		return Err(Error::corrupt("no IDAT chunk"));
	}
	let palette = match (h.stored, palette) {
		(Stored::Palette, None) => return Err(Error::corrupt("a palette image with no PLTE")),
		(_, p) => p.unwrap_or(&[]),
	};

	let has_trns = !matches!(trns, Transparency::None);
	let color = match h.stored {
		Stored::Gray if has_trns => ColorType::GrayAlpha,
		Stored::Gray => ColorType::Gray,
		Stored::GrayAlpha => ColorType::GrayAlpha,
		Stored::Rgb | Stored::Palette if has_trns => ColorType::Rgba,
		Stored::Rgb | Stored::Palette => ColorType::Rgb,
		Stored::Rgba => ColorType::Rgba,
	};
	let (w, ht) = (h.width as usize, h.height as usize);
	let channels = color.channels();
	let size = w.checked_mul(ht).and_then(|n| n.checked_mul(channels)).filter(|&n| n <= max_bytes);
	let Some(size) = size else {
		return Err(Error::TooLarge { limit: max_bytes });
	};

	// Each pass is a smaller image of its own; a pass with no pixels has no rows at all.
	let passes: &[(usize, usize, usize, usize)] = if h.interlaced { &ADAM7 } else { &ONE_PASS };
	let pass_size =
		|&(x0, y0, dx, dy): &(usize, usize, usize, usize)| (if w > x0 { (w - x0).div_ceil(dx) } else { 0 }, if ht > y0 { (ht - y0).div_ceil(dy) } else { 0 });
	let filtered_len: usize = passes
		.iter()
		.map(|p| match pass_size(p) {
			(0, _) | (_, 0) => 0,
			(pw, ph) => ph * (1 + h.row_bytes(pw)),
		})
		.sum();
	let filtered = black_zlib::zlib::decompress(&idat, filtered_len).map_err(|e| match e {
		black_zlib::Error::TooLarge { .. } => Error::corrupt("more pixel data than the image holds"),
		e => e.into(),
	})?;
	if filtered.len() != filtered_len {
		return Err(Error::corrupt(format!("{} bytes of pixel data where {filtered_len} were expected", filtered.len())));
	}

	let mut pixels = vec![0u8; size];
	let direct = h.depth == 8 && !h.interlaced && !has_trns && h.stored != Stored::Palette;
	let filter_bpp = h.bits_per_pixel().div_ceil(8);
	let mut pos = 0;
	for pass in passes {
		let (pw, ph) = pass_size(pass);
		if pw == 0 || ph == 0 {
			continue;
		}
		let (x0, y0, dx, dy) = *pass;
		let rb = h.row_bytes(pw);
		let mut prev = vec![0u8; rb];
		let mut row = vec![0u8; rb];
		for j in 0..ph {
			let kind = filtered[pos];
			row.copy_from_slice(&filtered[pos + 1..pos + 1 + rb]);
			pos += 1 + rb;
			if !filter::undo(kind, &mut row, &prev, filter_bpp) {
				return Err(Error::corrupt(format!("unknown filter type {kind}")));
			}
			let y = y0 + j * dy;
			if direct {
				// Stored exactly as decoded: the row is the pixels.
				pixels[y * w * channels..(y + 1) * w * channels].copy_from_slice(&row);
			} else {
				for i in 0..pw {
					let x = x0 + i * dx;
					let at = (y * w + x) * channels;
					pixel(&h, &row, i, palette, &trns, &mut pixels[at..at + channels])?;
				}
			}
			std::mem::swap(&mut prev, &mut row);
		}
	}
	Ok(Image { width: h.width, height: h.height, color, pixels })
}

/// Sample `k` of a row (counting every channel of every pixel), as stored.
fn sample(row: &[u8], depth: u8, k: usize) -> u16 {
	match depth {
		8 => row[k] as u16,
		16 => u16::from_be_bytes([row[2 * k], row[2 * k + 1]]),
		d => {
			// Packed with the leftmost pixel in the highest bits.
			let bit = k * d as usize;
			((row[bit / 8] >> (8 - d as usize - bit % 8)) & ((1 << d) - 1)) as u16
		}
	}
}

/// A sample scaled to 8 bits.
fn scale(v: u16, depth: u8) -> u8 {
	match depth {
		1 => (v * 255) as u8,
		2 => (v * 85) as u8,
		4 => (v * 17) as u8,
		8 => v as u8,
		_ => ((v as u32 * 255 + 32767) / 65535) as u8,
	}
}

/// Write pixel `i` of a row into `out`, in the decoded color type.
fn pixel(h: &Header, row: &[u8], i: usize, palette: &[u8], trns: &Transparency, out: &mut [u8]) -> Result<()> {
	let d = h.depth;
	let n = h.stored.samples();
	let s = |c: usize| sample(row, d, i * n + c);
	match h.stored {
		Stored::Palette => {
			let idx = s(0) as usize;
			let Some(rgb) = palette.get(idx * 3..idx * 3 + 3) else {
				return Err(Error::corrupt(format!("palette index {idx} past the {}-color palette", palette.len() / 3)));
			};
			out[..3].copy_from_slice(rgb);
			if let Transparency::Palette(alpha) = trns {
				out[3] = alpha.get(idx).copied().unwrap_or(255);
			}
		}
		Stored::Gray => {
			let v = s(0);
			out[0] = scale(v, d);
			if let Transparency::Gray(t) = trns {
				out[1] = if v == *t { 0 } else { 255 };
			}
		}
		Stored::Rgb => {
			let rgb = [s(0), s(1), s(2)];
			for c in 0..3 {
				out[c] = scale(rgb[c], d);
			}
			if let Transparency::Rgb(t) = trns {
				out[3] = if rgb == *t { 0 } else { 255 };
			}
		}
		Stored::GrayAlpha | Stored::Rgba => {
			for (c, o) in out.iter_mut().enumerate() {
				*o = scale(s(c), d);
			}
		}
	}
	Ok(())
}

fn header(d: &[u8]) -> Result<Header> {
	let Ok(d) = <&[u8; 13]>::try_from(d) else {
		return Err(Error::corrupt("IHDR isn't 13 bytes"));
	};
	let [w0, w1, w2, w3, h0, h1, h2, h3, depth, color, compression, filter, interlace] = *d;
	let width = u32::from_be_bytes([w0, w1, w2, w3]);
	let height = u32::from_be_bytes([h0, h1, h2, h3]);
	if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
		return Err(Error::corrupt(format!("a PNG can't be {width}x{height}")));
	}
	if compression != 0 || filter != 0 || interlace > 1 {
		return Err(Error::corrupt("unknown compression, filter or interlace method"));
	}
	let (stored, depths): (Stored, &[u8]) = match color {
		0 => (Stored::Gray, &[1, 2, 4, 8, 16]),
		2 => (Stored::Rgb, &[8, 16]),
		3 => (Stored::Palette, &[1, 2, 4, 8]),
		4 => (Stored::GrayAlpha, &[8, 16]),
		6 => (Stored::Rgba, &[8, 16]),
		c => return Err(Error::corrupt(format!("unknown color type {c}"))),
	};
	if !depths.contains(&depth) {
		return Err(Error::corrupt(format!("{depth}-bit {stored:?} isn't a PNG format")));
	}
	Ok(Header { width, height, depth, stored, interlaced: interlace == 1 })
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
		let [l0, l1, l2, l3, k0, k1, k2, k3] = *head;
		let len = u32::from_be_bytes([l0, l1, l2, l3]) as usize;
		let kind = [k0, k1, k2, k3];
		if len > i32::MAX as usize {
			return Some(Err(Error::corrupt("a chunk claims to be over 2 GiB")));
		}
		let Some(crc) = r.get(8 + len..12 + len).and_then(|c| <[u8; 4]>::try_from(c).ok()) else {
			return cut();
		};
		if u32::from_be_bytes(crc) != crc32(&r[4..8 + len]) {
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

	fn hand_made(width: u32, height: u32, color: u8, rows: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
		hand_made_with(width, height, 8, color, false, rows, extra)
	}

	/// A PNG built by hand from an IHDR and the filtered rows, with `extra` chunks between.
	fn hand_made_with(width: u32, height: u32, depth: u8, color: u8, interlaced: bool, rows: &[u8], extra: &[Vec<u8>]) -> Vec<u8> {
		let mut ihdr = width.to_be_bytes().to_vec();
		ihdr.extend(height.to_be_bytes());
		ihdr.extend([depth, color, 0, 0, interlaced as u8]);
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
	fn samples_and_scaling() {
		let row = [0b1011_0010];
		assert_eq!((0..8).map(|k| sample(&row, 1, k)).collect::<Vec<_>>(), [1, 0, 1, 1, 0, 0, 1, 0]);
		assert_eq!((0..4).map(|k| sample(&row, 2, k)).collect::<Vec<_>>(), [2, 3, 0, 2]);
		assert_eq!((0..2).map(|k| sample(&row, 4, k)).collect::<Vec<_>>(), [0xB, 0x2]);
		assert_eq!(sample(&[0x12, 0x34, 0x56, 0x78], 16, 1), 0x5678);
		assert_eq!([scale(1, 1), scale(3, 2), scale(15, 4), scale(65535, 16)], [255; 4]);
		assert_eq!([scale(1, 2), scale(7, 4), scale(0, 16)], [85, 119, 0]);
		// 16 bits round to the nearest 8-bit value: 128.5 / 65535 * 255 is the halfway point.
		assert_eq!([scale(128, 16), scale(129, 16), scale(32767, 16), scale(32768, 16)], [0, 1, 127, 128]);
	}

	#[test]
	fn single_color_transparency() {
		// 8-bit gray where 5 is transparent.
		let img = decode(&hand_made(3, 1, 0, &[0, 5, 9, 5], &[chunk(b"tRNS", &[0, 5])]), 100).unwrap();
		assert_eq!((img.color, img.pixels), (ColorType::GrayAlpha, vec![5, 0, 9, 255, 5, 0]));
		// 16-bit gray compares the full value: 0x0102 is transparent and 0x0103 isn't,
		// though both round to 1.
		let png = hand_made_with(2, 1, 16, 0, false, &[0, 1, 2, 1, 3], &[chunk(b"tRNS", &[1, 2])]);
		assert_eq!(decode(&png, 100).unwrap().pixels, [1, 0, 1, 255]);
		// RGB where (1, 2, 3) is transparent.
		let png = hand_made(2, 1, 2, &[0, 1, 2, 3, 1, 2, 4], &[chunk(b"tRNS", &[0, 1, 0, 2, 0, 3])]);
		let img = decode(&png, 100).unwrap();
		assert_eq!((img.color, img.pixels), (ColorType::Rgba, vec![1, 2, 3, 0, 1, 2, 4, 255]));
		// An image with alpha ignores tRNS, as libpng does.
		let png = hand_made(1, 1, 6, &[0, 1, 2, 3, 4], &[chunk(b"tRNS", &[0, 1, 0, 2, 0, 3])]);
		assert_eq!(decode(&png, 100).unwrap().pixels, [1, 2, 3, 4]);
	}

	#[test]
	fn palettes() {
		// 2-bit indices 0, 1, 2 into three colors, the first two partly transparent.
		let plte = chunk(b"PLTE", &[10, 20, 30, 40, 50, 60, 70, 80, 90]);
		let png = hand_made_with(3, 1, 2, 3, false, &[0, 0b00_01_10_00], &[plte.clone(), chunk(b"tRNS", &[0, 128])]);
		let img = decode(&png, 100).unwrap();
		assert_eq!(img.color, ColorType::Rgba);
		assert_eq!(img.pixels, [10, 20, 30, 0, 40, 50, 60, 128, 70, 80, 90, 255]);
		// Without tRNS it's plain RGB.
		let img = decode(&hand_made(2, 1, 3, &[0, 2, 0], &[plte]), 100).unwrap();
		assert_eq!((img.color, img.pixels), (ColorType::Rgb, vec![70, 80, 90, 10, 20, 30]));
	}

	#[test]
	fn sixteen_bit_gray_alpha() {
		let img = decode(&hand_made_with(1, 1, 16, 4, false, &[0, 0xFF, 0xFF, 0x80, 0x00], &[]), 100).unwrap();
		assert_eq!((img.color, img.pixels), (ColorType::GrayAlpha, vec![255, 128]));
	}

	/// Interlace 8-bit gray pixels the Adam7 way, filter type 0 on every row.
	fn adam7(px: &[u8], w: usize, h: usize) -> Vec<u8> {
		let mut out = Vec::new();
		for (x0, y0, dx, dy) in ADAM7 {
			let xs: Vec<usize> = (x0..w).step_by(dx).collect();
			if xs.is_empty() {
				continue;
			}
			for y in (y0..h).step_by(dy) {
				out.push(0);
				out.extend(xs.iter().map(|&x| px[y * w + x]));
			}
		}
		out
	}

	#[test]
	fn interlaced() {
		for (w, h) in [(1, 1), (2, 3), (8, 8), (9, 9), (17, 5), (3, 20)] {
			let px: Vec<u8> = (0..w * h).map(|i| (i * 7 + 1) as u8).collect();
			let png = hand_made_with(w as u32, h as u32, 8, 0, true, &adam7(&px, w, h), &[]);
			assert_eq!(decode(&png, 1000).unwrap().pixels, px, "{w}x{h}");
		}
	}

	#[test]
	fn rejects_bad_colors() {
		let corrupt = |png: &[u8]| matches!(decode(png, 1000), Err(Error::Corrupt(_)));
		let plte = || chunk(b"PLTE", &[1, 2, 3]);
		assert!(corrupt(&hand_made(1, 1, 0, &[0, 1], &[chunk(b"tRNS", &[0, 1, 2])])), "gray tRNS length");
		assert!(corrupt(&hand_made(1, 1, 2, &[0, 1, 2, 3], &[chunk(b"tRNS", &[0, 1])])), "RGB tRNS length");
		assert!(corrupt(&hand_made(1, 1, 3, &[0, 0], &[chunk(b"tRNS", &[0])])), "tRNS before PLTE");
		assert!(corrupt(&hand_made(1, 1, 3, &[0, 0], &[plte(), chunk(b"tRNS", &[0, 0])])), "tRNS longer than the palette");
		assert!(corrupt(&hand_made(1, 1, 0, &[0, 1], &[plte()])), "PLTE in gray");
		assert!(corrupt(&hand_made(1, 1, 3, &[0, 1], &[chunk(b"PLTE", &[1, 2, 3, 4])])), "palette length");
		assert!(corrupt(&hand_made(1, 1, 3, &[0, 1], &[plte()])), "index past the palette");
		assert!(corrupt(&hand_made_with(1, 1, 4, 2, false, &[0, 0], &[])), "4-bit RGB");
		assert!(corrupt(&hand_made_with(1, 1, 16, 3, false, &[0, 0, 0], &[plte()])), "16-bit palette");
		// PLTE after the pixel data.
		let mut late = hand_made(1, 1, 3, &[0, 0], &[plte()]);
		let iend = late.len() - 12;
		late.splice(iend..iend, plte());
		assert!(corrupt(&late));
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
		assert!(corrupt(&hand_made(1, 1, 3, &[0, 0], &[])), "palette with no PLTE");
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
