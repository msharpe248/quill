//! Themes: TOML colour tables, and the styles the views draw with.
//!
//! A theme names colours as `"#rrggbb"`, an ANSI name such as `"blue"` or
//! `"bright-black"`, a 256-colour index, or `"default"`. Hex colours fall back
//! to the nearest 256-colour index when the terminal lacks true colour, and
//! `NO_COLOR` turns colour off altogether. State is never shown by colour
//! alone: every style here also carries a modifier or the views add a marker.
//!
//! A theme written for the Ghostty terminal is read too ([`ghostty`]): it
//! has fewer colours than Quill draws with, and the rest are worked out
//! from it, held to a measure of what can be read ([`contrast`]).

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

pub mod contrast;
pub mod ghostty;

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

/// What the sixteen colours a terminal names are in the terminal most
/// others took them from (xterm). Every terminal sets its own, so this is
/// only what such a colour is shown as where one has to be shown.
const XTERM: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0xcd, 0x00, 0x00),
    (0x00, 0xcd, 0x00),
    (0xcd, 0xcd, 0x00),
    (0x00, 0x00, 0xee),
    (0xcd, 0x00, 0xcd),
    (0x00, 0xcd, 0xcd),
    (0xe5, 0xe5, 0xe5),
    (0x7f, 0x7f, 0x7f),
    (0xff, 0x00, 0x00),
    (0x00, 0xff, 0x00),
    (0xff, 0xff, 0x00),
    (0x5c, 0x5c, 0xff),
    (0xff, 0x00, 0xff),
    (0x00, 0xff, 0xff),
    (0xff, 0xff, 0xff),
];

impl ColorSpec {
    /// The name a theme file has for one of the terminal's own sixteen
    /// colours: `blue`, `bright-black`.
    pub fn ansi_name(index: u8) -> Option<&'static str> {
        ANSI_NAMES.get(usize::from(index)).copied()
    }

    /// The colour as red, green and blue, as near as can be said: itself,
    /// its entry in the 256, and for what only the terminal can say (its
    /// own sixteen, its own background and text) what a terminal usually
    /// has. `ground` says the colour is one that is drawn on, and `dark`
    /// that Quill is dark: the terminal's own background is then taken
    /// for black, and its own text for white.
    ///
    /// For showing a colour to someone who is choosing another. Nothing is
    /// drawn with it.
    pub fn roughly(self, ground: bool, dark: bool) -> (u8, u8, u8) {
        match self {
            Self::Rgb(red, green, blue) => (red, green, blue),
            Self::Indexed(index @ 0..=15) => XTERM[usize::from(index)],
            Self::Indexed(index) => rgb_of(Color::Indexed(index)).unwrap_or((0, 0, 0)),
            Self::Default if ground == dark => (0, 0, 0),
            Self::Default => (0xff, 0xff, 0xff),
        }
    }

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
    /// The bar of the selected row in a pane that does not have the
    /// keyboard, where each word keeps its own colour. Left out, it is
    /// `selection_bg`.
    pub selection_inactive_bg: Option<ColorSpec>,
    pub unread: ColorSpec,
    pub starred: ColorSpec,
    pub accent: ColorSpec,
    pub key: ColorSpec,
    pub error: ColorSpec,
    /// One colour per account, reused in order when there are more accounts.
    pub accounts: Vec<ColorSpec>,
}

/// One of the colours a theme has, by what it is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Background,
    Text,
    Dim,
    Border,
    BorderFocused,
    SelectionFg,
    SelectionBg,
    SelectionInactiveBg,
    Unread,
    Starred,
    Accent,
    Key,
    Error,
    /// The colour of the account at this place among the accounts.
    Account(usize),
}

