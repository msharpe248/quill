//! How well one colour can be read on another, and the sums for moving a
//! colour until it can be.
//!
//! The measure is the contrast ratio of WCAG 2: the relative luminance of
//! the lighter colour over that of the darker, each with 0.05 added. It
//! runs from 1 (the same colour) to 21 (black on white). A theme Quill
//! makes from someone else's palette ([`super::ghostty`]) is held to three
//! bars:
//!
//! - [`BODY`], 4.5, for what is read as text: the body text, an unread
//!   row, and the text of the selected row on its bar. It is what WCAG asks
//!   of ordinary text (level AA).
//! - [`LABEL`], 3.0, for short coloured words and marks, most of them bold
//!   (dim text, headings, keys, errors, the star, account names) and for
//!   the border of the pane that has the keyboard. It is what WCAG asks of
//!   large text and of the parts of a control.
//! - [`LINE`], 1.5, for what only has to be seen to be there: the borders
//!   of the other panes.
//!
//! White or black always reaches 4.5 on any colour (the worst case is a
//! mid grey, where both have 4.58), so a colour moved toward whichever of
//! the two is further from its background always gets there.

/// A colour as red, green and blue.
pub type Rgb = (u8, u8, u8);

/// The least contrast for what is read as text.
pub const BODY: f64 = 4.5;
/// The least for short coloured words and marks, and the focused border.
pub const LABEL: f64 = 3.0;
/// The least for a line that only has to be seen.
pub const LINE: f64 = 1.5;

pub const WHITE: Rgb = (255, 255, 255);
pub const BLACK: Rgb = (0, 0, 0);

/// The relative luminance of a colour, from 0 (black) to 1 (white), as
/// WCAG 2 defines it for sRGB.
pub fn luminance((red, green, blue): Rgb) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

