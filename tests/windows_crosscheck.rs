//! Checks black-png against another implementation: Windows' own PNG code (GDI+, through
//! .NET's System.Drawing in Windows PowerShell). Each PNG written here must load there with
//! every pixel as we wrote it, and each PNG GDI+ writes must decode here the same way. Only
//! runs on Windows.

#![cfg(windows)]

use std::process::Command;

use black_png::{ColorType, Compression, decode, encode};

/// Load a PNG with System.Drawing: its width, height and each pixel as ARGB.
fn windows_decode(png: &[u8], name: &str) -> (u32, u32, Vec<u32>) {
	let path = std::env::temp_dir().join(format!("black-png-{}-{name}.png", std::process::id()));
	std::fs::write(&path, png).unwrap();
	let script = format!(
		"Add-Type -AssemblyName System.Drawing; \
         $b = [System.Drawing.Bitmap]::new('{}'); \
         $s = [System.Text.StringBuilder]::new(); \
         [void]$s.Append(\"$($b.Width) $($b.Height)\"); \
         for ($y = 0; $y -lt $b.Height; $y++) {{ for ($x = 0; $x -lt $b.Width; $x++) {{ \
           [void]$s.Append(' ').Append($b.GetPixel($x, $y).ToArgb().ToString('x8')) }} }}; \
         $b.Dispose(); $s.ToString()",
		path.display()
	);
	let out = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap();
	let _ = std::fs::remove_file(&path);
	assert!(out.status.success(), "PowerShell failed: {}", String::from_utf8_lossy(&out.stderr));
	let text = String::from_utf8(out.stdout).unwrap();
	let mut words = text.split_whitespace();
	let w = words.next().unwrap().parse().unwrap();
	let h = words.next().unwrap().parse().unwrap();
	(w, h, words.map(|p| u32::from_str_radix(p, 16).unwrap()).collect())
}

/// What each pixel should read as in ARGB.
fn argb(color: ColorType, px: &[u8]) -> Vec<u32> {
	let pack = |a: u8, r: u8, g: u8, b: u8| u32::from_be_bytes([a, r, g, b]);
	px.chunks(color.channels())
		.map(|p| match color {
			ColorType::Gray => pack(255, p[0], p[0], p[0]),
			ColorType::GrayAlpha => pack(p[1], p[0], p[0], p[0]),
			ColorType::Rgb => pack(255, p[0], p[1], p[2]),
			ColorType::Rgba => pack(p[3], p[0], p[1], p[2]),
		})
		.collect()
}

#[test]
fn windows_reads_every_color_type() {
	let (w, h) = (29u32, 17u32);
	for color in [ColorType::Gray, ColorType::GrayAlpha, ColorType::Rgb, ColorType::Rgba] {
		let mut seed = 7u32;
		let px: Vec<u8> = (0..w * h * color.channels() as u32)
			.map(|i| {
				seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
				let v = if i % 5 == 0 { (seed >> 16) as u8 } else { (i / 7) as u8 };
				// Keep alpha opaque-ish: GDI+ may premultiply, which loses detail at low alpha.
				if matches!(color, ColorType::GrayAlpha | ColorType::Rgba) && (i as usize + 1).is_multiple_of(color.channels()) { v | 0xC0 } else { v }
			})
			.collect();
		for level in [Compression::Fast, Compression::Best] {
			let png = encode(w, h, color, &px, level).unwrap();
			let (dw, dh, got) = windows_decode(&png, &format!("{color:?}-{level:?}"));
			assert_eq!((dw, dh), (w, h));
			let want = argb(color, &px);
			assert_eq!(got.len(), want.len(), "{color:?} at {level:?}: pixel count");
			let wrong = got.iter().zip(&want).position(|(g, w)| g != w);
			assert!(wrong.is_none(), "{color:?} at {level:?}: pixel {:?} is {:08x}, should be {:08x}", wrong, got[wrong.unwrap()], want[wrong.unwrap()]);
		}
	}
}

/// The test picture's pixel at (x, y), as RGBA.
fn pattern(x: u32, y: u32) -> [u8; 4] {
	[(x * 7 + y * 3) as u8, (x * y) as u8, (x + y * 11) as u8, (128 + (x * 5 + y) % 128) as u8]
}

/// Have GDI+ draw the pattern into a bitmap of `format` and save it as a PNG.
fn windows_encode(w: u32, h: u32, format: &str) -> Vec<u8> {
	let path = std::env::temp_dir().join(format!("black-png-{}-gdi-{format}.png", std::process::id()));
	let script = format!(
		"Add-Type -AssemblyName System.Drawing; \
         $b = [System.Drawing.Bitmap]::new({w}, {h}, [System.Drawing.Imaging.PixelFormat]::{format}); \
         for ($y = 0; $y -lt {h}; $y++) {{ for ($x = 0; $x -lt {w}; $x++) {{ \
           $c = [System.Drawing.Color]::FromArgb((128 + ($x * 5 + $y) % 128), ($x * 7 + $y * 3) % 256, ($x * $y) % 256, ($x + $y * 11) % 256); \
           $b.SetPixel($x, $y, $c) }} }}; \
         $b.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png); $b.Dispose()",
		path.display()
	);
	let out = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap();
	assert!(out.status.success(), "PowerShell failed: {}", String::from_utf8_lossy(&out.stderr));
	let png = std::fs::read(&path).unwrap();
	let _ = std::fs::remove_file(&path);
	png
}

