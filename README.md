# black-png

PNG images with **no third-party dependencies**. Compression comes from
[black-zlib](https://github.com/Brendan-Black/black-zlib), a sibling repo of ours; clone it
next to this one. Works on any platform.

- `encode`: write 8-bit grayscale, gray + alpha, RGB or RGBA pixels as a PNG, choosing
  each row's filter the way libpng does, at black-zlib's `Fast`, `Default` or `Best` level.
- `decode`: read a PNG into an `Image` (width, height, color type, pixels), with `to_rgba`
  for any color type. Checks every chunk's CRC, skips ancillary chunks, and takes a size
  limit so a hostile file can't exhaust memory. So far: 8-bit, not interlaced. Palettes,
  1/2/4/16-bit channels and interlacing come next.

```rust
use black_png::{ColorType, Compression};

// A 2x1 image: one red pixel, one blue.
let png = black_png::encode(2, 1, ColorType::Rgb, &[255, 0, 0, 0, 0, 255], Compression::Default)?;
std::fs::write("two.png", png)?;
let back = black_png::decode(&std::fs::read("two.png")?, 1 << 20)?;
assert_eq!(back.pixels, [255, 0, 0, 0, 0, 255]);
```

## Tests

`cargo test` runs everywhere. On Windows, `tests/windows_crosscheck.rs` also checks against
Windows' own PNG code (GDI+, through PowerShell) both ways: GDI+ must read every pixel we
write, and we must read every pixel GDI+ writes. From WSL, `winrun --test` builds the
tests and runs them there.
