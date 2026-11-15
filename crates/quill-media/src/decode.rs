//! Turning a picture file into pixels the terminal can be sent.
//!
//! The file is from a message, so it is untrusted: its header may claim any
//! size, and a small file can unpack to gigabytes. The decoder is given
//! limits it may not pass, and a picture larger than the terminal has use
//! for is scaled down before anything keeps it.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{ImageReader, Limits};

use crate::Image;

/// The most pixels a picture may have on a side to be decoded at all.
const MAX_SIDE: u32 = 16_384;
/// The most memory decoding one picture may take.
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
/// A picture this small on either side is a tracking pixel or a spacer,
/// not something to show.
pub const MIN_SIDE: u32 = 24;

/// Decodes a PNG, JPEG, GIF or WebP file and scales it to fit within
/// `max_width` by `max_height` pixels, keeping its shape. An animation
/// gives its first frame.
///
/// `None` for a file that is not one of those, is damaged, is too large to
/// decode within the limits, or is too small to be worth showing.
pub fn decode(bytes: &[u8], max_width: u32, max_height: u32) -> Option<Image> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    if decoded.width() < MIN_SIDE || decoded.height() < MIN_SIDE {
        return None;
    }
    let (max_width, max_height) = (max_width.max(1), max_height.max(1));
    let fitted = if decoded.width() > max_width || decoded.height() > max_height {
        // `resize` keeps the aspect ratio within the box it is given.
        decoded.resize(max_width, max_height, FilterType::Triangle)
    } else {
        decoded
    };
    let rgba = fitted.to_rgba8();
    Image::new(rgba.width(), rgba.height(), rgba.into_raw())
}

/// Whether [`decode`] reads files of this media type.
pub fn can_decode(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    )
}

#[cfg(test)]
mod tests {
    use image::{ImageFormat, Rgba, RgbaImage};

    use super::*;

    /// A picture of `width` by `height` in `format`, with a pixel that
    /// differs from its neighbours so scaling can be seen.
    fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let mut picture = RgbaImage::from_pixel(width, height, Rgba([20, 120, 220, 255]));
        picture.put_pixel(0, 0, Rgba([250, 10, 10, 255]));
        let mut bytes = Vec::new();
        let picture = image::DynamicImage::ImageRgba8(picture);
        // JPEG has no alpha channel.
        let picture = if format == ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(picture.to_rgb8())
        } else {
            picture
        };
        picture
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn each_format_decodes_to_its_own_size() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
        ] {
            let image = decode(&encoded(64, 40, format), 1000, 1000)
                .unwrap_or_else(|| panic!("{format:?} did not decode"));
            assert_eq!((image.width, image.height), (64, 40), "{format:?}");
            assert_eq!(image.rgba.len(), 64 * 40 * 4);
        }
    }

    #[test]
    fn a_large_picture_is_scaled_to_fit_and_keeps_its_shape() {
        let bytes = encoded(800, 400, ImageFormat::Png);
        let image = decode(&bytes, 200, 200).unwrap();
        assert_eq!((image.width, image.height), (200, 100));
        // Limited by height instead.
        let image = decode(&bytes, 1000, 50).unwrap();
        assert_eq!((image.width, image.height), (100, 50));
        // A picture that already fits is not enlarged.
        let image = decode(&encoded(64, 40, ImageFormat::Png), 2000, 2000).unwrap();
        assert_eq!((image.width, image.height), (64, 40));
    }

    #[test]
    fn what_is_not_a_picture_is_refused_without_fuss() {
        assert!(decode(b"", 100, 100).is_none());
        assert!(decode(b"Hi Robin,\n\nSee attached.\n", 100, 100).is_none());
        assert!(decode(b"%PDF-1.4\n", 100, 100).is_none());
        // A PNG cut off part way.
        let bytes = encoded(64, 40, ImageFormat::Png);
        assert!(decode(&bytes[..bytes.len() / 2], 100, 100).is_none());
        // Every prefix of a JPEG, and the file with bytes flipped: no
        // panic, whatever comes back.
        let jpeg = encoded(48, 48, ImageFormat::Jpeg);
        for end in (0..jpeg.len()).step_by(7) {
            let _ = decode(&jpeg[..end], 100, 100);
        }
        for index in (0..jpeg.len()).step_by(5) {
            let mut damaged = jpeg.clone();
            damaged[index] ^= 0xA5;
            let _ = decode(&damaged, 100, 100);
        }
    }

    #[test]
    fn a_header_that_claims_an_enormous_picture_is_not_believed() {
        // A PNG header for 60000 x 60000 pixels with no data to match:
        // refused by the limits, not by running out of memory.
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = Vec::new();
        header.extend_from_slice(b"IHDR");
        header.extend_from_slice(&60_000u32.to_be_bytes());
        header.extend_from_slice(&60_000u32.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        assert!(decode(&bytes, 100, 100).is_none());
    }

    #[test]
    fn a_tracking_pixel_is_not_a_picture_to_show() {
        assert!(decode(&encoded(1, 1, ImageFormat::Gif), 100, 100).is_none());
        assert!(decode(&encoded(600, 2, ImageFormat::Png), 100, 100).is_none());
        assert!(decode(&encoded(MIN_SIDE, MIN_SIDE, ImageFormat::Png), 100, 100).is_some());
    }

    #[test]
    fn only_the_four_formats_are_claimed() {
        for kind in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            assert!(can_decode(kind), "{kind}");
        }
        for kind in [
            "image/heic",
            "image/svg+xml",
            "application/pdf",
            "text/plain",
            "",
        ] {
            assert!(!can_decode(kind), "{kind}");
        }
    }
}
