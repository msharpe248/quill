//! Themes written for the Ghostty terminal, read as Quill themes.
//!
//! A Ghostty theme is a few lines of `key = value`: a background, a
//! foreground, the sixteen colours a terminal program names, and the
//! colours of the cursor and the selection.
//!
//! ```text
//! palette = 0=#21222c
//! palette = 4=#bd93f9
//! background = #282a36
//! foreground = #f8f8f2
//! selection-background = #44475a
//! selection-foreground = #ffffff
//! ```
//!
//! It is read liberally ([`parse`]): blank lines and `#` comments are
//! skipped, so is any key Quill has no use for and any colour it cannot
//! read, and a colour is `#rrggbb`, `rrggbb`, `#rgb` or `rgb`, with or
//! without quotes. A file is refused only when it is too large, or names no
//! background and foreground at all: then it is not a theme.
//!
//! A theme file comes from somewhere else. Nothing in it is run, included
//! or looked up: it is a list of colours, and whatever is not one is
//! ignored.
//!
//! Quill draws with more colours than a terminal theme has, so the rest
//! are worked out ([`Ghostty::mapped`]). The palette goes to the roles the
//! `terminal` theme gives it, so Quill looks as it does in a Ghostty of
//! that theme: blue for the focused border, cyan for headings, magenta for
//! keys, red for errors, yellow for the star, and blue, green, yellow and
//! magenta for the accounts. What that would leave unreadable is put right,
//! by the measure in [`super::contrast`]:
//!
//! - Body text is the foreground, moved toward white or black if it has
//!   less than 4.5 on the background.
//! - Dim text is the text two fifths of the way to the background, or
//!   nearer the text where that has less than 3. Not palette 8: in many
//!   themes (Solarized among them) that is the background itself.
//! - A coloured role takes its palette colour if that has 3 on the
//!   background, otherwise the bright one of the pair, otherwise the colour
//!   moved toward the text until it has.
//! - The selected row of the pane with the keyboard is the theme's own
//!   selection: its background as the bar, unless that cannot be told from
//!   the background, and its foreground on it, unless that has less than
//!   4.5 there (then the text, the background, or white or black).
//! - The selected row of a pane without the keyboard keeps each word's own
//!   colour, so its bar is a faint tint of the background that the text
//!   still has 3 on. A theme's selection is made to be read with the
//!   selection's own foreground, and often cannot be read with any other.
//! - Borders are tints of the background that can be seen (1.5).

use super::contrast::{self, BODY, LABEL, LINE, Rgb, contrast, mix, pole, tint, toward};
use super::{ColorSpec, Colors, Theme};

/// A theme file larger than this is not read. One of Ghostty's own is
/// under half a kilobyte.
pub const MAX_BYTES: usize = 64 * 1024;
/// Nor one with more lines than this.
pub const MAX_LINES: usize = 2_000;

/// The contrast the bar of a selected row has with the background, at
/// least: enough to be seen, and little enough that the text is still read
/// on it.
const BAR: f64 = 1.25;
/// The least a theme's own selection must differ from the background to
/// be used as the bar.
const BAR_SEEN: f64 = 1.15;

/// What a Ghostty theme says, as far as Quill uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ghostty {
    pub background: Rgb,
    pub foreground: Rgb,
    /// The sixteen colours a terminal program names, 0 to 15. One the file
    /// leaves out is `None`.
    pub palette: [Option<Rgb>; 16],
    pub selection_background: Option<Rgb>,
    pub selection_foreground: Option<Rgb>,
}

/// A colour as a Ghostty theme writes it: six hexadecimal digits or three,
/// with or without `#`. A colour by name is not read.
pub fn color(text: &str) -> Option<Rgb> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = |range: std::ops::Range<usize>| {
        digits
            .get(range)
            .and_then(|part| u8::from_str_radix(part, 16).ok())
    };
    match digits.len() {
        6 => Some((value(0..2)?, value(2..4)?, value(4..6)?)),
        // `#abc` is `#aabbcc`.
        3 => Some((value(0..1)? * 17, value(1..2)? * 17, value(2..3)? * 17)),
        _ => None,
    }
}

