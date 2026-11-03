//! Checks the encoder against another implementation: Windows' own PNG decoder (GDI+,
//! through .NET's System.Drawing in Windows PowerShell). Each PNG written here must load
//! there with every pixel as we wrote it. Only runs on Windows.

#![cfg(windows)]

use std::process::Command;

use black_png::{ColorType, Compression, encode};

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
                if matches!(color, ColorType::GrayAlpha | ColorType::Rgba) && (i as usize + 1) % color.channels() == 0 {
                    v | 0xC0
                } else {
                    v
                }
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
