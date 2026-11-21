//! Keys, key sequences and the keymap that turns them into commands.
//!
//! Keys are written the way the help overlay shows them: `j`, `G`, `enter`,
//! `ctrl-k`, `shift-tab`, `cmd-enter`, and `g g` for a two-key sequence.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use quill_core::config::KeymapPreset;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::command::{CommandId, Context, Registry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

/// One key press with its modifiers.
///
/// Shift is folded into printable characters: shift with `g` is `G`, and `?`
/// is just `?`. It stays a modifier only on named keys such as `shift-tab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: Key,
    pub mods: Mods,
}

impl KeyChord {
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            mods: Mods::default(),
        }
    }

    /// Translates a terminal key event. Key releases and keys Quill has no
    /// name for give `None`.
    pub fn from_event(event: &KeyEvent) -> Option<Self> {
        if event.kind == KeyEventKind::Release {
            return None;
        }
        let mut mods = Mods {
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
            shift: event.modifiers.contains(KeyModifiers::SHIFT),
            cmd: event.modifiers.contains(KeyModifiers::SUPER),
        };
        let key = match event.code {
            KeyCode::Char(c) => {
                let c = if mods.shift { upper(c) } else { c };
                mods.shift = false;
                Key::Char(c)
            }
            KeyCode::BackTab => {
                mods.shift = true;
                Key::Tab
            }
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab => Key::Tab,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::F(n) => Key::F(n),
            _ => return None,
        };
        Some(Self { key, mods })
    }
}

fn upper(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyParseError {
    #[error("empty key")]
    Empty,
    #[error("unknown key \"{0}\"")]
    UnknownKey(String),
    #[error("unknown modifier \"{0}\"")]
    UnknownModifier(String),
}

impl FromStr for KeyChord {
    type Err = KeyParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() {
            return Err(KeyParseError::Empty);
        }
        // The last piece is the key, which may itself be "-".
        let (prefix, key_text) = if text == "-" {
            ("", "-")
        } else if let Some(prefix) = text.strip_suffix("--") {
            (prefix, "-")
        } else {
            match text.rsplit_once('-') {
                Some((prefix, key)) => (prefix, key),
                None => ("", text),
            }
        };
        let mut mods = Mods::default();
        for modifier in prefix.split('-').filter(|part| !part.is_empty()) {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "alt" | "opt" | "option" => mods.alt = true,
                "shift" => mods.shift = true,
                "cmd" | "super" => mods.cmd = true,
                _ => return Err(KeyParseError::UnknownModifier(modifier.to_owned())),
            }
        }
        let mut chars = key_text.chars();
        let key = match (chars.next(), chars.next()) {
            (None, _) => return Err(KeyParseError::Empty),
            (Some(c), None) => {
                let c = if mods.shift { upper(c) } else { c };
                mods.shift = false;
                Key::Char(c)
            }
            _ => match key_text.to_ascii_lowercase().as_str() {
                "enter" | "return" => Key::Enter,
                "esc" | "escape" => Key::Esc,
                "tab" => Key::Tab,
                "space" => Key::Char(' '),
                "backspace" => Key::Backspace,
                "delete" | "del" => Key::Delete,
                "up" => Key::Up,
                "down" => Key::Down,
                "left" => Key::Left,
                "right" => Key::Right,
                "home" => Key::Home,
                "end" => Key::End,
                "pageup" | "pgup" => Key::PageUp,
                "pagedown" | "pgdn" => Key::PageDown,
                name => match name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                    Some(n) if (1..=24).contains(&n) => Key::F(n),
                    _ => return Err(KeyParseError::UnknownKey(key_text.to_owned())),
                },
            },
        };
        Ok(Self { key, mods })
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.mods.ctrl, "ctrl-"),
            (self.mods.alt, "alt-"),
            (self.mods.shift, "shift-"),
            (self.mods.cmd, "cmd-"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        match self.key {
            Key::Char(' ') => f.write_str("space"),
            Key::Char(c) => write!(f, "{c}"),
            Key::Enter => f.write_str("enter"),
            Key::Esc => f.write_str("esc"),
            Key::Tab => f.write_str("tab"),
            Key::Backspace => f.write_str("backspace"),
            Key::Delete => f.write_str("delete"),
            Key::Up => f.write_str("up"),
            Key::Down => f.write_str("down"),
            Key::Left => f.write_str("left"),
            Key::Right => f.write_str("right"),
            Key::Home => f.write_str("home"),
            Key::End => f.write_str("end"),
            Key::PageUp => f.write_str("pageup"),
            Key::PageDown => f.write_str("pagedown"),
            Key::F(n) => write!(f, "f{n}"),
        }
    }
}