/// A value with the quotes Ghostty allows round it taken off.
fn unquoted(value: &str) -> &str {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value)
        .trim()
}

/// Reads a theme in Ghostty's format. `Err` says why the text is not one,
/// in words that follow the file's name: "is larger than 64 KiB".
pub fn parse(text: &str) -> Result<Ghostty, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("is larger than {} KiB", MAX_BYTES / 1024));
    }
    let mut background = None;
    let mut foreground = None;
    let mut selection_background = None;
    let mut selection_foreground = None;
    let mut palette = [None; 16];
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_LINES {
            return Err(format!("has more than {MAX_LINES} lines"));
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unquoted(value);
        // A later line wins over an earlier one, as in Ghostty.
        match key.trim() {
            "background" => background = color(value).or(background),
            "foreground" => foreground = color(value).or(foreground),
            "selection-background" => {
                selection_background = color(value).or(selection_background);
            }
            "selection-foreground" => {
                selection_foreground = color(value).or(selection_foreground);
            }
            "palette" => {
                // `N=COLOUR`. Ghostty has 256 of them; Quill uses sixteen.
                let entry = value.split_once('=').and_then(|(place, shade)| {
                    let place: usize = place.trim().parse().ok()?;
                    Some((place, color(unquoted(shade))?))
                });
                if let Some((place, shade)) = entry
                    && let Some(slot) = palette.get_mut(place)
                {
                    *slot = Some(shade);
                }
            }
            // The cursor's colours, and whatever else a theme may say: no
            // use to Quill.
            _ => {}
        }
    }
    match (background, foreground) {
        (Some(background), Some(foreground)) => Ok(Ghostty {
            background,
            foreground,
            palette,
            selection_background,
            selection_foreground,
        }),
        _ => Err(
            "is not a theme in Ghostty\u{2019}s format: it names no background and foreground \
             colour"
                .to_owned(),
        ),
    }
}

/// Something Quill draws in a colour of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Text,
    Dim,
    BorderFocused,
    /// The bar of the selected row, in the pane with the keyboard.
    SelectionBar,
    /// The text on that bar.
    SelectionText,
    Starred,
    Accent,
    Key,
    Error,
    /// The account at this place, from 0.
    Account(usize),
}

/// A Ghostty theme worked out as the colours Quill draws with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub background: Rgb,
    pub text: Rgb,
    pub dim: Rgb,
    pub border: Rgb,
    pub border_focused: Rgb,
    pub selection_fg: Rgb,
    pub selection_bg: Rgb,
    /// The bar of the selected row in a pane without the keyboard.
    pub selection_inactive_bg: Rgb,
    pub unread: Rgb,
    pub starred: Rgb,
    pub accent: Rgb,
    pub key: Rgb,
    pub error: Rgb,
    pub accounts: [Rgb; 4],
    /// The roles that could not have the colour the theme has for them,
    /// because it could not be read, and have another in its place.
    pub fallbacks: Vec<Role>,
}

/// The places in the palette of the colours Quill has roles for, each with
/// its bright one eight places on.
const RED: usize = 1;
const GREEN: usize = 2;
const YELLOW: usize = 3;
const BLUE: usize = 4;
const MAGENTA: usize = 5;
const CYAN: usize = 6;
const WHITE_PLACE: usize = 7;
const BLACK_PLACE: usize = 0;
const BRIGHT: usize = 8;

/// The accounts' colours, in order: as the `terminal` theme has them.
const ACCOUNTS: [usize; 4] = [BLUE, GREEN, YELLOW, MAGENTA];

impl Ghostty {
    fn at(&self, place: usize) -> Option<Rgb> {
        self.palette.get(place).copied().flatten()
    }

    /// Whether it is a dark theme: one whose background white stands out
    /// on more than black.
    pub fn is_dark(&self) -> bool {
        contrast::is_dark(self.background)
    }

