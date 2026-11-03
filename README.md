# black-png

PNG images with **no third-party dependencies**. Compression comes from
[black-zlib](https://github.com/Brendan-Black/black-zlib), a sibling repo of ours; clone it
next to this one. Works on any platform.

- `encode`: write 8-bit grayscale, gray + alpha, RGB or RGBA pixels as a PNG, choosing
  each row's filter the way libpng does, at black-zlib's `Fast`, `Default` or `Best` level.

```rust
use black_png::{ColorType, Compression};

// A 2x1 image: one red pixel, one blue.
let png = black_png::encode(2, 1, ColorType::Rgb, &[255, 0, 0, 0, 0, 255], Compression::Default)?;
std::fs::write("two.png", png)?;
```

## Tests

`cargo test` runs everywhere. On Windows, `tests/windows_decodes.rs` also loads the PNGs
with Windows' own decoder (GDI+, through PowerShell) and checks every pixel. From WSL,
`winrun --test` builds the tests and runs them there.
