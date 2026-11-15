//! Themes: TOML colour tables, and the styles the views draw with.
//!
//! A theme names colours as `"#rrggbb"`, an ANSI name such as `"blue"` or
//! `"bright-black"`, a 256-colour index, or `"default"`. Hex colours fall back
//! to the nearest 256-colour index when the terminal lacks true colour, and
//! `NO_COLOR` turns colour off altogether. State is never shown by colour
//! alone: every style here also carries a modifier or the views add a marker.

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

/// A colour as a theme file writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpec {
    /// The terminal's own foreground or background.
    Default,
    /// An entry of the 256-colour palette. 0 to 15 are the ANSI colours.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

impl ColorSpec {
    fn parse(text: &str) -> Result<Self, String> {
        let lower = text.trim().to_ascii_lowercase();
        if lower == "default" {
            return Ok(Self::Default);
        }
        if let Some(hex) = lower.strip_prefix('#') {
            let channel = |range: std::ops::Range<usize>| {
                hex.get(range)
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            };
            return match (hex.len(), channel(0..2), channel(2..4), channel(4..6)) {
                (6, Some(r), Some(g), Some(b)) => Ok(Self::Rgb(r, g, b)),
                _ => Err(format!("\"{text}\" is not a #rrggbb colour")),
            };
        }
        match ANSI_NAMES.iter().position(|name| *name == lower) {
            Some(index) => Ok(Self::Indexed(index as u8)),
            None => Err(format!("unknown colour \"{text}\"")),
        }
    }
}

impl<'de> Deserialize<'de> for ColorSpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Index(i64),
            Name(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Index(index) => u8::try_from(index).map(Self::Indexed).map_err(|_| {
                serde::de::Error::custom(format!("colour index {index} is not between 0 and 255"))
            }),
            Raw::Name(name) => Self::parse(&name).map_err(serde::de::Error::custom),
        }
    }
}

/// How much colour the terminal gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    /// `NO_COLOR` is set: modifiers only.
    None,
}

impl ColorMode {
    /// Chooses from the values of `NO_COLOR` and `COLORTERM`.
    pub fn detect(no_color: Option<&str>, colorterm: Option<&str>) -> Self {
        if no_color.is_some_and(|value| !value.is_empty()) {
            return Self::None;
        }
        match colorterm {
            Some("truecolor" | "24bit") => Self::TrueColor,
            _ => Self::Ansi256,
        }
    }
}

/// The colours of a theme. Any colour a file leaves out takes its value from
/// the terminal-palette theme.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub background: ColorSpec,
    pub text: ColorSpec,
    pub dim: ColorSpec,
    pub border: ColorSpec,
    pub border_focused: ColorSpec,
    pub selection_fg: ColorSpec,
    pub selection_bg: ColorSpec,
    pub unread: ColorSpec,
    pub starred: ColorSpec,
    pub accent: ColorSpec,
    pub key: ColorSpec,
    pub error: ColorSpec,
    /// One colour per account, reused in order when there are more accounts.
    pub accounts: Vec<ColorSpec>,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: ColorSpec::Default,
            text: ColorSpec::Default,
            dim: ColorSpec::Indexed(8),
            border: ColorSpec::Indexed(8),
            border_focused: ColorSpec::Indexed(4),
            selection_fg: ColorSpec::Indexed(15),
            selection_bg: ColorSpec::Indexed(4),
            unread: ColorSpec::Default,
            starred: ColorSpec::Indexed(3),
            accent: ColorSpec::Indexed(6),
            key: ColorSpec::Indexed(5),
            error: ColorSpec::Indexed(1),
            accounts: vec![
                ColorSpec::Indexed(4),
                ColorSpec::Indexed(2),
                ColorSpec::Indexed(3),
                ColorSpec::Indexed(5),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub colors: Colors,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("theme line {line}: {message}")]
pub struct ThemeError {
    pub line: usize,
    pub message: String,
}

const BUILTIN: [(&str, &str); 3] = [
    ("dark", include_str!("../themes/dark.toml")),
    ("light", include_str!("../themes/light.toml")),
    ("terminal", include_str!("../themes/terminal.toml")),
];

impl Theme {
    pub fn parse(text: &str) -> Result<Self, ThemeError> {
        toml::from_str(text).map_err(|err| ThemeError {
            line: err.span().map_or(1, |span| {
                text.as_bytes()[..span.start.min(text.len())]
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count()
                    + 1
            }),
            message: err.message().to_owned(),
        })
    }

    /// The names of the themes shipped inside the binary.
    pub fn builtin_names() -> impl Iterator<Item = &'static str> {
        BUILTIN.iter().map(|(name, _)| *name)
    }

    /// A shipped theme: `dark`, `light` or `terminal`.
    pub fn builtin(name: &str) -> Option<Self> {
        let (_, text) = BUILTIN.iter().find(|(builtin, _)| *builtin == name)?;
        Self::parse(text).ok()
    }

    /// The theme that uses only the terminal's own palette. Needs no parsing,
    /// so it is the fallback of last resort.
    pub fn terminal() -> Self {
        Self {
            name: "terminal".to_owned(),
            colors: Colors::default(),
        }
    }
}