impl Slot {
    /// Every colour a theme has but the accounts', in the order a file
    /// has them.
    pub const FIXED: [Slot; 13] = [
        Slot::Background,
        Slot::Text,
        Slot::Dim,
        Slot::Border,
        Slot::BorderFocused,
        Slot::SelectionFg,
        Slot::SelectionBg,
        Slot::SelectionInactiveBg,
        Slot::Unread,
        Slot::Starred,
        Slot::Accent,
        Slot::Key,
        Slot::Error,
    ];

    /// The most account colours a theme is shown and changed with.
    pub const MOST_ACCOUNTS: usize = 6;

    /// The key a theme file has it under, in `[colors]`. The accounts'
    /// colours are one key between them, `accounts`.
    pub fn key(self) -> &'static str {
        match self {
            Slot::Background => "background",
            Slot::Text => "text",
            Slot::Dim => "dim",
            Slot::Border => "border",
            Slot::BorderFocused => "border_focused",
            Slot::SelectionFg => "selection_fg",
            Slot::SelectionBg => "selection_bg",
            Slot::SelectionInactiveBg => "selection_inactive_bg",
            Slot::Unread => "unread",
            Slot::Starred => "starred",
            Slot::Accent => "accent",
            Slot::Key => "key",
            Slot::Error => "error",
            Slot::Account(_) => "accounts",
        }
    }

    /// What it is called where a person chooses it.
    pub fn title(self) -> String {
        match self {
            Slot::Background => "Background".to_owned(),
            Slot::Text => "Text".to_owned(),
            Slot::Dim => "Quiet text".to_owned(),
            Slot::Border => "Borders".to_owned(),
            Slot::BorderFocused => "Border in use".to_owned(),
            Slot::SelectionFg => "Selected text".to_owned(),
            Slot::SelectionBg => "Selection".to_owned(),
            Slot::SelectionInactiveBg => "Selection, at rest".to_owned(),
            Slot::Unread => "Unread".to_owned(),
            Slot::Starred => "Starred".to_owned(),
            Slot::Accent => "Headings".to_owned(),
            Slot::Key => "Keys".to_owned(),
            Slot::Error => "Errors".to_owned(),
            Slot::Account(index) => format!("Account {}", index + 1),
        }
    }

    /// Whether it is a colour other colours are drawn on.
    pub fn is_ground(self) -> bool {
        matches!(
            self,
            Slot::Background | Slot::SelectionBg | Slot::SelectionInactiveBg
        )
    }
}

impl Colors {
    /// The colour in a slot. The bar of a pane at rest is the selection's
    /// own where the theme has none for it; an account's is `None` where
    /// the theme has fewer accounts' colours than that.
    pub fn get(&self, slot: Slot) -> Option<ColorSpec> {
        Some(match slot {
            Slot::Background => self.background,
            Slot::Text => self.text,
            Slot::Dim => self.dim,
            Slot::Border => self.border,
            Slot::BorderFocused => self.border_focused,
            Slot::SelectionFg => self.selection_fg,
            Slot::SelectionBg => self.selection_bg,
            Slot::SelectionInactiveBg => self.selection_inactive_bg.unwrap_or(self.selection_bg),
            Slot::Unread => self.unread,
            Slot::Starred => self.starred,
            Slot::Accent => self.accent,
            Slot::Key => self.key,
            Slot::Error => self.error,
            Slot::Account(index) => return self.accounts.get(index).copied(),
        })
    }

    /// Gives a slot another colour. An account the theme has no colour
    /// for is left without one.
    pub fn set(&mut self, slot: Slot, spec: ColorSpec) {
        match slot {
            Slot::Background => self.background = spec,
            Slot::Text => self.text = spec,
            Slot::Dim => self.dim = spec,
            Slot::Border => self.border = spec,
            Slot::BorderFocused => self.border_focused = spec,
            Slot::SelectionFg => self.selection_fg = spec,
            Slot::SelectionBg => self.selection_bg = spec,
            Slot::SelectionInactiveBg => self.selection_inactive_bg = Some(spec),
            Slot::Unread => self.unread = spec,
            Slot::Starred => self.starred = spec,
            Slot::Accent => self.accent = spec,
            Slot::Key => self.key = spec,
            Slot::Error => self.error = spec,
            Slot::Account(index) => {
                if let Some(account) = self.accounts.get_mut(index) {
                    *account = spec;
                }
            }
        }
    }