/// The contrast ratio of two colours, from 1 to 21. The order does not
/// matter.
pub fn contrast(one: Rgb, other: Rgb) -> f64 {
    let (one, other) = (luminance(one), luminance(other));
    let (lighter, darker) = if one >= other {
        (one, other)
    } else {
        (other, one)
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// The colour `share` of the way from `from` to `to`: `from` at 0, `to` at
/// 1, and clamped to those. Each channel is moved by itself.
pub fn mix(from: Rgb, to: Rgb, share: f64) -> Rgb {
    let share = if share.is_finite() {
        share.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let channel = |from: u8, to: u8| {
        let moved = f64::from(from) + (f64::from(to) - f64::from(from)) * share;
        // Within 0 to 255 by construction; the clamp is for the cast.
        moved.round().clamp(0.0, 255.0) as u8
    };
    (
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

/// White or black: whichever stands out more on `ground`.
pub fn pole(ground: Rgb) -> Rgb {
    if contrast(WHITE, ground) >= contrast(BLACK, ground) {
        WHITE
    } else {
        BLACK
    }
}

/// Whether a colour is a dark one to draw on: white stands out on it more
/// than black does.
pub fn is_dark(ground: Rgb) -> bool {
    pole(ground) == WHITE
}

/// `color` if it has at least `need` contrast on `ground`; otherwise the
/// first step from it toward `target`, in eighths of the way, that has;
/// otherwise `target` itself. So as much of the colour is kept as can be
/// read. Whether `target` reaches `need` is the caller's to know.
pub fn toward(color: Rgb, target: Rgb, ground: Rgb, need: f64) -> Rgb {
    (0..8u8)
        .map(|step| mix(color, target, f64::from(step) / 8.0))
        .find(|candidate| contrast(*candidate, ground) >= need)
        .unwrap_or(target)
}

/// The colour between `ground` and `ink` that is nearest to `ground` and
/// still has `need` contrast with it, looked for from `least` of the way
/// in steps of a fiftieth; `ink` itself when nothing short of it has. For
/// a bar or a line drawn on `ground` that should be seen and no more.
pub fn tint(ground: Rgb, ink: Rgb, least: f64, need: f64) -> Rgb {
    let start = (least.clamp(0.0, 1.0) * 50.0).round() as u8;
    (start..50)
        .map(|step| mix(ground, ink, f64::from(step) / 50.0))
        .find(|candidate| contrast(*candidate, ground) >= need)
        .unwrap_or(ink)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(value: f64, wanted: f64) -> bool {
        (value - wanted).abs() < 0.01
    }

    #[test]
    fn luminance_runs_from_black_to_white() {
        assert!(near(luminance(BLACK), 0.0));
        assert!(near(luminance(WHITE), 1.0));
        // The three primaries weigh what WCAG says they weigh.
        assert!(near(luminance((255, 0, 0)), 0.2126));
        assert!(near(luminance((0, 255, 0)), 0.7152));
        assert!(near(luminance((0, 0, 255)), 0.0722));
        // A mid grey is a fifth as bright as white, not half.
        assert!(near(luminance((128, 128, 128)), 0.2159));
    }

    #[test]
    fn contrast_is_the_wcag_ratio_whichever_way_round() {
        assert!(near(contrast(BLACK, WHITE), 21.0));
        assert!(near(contrast(WHITE, BLACK), 21.0));
        assert!(near(contrast((90, 90, 90), (90, 90, 90)), 1.0));
        // Values anyone can check against a contrast checker.
        assert!(near(contrast((0x77, 0x77, 0x77), WHITE), 4.48));
        assert!(near(contrast((0x76, 0x76, 0x76), WHITE), 4.54));
        assert!(near(contrast((0xff, 0x00, 0x00), WHITE), 4.0));
        assert!(near(contrast((0x00, 0x00, 0xff), WHITE), 8.59));
    }

    #[test]
    fn mixing_goes_from_one_colour_to_the_other_and_no_further() {
        let (from, to) = ((0, 100, 255), (200, 0, 55));
        assert_eq!(mix(from, to, 0.0), from);
        assert_eq!(mix(from, to, 1.0), to);
        assert_eq!(mix(from, to, 0.5), (100, 50, 155));
        assert_eq!(mix(from, to, -3.0), from);
        assert_eq!(mix(from, to, 7.0), to);
        assert_eq!(mix(from, to, f64::NAN), from);
        assert_eq!(mix(from, to, f64::INFINITY), from);
    }

    #[test]
    fn white_or_black_always_reads_as_text() {
        // On every grey, which is where the least is, and so on anything.
        for level in 0..=255u8 {
            let ground = (level, level, level);
            assert!(contrast(pole(ground), ground) >= BODY, "{level}");
        }
        assert_eq!(pole(BLACK), WHITE);
        assert_eq!(pole(WHITE), BLACK);
        assert!(is_dark((0x28, 0x2a, 0x36)));
        assert!(!is_dark((0xfd, 0xf6, 0xe3)));
    }

    #[test]
    fn a_colour_is_kept_when_it_reads_and_moved_only_as_far_as_it_must_be() {
        let ground = (0x00, 0x2b, 0x36);
        // Readable as it is: untouched.
        let yellow = (0xb5, 0x89, 0x00);
        assert_eq!(toward(yellow, WHITE, ground, LABEL), yellow);
        // The ground itself is not: it is moved toward the target until it
        // is, and no further than that.
        let moved = toward(ground, WHITE, ground, LABEL);
        assert!(contrast(moved, ground) >= LABEL);
        assert_ne!(moved, WHITE);
        let step_before = (1..8u8)
            .map(|step| mix(ground, WHITE, f64::from(step) / 8.0))
            .take_while(|candidate| *candidate != moved)
            .last();
        assert!(step_before.is_none_or(|before| contrast(before, ground) < LABEL));
        // What cannot be reached short of the target is the target.
        assert_eq!(toward(ground, WHITE, ground, 14.0), WHITE);
    }

    #[test]
    fn a_tint_is_the_faintest_that_is_still_seen() {
        for (ground, ink) in [
            (BLACK, WHITE),
            (WHITE, BLACK),
            ((0x28, 0x2a, 0x36), (0xf8, 0xf8, 0xf2)),
            ((0xfd, 0xf6, 0xe3), (0x65, 0x7b, 0x83)),
        ] {
            let line = tint(ground, ink, 0.1, LINE);
            let seen = contrast(line, ground);
            assert!((LINE..LINE + 0.4).contains(&seen), "{ground:?}: {seen}");
        }
        // Nothing between them is enough: the ink itself.
        assert_eq!(tint(BLACK, (40, 40, 40), 0.1, 3.0), (40, 40, 40));
    }
}
