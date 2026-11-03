# black-png

PNG images with **no third-party dependencies**. Compression comes from
[black-zlib](https://github.com/Brendan-Black/black-zlib), a sibling repo of ours; clone it
next to this one. Works on any platform.

- `encode`: write 8-bit grayscale, gray + alpha, RGB or RGBA pixels as a PNG, choosing
  each row's filter the way libpng does, at black-zlib's `Fast`, `Default` or `Best` level.
- `decode`: read any standard PNG into an `Image` (width, height, color type, pixels),
  with `to_rgba` for any color type: grayscale at 1, 2, 4, 8 or 16 bits, RGB, palette,
  gray + alpha and RGBA, at 8 or 16 bits, interlaced (Adam7) or not, with tRNS
  transparency. Pixels always come out as 8 bits per channel: 16-bit channels are rounded,
  low-bit gray is scaled up, palette images become RGB, and tRNS adds an alpha channel.
  Checks every chunk's CRC, skips ancillary chunks (text, gamma, color profiles), and
  takes a size limit so a hostile file can't exhaust memory.

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
Windows' own PNG code through PowerShell: GDI+ must read every pixel we write, and we must
read every pixel GDI+ writes and every format WIC writes (1, 2, 4, 8 and 16-bit gray,
1, 2, 4 and 8-bit palettes with transparency, 8 and 16-bit RGB and RGBA, each interlaced
and not, at several sizes). From WSL, `winrun --test` builds the tests and runs them there.