    /// The slots there are colours to choose for: every fixed one, and
    /// one for each account colour the theme has, up to a few.
    pub fn slots(&self) -> Vec<Slot> {
        let accounts = self.accounts.len().min(Slot::MOST_ACCOUNTS);
        Slot::FIXED
            .into_iter()
            .chain((0..accounts).map(Slot::Account))
            .collect()
    }
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
            selection_inactive_bg: None,
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

    /// A theme from the text of a file in the user's `themes` folder, to
    /// be known as `name`: one of Quill's own, in TOML, or one written for
    /// Ghostty. `toml` says which the file's name makes it out to be
    /// (`NAME.toml`, or plain `NAME`), and that is the format its mistakes
    /// are reported in; but a file that is a good theme in the other
    /// format is that theme, whatever it is called.
    ///
    /// A Quill theme names itself inside the file. A Ghostty theme has
    /// only its file's name, so it is called `name`.
    pub fn from_file(name: &str, text: &str, toml: bool) -> Result<Self, String> {
        let quill = Self::parse(text).map_err(|err| err.to_string());
        let ghostty = ghostty::parse(text).map(|theme| theme.theme(name));
        let (named, other) = if toml {
            (quill, ghostty)
        } else {
            (ghostty, quill)
        };
        named.or_else(|why| other.map_err(|_| why))
    }

    /// Whether the theme is a dark one: drawn on a background that white
    /// stands out on more than black does. `None` when the theme leaves
    /// its background to the terminal, or names one of the terminal's own
    /// sixteen colours, which only the terminal can say the look of.
    pub fn is_dark(&self) -> Option<bool> {
        let background = match self.colors.background {
            ColorSpec::Rgb(red, green, blue) => Some((red, green, blue)),
            ColorSpec::Indexed(index) => rgb_of(Color::Indexed(index)),
            ColorSpec::Default => None,
        }?;
        Some(contrast::is_dark(background))
    }
}

/// Whether Quill is light or dark, and the theme it has for each.
///
/// The settings say light, dark, or as the system is; what the system is
/// only a host can say, and only Quill's own app does. In a terminal
/// nobody says, and "as the system is" is dark: a terminal program cannot
/// ask what its terminal looks like and be sure of an answer, and dark is
/// what Quill was before there was a choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    /// What the settings say.
    pub appearance: quill_core::config::Appearance,
    /// Whether the system is dark, once a host has said.
    pub system_dark: Option<bool>,
    /// The theme while Quill is light.
    pub light: Theme,
    /// The theme while Quill is dark.
    pub dark: Theme,
    /// A theme being made, drawn with while it is: it is seen as its
    /// colours are chosen. It is in no file yet, and may never be.
    pub trying: Option<Theme>,
}

impl Look {
    /// One theme whatever the appearance, which is the system's.
    pub fn one(theme: Theme) -> Self {
        Self {
            appearance: quill_core::config::Appearance::System,
            system_dark: None,
            light: theme.clone(),
            dark: theme,
            trying: None,
        }
    }

    /// Whether Quill is dark now.
    pub fn is_dark(&self) -> bool {
        self.appearance.is_dark(self.system_dark)
    }

