//! Images in the terminal.
//!
//! Quill draws images with the Kitty graphics protocol and nothing else: its
//! own shell's terminal always has it, so there are no iTerm2, Sixel or
//! half-block code paths. [`decode`] turns a picture from a message into
//! pixels; pictures it cannot read (HEIC, a PDF's first page, a video's
//! poster frame) are made into PNGs by the app shell first.

pub mod decode;
pub mod kitty;

/// Pixels, eight bits each of red, green, blue and alpha, row by row from the
/// top left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// `None` when `rgba` is not exactly `width * height * 4` bytes, or the
    /// image has no pixels.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (expected > 0 && rgba.len() == expected).then_some(Self {
            width,
            height,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_must_have_exactly_its_pixels() {
        assert!(Image::new(2, 2, vec![0; 16]).is_some());
        assert!(Image::new(2, 2, vec![0; 15]).is_none());
        assert!(Image::new(0, 2, Vec::new()).is_none());
        assert!(Image::new(u32::MAX, u32::MAX, Vec::new()).is_none());
    }
}