/// One or more chords pressed in order, such as `g g`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySeq(pub Vec<KeyChord>);

impl KeySeq {
    /// Whether any key in it is held with `cmd`.
    pub fn uses_cmd(&self) -> bool {
        self.0.iter().any(|chord| chord.mods.cmd)
    }
}

impl FromStr for KeySeq {
    type Err = KeyParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let chords = text
            .split_whitespace()
            .map(KeyChord::from_str)
            .collect::<Result<Vec<_>, _>>()?;
        if chords.is_empty() {
            return Err(KeyParseError::Empty);
        }
        Ok(Self(chords))
    }
}

impl fmt::Display for KeySeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, chord) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{chord}")?;
        }
        Ok(())
    }
}

/// What the keys pressed so far mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Command(CommandId),
    /// The keys begin a longer sequence. Wait for more.
    Pending,
    Unbound,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeymapError {
    #[error("keys line {line}: {message}")]
    Parse { line: usize, message: String },
    #[error("keys: unknown command \"{0}\"")]
    UnknownCommand(String),
    #[error("keys: {command}: {source}")]
    BadKey {
        command: String,
        source: KeyParseError,
    },
}

/// The bindings of every command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: Vec<(CommandId, Vec<KeySeq>)>,
}

type PresetTable = &'static [(CommandId, &'static [&'static str])];

const VIM: PresetTable = &[
    (CommandId::NavNext, &["j", "down"]),
    (CommandId::NavPrev, &["k", "up"]),
    (CommandId::NavTop, &["g g", "home"]),
    (CommandId::NavBottom, &["G", "end"]),
    (CommandId::NavPageDown, &["space", "ctrl-d", "pagedown"]),
    (CommandId::NavPageUp, &["ctrl-u", "pageup"]),
    (CommandId::NavOpen, &["enter"]),
    (CommandId::WindowClose, &["q", "ctrl-c"]),
    (CommandId::AppQuit, &["Q"]),
    (CommandId::NavBack, &["esc", "q"]),
    (CommandId::FocusSidebar, &["1"]),
    (CommandId::FocusList, &["2"]),
    (CommandId::FocusReader, &["3"]),
    (CommandId::FocusNext, &["tab"]),
    (CommandId::FocusPrev, &["shift-tab"]),
    (CommandId::ViewSidebar, &["\\"]),
    (CommandId::ViewZoom, &["z", "ctrl-z"]),
    (CommandId::ViewThreads, &["T"]),
    (CommandId::ViewWebReading, &["w"]),
    (CommandId::ViewDarkPages, &["W"]),
    (CommandId::MessageToggleUnread, &["u"]),
    (CommandId::MessageToggleStar, &["s"]),
    (CommandId::MessageToggleHtml, &["t"]),
    (CommandId::MessageOpenLink, &["o"]),
    (CommandId::MessageArchive, &["e"]),
    (CommandId::MessageDelete, &["d"]),
    (CommandId::MessageMove, &["m"]),
    (CommandId::MessageSnooze, &["b"]),
    (CommandId::SelectToggle, &["x"]),
    (CommandId::SelectRange, &["V"]),
    (CommandId::SelectAll, &["ctrl-a"]),
    (CommandId::SearchStart, &["/", "cmd-f"]),
    (CommandId::PaletteOpen, &[":", "ctrl-k"]),
    (CommandId::MessageAttachments, &["a"]),
    (CommandId::MessageOpenInBrowser, &["B"]),
    (CommandId::MessageLoadRemote, &["i"]),
    (CommandId::SenderAlwaysRemote, &["I"]),
    (CommandId::MessageUnsubscribe, &["X"]),
    (CommandId::MessageHook, &["|"]),
    (CommandId::InviteAccept, &["y"]),
    (CommandId::InviteDecline, &["n"]),
    (CommandId::InviteTentative, &["M"]),
    (CommandId::AttachmentSave, &["s"]),
    (CommandId::AttachmentOpen, &["O"]),
    (CommandId::MessageCompose, &["c"]),
    (CommandId::MessageReply, &["r"]),
    (CommandId::MessageReplyAll, &["R"]),
    (CommandId::MessageForward, &["f"]),
    (
        CommandId::ComposeSend,
        &["cmd-enter", "ctrl-x", "ctrl-enter"],
    ),
    (CommandId::ComposeSendLater, &["cmd-L", "ctrl-l"]),
    (CommandId::ComposeSave, &["cmd-s", "ctrl-s"]),
    (CommandId::ComposeAttach, &["cmd-A", "ctrl-t"]),
    (CommandId::ComposeBcc, &["cmd-B", "ctrl-o"]),
    (CommandId::ComposeTemplate, &["cmd-T", "ctrl-r"]),
    (CommandId::ComposeVim, &["cmd-E", "ctrl-v"]),
    (CommandId::ComposeFields, &["cmd-up"]),
    (CommandId::ComposeBody, &["cmd-down"]),
    (CommandId::ComposeTips, &["cmd-/"]),
    (CommandId::ComposeSpellSuggest, &["cmd-G", "ctrl-g"]),
    (CommandId::ComposeSpellNext, &["cmd-;", "ctrl-y"]),
    (CommandId::EditCopy, &["cmd-c"]),
    (CommandId::EditCut, &["cmd-x"]),
    (CommandId::EditSelectAll, &["cmd-a"]),
    (CommandId::EditUndo, &["U"]),
    (CommandId::AccountAdd, &["A"]),
    (CommandId::AccountSignIn, &["S"]),
    (CommandId::AccountRemove, &["D"]),
    (CommandId::AccountEdit, &["E"]),
    (CommandId::AccountLetter, &["Y"]),
    // Two keys, so that it is never pressed by accident: it switches
    // settings off.
    (CommandId::AgentsStop, &["Z a"]),
    (CommandId::CommandLineInstall, &["Z c"]),
    // Rules: two keys each, beside the others that are about Quill and
    // not about the message under the cursor.
    (CommandId::RulesToggle, &["Z t"]),
    (CommandId::RulesRun, &["Z r"]),
    (CommandId::RulesRunAll, &["Z R"]),
    (CommandId::RulesApply, &["Z y"]),
    (CommandId::RulesCreate, &["Z N"]),
    (CommandId::RulesNew, &["Z n"]),
    (CommandId::RulesEdit, &["Z e"]),
    (CommandId::RulesLog, &["Z l"]),
    (CommandId::MessageCopy, &["C"]),
    (CommandId::MessagePutBack, &["P"]),
    (CommandId::FolderCreate, &["+"]),
    (CommandId::FolderRemove, &["_"]),
    (CommandId::MailboxSync, &["ctrl-r", "cmd-r"]),
    (CommandId::SidebarOpen, &["l", "right"]),
    (CommandId::SidebarClose, &["h", "left"]),
    (CommandId::HelpToggle, &["?"]),
    (CommandId::HelpKeys, &["K", "f1"]),
];

const MUTT: PresetTable = &[
    (CommandId::NavNext, &["j", "down"]),
    (CommandId::NavPrev, &["k", "up"]),
    (CommandId::NavTop, &["=", "home"]),
    (CommandId::NavBottom, &["*", "end"]),
    (CommandId::NavPageDown, &["space", "pagedown"]),
    (CommandId::NavPageUp, &["-", "pageup"]),
    (CommandId::NavOpen, &["enter"]),
    (CommandId::WindowClose, &["q", "ctrl-c"]),
    (CommandId::AppQuit, &["Q"]),
    (CommandId::NavBack, &["i", "esc", "q"]),
    (CommandId::FocusSidebar, &["1"]),
    (CommandId::FocusList, &["2"]),
    (CommandId::FocusReader, &["3"]),
    (CommandId::FocusNext, &["tab"]),
    (CommandId::FocusPrev, &["shift-tab"]),
    (CommandId::ViewSidebar, &["\\"]),
    (CommandId::ViewZoom, &["z", "ctrl-z"]),
    (CommandId::ViewThreads, &["T"]),
    (CommandId::ViewWebReading, &["w"]),
    (CommandId::ViewDarkPages, &["W"]),
    (CommandId::MessageToggleUnread, &["N"]),
    (CommandId::MessageToggleStar, &["F"]),
    (CommandId::MessageToggleHtml, &["t"]),
    (CommandId::MessageOpenLink, &["o"]),
    (CommandId::MessageArchive, &["e"]),
    (CommandId::MessageDelete, &["d"]),
    (CommandId::MessageMove, &["s"]),
    (CommandId::MessageSnooze, &["b"]),
    (CommandId::SelectToggle, &["x"]),
    (CommandId::SelectRange, &["V"]),
    (CommandId::SelectAll, &["ctrl-a"]),
    (CommandId::SearchStart, &["/", "cmd-f"]),
    (CommandId::PaletteOpen, &[":", "ctrl-k"]),
    (CommandId::MessageAttachments, &["v"]),
    (CommandId::MessageOpenInBrowser, &["B"]),
    (CommandId::MessageLoadRemote, &["L"]),
    (CommandId::SenderAlwaysRemote, &["ctrl-l"]),
    (CommandId::MessageUnsubscribe, &["X"]),
    (CommandId::MessageHook, &["|"]),
    (CommandId::InviteAccept, &["y"]),
    (CommandId::InviteDecline, &["n"]),
    (CommandId::InviteTentative, &["M"]),
    (CommandId::AttachmentSave, &["s"]),
    (CommandId::AttachmentOpen, &["O"]),
    (CommandId::MessageCompose, &["m"]),
    (CommandId::MessageReply, &["r"]),
    (CommandId::MessageReplyAll, &["g"]),
    (CommandId::MessageForward, &["f"]),
    (
        CommandId::ComposeSend,
        &["cmd-enter", "ctrl-x", "ctrl-enter"],
    ),
    (CommandId::ComposeSendLater, &["cmd-L", "ctrl-l"]),
    (CommandId::ComposeSave, &["cmd-s", "ctrl-s"]),
    (CommandId::ComposeAttach, &["cmd-A", "ctrl-t"]),
    (CommandId::ComposeBcc, &["cmd-B", "ctrl-o"]),
    (CommandId::ComposeTemplate, &["cmd-T", "ctrl-r"]),
    (CommandId::ComposeVim, &["cmd-E", "ctrl-v"]),
    (CommandId::ComposeFields, &["cmd-up"]),
    (CommandId::ComposeBody, &["cmd-down"]),
    (CommandId::ComposeTips, &["cmd-/"]),
    (CommandId::ComposeSpellSuggest, &["cmd-G", "ctrl-g"]),
    (CommandId::ComposeSpellNext, &["cmd-;", "ctrl-y"]),
    (CommandId::EditCopy, &["cmd-c"]),
    (CommandId::EditCut, &["cmd-x"]),
    (CommandId::EditSelectAll, &["cmd-a"]),
    (CommandId::EditUndo, &["U"]),
    (CommandId::AccountAdd, &["A"]),
    (CommandId::AccountSignIn, &["S"]),
    (CommandId::AccountRemove, &["D"]),
    (CommandId::AccountEdit, &["E"]),
    (CommandId::AccountLetter, &["Y"]),
    // Two keys, so that it is never pressed by accident: it switches
    // settings off.
    (CommandId::AgentsStop, &["Z a"]),
    (CommandId::CommandLineInstall, &["Z c"]),
    // Rules: two keys each, beside the others that are about Quill and
    // not about the message under the cursor.
    (CommandId::RulesToggle, &["Z t"]),
    (CommandId::RulesRun, &["Z r"]),
    (CommandId::RulesRunAll, &["Z R"]),
    (CommandId::RulesApply, &["Z y"]),
    (CommandId::RulesCreate, &["Z N"]),
    (CommandId::RulesNew, &["Z n"]),
    (CommandId::RulesEdit, &["Z e"]),
    (CommandId::RulesLog, &["Z l"]),
    (CommandId::MessageCopy, &["C"]),
    (CommandId::MessagePutBack, &["P"]),
    (CommandId::FolderCreate, &["+"]),
    (CommandId::FolderRemove, &["_"]),
    (CommandId::MailboxSync, &["G", "ctrl-r", "cmd-r"]),
    (CommandId::SidebarOpen, &["right"]),
    (CommandId::SidebarClose, &["left"]),
    (CommandId::HelpToggle, &["?"]),
    (CommandId::HelpKeys, &["K", "f1"]),
];

impl Keymap {
    pub fn preset(preset: KeymapPreset) -> Self {
        let table = match preset {
            KeymapPreset::Vim => VIM,
            KeymapPreset::Mutt => MUTT,
        };
        Self {
            bindings: table
                .iter()
                .map(|(id, keys)| {
                    // The preset tables are checked by a test, so nothing is
                    // ever dropped here.
                    (
                        *id,
                        keys.iter().filter_map(|key| key.parse().ok()).collect(),
                    )
                })
                .collect(),
        }
    }

    /// Applies the text of a `keys.toml`: a table from command name to a key
    /// or a list of keys, each replacing that command's bindings.
    ///
    /// ```toml
    /// "nav.next" = ["j", "down", "ctrl-n"]
    /// "view.zoom" = "Z"
    /// "message.toggle_star" = []   # unbind
    /// ```
    ///
    /// On any error nothing is applied, so a bad edit keeps the last good keys.
    pub fn with_overrides(mut self, text: &str, registry: &Registry) -> Result<Self, KeymapError> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Keys {
            One(String),
            Many(Vec<String>),
        }
        let table: BTreeMap<String, Keys> =
            toml::from_str(text).map_err(|err| KeymapError::Parse {
                line: err.span().map_or(1, |span| {
                    text.as_bytes()[..span.start.min(text.len())]
                        .iter()
                        .filter(|&&byte| byte == b'\n')
                        .count()
                        + 1
                }),
                message: err.message().to_owned(),
            })?;
        for (name, keys) in table {
            let spec = registry
                .by_name(&name)
                .ok_or_else(|| KeymapError::UnknownCommand(name.clone()))?;
            let keys = match keys {
                Keys::One(key) => vec![key],
                Keys::Many(keys) => keys,
            };
            let sequences = keys
                .iter()
                .map(|key| key.parse::<KeySeq>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| KeymapError::BadKey {
                    command: name.clone(),
                    source,
                })?;
            match self.bindings.iter_mut().find(|(id, _)| *id == spec.id) {
                Some((_, bound)) => *bound = sequences,
                None => self.bindings.push((spec.id, sequences)),
            }
        }
        Ok(self)
    }

    /// The key sequences bound to a command, the preferred one first.
    pub fn keys_for(&self, id: CommandId) -> &[KeySeq] {
        self.bindings
            .iter()
            .find(|(bound, _)| *bound == id)
            .map_or(&[], |(_, keys)| keys.as_slice())
    }

    /// Looks up the keys pressed so far among the commands that apply in
    /// `context`. A complete match wins over a longer sequence that starts
    /// the same way, and an earlier command in the registry wins over a later.
    pub fn resolve(
        &self,
        registry: &Registry,
        pressed: &[KeyChord],
        context: Context,
    ) -> Resolution {
        let mut pending = false;
        for spec in registry.specs() {
            if !spec.applies_in(context) {
                continue;
            }
            for sequence in self.keys_for(spec.id) {
                if sequence.0 == pressed {
                    return Resolution::Command(spec.id);
                }
                pending |= sequence.0.starts_with(pressed);
            }
        }
        if pending {
            Resolution::Pending
        } else {
            Resolution::Unbound
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> KeyChord {
        text.parse().unwrap()
    }

    fn chords(text: &str) -> Vec<KeyChord> {
        text.parse::<KeySeq>().unwrap().0
    }

    #[test]
    fn parses_plain_and_modified_keys() {
        assert_eq!(chord("j"), KeyChord::plain(Key::Char('j')));
        assert_eq!(chord("G"), KeyChord::plain(Key::Char('G')));
        assert_eq!(chord("?"), KeyChord::plain(Key::Char('?')));
        assert_eq!(chord("enter"), KeyChord::plain(Key::Enter));
        assert_eq!(chord("space"), KeyChord::plain(Key::Char(' ')));
        assert_eq!(chord("f5"), KeyChord::plain(Key::F(5)));
        let k = chord("ctrl-k");
        assert!(k.mods.ctrl && !k.mods.alt && k.key == Key::Char('k'));
        let k = chord("cmd-shift-enter");
        assert!(k.mods.cmd && k.mods.shift && k.key == Key::Enter);
        let k = chord("opt-left");
        assert!(k.mods.alt && k.key == Key::Left);
    }

    #[test]
    fn the_minus_key_parses_alone_and_with_modifiers() {
        assert_eq!(chord("-"), KeyChord::plain(Key::Char('-')));
        let k = chord("ctrl--");
        assert!(k.mods.ctrl && k.key == Key::Char('-'));
    }

    #[test]
    fn shift_folds_into_letters() {
        assert_eq!(chord("shift-g"), chord("G"));
        assert!(chord("shift-tab").mods.shift);
    }

    #[test]
    fn bad_keys_are_rejected() {
        assert_eq!("".parse::<KeyChord>(), Err(KeyParseError::Empty));
        assert!(matches!(
            "hyper-x".parse::<KeyChord>(),
            Err(KeyParseError::UnknownModifier(_))
        ));
        assert!(matches!(
            "enterr".parse::<KeyChord>(),
            Err(KeyParseError::UnknownKey(_))
        ));
        assert!(matches!(
            "f99".parse::<KeyChord>(),
            Err(KeyParseError::UnknownKey(_))
        ));
        assert_eq!("   ".parse::<KeySeq>(), Err(KeyParseError::Empty));
    }

    #[test]
    fn display_round_trips() {
        for text in [
            "j",
            "G",
            "?",
            "-",
            "enter",
            "esc",
            "space",
            "ctrl-k",
            "ctrl--",
            "shift-tab",
            "ctrl-alt-shift-cmd-f12",
            "pagedown",
        ] {
            assert_eq!(chord(text).to_string(), text);
        }
        assert_eq!("g g".parse::<KeySeq>().unwrap().to_string(), "g g");
    }

    #[test]
    fn terminal_events_translate() {
        let event = |code, modifiers| KeyEvent::new(code, modifiers);
        assert_eq!(
            KeyChord::from_event(&event(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(chord("j"))
        );
        // Terminals disagree on whether shift-g arrives as 'G' or as 'g' with
        // shift. Both must mean G.
        assert_eq!(
            KeyChord::from_event(&event(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(chord("G"))
        );
        assert_eq!(
            KeyChord::from_event(&event(KeyCode::Char('g'), KeyModifiers::SHIFT)),
            Some(chord("G"))
        );
        assert_eq!(
            KeyChord::from_event(&event(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            Some(chord("?"))
        );
        assert_eq!(
            KeyChord::from_event(&event(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(chord("shift-tab"))
        );
        assert_eq!(
            KeyChord::from_event(&event(KeyCode::Enter, KeyModifiers::SUPER)),
            Some(chord("cmd-enter"))
        );
        assert_eq!(
            KeyChord::from_event(&event(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(chord("ctrl-c"))
        );
        let mut release = event(KeyCode::Char('j'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(KeyChord::from_event(&release), None);
    }

    #[test]
    fn every_preset_key_parses_and_every_command_is_covered() {
        let registry = Registry::builtin();
        for (table, preset) in [(VIM, KeymapPreset::Vim), (MUTT, KeymapPreset::Mutt)] {
            let keymap = Keymap::preset(preset);
            for (id, keys) in table {
                for key in *keys {
                    assert!(key.parse::<KeySeq>().is_ok(), "{key}");
                }
                assert_eq!(keymap.keys_for(*id).len(), keys.len());
            }
            for spec in registry.specs() {
                assert!(
                    !keymap.keys_for(spec.id).is_empty(),
                    "{preset:?} leaves {} unbound",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn no_preset_binds_one_key_to_two_commands_in_the_same_context_by_accident() {
        // The only intended overlap is close and back sharing `q`.
        let registry = Registry::builtin();
        for preset in [KeymapPreset::Vim, KeymapPreset::Mutt] {
            let keymap = Keymap::preset(preset);
            for (i, a) in registry.specs().iter().enumerate() {
                for b in &registry.specs()[i + 1..] {
                    let shared = a.contexts.iter().any(|c| b.contexts.contains(c));
                    if !shared {
                        continue;
                    }
                    for key in keymap.keys_for(a.id) {
                        let clash = keymap.keys_for(b.id).contains(key);
                        let intended = a.id == CommandId::WindowClose
                            && b.id == CommandId::NavBack
                            && key.to_string() == "q";
                        assert!(
                            !clash || intended,
                            "{preset:?}: {key} is bound to {} and {}",
                            a.name,
                            b.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn resolves_single_keys_and_sequences() {
        let registry = Registry::builtin();
        let keymap = Keymap::preset(KeymapPreset::Vim);
        let resolve = |keys: &str, context| keymap.resolve(&registry, &chords(keys), context);
        assert_eq!(
            resolve("j", Context::List),
            Resolution::Command(CommandId::NavNext)
        );
        assert_eq!(resolve("g", Context::List), Resolution::Pending);
        assert_eq!(
            resolve("g g", Context::List),
            Resolution::Command(CommandId::NavTop)
        );
        assert_eq!(resolve("g Q", Context::List), Resolution::Unbound);
        assert_eq!(resolve("ctrl-q", Context::List), Resolution::Unbound);
    }

    #[test]
    fn the_context_decides_what_a_key_means() {
        let registry = Registry::builtin();
        let keymap = Keymap::preset(KeymapPreset::Vim);
        let resolve = |keys: &str, context| keymap.resolve(&registry, &chords(keys), context);
        assert_eq!(
            resolve("q", Context::List),
            Resolution::Command(CommandId::WindowClose)
        );
        assert_eq!(
            resolve("Q", Context::List),
            Resolution::Command(CommandId::AppQuit)
        );
        assert_eq!(
            resolve("q", Context::Help),
            Resolution::Command(CommandId::NavBack)
        );
        // Starring makes no sense in the sidebar, so `s` is free there.
        assert_eq!(resolve("s", Context::Sidebar), Resolution::Unbound);
        assert_eq!(
            resolve("s", Context::Reader),
            Resolution::Command(CommandId::MessageToggleStar)
        );
    }

    #[test]
    fn the_mutt_preset_differs_where_mutt_does() {
        let registry = Registry::builtin();
        let keymap = Keymap::preset(KeymapPreset::Mutt);
        let resolve = |keys: &str, context| keymap.resolve(&registry, &chords(keys), context);
        assert_eq!(
            resolve("N", Context::List),
            Resolution::Command(CommandId::MessageToggleUnread)
        );
        assert_eq!(
            resolve("i", Context::Reader),
            Resolution::Command(CommandId::NavBack)
        );
        assert_eq!(resolve("u", Context::List), Resolution::Unbound);
    }

    #[test]
    fn overrides_replace_a_commands_keys() {
        let registry = Registry::builtin();
        let keymap = Keymap::preset(KeymapPreset::Vim)
            .with_overrides(
                "\"nav.next\" = [\"n\", \"ctrl-n\"]\n\"view.zoom\" = \"Z\"\n\"message.toggle_star\" = []\n",
                &registry,
            )
            .unwrap();
        let resolve = |keys: &str| keymap.resolve(&registry, &chords(keys), Context::List);
        assert_eq!(resolve("n"), Resolution::Command(CommandId::NavNext));
        assert_eq!(resolve("ctrl-n"), Resolution::Command(CommandId::NavNext));
        assert_eq!(resolve("j"), Resolution::Unbound);
        assert_eq!(resolve("Z"), Resolution::Command(CommandId::ViewZoom));
        assert_eq!(resolve("z"), Resolution::Unbound);
        assert_eq!(resolve("s"), Resolution::Unbound);
        // Untouched commands keep their preset keys.
        assert_eq!(resolve("k"), Resolution::Command(CommandId::NavPrev));
    }

    #[test]
    fn bad_overrides_are_rejected_whole() {
        let registry = Registry::builtin();
        let base = || Keymap::preset(KeymapPreset::Vim);
        assert_eq!(
            base().with_overrides("\"nav.nxt\" = \"n\"\n", &registry),
            Err(KeymapError::UnknownCommand("nav.nxt".to_owned()))
        );
        assert!(matches!(
            base().with_overrides("\"nav.next\" = \"hyper-n\"\n", &registry),
            Err(KeymapError::BadKey { .. })
        ));
        assert!(matches!(
            base().with_overrides("\"nav.next\" = \n", &registry),
            Err(KeymapError::Parse { line: 1, .. })
        ));
        assert!(matches!(
            base().with_overrides("\"nav.next\" = 5\n", &registry),
            Err(KeymapError::Parse { .. })
        ));
        assert_eq!(base().with_overrides("", &registry), Ok(base()));
    }
}