#[test]
fn we_read_what_windows_writes() {
	let (w, h) = (64u32, 40u32);
	for (format, color) in [("Format32bppArgb", ColorType::Rgba), ("Format24bppRgb", ColorType::Rgb)] {
		let img = decode(&windows_encode(w, h, format), usize::MAX).unwrap();
		assert_eq!((img.width, img.height, img.color), (w, h, color), "{format}");
		let rgba = img.to_rgba();
		for y in 0..h {
			for x in 0..w {
				let mut want = pattern(x, y);
				if color == ColorType::Rgb {
					want[3] = 255;
				}
				let i = ((y * w + x) * 4) as usize;
				assert_eq!(rgba[i..i + 4], want, "{format} pixel ({x}, {y})");
			}
		}
	}
}

/// One of WIC's pixel formats, and the PNG it should become.
struct WicFormat {
	name: &'static str,
	bits: usize,
	palette: usize,
	/// (bit depth, color type) of the PNG WIC writes.
	png: (u8, u8),
}

const WIC_FORMATS: &[WicFormat] = &[
	WicFormat { name: "BlackWhite", bits: 1, palette: 0, png: (1, 0) },
	WicFormat { name: "Gray2", bits: 2, palette: 0, png: (2, 0) },
	WicFormat { name: "Gray4", bits: 4, palette: 0, png: (4, 0) },
	WicFormat { name: "Gray8", bits: 8, palette: 0, png: (8, 0) },
	WicFormat { name: "Gray16", bits: 16, palette: 0, png: (16, 0) },
	WicFormat { name: "Indexed1", bits: 1, palette: 2, png: (1, 3) },
	WicFormat { name: "Indexed2", bits: 2, palette: 4, png: (2, 3) },
	WicFormat { name: "Indexed4", bits: 4, palette: 16, png: (4, 3) },
	WicFormat { name: "Indexed8", bits: 8, palette: 256, png: (8, 3) },
	WicFormat { name: "Indexed8", bits: 8, palette: 200, png: (8, 3) },
	WicFormat { name: "Bgr24", bits: 24, palette: 0, png: (8, 2) },
	WicFormat { name: "Bgra32", bits: 32, palette: 0, png: (8, 6) },
	WicFormat { name: "Rgb48", bits: 48, palette: 0, png: (16, 2) },
	WicFormat { name: "Rgba64", bits: 64, palette: 0, png: (16, 6) },
];

/// Palette entry `i` of `n`, as [a, r, g, b]: every entry differs, and alpha varies.
fn palette_entry(i: usize, n: usize) -> [u8; 4] {
	[(255 - (i % 3) * 60) as u8, (i * 255 / n.max(2)) as u8, (255 - i) as u8, (i * 7) as u8]
}

/// Raw pixel bytes in WIC's layout: rows `stride` bytes apart, sub-byte pixels packed from
/// the highest bit, 16-bit channels little-endian.
fn wic_pixels(f: &WicFormat, w: usize, h: usize) -> Vec<u8> {
	let stride = (w * f.bits).div_ceil(8);
	let mut seed = 0x2545_F491u32;
	let mut raw: Vec<u8> = (0..stride * h)
		.map(|i| {
			seed ^= seed << 13;
			seed ^= seed >> 17;
			seed ^= seed << 5;
			if i % 4 == 0 { (i * 3) as u8 } else { seed as u8 }
		})
		.collect();
	if f.palette > 0 && f.palette < 1 << f.bits {
		for b in raw.iter_mut() {
			*b %= f.palette as u8;
		}
	}
	raw
}

/// What each pixel should decode to, as RGBA, worked out from WIC's raw layout.
fn wic_expected(f: &WicFormat, raw: &[u8], w: usize, h: usize) -> Vec<u8> {
	let stride = (w * f.bits).div_ceil(8);
	let le16 = |b: &[u8], i: usize| ((u16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as u32 * 255 + 32767) / 65535) as u8;
	let mut out = Vec::new();
	for y in 0..h {
		let row = &raw[y * stride..(y + 1) * stride];
		for x in 0..w {
			let px = if f.bits < 8 {
				let bit = x * f.bits;
				let v = (row[bit / 8] >> (8 - f.bits - bit % 8)) & ((1 << f.bits) - 1);
				if f.palette > 0 {
					let [a, r, g, b] = palette_entry(v as usize, f.palette);
					[r, g, b, a]
				} else {
					let g = (v as u32 * 255 / ((1 << f.bits) - 1)) as u8;
					[g, g, g, 255]
				}
			} else {
				let p = &row[x * f.bits / 8..(x + 1) * f.bits / 8];
				match (f.name, f.palette) {
					(_, n) if n > 0 => {
						let [a, r, g, b] = palette_entry(p[0] as usize, n);
						[r, g, b, a]
					}
					("Gray8", _) => [p[0], p[0], p[0], 255],
					("Gray16", _) => [le16(p, 0), le16(p, 0), le16(p, 0), 255],
					("Bgr24", _) => [p[2], p[1], p[0], 255],
					("Bgra32", _) => [p[2], p[1], p[0], p[3]],
					("Rgb48", _) => [le16(p, 0), le16(p, 1), le16(p, 2), 255],
					("Rgba64", _) => [le16(p, 0), le16(p, 1), le16(p, 2), le16(p, 3)],
					(name, _) => unreachable!("{name}"),
				}
			};
			out.extend(px);
		}
	}
	out
}