    /// The colours Quill draws with, worked out from the theme's. See the
    /// module's own words for the rules.
    pub fn mapped(&self) -> Mapped {
        let ground = self.background;
        let mut fallbacks = Vec::new();

        // Body text: the foreground, strengthened if it must be.
        let text = toward(self.foreground, pole(ground), ground, BODY);
        if text != self.foreground {
            fallbacks.push(Role::Text);
        }

        // Dim text: toward the background, and back toward the text as far
        // as it takes to be read.
        let wished = mix(text, ground, 0.4);
        let dim = toward(wished, text, ground, LABEL);
        if dim != wished {
            fallbacks.push(Role::Dim);
        }

        // A coloured word: the palette's colour, its bright one, or the
        // nearest to it that can be read.
        let mut label = |place: usize, role: Role| -> Rgb {
            let plain = self.at(place);
            let bright = self.at(place + BRIGHT);
            if let Some(plain) = plain
                && contrast(plain, ground) >= LABEL
            {
                return plain;
            }
            fallbacks.push(role);
            if let Some(bright) = bright
                && contrast(bright, ground) >= LABEL
            {
                return bright;
            }
            match plain.or(bright) {
                Some(shade) => toward(shade, text, ground, LABEL),
                None => text,
            }
        };
        let border_focused = label(BLUE, Role::BorderFocused);
        let starred = label(YELLOW, Role::Starred);
        let accent = label(CYAN, Role::Accent);
        let key = label(MAGENTA, Role::Key);
        let error = label(RED, Role::Error);
        let mut accounts = [text; 4];
        for (index, place) in ACCOUNTS.into_iter().enumerate() {
            accounts[index] = label(place, Role::Account(index));
        }

        // An unread row: the strongest of the theme's own whites and
        // blacks, where one is stronger than the text.
        let unread = [
            self.at(WHITE_PLACE + BRIGHT),
            self.at(WHITE_PLACE),
            self.at(BLACK_PLACE),
        ]
        .into_iter()
        .flatten()
        .fold(text, |best, shade| {
            if contrast(shade, ground) > contrast(best, ground) {
                shade
            } else {
                best
            }
        });

        // The selected row where the keyboard is: the theme's selection.
        let selection_bg = match self.selection_background {
            Some(bar) if contrast(bar, ground) >= BAR_SEEN => bar,
            _ => {
                fallbacks.push(Role::SelectionBar);
                tint(ground, text, 0.3, BAR_SEEN)
            }
        };
        let reads = |shade: &Rgb| contrast(*shade, selection_bg) >= BODY;
        let selection_fg = match self.selection_foreground.filter(reads) {
            Some(own) => own,
            None => {
                fallbacks.push(Role::SelectionText);
                let better = if contrast(text, selection_bg) >= contrast(ground, selection_bg) {
                    text
                } else {
                    ground
                };
                Some(better)
                    .filter(reads)
                    .unwrap_or_else(|| pole(selection_bg))
            }
        };

        Mapped {
            background: ground,
            text,
            dim,
            border: tint(ground, text, 0.1, LINE),
            border_focused,
            selection_fg,
            selection_bg,
            selection_inactive_bg: tint(ground, text, 0.06, BAR),
            unread,
            starred,
            accent,
            key,
            error,
            accounts,
            fallbacks,
        }
    }

    /// The theme as a Quill theme called `name`.
    pub fn theme(&self, name: &str) -> Theme {
        let mapped = self.mapped();
        let rgb = |(red, green, blue): Rgb| ColorSpec::Rgb(red, green, blue);
        Theme {
            name: name.to_owned(),
            colors: Colors {
                background: rgb(mapped.background),
                text: rgb(mapped.text),
                dim: rgb(mapped.dim),
                border: rgb(mapped.border),
                border_focused: rgb(mapped.border_focused),
                selection_fg: rgb(mapped.selection_fg),
                selection_bg: rgb(mapped.selection_bg),
                selection_inactive_bg: Some(rgb(mapped.selection_inactive_bg)),
                unread: rgb(mapped.unread),
                starred: rgb(mapped.starred),
                accent: rgb(mapped.accent),
                key: rgb(mapped.key),
                error: rgb(mapped.error),
                accounts: mapped.accounts.into_iter().map(rgb).collect(),
            },
        }
    }
}

