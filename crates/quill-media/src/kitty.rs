//! The Kitty graphics protocol: the escape sequences that send an image to
//! the terminal, place it over cells, and remove it again.
//!
//! An image is sent once and kept by the terminal under an id. Showing it is
//! then a short "place" command, which is what a redraw after a scroll or a
//! resize costs. Every command asks the terminal not to reply (`q=2`): a
//! reply would arrive on stdin and be read as typing.

use std::fmt::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::Image;

/// The most base64 text one escape sequence may carry, per the protocol.
const CHUNK: usize = 4096;

/// Sends `image` to the terminal, to be kept under `id`. Draws nothing.
pub fn transmit(id: u32, image: &Image) -> String {
    let payload = STANDARD.encode(&image.rgba);
    let mut out = String::with_capacity(payload.len() + payload.len() / CHUNK * 16 + 64);
    let mut chunks = payload.as_bytes().chunks(CHUNK).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        out.push_str("\x1b_G");
        if first {
            // f=32: raw RGBA. s, v: the size in pixels.
            let _ = write!(
                out,
                "a=t,f=32,s={},v={},i={id},q=2,",
                image.width, image.height
            );
            first = false;
        }
        let _ = write!(out, "m={more};");
        // Base64 is ASCII, so any split of it is valid UTF-8.
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push_str("\x1b\\");
    }
    out
}

/// Draws image `id` with its top left at the cursor, scaled to fill
/// `columns` by `rows` cells. The cursor stays where it was.
pub fn place(id: u32, columns: u16, rows: u16) -> String {
    format!("\x1b_Ga=p,i={id},c={columns},r={rows},C=1,q=2\x1b\\")
}

/// Removes every placement of image `id` from the screen. The terminal keeps
/// the image, so it can be placed again without resending it.
pub fn clear(id: u32) -> String {
    format!("\x1b_Ga=d,d=i,i={id},q=2\x1b\\")
}

/// Removes image `id` from the screen and from the terminal's memory.
pub fn forget(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Whether the terminal is known, from its environment, to draw Kitty
/// graphics. `var` looks up one environment variable.
///
/// This is a stand-in for asking the terminal itself, which arrives with
/// message rendering in M4. Quill's own shell is not detected here; the
/// caller knows when it is hosted.
pub fn supported_by_environment(var: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
    let term = var("TERM").unwrap_or_default();
    let program = var("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
    term.contains("kitty")
        || term.contains("ghostty")
        || program == "ghostty"
        || set("KITTY_WINDOW_ID")
        || set("GHOSTTY_RESOURCES_DIR")
}

/// How many columns an image should span to keep its shape when it is
/// `rows` cells tall, for cells twice as tall as they are wide.
pub fn columns_for(image: &Image, rows: u16) -> u16 {
    let columns = u64::from(rows) * 2 * u64::from(image.width) / u64::from(image.height.max(1));
    u16::try_from(columns.max(1)).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32) -> Image {
        let rgba = (0..width * height * 4).map(|i| (i % 251) as u8).collect();
        Image::new(width, height, rgba).unwrap()
    }

    /// Splits a transmission into `(control, payload)` per escape sequence.
    fn sequences(text: &str) -> Vec<(String, String)> {
        text.split("\x1b\\")
            .filter(|part| !part.is_empty())
            .map(|part| {
                let body = part.strip_prefix("\x1b_G").expect("starts with APC G");
                let (control, payload) = body.split_once(';').expect("has a payload");
                (control.to_owned(), payload.to_owned())
            })
            .collect()
    }

    #[test]
    fn a_small_image_is_one_sequence() {
        let image = image(2, 2);
        let parts = sequences(&transmit(7, &image));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].0, "a=t,f=32,s=2,v=2,i=7,q=2,m=0");
        assert_eq!(STANDARD.decode(&parts[0].1).unwrap(), image.rgba);
    }

    #[test]
    fn a_large_image_is_chunked_and_reassembles_exactly() {
        let image = image(64, 48);
        let text = transmit(3, &image);
        let parts = sequences(&text);
        assert!(parts.len() > 1);
        // Only the first sequence describes the image.
        assert!(parts[0].0.starts_with("a=t,f=32,s=64,v=48,i=3,q=2,m=1"));
        for (control, payload) in &parts[1..parts.len() - 1] {
            assert_eq!(control, "m=1");
            assert_eq!(payload.len(), CHUNK);
        }
        assert_eq!(parts.last().unwrap().0, "m=0");
        for (_, payload) in &parts {
            assert!(payload.len() <= CHUNK);
            // The protocol wants every chunk but the last to be a multiple of four.
            assert!(payload.len() % 4 == 0);
        }
        let joined: String = parts.iter().map(|(_, payload)| payload.as_str()).collect();
        assert_eq!(STANDARD.decode(joined).unwrap(), image.rgba);
    }

    #[test]
    fn an_image_that_fills_a_chunk_exactly_ends_cleanly() {
        // 3072 bytes of pixels are exactly 4096 characters of base64.
        let image = image(32, 24);
        assert_eq!(image.rgba.len(), 3072);
        let parts = sequences(&transmit(1, &image));
        assert_eq!(parts.len(), 1);
        assert!(parts[0].0.ends_with("m=0"));
    }

    #[test]
    fn no_command_asks_for_a_reply() {
        for command in [
            transmit(1, &image(2, 2)),
            place(1, 10, 5),
            clear(1),
            forget(1),
        ] {
            assert!(command.contains("q=2"), "{command:?}");
            assert!(command.starts_with("\x1b_G") && command.ends_with("\x1b\\"));
            // Nothing a terminal without the protocol would print.
            assert!(!command.contains('\n'));
        }
    }

    #[test]
    fn placing_does_not_move_the_cursor_or_resend_pixels() {
        assert_eq!(place(9, 40, 12), "\x1b_Ga=p,i=9,c=40,r=12,C=1,q=2\x1b\\");
    }

    #[test]
    fn clearing_keeps_the_image_and_forgetting_frees_it() {
        assert!(clear(4).contains("d=i,i=4"));
        assert!(forget(4).contains("d=I,i=4"));
    }

    #[test]
    fn the_environment_names_terminals_that_draw_images() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert!(supported_by_environment(env(&[("TERM", "xterm-kitty")])));
        assert!(supported_by_environment(env(&[("TERM", "xterm-ghostty")])));
        assert!(supported_by_environment(env(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "ghostty")
        ])));
        assert!(supported_by_environment(env(&[("KITTY_WINDOW_ID", "1")])));
        assert!(!supported_by_environment(env(&[
            ("TERM", "xterm-256color"),
            ("TERM_PROGRAM", "Apple_Terminal")
        ])));
        assert!(!supported_by_environment(env(&[("KITTY_WINDOW_ID", "")])));
        assert!(!supported_by_environment(env(&[])));
    }

    #[test]
    fn columns_keep_the_shape_for_two_to_one_cells() {
        assert_eq!(columns_for(&image(200, 100), 10), 40);
        assert_eq!(columns_for(&image(100, 100), 10), 20);
        assert_eq!(columns_for(&image(1, 100), 1), 1);
    }
}