/// A theme resolved for one terminal: what the views actually draw with.
#[derive(Debug, Clone)]
pub struct Styles {
    theme: Theme,
    mode: ColorMode,
}

impl Styles {
    pub fn new(theme: Theme, mode: ColorMode) -> Self {
        Self { theme, mode }
    }

    pub fn theme_name(&self) -> &str {
        &self.theme.name
    }

    pub fn mode(&self) -> ColorMode {
        self.mode
    }

    fn color(&self, spec: ColorSpec) -> Option<Color> {
        match (self.mode, spec) {
            (ColorMode::None, _) | (_, ColorSpec::Default) => None,
            (_, ColorSpec::Indexed(index)) => Some(Color::Indexed(index)),
            (ColorMode::TrueColor, ColorSpec::Rgb(r, g, b)) => Some(Color::Rgb(r, g, b)),
            (ColorMode::Ansi256, ColorSpec::Rgb(r, g, b)) => {
                Some(Color::Indexed(nearest_256(r, g, b)))
            }
        }
    }

    fn fg(&self, spec: ColorSpec) -> Style {
        match self.color(spec) {
            Some(color) => Style::new().fg(color),
            None => Style::new(),
        }
    }

    /// The whole screen's base: body text on the theme background.
    pub fn base(&self) -> Style {
        let style = self.fg(self.theme.colors.text);
        match self.color(self.theme.colors.background) {
            Some(color) => style.bg(color),
            None => style,
        }
    }

    pub fn text(&self) -> Style {
        self.fg(self.theme.colors.text)
    }