impl Mapped {
    /// Every pair of colours that is read as text, with the least contrast
    /// it is held to: what is drawn, on what it is drawn on. The roles
    /// that keep their own colour on the bar of an unfocused selected row
    /// are held to the bar as well, where they are the row's words.
    pub fn read_as_text(&self) -> Vec<(&'static str, Rgb, Rgb, f64)> {
        let mut pairs = vec![
            ("text", self.text, self.background, BODY),
            ("unread", self.unread, self.background, BODY),
            (
                "the selected row",
                self.selection_fg,
                self.selection_bg,
                BODY,
            ),
            ("dim", self.dim, self.background, LABEL),
            ("starred", self.starred, self.background, LABEL),
            ("accent", self.accent, self.background, LABEL),
            ("key", self.key, self.background, LABEL),
            ("error", self.error, self.background, LABEL),
            (
                "text on an unfocused selected row",
                self.text,
                self.selection_inactive_bg,
                LABEL,
            ),
            (
                "unread on an unfocused selected row",
                self.unread,
                self.selection_inactive_bg,
                LABEL,
            ),
        ];
        for account in self.accounts {
            pairs.push(("an account", account, self.background, LABEL));
        }
        pairs
    }

    /// Every pair that is not text and still has to be seen: the borders,
    /// and the bars against the background.
    pub fn seen(&self) -> Vec<(&'static str, Rgb, Rgb, f64)> {
        vec![
            (
                "the focused border",
                self.border_focused,
                self.background,
                LABEL,
            ),
            ("a border", self.border, self.background, LINE),
            (
                "the selected row\u{2019}s bar",
                self.selection_bg,
                self.background,
                BAR_SEEN,
            ),
            (
                "an unfocused selected row\u{2019}s bar",
                self.selection_inactive_bg,
                self.background,
                BAR,
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark theme, written here: the shape of one of Ghostty's.
    const NIGHT: &str = "\
palette = 0=#1b1d2a
palette = 1=#f0606a
palette = 2=#6fd08c
palette = 3=#e8c46a
palette = 4=#7aa0f0
palette = 5=#c58af0
palette = 6=#6fd0e0
palette = 7=#c8cce0
palette = 8=#50546a
palette = 9=#ff7a84
palette = 10=#8be8a6
palette = 11=#ffe08a
palette = 12=#9ab8ff
palette = 13=#dba6ff
palette = 14=#8ae8f6
palette = 15=#ffffff
background = #1e2030
foreground = #d0d4ea
cursor-color = #d0d4ea
cursor-text = #1e2030
selection-background = #3a3f5c
selection-foreground = #ffffff
";

    /// A light one whose palette was made for a dark background, whose
    /// bright black is its background, and whose selection is the text
    /// turned round.
    const PAPER: &str = "\
palette = 0=#f4efe2
palette = 1=#f2a0a0
palette = 2=#a8e0a0
palette = 3=#f0e090
palette = 4=#a0c0f0
palette = 5=#e0a8e8
palette = 6=#a0e0e8
palette = 7=#5a6068
palette = 8=#fbf6e8
palette = 9=#c0392b
palette = 10=#2e7d32
palette = 11=#9a6a00
palette = 12=#2a5db0
palette = 13=#8e3a9d
palette = 14=#1a7f8c
palette = 15=#20242a
background = #fbf6e8
foreground = #5a6068
selection-background = #5a6068
selection-foreground = #fbf6e8
";

    fn rgb(spec: ColorSpec) -> Rgb {
        match spec {
            ColorSpec::Rgb(red, green, blue) => (red, green, blue),
            other => panic!("{other:?} is not a fixed colour"),
        }
    }

    #[test]
    fn a_colour_is_six_digits_or_three_with_or_without_the_hash() {
        assert_eq!(color("#282a36"), Some((0x28, 0x2a, 0x36)));
        assert_eq!(color("282A36"), Some((0x28, 0x2a, 0x36)));
        assert_eq!(color("#fa0"), Some((0xff, 0xaa, 0x00)));
        assert_eq!(color("FA0"), Some((0xff, 0xaa, 0x00)));
        for not in [
            "",
            "#",
            "#12345",
            "#1234567",
            "12",
            "#ggg",
            "red",
            "#12 345",
            "##123456",
            "\u{661}\u{662}\u{663}",
            "+12345",
            "#-1-1-1",
        ] {
            assert_eq!(color(not), None, "{not:?}");
        }
    }

    #[test]
    fn a_theme_of_ghosttys_is_read() {
        let theme = parse(NIGHT).unwrap();
        assert_eq!(theme.background, (0x1e, 0x20, 0x30));
        assert_eq!(theme.foreground, (0xd0, 0xd4, 0xea));
        assert_eq!(theme.palette[0], Some((0x1b, 0x1d, 0x2a)));
        assert_eq!(theme.palette[4], Some((0x7a, 0xa0, 0xf0)));
        assert_eq!(theme.palette[15], Some((0xff, 0xff, 0xff)));
        assert!(theme.palette.iter().all(Option::is_some));
        assert_eq!(theme.selection_background, Some((0x3a, 0x3f, 0x5c)));
        assert_eq!(theme.selection_foreground, Some((0xff, 0xff, 0xff)));
        assert!(theme.is_dark());
        assert!(!parse(PAPER).unwrap().is_dark());
    }

    #[test]
    fn odd_spacing_comments_quotes_and_other_keys_are_read_through() {
        let theme = parse(
            "# A theme of my own\r\n\
             \r\n\
             \tbackground=#102030   \r\n\
             foreground   =   \"d0d0d0\"\n\
             palette=1=#f00\n\
             palette = 4 = \"#0000ff\"\n\
             palette = 200=#123456\n\
             palette = 16=#123456\n\
             font-family = Menlo\n\
             cursor-color = #ffffff\n\
             cursor-style = block\n\
             keybind = ctrl+a=reload_config\n\
             a line with no equals sign\n\
             = #ffffff\n\
             # background = #ffffff\n",
        )
        .unwrap();
        assert_eq!(theme.background, (0x10, 0x20, 0x30));
        assert_eq!(theme.foreground, (0xd0, 0xd0, 0xd0));
        assert_eq!(theme.palette[1], Some((0xff, 0x00, 0x00)));
        assert_eq!(theme.palette[4], Some((0x00, 0x00, 0xff)));
        // Places past the sixteen are Ghostty's and not Quill's.
        assert_eq!(theme.palette.iter().flatten().count(), 2);
        assert_eq!(
            (theme.selection_background, theme.selection_foreground),
            (None, None)
        );
    }

    #[test]
    fn a_colour_that_cannot_be_read_is_skipped_and_a_later_line_wins() {
        let theme = parse(
            "background = #000000\n\
             background = papayawhip\n\
             foreground = #111111\n\
             foreground = #eeeeee\n\
             palette = 1=crimson\n\
             palette = two=#ff0000\n\
             palette = -1=#ff0000\n\
             palette = 3\n\
             selection-background = #12\n",
        )
        .unwrap();
        // A name is not read, and what was said before it stands.
        assert_eq!(theme.background, (0, 0, 0));
        assert_eq!(theme.foreground, (0xee, 0xee, 0xee));
        assert!(theme.palette.iter().all(Option::is_none));
        assert_eq!(theme.selection_background, None);
    }

    #[test]
    fn what_is_not_a_theme_is_refused_and_says_why() {
        for (text, why) in [
            ("", "names no background and foreground"),
            ("\n\n# nothing here\n", "names no background and foreground"),
            (
                "background = #000000\n",
                "names no background and foreground",
            ),
            (
                "foreground = #ffffff\n",
                "names no background and foreground",
            ),
            (
                "palette = 0=#000000\npalette = 1=#ff0000\n",
                "names no background",
            ),
            // One of Quill's own, in TOML: not Ghostty's format.
            (
                "name = \"mine\"\n[colors]\nbackground = \"#1a1b26\"\ntext = \"#c0caf5\"\n",
                "names no background and foreground",
            ),
            (
                "\u{0}\u{1}\u{2}\u{ff}binary = junk\n",
                "names no background",
            ),
            (
                "<html><body>background = red</body></html>",
                "names no background",
            ),
        ] {
            let refused = parse(text).unwrap_err();
            assert!(refused.contains(why), "{text:?}: {refused}");
        }
        // Too large, however good the start of it is.
        let mut large = String::from(NIGHT);
        large.push_str(&"#\n".repeat(MAX_BYTES / 2));
        assert_eq!(parse(&large).unwrap_err(), "is larger than 64 KiB");
        // Too many lines, within the size.
        let long = format!("{NIGHT}{}", "\n".repeat(MAX_LINES));
        assert!(long.len() <= MAX_BYTES);
        assert_eq!(parse(&long).unwrap_err(), "has more than 2000 lines");
        // Exactly at the limits is read.
        let most = format!("{NIGHT}{}", "\n".repeat(MAX_LINES - NIGHT.lines().count()));
        assert!(parse(&most).is_ok());
    }

    #[test]
    fn nothing_in_a_theme_makes_the_reader_panic() {
        // Every prefix of a theme, cut anywhere, a character at a time;
        // and lines made of the pieces a real line is made of.
        for end in 0..=NIGHT.len() {
            if NIGHT.is_char_boundary(end) {
                let _ = parse(&NIGHT[..end]);
            }
        }
        let pieces = [
            "palette",
            "=",
            "#",
            "0",
            "255",
            "99999999999999999999",
            "ffffff",
            "\"",
            " ",
            "\t",
            "é",
            "background",
            "-",
        ];
        for first in pieces {
            for second in pieces {
                for third in pieces {
                    let _ = parse(&format!(
                        "{first}{second}{third}\n{first} = {second}={third}\n"
                    ));
                }
            }
        }
    }

    #[test]
    fn the_palette_goes_to_the_roles_the_terminal_theme_gives_it() {
        let read = parse(NIGHT).unwrap();
        let mapped = read.mapped();
        // Every colour of this theme can be read as it is.
        assert_eq!(mapped.fallbacks, []);
        assert_eq!(mapped.background, read.background);
        assert_eq!(mapped.text, read.foreground);
        assert_eq!(Some(mapped.border_focused), read.palette[4]);
        assert_eq!(Some(mapped.accent), read.palette[6]);
        assert_eq!(Some(mapped.key), read.palette[5]);
        assert_eq!(Some(mapped.error), read.palette[1]);
        assert_eq!(Some(mapped.starred), read.palette[3]);
        assert_eq!(
            mapped.accounts.map(Some),
            [
                read.palette[4],
                read.palette[2],
                read.palette[3],
                read.palette[5]
            ]
        );
        // The selected row is the theme's own selection.
        assert_eq!(Some(mapped.selection_bg), read.selection_background);
        assert_eq!(Some(mapped.selection_fg), read.selection_foreground);
        // An unread row is the theme's brightest white.
        assert_eq!(mapped.unread, (0xff, 0xff, 0xff));
        // Dim text is between the text and the background, and is not
        // palette 8.
        assert_eq!(mapped.dim, mix(mapped.text, mapped.background, 0.4));
        assert_ne!(Some(mapped.dim), read.palette[8]);
        for (what, ink, ground, need) in mapped.read_as_text().into_iter().chain(mapped.seen()) {
            assert!(contrast(ink, ground) >= need, "{what}");
        }
    }

    #[test]
    fn what_could_not_be_read_is_given_a_colour_that_can() {
        let read = parse(PAPER).unwrap();
        let mapped = read.mapped();
        for (what, ink, ground, need) in mapped.read_as_text().into_iter().chain(mapped.seen()) {
            assert!(
                contrast(ink, ground) >= need,
                "{what}: {:.2}",
                contrast(ink, ground)
            );
        }
        // The pale colours of each pair cannot be read on paper: the
        // bright one of the pair, which is the dark one here, is used.
        assert_eq!(Some(mapped.border_focused), read.palette[12]);
        assert_eq!(Some(mapped.accent), read.palette[14]);
        assert_eq!(Some(mapped.key), read.palette[13]);
        assert_eq!(Some(mapped.error), read.palette[9]);
        assert_eq!(Some(mapped.starred), read.palette[11]);
        assert_eq!(
            mapped.accounts.map(Some),
            [
                read.palette[12],
                read.palette[10],
                read.palette[11],
                read.palette[13]
            ]
        );
        for role in [
            Role::BorderFocused,
            Role::Accent,
            Role::Key,
            Role::Error,
            Role::Starred,
            Role::Account(0),
            Role::Account(3),
        ] {
            assert!(mapped.fallbacks.contains(&role), "{role:?}");
        }
        // The text is as the theme has it: it could be read.
        assert_eq!(mapped.text, read.foreground);
        assert!(!mapped.fallbacks.contains(&Role::Text));
        // Dim text is not bright black, which here is the background.
        assert_eq!(read.palette[8], Some(read.background));
        assert!(contrast(mapped.dim, mapped.background) >= LABEL);
        // The selection is the text turned round: used as it is where the
        // keyboard is, where its own foreground is drawn on it.
        assert_eq!(mapped.selection_bg, read.foreground);
        assert_eq!(mapped.selection_fg, read.background);
        // And not where each word keeps its own colour: there the bar is
        // a tint the text can be read on.
        assert_ne!(mapped.selection_inactive_bg, mapped.selection_bg);
        assert!(contrast(mapped.text, mapped.selection_inactive_bg) >= LABEL);
        // An unread row is the darkest the theme has.
        assert_eq!(mapped.unread, (0x20, 0x24, 0x2a));
    }

    #[test]
    fn a_theme_with_gaps_or_no_contrast_at_all_still_comes_out_readable() {
        for text in [
            // Nothing but the two colours.
            "background = #202020\nforeground = #e0e0e0\n",
            // A foreground too close to the background to read.
            "background = #303030\nforeground = #484848\n",
            // The same colour for both.
            "background = #808080\nforeground = #808080\n",
            "background = #000000\nforeground = #000000\n",
            "background = #ffffff\nforeground = #ffffff\n",
            // A selection that is the background, with text that is too.
            "background = #101010\nforeground = #f0f0f0\nselection-background = #101010\n\
             selection-foreground = #101010\n",
            // A selection whose foreground cannot be read on it.
            "background = #101010\nforeground = #f0f0f0\nselection-background = #707070\n\
             selection-foreground = #808080\n",
            // Every colour of the palette the background itself.
            "background = #204060\nforeground = #c0d0e0\npalette = 1=#204060\npalette = 9=#204060\n\
             palette = 4=#204060\npalette = 6=#214161\n",
        ] {
            let mapped = parse(text).unwrap().mapped();
            for (what, ink, ground, need) in mapped.read_as_text().into_iter().chain(mapped.seen())
            {
                assert!(
                    contrast(ink, ground) >= need,
                    "{text:?}: {what} has {:.2}",
                    contrast(ink, ground)
                );
            }
        }
        // Text that had to be strengthened is said to have been, and keeps
        // what it can of its colour.
        let weak = parse("background = #101820\nforeground = #2a5a3a\n")
            .unwrap()
            .mapped();
        assert!(weak.fallbacks.contains(&Role::Text));
        assert!(
            weak.text.1 > weak.text.0 && weak.text.1 > weak.text.2,
            "{:?}",
            weak.text
        );
        // A role with no colour in the palette is the text's colour.
        let bare = parse("background = #202020\nforeground = #e0e0e0\n")
            .unwrap()
            .mapped();
        assert_eq!(bare.error, bare.text);
        assert_eq!(bare.accounts, [bare.text; 4]);
        assert_eq!(bare.unread, bare.text);
    }

    #[test]
    fn it_becomes_a_quill_theme_of_fixed_colours_under_the_name_given() {
        let theme = parse(NIGHT).unwrap().theme("Night Owl+");
        assert_eq!(theme.name, "Night Owl+");
        let colors = &theme.colors;
        assert_eq!(rgb(colors.background), (0x1e, 0x20, 0x30));
        assert_eq!(rgb(colors.text), (0xd0, 0xd4, 0xea));
        assert_eq!(rgb(colors.selection_bg), (0x3a, 0x3f, 0x5c));
        assert_eq!(colors.accounts.len(), 4);
        let inactive = colors.selection_inactive_bg.map(rgb).unwrap();
        assert_ne!(inactive, rgb(colors.selection_bg));
        // Nothing is left to the terminal: the theme is the same in any.
        let mut all = vec![
            colors.background,
            colors.text,
            colors.dim,
            colors.border,
            colors.border_focused,
            colors.selection_fg,
            colors.selection_bg,
            colors.unread,
            colors.starred,
            colors.accent,
            colors.key,
            colors.error,
        ];
        all.extend(colors.accounts.iter().copied());
        for spec in all {
            rgb(spec);
        }
    }

    /// Ghostty's own themes, where Ghostty is installed. Read, and never
    /// copied or changed.
    const INSTALLED: &str = "/Applications/Ghostty.app/Contents/Resources/ghostty/themes";

    /// Every theme Ghostty ships comes out readable: each pair that is
    /// read as text has its contrast, and each line and bar can be seen.
    /// Skipped where Ghostty is not installed.
    #[test]
    fn every_installed_ghostty_theme_comes_out_readable() {
        let Ok(folder) = std::fs::read_dir(INSTALLED) else {
            eprintln!("skipped: no Ghostty themes at {INSTALLED}");
            return;
        };
        let mut files: Vec<_> = folder
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        let (mut read, mut with_fallbacks, mut dark) = (0usize, 0usize, 0usize);
        let mut by_role = std::collections::BTreeMap::<String, usize>::new();
        let mut failures = Vec::new();
        for path in &files {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(path).unwrap();
            let theme = match parse(&text) {
                Ok(theme) => theme,
                Err(why) => {
                    failures.push(format!("{name} {why}"));
                    continue;
                }
            };
            read += 1;
            dark += usize::from(theme.is_dark());
            let mapped = theme.mapped();
            if !mapped.fallbacks.is_empty() {
                with_fallbacks += 1;
            }
            for role in &mapped.fallbacks {
                let role = match role {
                    Role::Account(_) => "Account".to_owned(),
                    other => format!("{other:?}"),
                };
                *by_role.entry(role).or_default() += 1;
            }
            for (what, ink, ground, need) in mapped.read_as_text().into_iter().chain(mapped.seen())
            {
                let has = contrast(ink, ground);
                if has < need {
                    failures.push(format!("{name}: {what} has {has:.2}, needs {need}"));
                }
            }
            // And as a theme it draws in the colours worked out.
            let made = theme.theme(&name);
            assert_eq!(rgb(made.colors.text), mapped.text, "{name}");
        }
        eprintln!(
            "{read} of {} installed Ghostty themes read ({dark} dark, {} light); {with_fallbacks} \
             needed a fallback for at least one role; by role (an account counted once for \
             each of the four): {by_role:?}",
            files.len(),
            read - dark,
        );
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(read > 0, "the folder is there and has no themes in it");
    }
}