    /// The theme in force: the one being made while one is, and
    /// otherwise the one for the appearance.
    pub fn theme(&self) -> &Theme {
        if let Some(trying) = &self.trying {
            return trying;
        }
        if self.is_dark() {
            &self.dark
        } else {
            &self.light
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

    /// The colour the screen is drawn on, as red, green and blue, for a
    /// host that shows another terminal beside this one and wants it to
    /// match. `None` when the theme leaves the background to the terminal,
    /// or names one of the terminal's own sixteen colours, which only the
    /// terminal can say the meaning of; and with colour switched off.
    pub fn background_rgb(&self) -> Option<(u8, u8, u8)> {
        self.color(self.theme.colors.background).and_then(rgb_of)
    }

    /// The colour of body text, likewise.
    pub fn text_rgb(&self) -> Option<(u8, u8, u8)> {
        self.color(self.theme.colors.text).and_then(rgb_of)
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
            // A theme may have a quieter bar for that, to be read on.
            let quiet = colors
                .selection_inactive_bg
                .and_then(|spec| self.color(spec));
            Style::new().bg(quiet.unwrap_or(bg))
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

/// The steps of each channel in the 6x6x6 cube of the xterm 256-colour
/// palette.
const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// What a colour as drawn is in red, green and blue: itself, or its entry
/// in the cube or the grey ramp of the xterm 256-colour palette. `None` for
/// the 16 ANSI colours, which every terminal sets for itself.
fn rgb_of(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(index @ 16..=231) => {
            let cube = usize::from(index - 16);
            Some((LEVELS[cube / 36], LEVELS[cube / 6 % 6], LEVELS[cube % 6]))
        }
        Color::Indexed(index @ 232..) => {
            let grey = 8 + 10 * (index - 232);
            Some((grey, grey, grey))
        }
        _ => None,
    }
}

/// The closest entry of the xterm 256-colour palette, from the 6x6x6 cube and
/// the grey ramp. The 16 ANSI colours are skipped because terminals remap them.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
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
    fn a_theme_may_have_a_quieter_bar_for_a_pane_without_the_keyboard() {
        // Without one the bar is the same in both, as it always was.
        let dark = Styles::new(Theme::builtin("dark").unwrap(), ColorMode::TrueColor);
        assert_eq!(dark.selected(true).bg, dark.selected(false).bg);
        assert_eq!(dark.selected(false).fg, None);
        for name in Theme::builtin_names() {
            let theme = Theme::builtin(name).unwrap();
            assert_eq!(theme.colors.selection_inactive_bg, None, "{name}");
        }

        let own = Theme::parse(
            "name = \"t\"\n[colors]\nselection_fg = \"#ffffff\"\nselection_bg = \"#204080\"\n\
             selection_inactive_bg = \"#303030\"\n",
        )
        .unwrap();
        let styles = Styles::new(own, ColorMode::TrueColor);
        assert_eq!(styles.selected(true).bg, Some(Color::Rgb(0x20, 0x40, 0x80)));
        assert_eq!(styles.selected(true).fg, Some(Color::Rgb(0xff, 0xff, 0xff)));
        assert_eq!(
            styles.selected(false).bg,
            Some(Color::Rgb(0x30, 0x30, 0x30))
        );
        assert_eq!(styles.selected(false).fg, None);
    }

    #[test]
    fn a_file_is_a_theme_of_quills_or_of_ghosttys_whatever_it_is_called() {
        let quill = "name = \"mine\"\n[colors]\ntext = \"#112233\"\n";
        let ghostty = "background = #282a36\nforeground = #f8f8f2\npalette = 4=#bd93f9\n";
        // Each under the name its format has.
        let theme = Theme::from_file("mine", quill, true).unwrap();
        assert_eq!(theme.name, "mine");
        assert_eq!(theme.colors.text, ColorSpec::Rgb(0x11, 0x22, 0x33));
        let theme = Theme::from_file("Dracula", ghostty, false).unwrap();
        assert_eq!(theme.name, "Dracula");
        assert_eq!(theme.colors.background, ColorSpec::Rgb(0x28, 0x2a, 0x36));
        assert_eq!(
            theme.colors.border_focused,
            ColorSpec::Rgb(0xbd, 0x93, 0xf9)
        );
        // And each under the other's: a good theme is that theme.
        assert_eq!(Theme::from_file("x", ghostty, true).unwrap().name, "x");
        assert_eq!(Theme::from_file("x", quill, false).unwrap().name, "mine");
        // What is neither is refused in the words of the format its name
        // makes it out to be.
        let broken = "name = \"mine\"\n[colors]\ntext = \"mauve\"\n";
        let refused = Theme::from_file("mine", broken, true).unwrap_err();
        assert!(refused.starts_with("theme line 3: "), "{refused}");
        let refused = Theme::from_file("notes", "Remember the milk.\n", false).unwrap_err();
        assert!(refused.contains("not a theme in Ghostty"), "{refused}");
    }

    #[test]
    fn a_theme_is_dark_or_light_by_its_background_where_it_fixes_one() {
        assert_eq!(Theme::builtin("dark").unwrap().is_dark(), Some(true));
        assert_eq!(Theme::builtin("light").unwrap().is_dark(), Some(false));
        // The terminal's own colours are the terminal's to say.
        assert_eq!(Theme::terminal().is_dark(), None);
        let own = |background: &str| {
            Theme::parse(&format!(
                "name = \"t\"\n[colors]\nbackground = {background}\n"
            ))
            .unwrap()
            .is_dark()
        };
        assert_eq!(own("236"), Some(true));
        assert_eq!(own("255"), Some(false));
        assert_eq!(own("\"black\""), None);
    }

    #[test]
    fn the_theme_in_force_goes_by_the_appearance_and_by_the_system_where_one_is_known() {
        use quill_core::config::Appearance;

        let mut look = Look {
            appearance: Appearance::System,
            system_dark: None,
            light: Theme::builtin("light").unwrap(),
            dark: Theme::builtin("dark").unwrap(),
            trying: None,
        };
        // Nobody has said what the system is, as in a terminal: dark.
        assert!(look.is_dark());
        assert_eq!(look.theme().name, "dark");
        // Quill's app says, and says again when the Mac changes.
        look.system_dark = Some(false);
        assert_eq!(look.theme().name, "light");
        look.system_dark = Some(true);
        assert_eq!(look.theme().name, "dark");
        // Light or dark by the settings, whatever the system is.
        look.appearance = Appearance::Light;
        assert_eq!(look.theme().name, "light");
        look.system_dark = None;
        assert_eq!(look.theme().name, "light");
        look.appearance = Appearance::Dark;
        look.system_dark = Some(false);
        assert_eq!(look.theme().name, "dark");

        // A theme being made is drawn with whatever the appearance.
        look.trying = Some(Theme::terminal());
        assert_eq!(look.theme().name, "terminal");
        look.appearance = Appearance::Light;
        assert_eq!(look.theme().name, "terminal");
        look.trying = None;
        assert_eq!(look.theme().name, "light");

        // One theme for both is that theme always.
        let mut one = Look::one(Theme::terminal());
        assert_eq!(one.theme().name, "terminal");
        one.system_dark = Some(false);
        assert_eq!(one.theme().name, "terminal");
    }

    #[test]
    fn every_colour_of_a_theme_has_a_slot_to_read_and_change_it_by() {
        let mut colors = Theme::builtin("dark").unwrap().colors;
        let slots = colors.slots();
        assert_eq!(slots.len(), 13 + 4);
        // Every key a theme file may have under `[colors]` is a slot's.
        let mut keys: Vec<&str> = slots.iter().map(|slot| slot.key()).collect();
        keys.dedup();
        for key in &keys {
            let text = format!("name = \"t\"\n[colors]\n{key} = {}\n", {
                if *key == "accounts" {
                    "[\"red\"]"
                } else {
                    "\"red\""
                }
            });
            assert!(Theme::parse(&text).is_ok(), "{key}");
        }
        assert_eq!(keys.len(), 14);
        // The bar of a pane at rest is the selection's until it has its own.
        assert_eq!(
            colors.get(Slot::SelectionInactiveBg),
            Some(colors.selection_bg)
        );
        let red = ColorSpec::Rgb(0xff, 0, 0);
        for slot in slots {
            colors.set(slot, red);
            assert_eq!(colors.get(slot), Some(red), "{slot:?}");
        }
        assert_eq!(colors.selection_inactive_bg, Some(red));
        assert_eq!(colors.accounts, vec![red; 4]);
        // An account the theme has no colour for is left without one.
        colors.set(Slot::Account(9), red);
        assert_eq!(colors.get(Slot::Account(9)), None);
        assert_eq!(colors.accounts.len(), 4);
    }

    #[test]
    fn a_colour_only_the_terminal_can_say_is_shown_as_terminals_usually_have_it() {
        assert_eq!(ColorSpec::Rgb(1, 2, 3).roughly(false, true), (1, 2, 3));
        assert_eq!(ColorSpec::Indexed(1).roughly(false, true), (0xcd, 0, 0));
        assert_eq!(ColorSpec::Indexed(196).roughly(false, true), (255, 0, 0));
        // The terminal's own background and text, in a dark Quill and a
        // light one.
        assert_eq!(ColorSpec::Default.roughly(true, true), (0, 0, 0));
        assert_eq!(ColorSpec::Default.roughly(false, true), (255, 255, 255));
        assert_eq!(ColorSpec::Default.roughly(true, false), (255, 255, 255));
        assert_eq!(ColorSpec::Default.roughly(false, false), (0, 0, 0));
        assert_eq!(ColorSpec::ansi_name(8), Some("bright-black"));
        assert_eq!(ColorSpec::ansi_name(16), None);
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
    fn a_host_is_told_the_colours_the_screen_is_drawn_in() {
        let of = |theme: Theme, mode| {
            let styles = Styles::new(theme, mode);
            (styles.background_rgb(), styles.text_rgb())
        };
        let dark = Theme::builtin("dark").unwrap();
        assert_eq!(
            of(dark.clone(), ColorMode::TrueColor),
            (Some((0x1a, 0x1b, 0x26)), Some((0xc0, 0xca, 0xf5)))
        );
        assert_eq!(
            of(Theme::builtin("light").unwrap(), ColorMode::TrueColor),
            (Some((0xfa, 0xfa, 0xfa)), Some((0x38, 0x3a, 0x42)))
        );
        // The terminal's own colours are the terminal's to say.
        assert_eq!(of(Theme::terminal(), ColorMode::TrueColor), (None, None));
        assert_eq!(of(dark.clone(), ColorMode::None), (None, None));

        // With 256 colours the screen is drawn in the nearest of them, and
        // that is the colour told: what is drawn, not what was asked for.
        let (background, text) = of(dark, ColorMode::Ansi256);
        let (r, g, b) = background.unwrap();
        assert_eq!(rgb_of(Color::Indexed(nearest_256(r, g, b))), background);
        assert_ne!(background, Some((0x1a, 0x1b, 0x26)));
        assert!(text.is_some());

        // A place in the palette: the cube and the greys are the same in
        // every terminal, the first sixteen are not.
        let own =
            Theme::parse("name = \"t\"\n[colors]\nbackground = 236\ntext = \"bright-white\"\n")
                .unwrap();
        assert_eq!(of(own, ColorMode::TrueColor), (Some((48, 48, 48)), None));
        assert_eq!(rgb_of(Color::Indexed(16)), Some((0, 0, 0)));
        assert_eq!(rgb_of(Color::Indexed(196)), Some((255, 0, 0)));
        assert_eq!(rgb_of(Color::Indexed(231)), Some((255, 255, 255)));
        assert_eq!(rgb_of(Color::Indexed(232)), Some((8, 8, 8)));
        assert_eq!(rgb_of(Color::Indexed(255)), Some((238, 238, 238)));
        assert_eq!(rgb_of(Color::Indexed(15)), None);
        assert_eq!(rgb_of(Color::Reset), None);
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