    pub fn dim(&self) -> Style {
        let style = self.fg(self.theme.colors.dim);
        if self.mode == ColorMode::None {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    }

    pub fn border(&self, focused: bool) -> Style {
        if focused {
            self.fg(self.theme.colors.border_focused)
                .add_modifier(Modifier::BOLD)
        } else {
            self.fg(self.theme.colors.border)
        }
    }

    /// The selected row. With colour it is a background bar; without, the
    /// focused pane's row is reversed and an unfocused pane's is underlined.
    pub fn selected(&self, pane_focused: bool) -> Style {
        if self.mode == ColorMode::None {
            return Style::new().add_modifier(if pane_focused {
                Modifier::REVERSED
            } else {
                Modifier::UNDERLINED
            });
        }
        let colors = &self.theme.colors;
        let (Some(fg), Some(bg)) = (
            self.color(colors.selection_fg),
            self.color(colors.selection_bg),
        ) else {
            return Style::new().add_modifier(Modifier::REVERSED);
        };
        if pane_focused {
            Style::new().fg(fg).bg(bg)
        } else {
            // Out of focus the bar stays but the text keeps its own colour.
            Style::new().bg(bg)
        }
    }

    pub fn unread(&self) -> Style {
        self.fg(self.theme.colors.unread)
            .add_modifier(Modifier::BOLD)
    }

    pub fn starred(&self) -> Style {
        self.fg(self.theme.colors.starred)
    }

    pub fn accent(&self) -> Style {
        self.fg(self.theme.colors.accent)
    }

    pub fn heading(&self) -> Style {
        self.fg(self.theme.colors.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn key(&self) -> Style {
        self.fg(self.theme.colors.key).add_modifier(Modifier::BOLD)
    }

    pub fn error(&self) -> Style {
        self.fg(self.theme.colors.error)
            .add_modifier(Modifier::BOLD)
    }

    /// The marker colour of the account at `index`.
    pub fn account(&self, index: usize) -> Style {
        let accounts = &self.theme.colors.accounts;
        let style = match accounts.get(index % accounts.len().max(1)) {
            Some(spec) => self.fg(*spec),
            None => Style::new(),
        };
        style.add_modifier(Modifier::BOLD)
    }
}

/// The closest entry of the xterm 256-colour palette, from the 6x6x6 cube and
/// the grey ramp. The 16 ANSI colours are skipped because terminals remap them.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_level = |value: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| value.abs_diff(**level))
            .map_or(0, |(index, _)| index)
    };
    let distance = |candidate: (u8, u8, u8)| {
        let square = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
        square(r, candidate.0) + square(g, candidate.1) + square(b, candidate.2)
    };
    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;

    let average = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let grey_step = (average.saturating_sub(8) + 5) / 10;
    let grey_step = grey_step.min(23);
    let grey_value = (8 + 10 * grey_step) as u8;
    let grey = (grey_value, grey_value, grey_value);

    if distance(grey) < distance(cube) {
        (232 + grey_step) as u8
    } else {
        cube_index as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_themes_parse_and_name_themselves() {
        for name in Theme::builtin_names() {
            let theme = Theme::builtin(name).unwrap_or_else(|| panic!("{name} does not parse"));
            assert_eq!(theme.name, name);
            assert!(!theme.colors.accounts.is_empty());
        }
        assert!(Theme::builtin("nope").is_none());
    }

    #[test]
    fn the_terminal_theme_file_matches_the_fallback() {
        assert_eq!(Theme::builtin("terminal").unwrap(), Theme::terminal());
    }

    #[test]
    fn the_terminal_theme_uses_no_fixed_rgb() {
        let colors = Theme::terminal().colors;
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
        all.extend(colors.accounts);
        for spec in all {
            assert!(
                matches!(spec, ColorSpec::Default | ColorSpec::Indexed(0..=15)),
                "{spec:?}"
            );
        }
    }

    #[test]
    fn colours_parse_in_every_form() {
        let theme = Theme::parse(
            "name = \"t\"\n[colors]\ntext = \"#C0FFEE\"\ndim = \"bright-black\"\nborder = 244\nbackground = \"default\"\n",
        )
        .unwrap();
        assert_eq!(theme.colors.text, ColorSpec::Rgb(0xc0, 0xff, 0xee));
        assert_eq!(theme.colors.dim, ColorSpec::Indexed(8));
        assert_eq!(theme.colors.border, ColorSpec::Indexed(244));
        assert_eq!(theme.colors.background, ColorSpec::Default);
        // Left out, so inherited from the terminal theme.
        assert_eq!(theme.colors.error, Colors::default().error);
    }

    #[test]
    fn bad_colours_and_unknown_keys_report_their_line() {
        let err = Theme::parse("name = \"t\"\n[colors]\ntext = \"#12345\"\n").unwrap_err();
        assert_eq!(err.line, 3);
        assert!(err.message.contains("#rrggbb"), "{err}");
        let err = Theme::parse("name = \"t\"\n[colors]\ntext = \"mauve\"\n").unwrap_err();
        assert!(err.message.contains("mauve"), "{err}");
        assert!(Theme::parse("name = \"t\"\n[colors]\nborder = 256\n").is_err());
        assert!(Theme::parse("name = \"t\"\n[colors]\ntxet = \"red\"\n").is_err());
        assert!(Theme::parse("[colors]\ntext = \"red\"\n").is_err());
    }

    #[test]
    fn colour_mode_follows_the_environment() {
        assert_eq!(
            ColorMode::detect(None, Some("truecolor")),
            ColorMode::TrueColor
        );
        assert_eq!(ColorMode::detect(None, Some("24bit")), ColorMode::TrueColor);
        assert_eq!(ColorMode::detect(None, None), ColorMode::Ansi256);
        assert_eq!(
            ColorMode::detect(Some("1"), Some("truecolor")),
            ColorMode::None
        );
        // An empty NO_COLOR does not count, per no-color.org.
        assert_eq!(
            ColorMode::detect(Some(""), Some("truecolor")),
            ColorMode::TrueColor
        );
    }

    #[test]
    fn nearest_256_hits_exact_cube_and_grey_entries() {
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(95, 135, 175), 16 + 36 + 6 * 2 + 3);
        assert_eq!(nearest_256(128, 128, 128), 244);
        assert_eq!(nearest_256(8, 8, 8), 232);
        assert_eq!(nearest_256(238, 238, 238), 255);
    }

    #[test]
    fn rgb_degrades_to_256_colours_and_then_to_none() {
        let theme = Theme::builtin("dark").unwrap();
        let rgb = Styles::new(theme.clone(), ColorMode::TrueColor);
        assert!(matches!(rgb.text().fg, Some(Color::Rgb(..))));
        let indexed = Styles::new(theme.clone(), ColorMode::Ansi256);
        assert!(matches!(indexed.text().fg, Some(Color::Indexed(16..))));
        let none = Styles::new(theme, ColorMode::None);
        assert_eq!(none.text().fg, None);
        assert_eq!(none.base().bg, None);
    }

    #[test]
    fn without_colour_every_state_still_has_a_modifier() {
        let styles = Styles::new(Theme::builtin("dark").unwrap(), ColorMode::None);
        let plain = Style::new();
        for (name, style) in [
            ("selected", styles.selected(true)),
            ("selected, unfocused", styles.selected(false)),
            ("unread", styles.unread()),
            ("dim", styles.dim()),
            ("focused border", styles.border(true)),
            ("key", styles.key()),
            ("error", styles.error()),
        ] {
            assert_eq!((style.fg, style.bg), (None, None), "{name}");
            assert_ne!(style, plain, "{name} is invisible without colour");
        }
        assert_ne!(styles.selected(true), styles.selected(false));
    }
}
