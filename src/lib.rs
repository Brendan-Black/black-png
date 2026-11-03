//! PNG images, with no third-party dependencies. Compression comes from black-zlib, a
//! sibling crate of ours.
//!
//! - [`encode`] writes 8-bit grayscale, gray + alpha, RGB or RGBA pixels as a PNG.
//! - [`decode`] reads any standard PNG into an [`Image`] of 8-bit channels.
//!
//! ```
//! use black_png::{ColorType, Compression};
//!
//! // A 2x1 image: one red pixel, one blue.
//! let png = black_png::encode(2, 1, ColorType::Rgb, &[255, 0, 0, 0, 0, 255], Compression::Default)?;
//! assert!(png.starts_with(b"\x89PNG"));
//! assert_eq!(black_png::decode(&png, 1 << 20)?.pixels, [255, 0, 0, 0, 0, 255]);
//! # Ok::<(), black_png::Error>(())
//! ```

mod decode;
mod encode;
mod error;
mod filter;

pub use black_zlib::Compression;
pub use decode::{Image, decode};
pub use encode::encode;
pub use error::{Error, Result};

/// Every PNG starts with these 8 bytes.
pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

/// How each pixel is laid out: 8 bits per channel, channels in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorType {
    /// One byte per pixel.
    Gray,
    /// Gray, then alpha.
    GrayAlpha,
    /// Red, green, blue.
    Rgb,
    /// Red, green, blue, alpha.
    Rgba,
}

impl ColorType {
    /// Bytes per pixel.
    pub fn channels(self) -> usize {
        match self {
            ColorType::Gray => 1,
            ColorType::GrayAlpha => 2,
            ColorType::Rgb => 3,
            ColorType::Rgba => 4,
        }
    }

    /// The color type number in a PNG header.
    fn code(self) -> u8 {
        match self {
            ColorType::Gray => 0,
            ColorType::Rgb => 2,
            ColorType::GrayAlpha => 4,
            ColorType::Rgba => 6,
        }
    }
}