#[test]
fn we_read_every_format_wic_writes() {
	let dir = std::env::temp_dir().join(format!("black-png-wic-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let sizes = [(13usize, 11usize), (1, 1), (3, 2)];
	// One line per PNG for the script: format, size, interlace, palette, raw file, PNG file.
	let mut jobs = Vec::new();
	let mut spec = String::new();
	for (k, f) in WIC_FORMATS.iter().enumerate() {
		for &(w, h) in &sizes {
			let raw = wic_pixels(f, w, h);
			let raw_path = dir.join(format!("{k}-{w}x{h}.bin"));
			std::fs::write(&raw_path, &raw).unwrap();
			let palette: Vec<String> = (0..f.palette).map(|i| palette_entry(i, f.palette).iter().map(|b| b.to_string()).collect::<Vec<_>>().join(".")).collect();
			for interlace in ["Off", "On"] {
				let png_path = dir.join(format!("{k}-{w}x{h}-{interlace}.png"));
				spec += &format!("{}|{w}|{h}|{interlace}|{}|{}|{}\n", f.name, palette.join(","), raw_path.display(), png_path.display());
				jobs.push((f, w, h, interlace, raw.clone(), png_path));
			}
		}
	}
	let spec_path = dir.join("spec.txt");
	std::fs::write(&spec_path, spec).unwrap();
	let script = format!(
		"Add-Type -AssemblyName PresentationCore; \
         foreach ($line in [IO.File]::ReadAllLines('{}')) {{ \
           $name, $w, $h, $il, $pal, $rawPath, $pngPath = $line.Split('|'); \
           $fmt = [System.Windows.Media.PixelFormats]::$name; \
           $palette = $null; \
           if ($pal) {{ \
             $colors = [System.Collections.Generic.List[System.Windows.Media.Color]]::new(); \
             foreach ($c in $pal.Split(',')) {{ $a, $r, $g, $b = $c.Split('.'); $colors.Add([System.Windows.Media.Color]::FromArgb($a, $r, $g, $b)) }}; \
             $palette = [System.Windows.Media.Imaging.BitmapPalette]::new($colors) }}; \
           $raw = [IO.File]::ReadAllBytes($rawPath); \
           $stride = [int][Math]::Ceiling([int]$w * $fmt.BitsPerPixel / 8); \
           $src = [System.Windows.Media.Imaging.BitmapSource]::Create([int]$w, [int]$h, 96, 96, $fmt, $palette, $raw, $stride); \
           $enc = [System.Windows.Media.Imaging.PngBitmapEncoder]::new(); $enc.Interlace = $il; \
           $enc.Frames.Add([System.Windows.Media.Imaging.BitmapFrame]::Create($src)); \
           $fs = [IO.File]::Create($pngPath); $enc.Save($fs); $fs.Close() }}",
		spec_path.display()
	);
	let out = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap();
	assert!(out.status.success(), "PowerShell failed: {}", String::from_utf8_lossy(&out.stderr));

	for (f, w, h, interlace, raw, png_path) in jobs {
		let what = format!("{} ({} colors) {w}x{h} interlace {interlace}", f.name, f.palette);
		let png = std::fs::read(&png_path).unwrap();
		// Make sure WIC wrote the format this case is meant to cover.
		assert_eq!((png[24], png[25], png[28]), (f.png.0, f.png.1, (interlace == "On") as u8), "{what}: IHDR");
		let img = decode(&png, usize::MAX).unwrap_or_else(|e| panic!("{what}: {e}"));
		assert_eq!((img.width, img.height), (w as u32, h as u32), "{what}");
		let (got, want) = (img.to_rgba(), wic_expected(f, &raw, w, h));
		let wrong = (0..w * h).find(|&i| got[i * 4..i * 4 + 4] != want[i * 4..i * 4 + 4]);
		if let Some(i) = wrong {
			panic!("{what}: pixel ({}, {}) is {:?}, should be {:?}", i % w, i / w, &got[i * 4..i * 4 + 4], &want[i * 4..i * 4 + 4]);
		}
	}
	let _ = std::fs::remove_dir_all(&dir);
}
