//! The messages of the shell-core bridge, as Rust types.
//!
//! `schema/bridge.json` is the definition. These types, the Swift types in
//! `app/Sources/Bridge` and the examples in `schema/examples` are all tested
//! against it; change them together.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A message the core sends to the shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum CoreToShell {
    /// Every command, once at startup, in menu order.
    #[serde(rename = "commands.list")]
    CommandsList(CommandsList),
    /// The whole of what the shell mirrors, whenever any of it changes.
    #[serde(rename = "state.update")]
    StateUpdate(StateUpdate),
    /// Show files in a Quick Look pane beside the terminal, one at a time.
    #[serde(rename = "pane.preview")]
    PanePreview(PanePreview),
    /// Play a video or audio file in a player pane.
    #[serde(rename = "pane.play")]
    PanePlay(PaneFile),
    /// Make a PNG thumbnail of a file. Answered by `thumbnail.made`.
    #[serde(rename = "thumbnail.make")]
    ThumbnailMake(ThumbnailMake),
    /// Open a file in the application the Mac opens its kind with.
    #[serde(rename = "file.open")]
    FileOpen(FileOpen),
    /// Ask where to save a copy of a file, and save it. Answered by
    /// `files.saved`.
    #[serde(rename = "files.save")]
    FilesSave(FilesSave),
    /// Close the second pane and return focus to the terminal.
    #[serde(rename = "pane.close")]
    PaneClose(NoParams),
    /// Open a web or mail address in the user's browser or mail program.
    #[serde(rename = "link.open")]
    LinkOpen(LinkOpen),
    /// Ask the user to choose files with the native open panel. What they
    /// choose arrives as `files.dropped`.
    #[serde(rename = "files.pick")]
    FilesPick(NoParams),
    /// Put text on the system clipboard.
    #[serde(rename = "clipboard.write")]
    ClipboardWrite(ClipboardWrite),
    /// Show a notification. What the user does with it arrives as
    /// `notify.used`.
    #[serde(rename = "notify.post")]
    NotifyPost(Notification),
    /// Take notifications down.
    #[serde(rename = "notify.clear")]
    NotifyClear(NotifyClear),
    /// Check the spelling of some text. Answered by `spell.checked`.
    #[serde(rename = "spell.check")]
    SpellCheck(SpellCheck),
    /// Ask what a misspelt word might have been. Answered by
    /// `spell.suggested`.
    #[serde(rename = "spell.suggest")]
    SpellSuggest(SpellSuggest),
    /// Run an editor on a draft file, in a pane. Answered by
    /// `editor.exited`.
    #[serde(rename = "pane.editor")]
    PaneEditor(PaneEditor),
    /// Show a form in a sheet on the window, in place of whatever form the
    /// sheet shows now. What the user does with it arrives as `form.used`.
    #[serde(rename = "form.show")]
    FormShow(FormShow),
    /// Take the sheet down: the form is over.
    #[serde(rename = "form.close")]
    FormClose(NoParams),
    /// Show sanitized HTML in the reading pane: the web view that stands
    /// in the reader's place while web reading is on. It stays until
    /// `reading.close`, and never takes the keyboard by itself.
    #[serde(rename = "reading.show")]
    ReadingShow(ReadingShow),
    /// Scroll what the reading pane shows.
    #[serde(rename = "reading.scroll")]
    ReadingScroll(ReadingScroll),
    /// Take the reading pane away: web reading is off.
    #[serde(rename = "reading.close")]
    ReadingClose(NoParams),
    /// Where the reading pane stands: how many columns the terminal keeps
    /// beside it.
    #[serde(rename = "reading.place")]
    ReadingPlace(ReadingPlace),
    /// Where the editor of `pane.editor` goes on the terminal's screen,
    /// and whether it has the keys.
    #[serde(rename = "editor.place")]
    EditorPlace(EditorPlace),
    /// What the settings window shows: every pane with its settings as
    /// they stand. What the user does there arrives as `settings.used`.
    #[serde(rename = "settings.show")]
    SettingsShow(SettingsShow),
    /// Ask the user to choose a theme to import, with the shell's own
    /// chooser. One that is imported arrives as `themes.imported`.
    #[serde(rename = "themes.pick")]
    ThemesPick(NoParams),
    /// Something to do for the editor messages are written in, which
    /// only the shell can do: asked for from the settings window.
    #[serde(rename = "editor.setup")]
    EditorSetup(EditorSetup),
    /// Every key Quill has, under headings, for the shell's window of
    /// them: at startup, and again whenever the keys change.
    #[serde(rename = "keys.list")]
    KeysList(KeysList),
    /// Bring up the window of keys, showing the latest `keys.list`.
    #[serde(rename = "keys.show")]
    KeysShow(NoParams),
}

/// A message the shell sends to the core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum ShellToCore {
    /// A menu item or toolbar button was used.
    #[serde(rename = "command.run")]
    CommandRun(CommandRun),
    /// The user chose files in the open panel or dropped them on the window.
    #[serde(rename = "files.dropped")]
    FilesDropped(FilesDropped),
    /// The answer to `thumbnail.make`.
    #[serde(rename = "thumbnail.made")]
    ThumbnailMade(ThumbnailMade),
    /// The answer to `files.save`.
    #[serde(rename = "files.saved")]
    FilesSaved(FilesSaved),
    /// A notification, or one of its actions, was clicked.
    #[serde(rename = "notify.used")]
    NotifyUsed(NotifyUsed),
    /// The Mac asked Quill to open a `mailto:` address.
    #[serde(rename = "url.open")]
    UrlOpen(UrlOpen),
    /// The answer to `spell.check`.
    #[serde(rename = "spell.checked")]
    SpellChecked(SpellChecked),
    /// The answer to `spell.suggest`.
    #[serde(rename = "spell.suggested")]
    SpellSuggested(SpellSuggested),
    /// The answer to `pane.editor`.
    #[serde(rename = "editor.exited")]
    EditorExited(EditorExited),
    /// A button of the form in the sheet was pressed, or one of its
    /// choices changed.
    #[serde(rename = "form.used")]
    FormUsed(FormUsed),
    /// How many columns the terminal would have with the window to
    /// itself, whenever that changes.
    #[serde(rename = "window.resized")]
    WindowResized(WindowResized),
    /// The settings window was opened: it wants `settings.show`.
    #[serde(rename = "settings.opened")]
    SettingsOpened(NoParams),
    /// The settings window was closed.
    #[serde(rename = "settings.closed")]
    SettingsClosed(NoParams),
    /// A setting was changed in the settings window, or something was
    /// asked for from one of its lists.
    #[serde(rename = "settings.used")]
    SettingsUsed(SettingsUsed),
    /// Whether the Mac is in its dark appearance: once the shell knows,
    /// and whenever it changes.
    #[serde(rename = "appearance.changed")]
    AppearanceChanged(AppearanceChanged),
    /// A theme file was put in the `themes` folder beside the settings
    /// file, under this name.
    #[serde(rename = "themes.imported")]
    ThemesImported(ThemesImported),
    /// Whether the shell holds a vimrc the user imported: once as it
    /// starts, and whenever that changes.
    #[serde(rename = "vimrc.changed")]
    VimrcChanged(VimrcChanged),
    /// The app is quitting: flush and exit cleanly.
    #[serde(rename = "app.will_quit")]
    AppWillQuit(NoParams),
}

/// `"params": {}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandsList {
    pub commands: Vec<CommandInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandInfo {
    /// The stable name, as used in `keys.toml` and `command.run`.
    pub name: String,
    pub title: String,
    /// Key sequences in Quill's notation, the preferred one first.
    pub keys: Vec<String>,
    /// The menu the command appears in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu: Option<Menu>,
    /// An SF Symbol name. A command with one may be shown in the toolbar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Menu {
    App,
    File,
    Edit,
    View,
    Mailbox,
    Message,
    Window,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateUpdate {
    /// Where the keyboard focus is inside the terminal UI.
    pub context: Context,
    /// Names of the commands that would do something right now.
    pub enabled: Vec<String>,
    /// Names of the commands that switch something on and off, and have
    /// it on right now: their menu items are shown checked.
    pub checked: Vec<String>,
    /// Unread messages across all inboxes, for the Dock badge.
    pub unread: u32,
    /// The window title.
    pub title: String,
    /// The colour the terminal UI is drawn on. Any other terminal the
    /// shell shows beside it (the editor's) is given the same, and so are
    /// the cells the core does not paint. Left out when the theme leaves
    /// the background to the terminal: the shell's terminals then keep
    /// their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<TerminalColor>,
    /// The colour of the terminal UI's text, likewise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<TerminalColor>,
    /// Whether the app is to be light, dark, or as the Mac is: what the
    /// settings say. The shell's own parts (its title bar, sheets, the
    /// settings window, the reading pane) go by it. Left out, it is as
    /// the Mac is.
    #[serde(default, skip_serializing_if = "Appearance::is_system")]
    pub appearance: Appearance,
}

/// Whether the app is light, dark, or as the Mac is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    fn is_system(&self) -> bool {
        *self == Self::System
    }
}

/// What the shell says of the Mac's appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceChanged {
    /// The Mac is in its dark appearance.
    pub dark: bool,
}

/// Whether the shell holds a vimrc the user imported, for the Vim that
/// runs inside the sandbox. The file is the shell's alone: the core is told
/// only that there is one, so that the settings window can offer to take
/// it away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VimrcChanged {
    pub imported: bool,
}

/// Something only the shell can do for the editor messages are written
/// in. A word, never a path: what file and what folder are the shell's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorSetup {
    pub action: EditorSetupAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorSetupAction {
    /// Ask the user for a vimrc and keep a copy of it, for the Vim that
    /// runs inside the sandbox. Answered by `vimrc.changed` if one is
    /// taken.
    ImportVimrc,
    /// Take the copy away. Answered by `vimrc.changed`.
    RemoveVimrc,
    /// Show the folder the user's own editor script goes in.
    ShowScripts,
}

/// Every key Quill has, as the shell's window of them shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeysList {
    /// What the window is called.
    pub title: String,
    pub sections: Vec<KeySection>,
}

/// The keys of one kind, or of one place, under a heading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeySection {
    pub title: String,
    pub keys: Vec<KeyLine>,
}

/// What one command is called and the keys it is on, in Quill's
/// notation, the preferred one first. None at all for a command no key
/// is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyLine {
    pub title: String,
    pub keys: Vec<String>,
}

/// A theme the shell has put in the `themes` folder. It cannot be built or
/// decoded with a name no theme can have: one that could point outside
/// the folder, or be hidden in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ThemesImportedParams", into = "ThemesImportedParams")]
pub struct ThemesImported {
    name: String,
}

impl ThemesImported {
    pub fn new(name: &str) -> Option<Self> {
        quill_core::config::is_theme_name(name).then(|| Self {
            name: name.to_owned(),
        })
    }

    /// The theme's name: the name of its file in the folder.
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemesImportedParams {
    name: String,
}

impl TryFrom<ThemesImportedParams> for ThemesImported {
    type Error = String;

    fn try_from(params: ThemesImportedParams) -> Result<Self, Self::Error> {
        Self::new(&params.name).ok_or_else(|| "not a name a theme can have".to_owned())
    }
}

impl From<ThemesImported> for ThemesImportedParams {
    fn from(imported: ThemesImported) -> Self {
        Self {
            name: imported.name,
        }
    }
}

/// A colour as `#rrggbb`, in lower case. It cannot be built or decoded as
/// anything else: a name or a place in a palette means whatever the
/// terminal reading it says, and this is for telling a terminal what to be.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TerminalColor(String);

impl TerminalColor {
    pub fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self(format!("#{red:02x}{green:02x}{blue:02x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TerminalColor {
    type Error = String;

    fn try_from(color: String) -> Result<Self, Self::Error> {
        let digits = color.strip_prefix('#').unwrap_or_default();
        if digits.len() == 6
            && digits
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            Ok(Self(color))
        } else {
            Err("not a #rrggbb colour".to_owned())
        }
    }
}

impl From<TerminalColor> for String {
    fn from(color: TerminalColor) -> Self {
        color.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Context {
    Sidebar,
    List,
    Reader,
    Help,
    Links,
    Setup,
    Certificate,
    Folders,
    Compose,
    Palette,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkOpen {
    pub url: OpenableUrl,
}

/// An address Quill will open: `http`, `https` or `mailto`, in printable
/// ASCII. It cannot be built or decoded holding anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpenableUrl(String);

impl OpenableUrl {
    pub fn new(url: &str) -> Option<Self> {
        quill_core::links::is_openable(url).then(|| Self(url.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for OpenableUrl {
    type Error = String;

    fn try_from(url: String) -> Result<Self, Self::Error> {
        if quill_core::links::is_openable(&url) {
            Ok(Self(url))
        } else {
            Err("not an http, https or mailto address".to_owned())
        }
    }
}

impl From<OpenableUrl> for String {
    fn from(url: OpenableUrl) -> Self {
        url.0
    }
}

/// The absolute path of a file. It cannot be built or decoded holding a
/// relative one: a path means the same to the core and the shell only when
/// it does not depend on where either is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AbsolutePath(String);

impl AbsolutePath {
    pub fn new(path: &std::path::Path) -> Option<Self> {
        let text = path.to_str()?;
        path.is_absolute().then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AbsolutePath {
    type Error = String;

    fn try_from(path: String) -> Result<Self, Self::Error> {
        if path.starts_with('/') {
            Ok(Self(path))
        } else {
            Err("not an absolute path".to_owned())
        }
    }
}

impl From<AbsolutePath> for String {
    fn from(path: AbsolutePath) -> Self {
        path.0
    }
}

/// A file to show in a pane, and what to call it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneFile {
    pub path: AbsolutePath,
    pub title: String,
}

/// Files for the Quick Look pane: at least one, and which to show first.
/// It cannot be built or decoded with an index past the last of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PanePreviewParams", into = "PanePreviewParams")]
pub struct PanePreview {
    items: Vec<PaneFile>,
    index: usize,
}

impl PanePreview {
    pub fn new(items: Vec<PaneFile>, index: usize) -> Option<Self> {
        (index < items.len()).then_some(Self { items, index })
    }

    pub fn items(&self) -> &[PaneFile] {
        &self.items
    }

    pub fn index(&self) -> usize {
        self.index
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PanePreviewParams {
    items: Vec<PaneFile>,
    index: usize,
}

impl TryFrom<PanePreviewParams> for PanePreview {
    type Error = String;

    fn try_from(params: PanePreviewParams) -> Result<Self, Self::Error> {
        Self::new(params.items, params.index)
            .ok_or_else(|| "index must be one of the items, and there must be one".to_owned())
    }
}

impl From<PanePreview> for PanePreviewParams {
    fn from(preview: PanePreview) -> Self {
        Self {
            items: preview.items,
            index: preview.index,
        }
    }
}

/// The most lines the reading pane's header may be given.
pub const MAX_READING_HEADER_LINES: usize = 8;
/// The longest any one of them may be, in bytes.
pub const MAX_READING_HEADER_BYTES: usize = 2000;

/// The most buttons a notice above a page may carry.
pub const MAX_READING_ACTIONS: usize = 3;
/// The longest a button's title may be, in bytes.
pub const MAX_READING_ACTION_TITLE: usize = 64;

/// Something to tell the user about the page in the reading pane, in a bar
/// of its own between the header and the page: that remote pictures were
/// left out of it, say. Each action is a button that runs a command, as a
/// menu item would. It cannot be built or decoded with words that are
/// empty, too long or more than one line, with more buttons than the bar
/// has room for, or with a button that names no command or says nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReadingNoticeParams", into = "ReadingNoticeParams")]
pub struct ReadingNotice {
    text: String,
    actions: Vec<ReadingAction>,
}

/// A button in a notice: the command it runs, by the name `command.run`
/// carries, and what the button says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadingAction {
    pub command: String,
    pub title: String,
}

/// One line of 1 to `most` bytes.
fn one_line(text: &str, most: usize) -> bool {
    !text.is_empty() && text.len() <= most && !text.chars().any(char::is_control)
}

/// A command's name as `commands.list` gives it: `message.load_remote`.
fn command_name(name: &str) -> bool {
    let mut parts = name.split('.');
    let first = parts.next().unwrap_or_default();
    let mut rest = parts.peekable();
    !first.is_empty()
        && first.bytes().all(|b| b.is_ascii_lowercase())
        && rest.peek().is_some()
        && rest.all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        })
}

impl ReadingNotice {
    pub fn new(text: &str, actions: Vec<ReadingAction>) -> Option<Self> {
        let fits = one_line(text, MAX_READING_HEADER_BYTES)
            && actions.len() <= MAX_READING_ACTIONS
            && actions.iter().all(|action| {
                command_name(&action.command) && one_line(&action.title, MAX_READING_ACTION_TITLE)
            });
        fits.then(|| Self {
            text: text.to_owned(),
            actions,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn actions(&self) -> &[ReadingAction] {
        &self.actions
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadingNoticeParams {
    text: String,
    actions: Vec<ReadingAction>,
}

impl TryFrom<ReadingNoticeParams> for ReadingNotice {
    type Error = String;

    fn try_from(params: ReadingNoticeParams) -> Result<Self, Self::Error> {
        Self::new(&params.text, params.actions).ok_or_else(|| {
            format!(
                "a notice is one line of 1 to {MAX_READING_HEADER_BYTES} bytes with at most \
                 {MAX_READING_ACTIONS} actions, each a command and a title of 1 to \
                 {MAX_READING_ACTION_TITLE} bytes"
            )
        })
    }
}

impl From<ReadingNotice> for ReadingNoticeParams {
    fn from(notice: ReadingNotice) -> Self {
        Self {
            text: notice.text,
            actions: notice.actions,
        }
    }
}

/// A page for the reading pane, and what to say above it. It cannot be
/// built or decoded with more header lines than the pane has room for, or
/// with a line that is empty, too long or more than one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReadingShowParams", into = "ReadingShowParams")]
pub struct ReadingShow {
    path: AbsolutePath,
    title: String,
    remote: bool,
    header: Vec<String>,
    notice: Option<ReadingNotice>,
}

impl ReadingShow {
    pub fn new(path: AbsolutePath, title: &str, remote: bool, header: Vec<String>) -> Option<Self> {
        let fits = header.len() <= MAX_READING_HEADER_LINES
            && header.iter().all(|line| {
                !line.is_empty()
                    && line.len() <= MAX_READING_HEADER_BYTES
                    && !line.chars().any(char::is_control)
            });
        fits.then(|| Self {
            path,
            title: title.to_owned(),
            remote,
            header,
            notice: None,
        })
    }

    /// The same page with `notice` in a bar above it, or with none.
    #[must_use]
    pub fn with_notice(mut self, notice: Option<ReadingNotice>) -> Self {
        self.notice = notice;
        self
    }

    /// What to tell the user about the page, with buttons to act on it.
    pub fn notice(&self) -> Option<&ReadingNotice> {
        self.notice.as_ref()
    }

    /// The sanitized HTML. Files beside it in its folder may be loaded.
    pub fn path(&self) -> &AbsolutePath {
        &self.path
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The user has allowed this message to load remote content.
    pub fn remote(&self) -> bool {
        self.remote
    }

    /// Lines of plain text shown above the page, outside it: who the
    /// message is from and to, and when it was sent.
    pub fn header(&self) -> &[String] {
        &self.header
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadingShowParams {
    path: AbsolutePath,
    title: String,
    remote: bool,
    header: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notice: Option<ReadingNotice>,
}

impl TryFrom<ReadingShowParams> for ReadingShow {
    type Error = String;

    fn try_from(params: ReadingShowParams) -> Result<Self, Self::Error> {
        Self::new(params.path, &params.title, params.remote, params.header)
            .map(|show| show.with_notice(params.notice))
            .ok_or_else(|| {
                format!(
                    "header must be at most {MAX_READING_HEADER_LINES} lines, each one line of 1 \
                     to {MAX_READING_HEADER_BYTES} bytes"
                )
            })
    }
}

impl From<ReadingShow> for ReadingShowParams {
    fn from(show: ReadingShow) -> Self {
        Self {
            path: show.path,
            title: show.title,
            remote: show.remote,
            header: show.header,
            notice: show.notice,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadingScroll {
    pub by: ScrollBy,
}

/// The reading pane's place in the window, said in the terminal's own
/// measure: the terminal keeps `columns` columns, and the reading pane has
/// the rest of the window. The core works this out from the columns the
/// whole window has room for (`window.resized`), so that the panes it
/// draws are as wide beside the reading pane as they are beside its own
/// reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ColumnsParams", into = "ColumnsParams")]
pub struct ReadingPlace {
    columns: u16,
}

impl ReadingPlace {
    /// `None` for no columns at all: a terminal is never that narrow.
    pub fn new(columns: u16) -> Option<Self> {
        (columns > 0).then_some(Self { columns })
    }

    pub fn columns(self) -> u16 {
        self.columns
    }
}

/// Where the editor that `pane.editor` started goes, said in the
/// terminal's own cells, and whether it has the keys.
///
/// The editor runs in a terminal of its own that the shell lays over the
/// core's, exactly over the cells where the text of the message being
/// written would be drawn. The core says which cells those are, and says
/// again whenever they change. With no cells the editor is not on show
/// (something of the core's is drawn where it would be) and cannot have
/// the keys; it goes on running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EditorPlaceParams", into = "EditorPlaceParams")]
pub struct EditorPlace {
    cells: Option<EditorCells>,
    keys: bool,
}

/// The cells an editor has: a rectangle of the terminal's screen, from
/// its top left, and how many of its rows from the top the core has
/// drawn something over. Those rows are not shown; the editor is not
/// made smaller for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorCells {
    pub column: u16,
    pub row: u16,
    pub columns: u16,
    pub rows: u16,
    pub covered: u16,
}

impl EditorPlace {
    /// The editor is not on show, and the terminal has the keys.
    pub fn hidden() -> Self {
        Self {
            cells: None,
            keys: false,
        }
    }

    /// The editor in these cells. `None` for a rectangle with no columns
    /// or no rows: an editor is never run in nothing.
    pub fn at(cells: EditorCells, keys: bool) -> Option<Self> {
        (cells.columns > 0 && cells.rows > 0).then_some(Self {
            cells: Some(cells),
            keys,
        })
    }

    pub fn cells(self) -> Option<EditorCells> {
        self.cells
    }

    /// Whether what is typed goes to the editor.
    pub fn keys(self) -> bool {
        self.keys
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditorPlaceParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    place: Option<EditorCells>,
    keys: bool,
}

impl TryFrom<EditorPlaceParams> for EditorPlace {
    type Error = String;

    fn try_from(params: EditorPlaceParams) -> Result<Self, Self::Error> {
        match params.place {
            Some(cells) => Self::at(cells, params.keys)
                .ok_or_else(|| "an editor's place has at least one column and one row".to_owned()),
            None if params.keys => {
                Err("an editor that is not on show cannot have the keys".to_owned())
            }
            None => Ok(Self::hidden()),
        }
    }
}

impl From<EditorPlace> for EditorPlaceParams {
    fn from(place: EditorPlace) -> Self {
        Self {
            place: place.cells,
            keys: place.keys,
        }
    }
}

/// How many columns the terminal would have if it had the whole window:
/// what the window has room for, whatever stands beside the terminal now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ColumnsParams", into = "ColumnsParams")]
pub struct WindowResized {
    columns: u16,
}

impl WindowResized {
    /// `None` for no columns at all.
    pub fn new(columns: u16) -> Option<Self> {
        (columns > 0).then_some(Self { columns })
    }

    pub fn columns(self) -> u16 {
        self.columns
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnsParams {
    columns: u16,
}

impl TryFrom<ColumnsParams> for ReadingPlace {
    type Error = String;

    fn try_from(params: ColumnsParams) -> Result<Self, Self::Error> {
        Self::new(params.columns).ok_or_else(|| "columns must be at least 1".to_owned())
    }
}

impl From<ReadingPlace> for ColumnsParams {
    fn from(place: ReadingPlace) -> Self {
        Self {
            columns: place.columns,
        }
    }
}

impl TryFrom<ColumnsParams> for WindowResized {
    type Error = String;

    fn try_from(params: ColumnsParams) -> Result<Self, Self::Error> {
        Self::new(params.columns).ok_or_else(|| "columns must be at least 1".to_owned())
    }
}

impl From<WindowResized> for ColumnsParams {
    fn from(resized: WindowResized) -> Self {
        Self {
            columns: resized.columns,
        }
    }
}

/// How far to scroll the reading pane, and which way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollBy {
    PageDown,
    PageUp,
}

/// The longest side a thumbnail may be asked for, in pixels.
pub const MAX_THUMBNAIL: u32 = 4096;
/// The shortest.
pub const MIN_THUMBNAIL: u32 = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ThumbnailMakeParams", into = "ThumbnailMakeParams")]
pub struct ThumbnailMake {
    id: u64,
    path: AbsolutePath,
    out: AbsolutePath,
    size: u32,
}

impl ThumbnailMake {
    /// `size` is brought within what may be asked for.
    pub fn new(id: u64, path: AbsolutePath, out: AbsolutePath, size: u32) -> Self {
        Self {
            id,
            path,
            out,
            size: size.clamp(MIN_THUMBNAIL, MAX_THUMBNAIL),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThumbnailMakeParams {
    id: u64,
    path: AbsolutePath,
    out: AbsolutePath,
    size: u32,
}

impl TryFrom<ThumbnailMakeParams> for ThumbnailMake {
    type Error = String;

    fn try_from(params: ThumbnailMakeParams) -> Result<Self, Self::Error> {
        if (MIN_THUMBNAIL..=MAX_THUMBNAIL).contains(&params.size) {
            Ok(Self {
                id: params.id,
                path: params.path,
                out: params.out,
                size: params.size,
            })
        } else {
            Err(format!(
                "size must be from {MIN_THUMBNAIL} to {MAX_THUMBNAIL}"
            ))
        }
    }
}

impl From<ThumbnailMake> for ThumbnailMakeParams {
    fn from(make: ThumbnailMake) -> Self {
        Self {
            id: make.id,
            path: make.path,
            out: make.out,
            size: make.size,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOpen {
    pub path: AbsolutePath,
}

/// A file to save a copy of, and the name to offer. The name is never
/// empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FilesSaveParams", into = "FilesSaveParams")]
pub struct FilesSave {
    path: AbsolutePath,
    name: String,
}

impl FilesSave {
    pub fn new(path: AbsolutePath, name: &str) -> Option<Self> {
        (!name.is_empty()).then(|| Self {
            path,
            name: name.to_owned(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesSaveParams {
    path: AbsolutePath,
    name: String,
}

impl TryFrom<FilesSaveParams> for FilesSave {
    type Error = String;

    fn try_from(params: FilesSaveParams) -> Result<Self, Self::Error> {
        Self::new(params.path, &params.name).ok_or_else(|| "name is empty".to_owned())
    }
}

impl From<FilesSave> for FilesSaveParams {
    fn from(save: FilesSave) -> Self {
        Self {
            path: save.path,
            name: save.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThumbnailMade {
    pub id: u64,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesSaved {
    pub name: String,
    pub saved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRun {
    pub name: String,
}

/// The longest id a notification may have, and the longest name an action
/// may have.
pub const MAX_NOTIFICATION_ID: usize = 128;
pub const MAX_ACTION_NAME: usize = 64;
/// The most buttons a notification may carry.
pub const MAX_ACTIONS: usize = 4;

/// A notification to show. It cannot be built or decoded with an empty or
/// overlong id, no title, or more actions than a notification can carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NotificationParams", into = "NotificationParams")]
pub struct Notification {
    id: String,
    title: String,
    subtitle: Option<String>,
    body: String,
    actions: Vec<NotificationAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationAction {
    /// What comes back in `notify.used`.
    pub name: String,
    /// What the button says.
    pub title: String,
    /// Using it brings the window forward: what it starts needs the user
    /// there, as a reply does.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub foreground: bool,
}

impl Notification {
    pub fn new(
        id: &str,
        title: &str,
        subtitle: Option<&str>,
        body: &str,
        actions: &[(&str, &str, bool)],
    ) -> Option<Self> {
        Self::try_from(NotificationParams {
            id: id.to_owned(),
            title: title.to_owned(),
            subtitle: subtitle.map(str::to_owned),
            body: body.to_owned(),
            actions: actions
                .iter()
                .map(|(name, title, foreground)| NotificationAction {
                    name: (*name).to_owned(),
                    title: (*title).to_owned(),
                    foreground: *foreground,
                })
                .collect(),
        })
        .ok()
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationParams {
    id: String,
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subtitle: Option<String>,
    body: String,
    actions: Vec<NotificationAction>,
}

fn valid_notification_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_NOTIFICATION_ID
}

impl TryFrom<NotificationParams> for Notification {
    type Error = String;

    fn try_from(params: NotificationParams) -> Result<Self, Self::Error> {
        if !valid_notification_id(&params.id) {
            return Err(format!(
                "a notification's id is 1 to {MAX_NOTIFICATION_ID} bytes"
            ));
        }
        if params.title.is_empty() {
            return Err("a notification needs a title".to_owned());
        }
        if params.actions.len() > MAX_ACTIONS {
            return Err(format!("a notification has at most {MAX_ACTIONS} actions"));
        }
        if params.actions.iter().any(|action| {
            action.name.is_empty() || action.name.len() > MAX_ACTION_NAME || action.title.is_empty()
        }) {
            return Err(format!(
                "an action needs a title and a name of 1 to {MAX_ACTION_NAME} bytes"
            ));
        }
        Ok(Self {
            id: params.id,
            title: params.title,
            subtitle: params.subtitle,
            body: params.body,
            actions: params.actions,
        })
    }
}

impl From<Notification> for NotificationParams {
    fn from(notification: Notification) -> Self {
        Self {
            id: notification.id,
            title: notification.title,
            subtitle: notification.subtitle,
            body: notification.body,
            actions: notification.actions,
        }
    }
}

/// Notifications to take down: at least one, each by a valid id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NotifyClearParams", into = "NotifyClearParams")]
pub struct NotifyClear {
    ids: Vec<String>,
}

impl NotifyClear {
    pub fn new(ids: Vec<String>) -> Option<Self> {
        Self::try_from(NotifyClearParams { ids }).ok()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyClearParams {
    ids: Vec<String>,
}

impl TryFrom<NotifyClearParams> for NotifyClear {
    type Error = String;

    fn try_from(params: NotifyClearParams) -> Result<Self, Self::Error> {
        if params.ids.is_empty() || !params.ids.iter().all(|id| valid_notification_id(id)) {
            return Err("at least one id, each 1 to 128 bytes".to_owned());
        }
        Ok(Self { ids: params.ids })
    }
}

impl From<NotifyClear> for NotifyClearParams {
    fn from(clear: NotifyClear) -> Self {
        Self { ids: clear.ids }
    }
}

/// What the user did with a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NotifyUsedParams", into = "NotifyUsedParams")]
pub struct NotifyUsed {
    id: String,
    action: Option<String>,
}

impl NotifyUsed {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The action chosen; `None` when the notification itself was clicked.
    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyUsedParams {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<String>,
}

impl TryFrom<NotifyUsedParams> for NotifyUsed {
    type Error = String;

    fn try_from(params: NotifyUsedParams) -> Result<Self, Self::Error> {
        if !valid_notification_id(&params.id) {
            return Err(format!(
                "a notification's id is 1 to {MAX_NOTIFICATION_ID} bytes"
            ));
        }
        if params
            .action
            .as_ref()
            .is_some_and(|action| action.is_empty() || action.len() > MAX_ACTION_NAME)
        {
            return Err(format!("an action's name is 1 to {MAX_ACTION_NAME} bytes"));
        }
        Ok(Self {
            id: params.id,
            action: params.action,
        })
    }
}

impl From<NotifyUsed> for NotifyUsedParams {
    fn from(used: NotifyUsed) -> Self {
        Self {
            id: used.id,
            action: used.action,
        }
    }
}

/// A draft file for the shell to run an editor on, and the draft's own
/// folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneEditor {
    pub path: AbsolutePath,
    pub dir: AbsolutePath,
    pub title: String,
    /// Which editor to run.
    pub editor: EditorKind,
    /// The line of the file its body starts on, counting from 1: where an
    /// editor that has no plugin of Quill's is started.
    pub line: LineNumber,
}

/// The highest line a `pane.editor` may name.
pub const MOST_LINES: u32 = 1_000_000;

/// A line of a file, counting from 1, and no higher than [`MOST_LINES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct LineNumber(u32);

impl LineNumber {
    /// The nearest line there can be to `line`.
    pub fn nearest(line: u32) -> Self {
        Self(line.clamp(1, MOST_LINES))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for LineNumber {
    type Error = String;

    fn try_from(line: u32) -> Result<Self, Self::Error> {
        if (1..=MOST_LINES).contains(&line) {
            Ok(Self(line))
        } else {
            Err(format!("a line is from 1 to {MOST_LINES}"))
        }
    }
}

impl From<LineNumber> for u32 {
    fn from(line: LineNumber) -> Self {
        line.0
    }
}

/// Which editor the shell runs: one of those that come with macOS, or the
/// user's own Vim by way of the script they keep in the app's scripts
/// folder. It is one of these words: the shell is never given a path to a
/// program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorKind {
    #[default]
    Vim,
    Nano,
    Script,
}

/// How the editor ended. `ran` is false when it could not be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "EditorExitedParams", into = "EditorExitedParams")]
pub struct EditorExited {
    ran: bool,
    status: i32,
}

impl EditorExited {
    pub fn ran(&self) -> bool {
        self.ran
    }

    pub fn status(&self) -> i32 {
        self.status
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditorExitedParams {
    ran: bool,
    status: i32,
}

impl TryFrom<EditorExitedParams> for EditorExited {
    type Error = String;

    fn try_from(params: EditorExitedParams) -> Result<Self, Self::Error> {
        if (-255..=255).contains(&params.status) {
            Ok(Self {
                ran: params.ran,
                status: params.status,
            })
        } else {
            Err("an exit status is from -255 to 255".to_owned())
        }
    }
}

impl From<EditorExited> for EditorExitedParams {
    fn from(exited: EditorExited) -> Self {
        Self {
            ran: exited.ran,
            status: exited.status,
        }
    }
}

/// The longest name a form's step, field, option or button may have.
pub const MAX_FORM_ID: usize = 64;
/// The most fields a form may have.
pub const MAX_FORM_FIELDS: usize = 32;
/// The most buttons.
pub const MAX_FORM_BUTTONS: usize = 6;
/// The most rows a list in a form may have.
pub const MAX_FORM_ROWS: usize = 200;
/// The most places after the point a number in a form is shown with.
pub const MAX_FORM_DECIMALS: u8 = 3;
/// The most panes the settings window may have.
pub const MAX_SETTINGS_PANES: usize = 12;
/// The longest text a setting may be given, in bytes.
pub const MAX_SETTING_TEXT: usize = 4096;

/// Whether `id` can name a step, field, option or button of a form: lower
/// case letters, digits and underscores, starting with a letter.
pub fn valid_form_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    id.len() <= MAX_FORM_ID
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Text that must not be logged: what was typed into a secret field of a
/// form, or what a form shows to be read off and carried somewhere else (a
/// code to sign in with, the page to type it at). `Debug` prints a
/// placeholder, and there is no `Display`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text itself, for the one place that uses it.
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// A form for the shell to show in a sheet on its window: what it is
/// called, what it says, its fields in order and its buttons from left to
/// right. The shell lays it out and knows nothing of what it is for.
///
/// It cannot be built or decoded with a step, field or button whose name
/// is not one, two fields or two buttons of one name, a choice whose value
/// is not one of its options, more than one field with the focus, or more
/// than one button for return or for esc. A secret field has no value: the
/// core never sends one. What a code field shows is a [`Secret`], so that
/// printing the form prints none of it.
///
/// A sheet asks, and waits for a button. It has none of the fields that
/// say what the user did as they do it (a toggle, a number, a list): those
/// are the settings window's ([`SettingsShow`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FormShowParams", into = "FormShowParams")]
pub struct FormShow {
    step: String,
    title: String,
    note: Option<String>,
    error: Option<String>,
    busy: Option<String>,
    fields: Vec<FormField>,
    buttons: Vec<FormButton>,
}

/// What a [`FormShow`] is made from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormShowParams {
    /// Which step of the form this is. The shell starts its fields afresh
    /// when the step changes, and sends the name back in `form.used`.
    pub step: String,
    pub title: String,
    /// Said above the fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What is wrong, said below the fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the core is waiting for, in words. While it is there the
    /// fields take no typing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<String>,
    pub fields: Vec<FormField>,
    pub buttons: Vec<FormButton>,
}

impl FormShow {
    pub fn new(params: FormShowParams) -> Option<Self> {
        Self::try_from(params).ok()
    }

    pub fn step(&self) -> &str {
        &self.step
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn busy(&self) -> Option<&str> {
        self.busy.as_deref()
    }

    pub fn fields(&self) -> &[FormField] {
        &self.fields
    }

    pub fn buttons(&self) -> &[FormButton] {
        &self.buttons
    }
}

impl TryFrom<FormShowParams> for FormShow {
    type Error = String;

    fn try_from(params: FormShowParams) -> Result<Self, Self::Error> {
        if !valid_form_id(&params.step) {
            return Err("a form's step is a name in lower case".to_owned());
        }
        if params.title.is_empty() {
            return Err("a form needs a title".to_owned());
        }
        if params.fields.len() > MAX_FORM_FIELDS || params.buttons.len() > MAX_FORM_BUTTONS {
            return Err(format!(
                "a form has at most {MAX_FORM_FIELDS} fields and {MAX_FORM_BUTTONS} buttons"
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for field in &params.fields {
            if !valid_form_id(field.id()) || !seen.insert(field.id()) {
                return Err("each field of a form has a name of its own".to_owned());
            }
            if field.reports_as_changed() {
                return Err(
                    "a sheet has no toggle, number or list: those are the settings window's"
                        .to_owned(),
                );
            }
            field.sound()?;
        }
        if params.fields.iter().filter(|field| field.focus()).count() > 1 {
            return Err("only one field of a form has the focus".to_owned());
        }
        let mut seen = std::collections::BTreeSet::new();
        if !params.buttons.iter().all(|button| {
            valid_form_id(&button.id) && !button.title.is_empty() && seen.insert(&button.id)
        }) {
            return Err("each button of a form has a title and a name of its own".to_owned());
        }
        for role in [FormRole::Default, FormRole::Cancel] {
            let with = |button: &&FormButton| button.role == Some(role);
            if params.buttons.iter().filter(with).count() > 1 {
                return Err("a form has one button for return and one for esc".to_owned());
            }
        }
        Ok(Self {
            step: params.step,
            title: params.title,
            note: params.note,
            error: params.error,
            busy: params.busy,
            fields: params.fields,
            buttons: params.buttons,
        })
    }
}

impl From<FormShow> for FormShowParams {
    fn from(form: FormShow) -> Self {
        Self {
            step: form.step,
            title: form.title,
            note: form.note,
            error: form.error,
            busy: form.busy,
            fields: form.fields,
            buttons: form.buttons,
        }
    }
}

/// One field of a form. The kinds differ in what they hold: a secret has
/// no value to show, and only a choice has options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum FormField {
    /// A line of text to type.
    Text {
        id: String,
        label: String,
        value: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// A line of text that is not shown as it is typed: a password. The
    /// core says nothing of what it holds.
    Secret {
        id: String,
        label: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// One of a few options, by its name.
    Choice {
        id: String,
        label: String,
        value: String,
        options: Vec<FormOption>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        focus: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// Text that is only shown.
    Static {
        id: String,
        label: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// Text shown to be read off, selected or copied, and typed somewhere
    /// else: a code to sign in with, the page to type it at. The shell
    /// shows it large, in letters that cannot be mistaken for one another.
    /// Neither side prints it or writes it to a dump.
    Code {
        id: String,
        label: String,
        value: Secret,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// Something that is on or off: a checkbox. `label` stands beside it
    /// as a field's does, and may be empty; `title` is the checkbox's own
    /// words.
    Toggle {
        id: String,
        label: String,
        title: String,
        value: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// A number from `min` to `max`, shown with at most `decimals` places
    /// after the point and stepped by `step`. With no `max` there is no
    /// most. `unit` is said after it: "seconds".
    Number {
        id: String,
        label: String,
        value: Number,
        min: Number,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<Number>,
        step: Number,
        decimals: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// Rows to look down and pick one of, and things to do: with the row
    /// picked, or with none. `empty` is said in the rows' place when
    /// there are none.
    List {
        id: String,
        label: String,
        rows: Vec<FormRow>,
        actions: Vec<FormAction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        empty: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
    /// Something to press, that starts something: a button. `label`
    /// stands beside it as a field's does, and may be empty; `title` is
    /// on the button. A press comes back in `settings.used` as the action
    /// [`BUTTON_PRESS`], with no row.
    Button {
        id: String,
        label: String,
        title: String,
        /// Whether it can be pressed now.
        enabled: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        help: Option<String>,
    },
}

/// The action a button of the settings window reports when it is pressed.
pub const BUTTON_PRESS: &str = "press";

impl FormField {
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. }
            | Self::Secret { id, .. }
            | Self::Choice { id, .. }
            | Self::Static { id, .. }
            | Self::Code { id, .. }
            | Self::Toggle { id, .. }
            | Self::Number { id, .. }
            | Self::List { id, .. }
            | Self::Button { id, .. } => id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Text { label, .. }
            | Self::Secret { label, .. }
            | Self::Choice { label, .. }
            | Self::Static { label, .. }
            | Self::Code { label, .. }
            | Self::Toggle { label, .. }
            | Self::Number { label, .. }
            | Self::List { label, .. }
            | Self::Button { label, .. } => label,
        }
    }

    /// Whether the field is of a kind that says what the user did with it
    /// as they do it, with no button to wait for: the settings window's
    /// kinds, which a sheet has none of.
    pub fn reports_as_changed(&self) -> bool {
        matches!(
            self,
            Self::Toggle { .. } | Self::Number { .. } | Self::List { .. } | Self::Button { .. }
        )
    }

    /// Whether what the field holds hangs together: a choice is one of
    /// its options, a number is within its bounds, and the rows and
    /// actions of a list have names of their own.
    fn sound(&self) -> Result<(), String> {
        match self {
            Self::Choice { value, options, .. } => {
                let mut ids = std::collections::BTreeSet::new();
                let named = options
                    .iter()
                    .all(|option| valid_form_id(&option.id) && ids.insert(option.id.as_str()));
                if !named || !ids.contains(value.as_str()) {
                    return Err(
                        "a choice is one of its options, each with a name of its own".to_owned(),
                    );
                }
            }
            Self::Toggle { title, .. } => {
                if title.is_empty() {
                    return Err("a toggle says what it switches".to_owned());
                }
            }
            Self::Button { title, .. } => {
                if title.is_empty() {
                    return Err("a button says what it does".to_owned());
                }
            }
            Self::Number {
                value,
                min,
                max,
                step,
                decimals,
                ..
            } => {
                let within = min.get() <= value.get()
                    && max.is_none_or(|max| value.get() <= max.get() && min.get() <= max.get());
                if !within || step.get() <= 0.0 || *decimals > MAX_FORM_DECIMALS {
                    return Err(format!(
                        "a number is within its bounds, steps by more than nothing and has at \
                         most {MAX_FORM_DECIMALS} places"
                    ));
                }
            }
            Self::List { rows, actions, .. } => {
                let mut named = std::collections::BTreeSet::new();
                let actions_named = actions.len() <= MAX_FORM_BUTTONS
                    && actions.iter().all(|action| {
                        valid_form_id(&action.id)
                            && !action.title.is_empty()
                            && named.insert(action.id.as_str())
                    });
                let mut seen = std::collections::BTreeSet::new();
                let rows_named = rows.len() <= MAX_FORM_ROWS
                    && rows.iter().all(|row| {
                        valid_form_id(&row.id)
                            && !row.title.is_empty()
                            && seen.insert(row.id.as_str())
                            && row.actions.iter().all(|id| named.contains(id.as_str()))
                    });
                if !actions_named || !rows_named {
                    return Err(format!(
                        "a list has at most {MAX_FORM_ROWS} rows and {MAX_FORM_BUTTONS} actions, \
                         each with a title and a name of its own, and a row takes only the \
                         list's actions"
                    ));
                }
            }
            Self::Text { .. } | Self::Secret { .. } | Self::Static { .. } | Self::Code { .. } => {}
        }
        Ok(())
    }

    /// What the field holds, as the core has it. A secret has nothing to
    /// say, and what a code field shows is asked for by name
    /// ([`FormField::code`]).
    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Text { value, .. } | Self::Choice { value, .. } | Self::Static { value, .. } => {
                Some(value)
            }
            Self::Secret { .. }
            | Self::Code { .. }
            | Self::Toggle { .. }
            | Self::Number { .. }
            | Self::List { .. }
            | Self::Button { .. } => None,
        }
    }

    /// What a code field shows.
    pub fn code(&self) -> Option<&Secret> {
        match self {
            Self::Code { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Whether the keyboard starts in this field.
    pub fn focus(&self) -> bool {
        match self {
            Self::Text { focus, .. } | Self::Secret { focus, .. } | Self::Choice { focus, .. } => {
                *focus
            }
            Self::Static { .. }
            | Self::Code { .. }
            | Self::Toggle { .. }
            | Self::Number { .. }
            | Self::List { .. }
            | Self::Button { .. } => false,
        }
    }

    /// Said under the field, in small print.
    pub fn help(&self) -> Option<&str> {
        match self {
            Self::Text { help, .. }
            | Self::Secret { help, .. }
            | Self::Choice { help, .. }
            | Self::Static { help, .. }
            | Self::Code { help, .. }
            | Self::Toggle { help, .. }
            | Self::Number { help, .. }
            | Self::List { help, .. }
            | Self::Button { help, .. } => help.as_deref(),
        }
    }
}

/// A number a form shows or takes. Never NaN and never infinite, so two
/// of them are either equal or not, and it cannot be built or decoded
/// holding either. A whole number is written on the bridge as one.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "f64")]
pub struct Number(f64);

impl Eq for Number {}

impl Number {
    pub fn new(value: f64) -> Option<Self> {
        // Adding nothing makes the two zeros one.
        value.is_finite().then_some(Self(value + 0.0))
    }

    pub fn get(self) -> f64 {
        self.0
    }

    /// The number to at most `decimals` places after the point.
    pub fn rounded(self, decimals: u8) -> Self {
        let scale = 10f64.powi(i32::from(decimals.min(MAX_FORM_DECIMALS)));
        Self::new((self.0 * scale).round() / scale).unwrap_or(self)
    }

    /// The number as a whole one, if it is one that an `i64` holds exactly.
    pub fn whole(self) -> Option<i64> {
        // Every whole number below 2^53 is exactly itself as a double.
        (self.0.fract() == 0.0 && self.0.abs() < 9_007_199_254_740_992.0).then_some(self.0 as i64)
    }
}

impl From<u32> for Number {
    fn from(value: u32) -> Self {
        Self(f64::from(value))
    }
}

impl TryFrom<f64> for Number {
    type Error = String;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value).ok_or_else(|| "not a number".to_owned())
    }
}

impl Serialize for Number {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.whole() {
            Some(whole) => serializer.serialize_i64(whole),
            None => serializer.serialize_f64(self.0),
        }
    }
}

/// One row of a list: its name, what it is called, and what is said under
/// that. `actions` names the list's actions that can be asked of this row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormRow {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub actions: Vec<String>,
}

/// Something a list offers to do: a button under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormAction {
    /// What comes back in `settings.used`.
    pub id: String,
    pub title: String,
    /// Whether it is done to the row picked. One that is can be pressed
    /// only while a row that takes it is picked.
    pub row: bool,
    /// Whether it can be pressed at all now.
    pub enabled: bool,
}

/// One of the options of a choice: its name, and what the user reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormOption {
    pub id: String,
    pub title: String,
}

/// A button of a form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormButton {
    /// What comes back in `form.used`.
    pub id: String,
    pub title: String,
    /// Which key presses it, if one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<FormRole>,
    /// Whether it can be pressed now.
    pub enabled: bool,
}

/// The key that presses a button: return, or esc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormRole {
    Default,
    Cancel,
}

/// What the user did with the form in the sheet: the button pressed, if
/// one was, and what the fields held at that moment. The values of secret
/// fields are apart from the rest, as [`Secret`]s, so that nothing prints
/// them.
///
/// It cannot be built or decoded with a step, button or field whose name
/// is not one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FormUsedParams", into = "FormUsedParams")]
pub struct FormUsed {
    step: String,
    button: Option<String>,
    values: std::collections::BTreeMap<String, String>,
    secrets: std::collections::BTreeMap<String, Secret>,
}

impl FormUsed {
    pub fn new(
        step: &str,
        button: Option<&str>,
        values: &[(&str, &str)],
        secrets: &[(&str, &str)],
    ) -> Option<Self> {
        Self::try_from(FormUsedParams {
            step: step.to_owned(),
            button: button.map(str::to_owned),
            values: values
                .iter()
                .map(|(id, value)| ((*id).to_owned(), (*value).to_owned()))
                .collect(),
            secrets: secrets
                .iter()
                .map(|(id, value)| ((*id).to_owned(), Secret::new(*value)))
                .collect(),
        })
        .ok()
    }

    /// The step the form was at when it was used.
    pub fn step(&self) -> &str {
        &self.step
    }

    /// The button pressed. `None`: a choice was changed, and the values
    /// are sent so that the form can change with it.
    pub fn button(&self) -> Option<&str> {
        self.button.as_deref()
    }

    /// What a text field or a choice held.
    pub fn value(&self, id: &str) -> Option<&str> {
        self.values.get(id).map(String::as_str)
    }

    /// What a secret field held.
    pub fn secret(&self, id: &str) -> Option<&Secret> {
        self.secrets.get(id)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FormUsedParams {
    step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    button: Option<String>,
    values: std::collections::BTreeMap<String, String>,
    secrets: std::collections::BTreeMap<String, Secret>,
}

impl TryFrom<FormUsedParams> for FormUsed {
    type Error = String;

    fn try_from(params: FormUsedParams) -> Result<Self, Self::Error> {
        let named = valid_form_id(&params.step)
            && params.button.as_deref().is_none_or(valid_form_id)
            && params.values.keys().all(|id| valid_form_id(id))
            && params.secrets.keys().all(|id| valid_form_id(id));
        if !named || params.values.len() + params.secrets.len() > MAX_FORM_FIELDS {
            return Err(format!(
                "a form's step, button and at most {MAX_FORM_FIELDS} fields are named in lower case"
            ));
        }
        Ok(Self {
            step: params.step,
            button: params.button,
            values: params.values,
            secrets: params.secrets,
        })
    }
}

impl From<FormUsed> for FormUsedParams {
    fn from(used: FormUsed) -> Self {
        Self {
            step: used.step,
            button: used.button,
            values: used.values,
            secrets: used.secrets,
        }
    }
}

/// What the settings window shows: its panes, each with a name, a title,
/// the symbol on its toolbar button and its settings as they stand. The
/// shell lays them out, shows the window with the pane the user last had,
/// and knows nothing of what any setting means.
///
/// `problem` is what is wrong: with no panes, the settings cannot be
/// shown or changed from here at all (the file has a mistake in it), and
/// the window says so and offers the file; with panes, the last change was
/// not made, and why.
///
/// It cannot be built or decoded with nothing to show, a pane or a field
/// whose name is not one, two panes or two fields of a pane of one name, a
/// secret or a code (settings have none), a field with the focus (the
/// keyboard is the user's to put), or a field that does not hang together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SettingsShowParams", into = "SettingsShowParams")]
pub struct SettingsShow {
    panes: Vec<SettingsPane>,
    problem: Option<String>,
}

/// What a [`SettingsShow`] is made from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsShowParams {
    pub panes: Vec<SettingsPane>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// One pane of the settings window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPane {
    /// What comes back in `settings.used`.
    pub id: String,
    /// On the pane's toolbar button, and the window's title while the
    /// pane is the one on show.
    pub title: String,
    /// The SF Symbol on the pane's toolbar button.
    pub symbol: String,
    /// Said above the fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub fields: Vec<FormField>,
    /// Said at the foot of the pane, beside a button that opens the
    /// settings file: what more can be set there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub more: Option<String>,
}

/// Whether `symbol` could be the name of an SF Symbol: lower case letters,
/// digits and dots.
fn valid_symbol(symbol: &str) -> bool {
    !symbol.is_empty()
        && symbol.len() <= MAX_FORM_ID
        && symbol
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.')
}

impl SettingsShow {
    pub fn new(params: SettingsShowParams) -> Option<Self> {
        Self::try_from(params).ok()
    }

    pub fn panes(&self) -> &[SettingsPane] {
        &self.panes
    }

    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The field of this name in the pane of this name.
    pub fn field(&self, pane: &str, field: &str) -> Option<&FormField> {
        self.panes
            .iter()
            .find(|described| described.id == pane)?
            .fields
            .iter()
            .find(|described| described.id() == field)
    }
}

impl TryFrom<SettingsShowParams> for SettingsShow {
    type Error = String;

    fn try_from(params: SettingsShowParams) -> Result<Self, Self::Error> {
        if params.panes.len() > MAX_SETTINGS_PANES {
            return Err(format!(
                "the settings have at most {MAX_SETTINGS_PANES} panes"
            ));
        }
        if params.panes.is_empty() && params.problem.as_deref().is_none_or(str::is_empty) {
            return Err("settings with no panes say what is wrong".to_owned());
        }
        let mut panes = std::collections::BTreeSet::new();
        for pane in &params.panes {
            if !valid_form_id(&pane.id) || !panes.insert(pane.id.as_str()) {
                return Err("each pane of the settings has a name of its own".to_owned());
            }
            if pane.title.is_empty() || !valid_symbol(&pane.symbol) {
                return Err("a pane of the settings has a title and a symbol".to_owned());
            }
            if pane.fields.len() > MAX_FORM_FIELDS {
                return Err(format!("a pane has at most {MAX_FORM_FIELDS} fields"));
            }
            let mut seen = std::collections::BTreeSet::new();
            for field in &pane.fields {
                if !valid_form_id(field.id()) || !seen.insert(field.id()) {
                    return Err("each field of a pane has a name of its own".to_owned());
                }
                if matches!(field, FormField::Secret { .. } | FormField::Code { .. }) {
                    return Err("settings have no secret and no code".to_owned());
                }
                if field.focus() {
                    return Err("no field of the settings has the focus".to_owned());
                }
                field.sound()?;
            }
        }
        Ok(Self {
            panes: params.panes,
            problem: params.problem,
        })
    }
}

impl From<SettingsShow> for SettingsShowParams {
    fn from(show: SettingsShow) -> Self {
        Self {
            panes: show.panes,
            problem: show.problem,
        }
    }
}

/// What a field of the settings window was given: on or off for a toggle,
/// a number for a number, text for a text field, and for a choice the name
/// of the option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FormValue {
    Flag(bool),
    Number(Number),
    Text(String),
}

/// What the user did in the settings window: gave a field another value,
/// or asked for one of a list's actions, of the row picked if the action
/// is done to a row.
///
/// It cannot be built or decoded with a pane, field, action or row whose
/// name is not one, with both a value and an action or neither, with a row
/// and no action, or with more text than a setting takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SettingsUsedParams", into = "SettingsUsedParams")]
pub struct SettingsUsed {
    pane: String,
    field: String,
    did: SettingsDid,
}

/// What was done with a field of the settings window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsDid {
    /// It was given this value.
    Set(FormValue),
    /// This action of a list was asked for, of this row if of any.
    Action { action: String, row: Option<String> },
}

impl SettingsUsed {
    /// A field given another value.
    pub fn set(pane: &str, field: &str, value: FormValue) -> Option<Self> {
        Self::try_from(SettingsUsedParams {
            pane: pane.to_owned(),
            field: field.to_owned(),
            value: Some(value),
            action: None,
            row: None,
        })
        .ok()
    }

    /// One of a list's actions asked for.
    pub fn action(pane: &str, field: &str, action: &str, row: Option<&str>) -> Option<Self> {
        Self::try_from(SettingsUsedParams {
            pane: pane.to_owned(),
            field: field.to_owned(),
            value: None,
            action: Some(action.to_owned()),
            row: row.map(str::to_owned),
        })
        .ok()
    }

    pub fn pane(&self) -> &str {
        &self.pane
    }

    pub fn field(&self) -> &str {
        &self.field
    }

    pub fn did(&self) -> &SettingsDid {
        &self.did
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsUsedParams {
    pane: String,
    field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<FormValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    row: Option<String>,
}

impl TryFrom<SettingsUsedParams> for SettingsUsed {
    type Error = String;

    fn try_from(params: SettingsUsedParams) -> Result<Self, Self::Error> {
        let named = valid_form_id(&params.pane)
            && valid_form_id(&params.field)
            && params.action.as_deref().is_none_or(valid_form_id)
            && params.row.as_deref().is_none_or(valid_form_id);
        if !named {
            return Err("a pane, field, action and row are named in lower case".to_owned());
        }
        let did = match (params.value, params.action) {
            (Some(FormValue::Text(text)), None) if text.len() > MAX_SETTING_TEXT => {
                return Err(format!(
                    "a setting takes at most {MAX_SETTING_TEXT} bytes of text"
                ));
            }
            (Some(value), None) if params.row.is_none() => SettingsDid::Set(value),
            (None, Some(action)) => SettingsDid::Action {
                action,
                row: params.row,
            },
            _ => {
                return Err("a field is given a value, or one action is asked of a list".to_owned());
            }
        };
        Ok(Self {
            pane: params.pane,
            field: params.field,
            did,
        })
    }
}

impl From<SettingsUsed> for SettingsUsedParams {
    fn from(used: SettingsUsed) -> Self {
        let (value, action, row) = match used.did {
            SettingsDid::Set(value) => (Some(value), None, None),
            SettingsDid::Action { action, row } => (None, Some(action), row),
        };
        Self {
            pane: used.pane,
            field: used.field,
            value,
            action,
            row,
        }
    }
}

/// The most text one spelling check takes, in bytes.
pub const MAX_SPELL_TEXT: usize = 100_000;
/// The longest word suggestions are asked for, in bytes.
pub const MAX_SPELL_WORD: usize = 200;
/// The most misspelt words one answer names.
pub const MAX_MISSPELT: usize = 2_000;
/// The most suggestions one answer makes.
pub const MAX_SUGGESTIONS: usize = 20;

/// Text to check the spelling of. It cannot be built or decoded longer
/// than a check takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SpellCheckParams", into = "SpellCheckParams")]
pub struct SpellCheck {
    id: u64,
    text: String,
}

impl SpellCheck {
    pub fn new(id: u64, text: &str) -> Option<Self> {
        (text.len() <= MAX_SPELL_TEXT).then(|| Self {
            id,
            text: text.to_owned(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpellCheckParams {
    id: u64,
    text: String,
}

impl TryFrom<SpellCheckParams> for SpellCheck {
    type Error = String;

    fn try_from(params: SpellCheckParams) -> Result<Self, Self::Error> {
        Self::new(params.id, &params.text)
            .ok_or_else(|| format!("at most {MAX_SPELL_TEXT} bytes of text are checked at once"))
    }
}

impl From<SpellCheck> for SpellCheckParams {
    fn from(check: SpellCheck) -> Self {
        Self {
            id: check.id,
            text: check.text,
        }
    }
}

/// Where a misspelt word is: the byte offsets of its start and end in the
/// text that was checked, as UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Misspelt {
    pub start: usize,
    pub end: usize,
}

/// The misspelt words of a text: each ends after it starts, and they are
/// in order and do not overlap. It cannot be decoded holding anything
/// else, so the core can use the offsets as given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SpellCheckedParams", into = "SpellCheckedParams")]
pub struct SpellChecked {
    id: u64,
    words: Vec<Misspelt>,
}

impl SpellChecked {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn words(&self) -> &[Misspelt] {
        &self.words
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpellCheckedParams {
    id: u64,
    words: Vec<Misspelt>,
}

impl TryFrom<SpellCheckedParams> for SpellChecked {
    type Error = String;

    fn try_from(params: SpellCheckedParams) -> Result<Self, Self::Error> {
        if params.words.len() > MAX_MISSPELT {
            return Err(format!("at most {MAX_MISSPELT} misspelt words"));
        }
        let mut last = 0;
        for word in &params.words {
            if word.start >= word.end || word.start < last {
                return Err("misspelt words must each end after they start, in order".to_owned());
            }
            last = word.end;
        }
        Ok(Self {
            id: params.id,
            words: params.words,
        })
    }
}

impl From<SpellChecked> for SpellCheckedParams {
    fn from(checked: SpellChecked) -> Self {
        Self {
            id: checked.id,
            words: checked.words,
        }
    }
}

/// A misspelt word to have suggestions for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SpellSuggestParams", into = "SpellSuggestParams")]
pub struct SpellSuggest {
    id: u64,
    word: String,
}

impl SpellSuggest {
    pub fn new(id: u64, word: &str) -> Option<Self> {
        (!word.is_empty() && word.len() <= MAX_SPELL_WORD).then(|| Self {
            id,
            word: word.to_owned(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpellSuggestParams {
    id: u64,
    word: String,
}

impl TryFrom<SpellSuggestParams> for SpellSuggest {
    type Error = String;

    fn try_from(params: SpellSuggestParams) -> Result<Self, Self::Error> {
        Self::new(params.id, &params.word)
            .ok_or_else(|| format!("a word of 1 to {MAX_SPELL_WORD} bytes"))
    }
}

impl From<SpellSuggest> for SpellSuggestParams {
    fn from(suggest: SpellSuggest) -> Self {
        Self {
            id: suggest.id,
            word: suggest.word,
        }
    }
}

/// Words to offer in place of a misspelt one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SpellSuggestedParams", into = "SpellSuggestedParams")]
pub struct SpellSuggested {
    id: u64,
    words: Vec<String>,
}

impl SpellSuggested {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpellSuggestedParams {
    id: u64,
    words: Vec<String>,
}

impl TryFrom<SpellSuggestedParams> for SpellSuggested {
    type Error = String;

    fn try_from(params: SpellSuggestedParams) -> Result<Self, Self::Error> {
        let fine = params.words.len() <= MAX_SUGGESTIONS
            && params
                .words
                .iter()
                .all(|word| !word.is_empty() && word.len() <= MAX_SPELL_WORD);
        if fine {
            Ok(Self {
                id: params.id,
                words: params.words,
            })
        } else {
            Err(format!(
                "at most {MAX_SUGGESTIONS} suggestions, each 1 to {MAX_SPELL_WORD} bytes"
            ))
        }
    }
}

impl From<SpellSuggested> for SpellSuggestedParams {
    fn from(suggested: SpellSuggested) -> Self {
        Self {
            id: suggested.id,
            words: suggested.words,
        }
    }
}

/// The longest address the Mac may ask Quill to open.
pub const MAX_OPENED_URL: usize = 8192;

/// A `mailto:` address the Mac asked Quill to open. It cannot be decoded
/// holding any other kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "UrlOpenParams", into = "UrlOpenParams")]
pub struct UrlOpen {
    url: String,
}

impl UrlOpen {
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UrlOpenParams {
    url: String,
}

impl TryFrom<UrlOpenParams> for UrlOpen {
    type Error = String;

    fn try_from(params: UrlOpenParams) -> Result<Self, Self::Error> {
        if params.url.starts_with("mailto:") && params.url.len() <= MAX_OPENED_URL {
            Ok(Self { url: params.url })
        } else {
            Err("only a mailto address can be opened".to_owned())
        }
    }
}

impl From<UrlOpen> for UrlOpenParams {
    fn from(open: UrlOpen) -> Self {
        Self { url: open.url }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardWrite {
    pub text: String,
}

/// Files handed to the core: at least one, each by an absolute path. It
/// cannot be built or decoded holding anything else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FilesDroppedParams", into = "FilesDroppedParams")]
pub struct FilesDropped {
    paths: Vec<String>,
    temporary: bool,
}

impl FilesDropped {
    pub fn new(paths: Vec<String>, temporary: bool) -> Option<Self> {
        let valid = !paths.is_empty() && paths.iter().all(|path| path.starts_with('/'));
        valid.then_some(Self { paths, temporary })
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The files are copies made for the core, to remove once read.
    pub fn temporary(&self) -> bool {
        self.temporary
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesDroppedParams {
    paths: Vec<String>,
    temporary: bool,
}

impl TryFrom<FilesDroppedParams> for FilesDropped {
    type Error = String;

    fn try_from(params: FilesDroppedParams) -> Result<Self, Self::Error> {
        Self::new(params.paths, params.temporary)
            .ok_or_else(|| "paths must be absolute, and there must be one".to_owned())
    }
}

impl From<FilesDropped> for FilesDroppedParams {
    fn from(files: FilesDropped) -> Self {
        Self {
            paths: files.paths,
            temporary: files.temporary,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("not a bridge message: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("unsupported JSON-RPC version \"{0}\"")]
    Version(String),
}

const VERSION: &str = "2.0";

#[derive(Serialize)]
struct Outgoing<'a, M> {
    jsonrpc: &'static str,
    #[serde(flatten)]
    message: &'a M,
}

#[derive(Deserialize)]
struct Incoming<M> {
    jsonrpc: String,
    #[serde(flatten)]
    message: M,
}

/// One message as one line of JSON, without the newline.
pub fn encode<M: Serialize>(message: &M) -> Result<String, BridgeError> {
    Ok(serde_json::to_string(&Outgoing {
        jsonrpc: VERSION,
        message,
    })?)
}

/// Parses one line. Unknown methods, missing fields and extra fields are all
/// errors: both ends ship together, so a mismatch is a bug to surface.
pub fn decode<M: DeserializeOwned>(line: &str) -> Result<M, BridgeError> {
    let incoming: Incoming<M> = serde_json::from_str(line)?;
    if incoming.jsonrpc != VERSION {
        return Err(BridgeError::Version(incoming.jsonrpc));
    }
    Ok(incoming.message)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::*;

    fn schema_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema")
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    fn examples(directory: &str) -> Vec<(String, String)> {
        let mut found: Vec<_> = fs::read_dir(schema_dir().join("examples").join(directory))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .map(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (name, read(&path))
            })
            .collect();
        found.sort();
        assert!(!found.is_empty(), "no examples in {directory}");
        found
    }

    /// The schema restricted to one direction, with every definition in scope.
    fn validator(direction: &str) -> jsonschema::Validator {
        let schema: Value = serde_json::from_str(&read(&schema_dir().join("bridge.json"))).unwrap();
        let one_direction = serde_json::json!({
            "$schema": schema["$schema"],
            "$ref": format!("#/$defs/{direction}"),
            "$defs": schema["$defs"],
        });
        jsonschema::validator_for(&one_direction).unwrap()
    }

    /// Every example is valid by the schema, parses into the Rust type, and
    /// comes back out as the same JSON.
    fn round_trip<M>(directory: &str, direction: &str)
    where
        M: Serialize + DeserializeOwned,
    {
        let validator = validator(direction);
        for (name, text) in examples(directory) {
            let value: Value = serde_json::from_str(&text).unwrap();
            let problems: Vec<String> = validator
                .iter_errors(&value)
                .map(|err| err.to_string())
                .collect();
            assert!(
                problems.is_empty(),
                "{name} breaks the schema: {problems:?}"
            );

            let message: M = decode(&text).unwrap_or_else(|err| panic!("{name}: {err}"));
            let line = encode(&message).unwrap();
            assert!(!line.contains('\n'), "{name} does not encode to one line");
            let again: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(again, value, "{name} does not round-trip");
        }
    }

    #[test]
    fn core_to_shell_examples_match_the_schema_and_the_types() {
        round_trip::<CoreToShell>("core-to-shell", "coreToShell");
    }

    #[test]
    fn shell_to_core_examples_match_the_schema_and_the_types() {
        round_trip::<ShellToCore>("shell-to-core", "shellToCore");
    }

    #[test]
    fn every_method_in_the_schema_has_an_example() {
        let schema: Value = serde_json::from_str(&read(&schema_dir().join("bridge.json"))).unwrap();
        for (directory, direction) in [
            ("core-to-shell", "coreToShell"),
            ("shell-to-core", "shellToCore"),
        ] {
            let mut declared: Vec<String> = schema["$defs"][direction]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .map(|entry| {
                    let reference = entry["$ref"].as_str().unwrap();
                    reference.trim_start_matches("#/$defs/").to_owned()
                })
                .collect();
            declared.sort();
            let exemplified: Vec<String> = examples(directory)
                .into_iter()
                .map(|(name, _)| name.trim_end_matches(".json").to_owned())
                .collect();
            assert_eq!(declared, exemplified, "{directory}");
        }
    }

    #[test]
    fn a_direction_does_not_accept_the_other_directions_messages() {
        let core_to_shell = validator("coreToShell");
        for (name, text) in examples("shell-to-core") {
            let value: Value = serde_json::from_str(&text).unwrap();
            assert!(!core_to_shell.is_valid(&value), "{name}");
            assert!(decode::<CoreToShell>(&text).is_err(), "{name}");
        }
        let shell_to_core = validator("shellToCore");
        for (name, text) in examples("core-to-shell") {
            let value: Value = serde_json::from_str(&text).unwrap();
            assert!(!shell_to_core.is_valid(&value), "{name}");
            assert!(decode::<ShellToCore>(&text).is_err(), "{name}");
        }
    }

    #[test]
    fn invalid_examples_are_rejected_by_the_schema_and_by_both_types() {
        let schema: Value = serde_json::from_str(&read(&schema_dir().join("bridge.json"))).unwrap();
        let whole = jsonschema::validator_for(&schema).unwrap();
        for (name, text) in examples("invalid") {
            let value: Value = serde_json::from_str(&text).unwrap();
            assert!(!whole.is_valid(&value), "the schema accepts {name}");
            assert!(
                decode::<CoreToShell>(&text).is_err(),
                "CoreToShell accepts {name}"
            );
            assert!(
                decode::<ShellToCore>(&text).is_err(),
                "ShellToCore accepts {name}"
            );
        }
    }

    #[test]
    fn a_terminals_colours_are_six_hex_digits_or_left_out() {
        let state = |colours: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"state.update","params":{{"context":"list","enabled":[],"checked":[],"unread":0,"title":"Inbox"{colours}}}}}"#
            )
        };
        // Left out, nothing is said: the terminals keep their own.
        let CoreToShell::StateUpdate(plain) = decode(&state("")).unwrap() else {
            panic!("not a state.update");
        };
        assert_eq!((&plain.background, &plain.foreground), (&None, &None));
        assert_eq!(encode(&CoreToShell::StateUpdate(plain)).unwrap(), state(""));

        let both = state(r##","background":"#1a1b26","foreground":"#c0caf5""##);
        let CoreToShell::StateUpdate(coloured) = decode(&both).unwrap() else {
            panic!("not a state.update");
        };
        assert_eq!(
            coloured.background,
            Some(TerminalColor::rgb(0x1a, 0x1b, 0x26))
        );
        assert_eq!(
            coloured.foreground.as_ref().map(TerminalColor::as_str),
            Some("#c0caf5")
        );
        assert_eq!(encode(&CoreToShell::StateUpdate(coloured)).unwrap(), both);

        for bad in [
            "blue",
            "#fff",
            "#1A1B26",
            "1a1b26",
            "#1a1b2g",
            "#1a1b26ff",
            "",
            "default",
        ] {
            let line = state(&format!(r#","background":"{bad}""#));
            assert!(decode::<CoreToShell>(&line).is_err(), "{bad}");
            let line = state(&format!(r#","foreground":"{bad}""#));
            assert!(decode::<CoreToShell>(&line).is_err(), "{bad}");
        }
        assert!(decode::<CoreToShell>(&state(r#","background":7"#)).is_err());
    }

    #[test]
    fn the_version_is_checked() {
        let line = r#"{"jsonrpc":"1.0","method":"pane.close","params":{}}"#;
        assert!(matches!(
            decode::<CoreToShell>(line),
            Err(BridgeError::Version(version)) if version == "1.0"
        ));
        assert!(matches!(
            decode::<CoreToShell>("not json"),
            Err(BridgeError::Malformed(_))
        ));
    }

    #[test]
    fn an_editors_place_is_cells_of_the_screen_or_nowhere_and_never_keys_without_a_place() {
        let cells = EditorCells {
            column: 70,
            row: 7,
            columns: 76,
            rows: 30,
            covered: 2,
        };
        let place = EditorPlace::at(cells, true).unwrap();
        assert_eq!((place.cells(), place.keys()), (Some(cells), true));
        let line = encode(&CoreToShell::EditorPlace(place)).unwrap();
        assert_eq!(
            line,
            r#"{"jsonrpc":"2.0","method":"editor.place","params":{"place":{"column":70,"row":7,"columns":76,"rows":30,"covered":2},"keys":true}}"#
        );
        assert_eq!(
            decode::<CoreToShell>(&line).unwrap(),
            CoreToShell::EditorPlace(place)
        );
        let hidden = encode(&CoreToShell::EditorPlace(EditorPlace::hidden())).unwrap();
        assert_eq!(
            hidden,
            r#"{"jsonrpc":"2.0","method":"editor.place","params":{"keys":false}}"#
        );
        assert_eq!(
            decode::<CoreToShell>(&hidden).unwrap(),
            CoreToShell::EditorPlace(EditorPlace::hidden())
        );
        // Nothing to run an editor in.
        for (columns, rows) in [(0, 30), (76, 0)] {
            assert!(
                EditorPlace::at(
                    EditorCells {
                        columns,
                        rows,
                        ..cells
                    },
                    false
                )
                .is_none()
            );
        }
        for bad in [
            r#"{}"#,
            r#"{"keys":true}"#,
            r#"{"keys":"yes"}"#,
            r#"{"keys":false,"place":{"column":70,"row":7,"columns":0,"rows":30,"covered":0}}"#,
            r#"{"keys":false,"place":{"column":70,"row":7,"columns":76,"rows":0,"covered":0}}"#,
            r#"{"keys":false,"place":{"column":70,"row":7,"columns":76,"rows":30}}"#,
            r#"{"keys":false,"place":{"column":-1,"row":7,"columns":76,"rows":30,"covered":0}}"#,
            r#"{"keys":false,"place":{"column":70,"row":7,"columns":76,"rows":30,"covered":0,"points":9}}"#,
            r#"{"keys":false,"hidden":true}"#,
        ] {
            let line = format!(r#"{{"jsonrpc":"2.0","method":"editor.place","params":{bad}}}"#);
            assert!(decode::<CoreToShell>(&line).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_place_of_the_reading_pane_is_a_whole_number_of_columns_both_ways() {
        assert!(ReadingPlace::new(0).is_none() && WindowResized::new(0).is_none());
        let place = ReadingPlace::new(78).unwrap();
        assert_eq!(place.columns(), 78);
        let line = encode(&CoreToShell::ReadingPlace(place)).unwrap();
        assert_eq!(
            line,
            r#"{"jsonrpc":"2.0","method":"reading.place","params":{"columns":78}}"#
        );
        assert_eq!(
            decode::<CoreToShell>(&line).unwrap(),
            CoreToShell::ReadingPlace(place)
        );
        let resized = decode::<ShellToCore>(
            r#"{"jsonrpc":"2.0","method":"window.resized","params":{"columns":147}}"#,
        )
        .unwrap();
        assert!(matches!(resized, ShellToCore::WindowResized(resized) if resized.columns() == 147));
        for bad in [
            r#"{"columns":0}"#,
            r#"{"columns":-1}"#,
            r#"{"columns":78.5}"#,
            r#"{"columns":"78"}"#,
            r#"{"columns":65536}"#,
            r#"{}"#,
            r#"{"columns":78,"points":624}"#,
        ] {
            let line = format!(r#"{{"jsonrpc":"2.0","method":"reading.place","params":{bad}}}"#);
            assert!(decode::<CoreToShell>(&line).is_err(), "{bad}");
            let line = format!(r#"{{"jsonrpc":"2.0","method":"window.resized","params":{bad}}}"#);
            assert!(decode::<ShellToCore>(&line).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_page_for_the_reading_pane_is_only_built_with_a_header_that_fits() {
        let path = || AbsolutePath::new(Path::new("/tmp/quill-view/m1/page/message.html")).unwrap();
        let show = |header: Vec<String>| ReadingShow::new(path(), "A subject", false, header);
        let line = |text: &str| vec![text.to_owned()];
        assert!(show(Vec::new()).is_some());
        assert!(show(vec!["x".to_owned(); MAX_READING_HEADER_LINES]).is_some());
        assert!(show(vec!["x".to_owned(); MAX_READING_HEADER_LINES + 1]).is_none());
        assert!(show(line("")).is_none());
        assert!(show(line("two\nlines")).is_none());
        assert!(show(line("an escape \u{1b}[31m")).is_none());
        assert!(show(line(&"é".repeat(MAX_READING_HEADER_BYTES / 2))).is_some());
        assert!(show(line(&"é".repeat(MAX_READING_HEADER_BYTES / 2 + 1))).is_none());

        // What is built is what is read back, and nothing else is.
        let built = show(line("From: Ana Ruiz <ana.ruiz@example.com>")).unwrap();
        let sent = encode(&CoreToShell::ReadingShow(built.clone())).unwrap();
        assert_eq!(
            decode::<CoreToShell>(&sent).unwrap(),
            CoreToShell::ReadingShow(built)
        );
        for bad in [
            r#"{"path":"/a","title":"","remote":false}"#,
            r#"{"path":"a","title":"","remote":false,"header":[]}"#,
            r#"{"path":"/a","title":"","remote":false,"header":["a\nb"]}"#,
            r#"{"path":"/a","title":"","remote":false,"header":[],"focus":true}"#,
            r#"{"path":"/a","title":"","remote":false,"header":[],"notice":{"text":"x"}}"#,
            r#"{"path":"/a","title":"","remote":false,"header":[],"notice":{"text":"","actions":[]}}"#,
            r#"{"path":"/a","title":"","remote":false,"header":[],"notice":{"text":"x","actions":[],"more":1}}"#,
            r#"{"path":"/a","title":"","remote":false,"header":[],"notice":{"text":"x","actions":[{"command":"Load","title":"Load"}]}}"#,
            r#"{"path":"/a","title":"","remote":false,"header":[],"notice":{"text":"x","actions":[{"command":"a.b","title":""}]}}"#,
            r#"{"path":"/a","title":"","remote":false,"header":[],"notice":{"text":"x","actions":[{"command":"a.b","title":"B","key":"i"}]}}"#,
        ] {
            let line = format!(r#"{{"jsonrpc":"2.0","method":"reading.show","params":{bad}}}"#);
            assert!(decode::<CoreToShell>(&line).is_err(), "{bad}");
        }
        assert!(
            decode::<CoreToShell>(
                r#"{"jsonrpc":"2.0","method":"reading.scroll","params":{"by":"line_down"}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn a_notice_above_a_page_is_one_line_with_buttons_that_run_commands() {
        let action = |command: &str, title: &str| ReadingAction {
            command: command.to_owned(),
            title: title.to_owned(),
        };
        let load = || action("message.load_remote", "Load Pictures");
        assert!(ReadingNotice::new("2 remote pictures were left out.", Vec::new()).is_some());
        assert!(ReadingNotice::new("Left out.", vec![load(); MAX_READING_ACTIONS]).is_some());
        assert!(ReadingNotice::new("Left out.", vec![load(); MAX_READING_ACTIONS + 1]).is_none());
        assert!(ReadingNotice::new("", vec![load()]).is_none());
        assert!(ReadingNotice::new("two\nlines", vec![load()]).is_none());
        for name in [
            "",
            "load",
            "Message.load",
            "message.",
            ".load",
            "message.load-remote",
            "a b.c",
        ] {
            assert!(
                ReadingNotice::new("Left out.", vec![action(name, "Load")]).is_none(),
                "{name}"
            );
        }
        assert!(ReadingNotice::new("Left out.", vec![action("a.b", "")]).is_none());
        assert!(ReadingNotice::new("Left out.", vec![action("a.b", "an escape \u{1b}")]).is_none());
        let long = "x".repeat(MAX_READING_ACTION_TITLE);
        assert!(ReadingNotice::new("Left out.", vec![action("a.b", &long)]).is_some());
        assert!(
            ReadingNotice::new("Left out.", vec![action("a.b", &format!("{long}x"))]).is_none()
        );

        // It goes with the page, and comes back as it went. A page with
        // none says nothing of one.
        let path = || AbsolutePath::new(Path::new("/tmp/quill-view/m1/page/message.html")).unwrap();
        let plain = ReadingShow::new(path(), "A subject", false, Vec::new()).unwrap();
        assert!(
            !encode(&CoreToShell::ReadingShow(plain.clone()))
                .unwrap()
                .contains("notice")
        );
        let notice = ReadingNotice::new("1 remote picture was left out.", vec![load()]).unwrap();
        let with = plain.with_notice(Some(notice.clone()));
        assert_eq!(with.notice(), Some(&notice));
        let sent = encode(&CoreToShell::ReadingShow(with.clone())).unwrap();
        assert_eq!(
            decode::<CoreToShell>(&sent).unwrap(),
            CoreToShell::ReadingShow(with)
        );
    }

    fn text_field(id: &str, focus: bool) -> FormField {
        FormField::Text {
            id: id.to_owned(),
            label: "Name:".to_owned(),
            value: String::new(),
            focus,
            help: None,
        }
    }

    fn button(id: &str, role: Option<FormRole>) -> FormButton {
        FormButton {
            id: id.to_owned(),
            title: "Continue".to_owned(),
            role,
            enabled: true,
        }
    }

    fn form(fields: Vec<FormField>, buttons: Vec<FormButton>) -> FormShowParams {
        FormShowParams {
            step: "server".to_owned(),
            title: "Add an account".to_owned(),
            fields,
            buttons,
            ..FormShowParams::default()
        }
    }

    #[test]
    fn a_form_is_only_built_whole() {
        let choice = |value: &str, options: &[&str]| FormField::Choice {
            id: "type".to_owned(),
            label: "Type:".to_owned(),
            value: value.to_owned(),
            options: options
                .iter()
                .map(|id| FormOption {
                    id: (*id).to_owned(),
                    title: (*id).to_uppercase(),
                })
                .collect(),
            focus: false,
            help: None,
        };
        let fine = form(
            vec![
                choice("imap", &["imap", "microsoft"]),
                text_field("name", true),
            ],
            vec![
                button("cancel", Some(FormRole::Cancel)),
                button("back", None),
                button("continue", Some(FormRole::Default)),
            ],
        );
        let built = FormShow::new(fine.clone()).unwrap();
        assert_eq!(built.step(), "server");
        assert_eq!(built.fields()[1].id(), "name");
        assert!(built.fields()[1].focus() && !built.fields()[0].focus());
        assert_eq!(built.fields()[0].value(), Some("imap"));
        assert_eq!(built.buttons().len(), 3);
        // No fields and no buttons is a form too: one that only says
        // something.
        assert!(FormShow::new(form(Vec::new(), Vec::new())).is_some());

        let broken: Vec<(&str, FormShowParams)> = vec![
            (
                "a step that is no name",
                FormShowParams {
                    step: "The Server".to_owned(),
                    ..fine.clone()
                },
            ),
            (
                "no title",
                FormShowParams {
                    title: String::new(),
                    ..fine.clone()
                },
            ),
            (
                "two fields of one name",
                form(
                    vec![text_field("name", false), text_field("name", false)],
                    Vec::new(),
                ),
            ),
            (
                "a field with no name",
                form(vec![text_field("", false)], Vec::new()),
            ),
            (
                "two fields with the focus",
                form(
                    vec![text_field("name", true), text_field("host", true)],
                    Vec::new(),
                ),
            ),
            (
                "a choice that is none of its options",
                form(vec![choice("pop3", &["imap", "microsoft"])], Vec::new()),
            ),
            (
                "a choice with no options",
                form(vec![choice("imap", &[])], Vec::new()),
            ),
            (
                "a choice with one option twice",
                form(vec![choice("imap", &["imap", "imap"])], Vec::new()),
            ),
            (
                "two buttons of one name",
                form(
                    Vec::new(),
                    vec![button("continue", None), button("continue", None)],
                ),
            ),
            (
                "two buttons for return",
                form(
                    Vec::new(),
                    vec![
                        button("continue", Some(FormRole::Default)),
                        button("save", Some(FormRole::Default)),
                    ],
                ),
            ),
            (
                "two buttons for esc",
                form(
                    Vec::new(),
                    vec![
                        button("cancel", Some(FormRole::Cancel)),
                        button("reject", Some(FormRole::Cancel)),
                    ],
                ),
            ),
            (
                "a button with no title",
                form(
                    Vec::new(),
                    vec![FormButton {
                        title: String::new(),
                        ..button("continue", None)
                    }],
                ),
            ),
            (
                "too many fields",
                form(
                    (0..=MAX_FORM_FIELDS)
                        .map(|n| text_field(&format!("f{n}"), false))
                        .collect(),
                    Vec::new(),
                ),
            ),
        ];
        for (what, params) in broken {
            assert!(FormShow::new(params.clone()).is_none(), "{what}");
            // And what cannot be built cannot be decoded either.
            let line = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "form.show",
                "params": serde_json::to_value(&params).unwrap(),
            })
            .to_string();
            assert!(decode::<CoreToShell>(&line).is_err(), "{what}");
        }
    }

    fn toggle(id: &str, value: bool) -> FormField {
        FormField::Toggle {
            id: id.to_owned(),
            label: String::new(),
            title: "Show a notification".to_owned(),
            value,
            help: None,
        }
    }

    fn number(value: f64, min: f64, max: Option<f64>, step: f64, decimals: u8) -> FormField {
        FormField::Number {
            id: "seconds".to_owned(),
            label: "Undo send:".to_owned(),
            value: Number::new(value).unwrap(),
            min: Number::new(min).unwrap(),
            max: max.map(|max| Number::new(max).unwrap()),
            step: Number::new(step).unwrap(),
            decimals,
            unit: None,
            help: None,
        }
    }

    fn list(rows: &[(&str, &[&str])], actions: &[&str]) -> FormField {
        FormField::List {
            id: "accounts".to_owned(),
            label: String::new(),
            rows: rows
                .iter()
                .map(|(id, takes)| FormRow {
                    id: (*id).to_owned(),
                    title: "Work".to_owned(),
                    detail: None,
                    actions: takes.iter().map(|id| (*id).to_owned()).collect(),
                })
                .collect(),
            actions: actions
                .iter()
                .map(|id| FormAction {
                    id: (*id).to_owned(),
                    title: "Sign In".to_owned(),
                    row: true,
                    enabled: true,
                })
                .collect(),
            empty: None,
            help: None,
        }
    }

    fn button_field(id: &str, title: &str) -> FormField {
        FormField::Button {
            id: id.to_owned(),
            label: String::new(),
            title: title.to_owned(),
            enabled: true,
            help: None,
        }
    }

    #[test]
    fn a_button_of_the_settings_says_what_it_does_and_is_pressed_by_name() {
        let shown = SettingsShow::new(settings(vec![pane(
            "general",
            vec![button_field("import_theme", "Import Theme\u{2026}")],
        )]))
        .unwrap();
        let field = shown.field("general", "import_theme").unwrap();
        assert_eq!((field.id(), field.label()), ("import_theme", ""));
        assert_eq!(
            (field.value(), field.focus(), field.help()),
            (None, false, None)
        );
        let sent = encode(&CoreToShell::SettingsShow(shown.clone())).unwrap();
        assert!(sent.contains(r#""kind":"button""#), "{sent}");
        assert_eq!(
            decode::<CoreToShell>(&sent).unwrap(),
            CoreToShell::SettingsShow(shown)
        );
        // One with nothing on it is not a button.
        assert!(
            SettingsShow::new(settings(vec![pane(
                "general",
                vec![button_field("go", "")]
            )]))
            .is_none()
        );
        // A press comes back as an action, with no row.
        let pressed = SettingsUsed::action("general", "import_theme", BUTTON_PRESS, None).unwrap();
        assert_eq!(
            pressed.did(),
            &SettingsDid::Action {
                action: "press".to_owned(),
                row: None
            }
        );
    }

    #[test]
    fn the_appearance_is_said_with_the_state_and_left_out_when_it_is_the_systems() {
        let state = |appearance: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"state.update","params":{{"context":"list","enabled":[],"checked":[],"unread":0,"title":"Inbox"{appearance}}}}}"#
            )
        };
        let CoreToShell::StateUpdate(plain) = decode(&state("")).unwrap() else {
            panic!("not a state.update");
        };
        assert_eq!(plain.appearance, Appearance::System);
        assert_eq!(encode(&CoreToShell::StateUpdate(plain)).unwrap(), state(""));
        for (word, appearance) in [("light", Appearance::Light), ("dark", Appearance::Dark)] {
            let line = state(&format!(r#","appearance":"{word}""#));
            let CoreToShell::StateUpdate(update) = decode(&line).unwrap() else {
                panic!("not a state.update");
            };
            assert_eq!(update.appearance, appearance);
            assert_eq!(encode(&CoreToShell::StateUpdate(update)).unwrap(), line);
        }
        // Said outright, the system's is read too.
        let CoreToShell::StateUpdate(said) = decode(&state(r#","appearance":"system""#)).unwrap()
        else {
            panic!("not a state.update");
        };
        assert_eq!(said.appearance, Appearance::System);
        for bad in ["Dark", "auto", ""] {
            let line = state(&format!(r#","appearance":"{bad}""#));
            assert!(decode::<CoreToShell>(&line).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_macs_appearance_and_an_imported_theme_come_from_the_shell() {
        let line = r#"{"jsonrpc":"2.0","method":"appearance.changed","params":{"dark":false}}"#;
        assert_eq!(
            decode::<ShellToCore>(line).unwrap(),
            ShellToCore::AppearanceChanged(AppearanceChanged { dark: false })
        );
        let imported = |name: &str| {
            let name = serde_json::to_string(name).unwrap();
            format!(r#"{{"jsonrpc":"2.0","method":"themes.imported","params":{{"name":{name}}}}}"#)
        };
        let schema: Value = serde_json::from_str(&read(&schema_dir().join("bridge.json"))).unwrap();
        let schema = jsonschema::validator_for(&schema).unwrap();
        // As people name themes.
        for name in [
            "Dracula",
            "3024 Day",
            "Dracula+ High Contrast",
            "Jack O' Lantern",
            "Builtin Tango (Light)",
            "v1.5",
            "x",
            "caf\u{e9}",
        ] {
            let line = imported(name);
            let ShellToCore::ThemesImported(theme) = decode(&line).unwrap() else {
                panic!("not themes.imported");
            };
            assert_eq!(theme.name(), name);
            assert_eq!(encode(&ShellToCore::ThemesImported(theme)).unwrap(), line);
            let value: Value = serde_json::from_str(&line).unwrap();
            assert!(schema.is_valid(&value), "the schema refuses {name:?}");
            assert!(ThemesImported::new(name).is_some());
        }
        // Never a path, a hidden file, or a name with something unseen in
        // it: by the type and by the schema alike.
        let long = "x".repeat(101);
        for name in [
            "",
            "../../config.toml",
            "a/b",
            "/etc/passwd",
            "a\\b",
            "a:b",
            ".hidden",
            "..",
            "Dracula.toml",
            "Dracula.TOML",
            " Dracula",
            "Dracula ",
            "two\nlines",
            "tab\there",
            "bell\u{7}",
            "del\u{7f}",
            long.as_str(),
        ] {
            let line = imported(name);
            assert!(decode::<ShellToCore>(&line).is_err(), "{name:?}");
            assert!(ThemesImported::new(name).is_none(), "{name:?}");
            let value: Value = serde_json::from_str(&line).unwrap();
            assert!(!schema.is_valid(&value), "the schema accepts {name:?}");
        }
    }

    fn pane(id: &str, fields: Vec<FormField>) -> SettingsPane {
        SettingsPane {
            id: id.to_owned(),
            title: "General".to_owned(),
            symbol: "gearshape".to_owned(),
            note: None,
            fields,
            more: None,
        }
    }

    fn settings(panes: Vec<SettingsPane>) -> SettingsShowParams {
        SettingsShowParams {
            panes,
            problem: None,
        }
    }

    #[test]
    fn a_sheet_has_none_of_the_fields_that_report_as_they_change() {
        for field in [
            toggle("on", true),
            number(10.0, 0.0, None, 1.0, 0),
            list(&[], &[]),
            button_field("import_theme", "Import Theme\u{2026}"),
        ] {
            assert!(field.reports_as_changed());
            assert!(FormShow::new(form(vec![field], Vec::new())).is_none());
        }
        assert!(!text_field("name", false).reports_as_changed());
    }

    #[test]
    fn the_settings_are_only_built_whole() {
        let fine = settings(vec![
            pane(
                "general",
                vec![
                    toggle("notifications", true),
                    text_field("name", false),
                    number(10.0, 0.0, Some(60.0), 1.0, 0),
                ],
            ),
            pane(
                "accounts",
                vec![list(&[("work", &["sign_in"]), ("home", &[])], &["sign_in"])],
            ),
        ]);
        let built = SettingsShow::new(fine.clone()).unwrap();
        assert_eq!(built.panes().len(), 2);
        assert_eq!(built.problem(), None);
        assert!(built.field("general", "notifications").is_some());
        assert!(built.field("accounts", "notifications").is_none());
        assert!(built.field("storage", "notifications").is_none());
        let sent = encode(&CoreToShell::SettingsShow(built.clone())).unwrap();
        assert_eq!(
            decode::<CoreToShell>(&sent).unwrap(),
            CoreToShell::SettingsShow(built)
        );
        // A number at either bound, and with no most.
        for field in [
            number(0.0, 0.0, Some(60.0), 1.0, 0),
            number(60.0, 0.0, Some(60.0), 1.0, 0),
            number(1e9, 0.0, None, 0.25, 3),
        ] {
            assert!(SettingsShow::new(settings(vec![pane("general", vec![field])])).is_some());
        }
        // With nothing to show but what is wrong, it says that.
        let broken_file = SettingsShowParams {
            panes: Vec::new(),
            problem: Some("config line 3: expected a value".to_owned()),
        };
        assert_eq!(
            SettingsShow::new(broken_file).unwrap().problem(),
            Some("config line 3: expected a value")
        );

        let secret = FormField::Secret {
            id: "password".to_owned(),
            label: "Password:".to_owned(),
            focus: false,
            help: None,
        };
        let code = FormField::Code {
            id: "code".to_owned(),
            label: "Code:".to_owned(),
            value: Secret::new("HJKL-MNPQ"),
            help: None,
        };
        let untitled = FormField::Toggle {
            id: "on".to_owned(),
            label: String::new(),
            title: String::new(),
            value: true,
            help: None,
        };
        let one = |field: FormField| settings(vec![pane("general", vec![field])]);
        let broken: Vec<(&str, SettingsShowParams)> = vec![
            ("nothing at all", settings(Vec::new())),
            (
                "nothing, and nothing said to be wrong",
                SettingsShowParams {
                    panes: Vec::new(),
                    problem: Some(String::new()),
                },
            ),
            (
                "two panes of one name",
                settings(vec![
                    pane("general", Vec::new()),
                    pane("general", Vec::new()),
                ]),
            ),
            (
                "a pane that is no name",
                settings(vec![pane("General", Vec::new())]),
            ),
            (
                "a pane with no title",
                settings(vec![SettingsPane {
                    title: String::new(),
                    ..pane("general", Vec::new())
                }]),
            ),
            (
                "a pane with no symbol",
                settings(vec![SettingsPane {
                    symbol: String::new(),
                    ..pane("general", Vec::new())
                }]),
            ),
            (
                "a symbol that is no symbol's name",
                settings(vec![SettingsPane {
                    symbol: "Gear Shape".to_owned(),
                    ..pane("general", Vec::new())
                }]),
            ),
            (
                "two fields of one name in a pane",
                settings(vec![pane(
                    "general",
                    vec![toggle("on", true), toggle("on", false)],
                )]),
            ),
            ("a secret", one(secret)),
            ("a code", one(code)),
            ("a field with the focus", one(text_field("name", true))),
            ("a toggle that says nothing", one(untitled)),
            (
                "a number below its least",
                one(number(-1.0, 0.0, None, 1.0, 0)),
            ),
            (
                "a number above its most",
                one(number(61.0, 0.0, Some(60.0), 1.0, 0)),
            ),
            (
                "a most below the least",
                one(number(5.0, 5.0, Some(1.0), 1.0, 0)),
            ),
            (
                "a number that steps by nothing",
                one(number(10.0, 0.0, None, 0.0, 0)),
            ),
            (
                "a number that steps backwards",
                one(number(10.0, 0.0, None, -1.0, 0)),
            ),
            (
                "more places than are shown",
                one(number(10.0, 0.0, None, 1.0, MAX_FORM_DECIMALS + 1)),
            ),
            (
                "two rows of one name",
                one(list(&[("work", &[]), ("work", &[])], &[])),
            ),
            ("a row that is no name", one(list(&[("Work", &[])], &[]))),
            (
                "two actions of one name",
                one(list(&[], &["sign_in", "sign_in"])),
            ),
            (
                "a row that takes an action the list has not",
                one(list(&[("work", &["remove"])], &["sign_in"])),
            ),
            (
                "too many actions",
                one(list(&[], &["a", "b", "c", "d", "e", "f", "g"])),
            ),
            (
                "too many panes",
                settings(
                    (0..=MAX_SETTINGS_PANES)
                        .map(|n| pane(&format!("p{n}"), Vec::new()))
                        .collect(),
                ),
            ),
            (
                "too many fields in a pane",
                settings(vec![pane(
                    "general",
                    (0..=MAX_FORM_FIELDS)
                        .map(|n| toggle(&format!("f{n}"), true))
                        .collect(),
                )]),
            ),
        ];
        for (what, params) in broken {
            assert!(SettingsShow::new(params.clone()).is_none(), "{what}");
            // And what cannot be built cannot be decoded either.
            let line = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "settings.show",
                "params": serde_json::to_value(&params).unwrap(),
            })
            .to_string();
            assert!(decode::<CoreToShell>(&line).is_err(), "{what}");
        }
        // The same field in two panes is two fields.
        assert!(
            SettingsShow::new(settings(vec![
                pane("general", vec![toggle("on", true)]),
                pane("privacy", vec![toggle("on", true)]),
            ]))
            .is_some()
        );
    }

    #[test]
    fn a_number_is_one_and_a_whole_one_is_written_as_one() {
        assert!(Number::new(f64::NAN).is_none());
        assert!(Number::new(f64::INFINITY).is_none());
        assert_eq!(Number::new(-0.0), Number::new(0.0));
        assert_eq!(Number::from(10u32).get(), 10.0);
        assert_eq!(Number::new(0.126).unwrap().rounded(2).get(), 0.13);
        assert_eq!(Number::new(7.0).unwrap().rounded(0).whole(), Some(7));
        assert_eq!(Number::new(0.5).unwrap().whole(), None);
        let text = |value: f64| serde_json::to_string(&Number::new(value).unwrap()).unwrap();
        assert_eq!(text(10.0), "10");
        assert_eq!(text(4_294_967_295.0), "4294967295");
        assert_eq!(text(0.5), "0.5");
        assert_eq!(text(-2.0), "-2");
        for (json, value) in [("10", 10.0), ("10.0", 10.0), ("0.25", 0.25), ("-3", -3.0)] {
            assert_eq!(serde_json::from_str::<Number>(json).unwrap().get(), value);
        }
        for bad in ["\"10\"", "true", "null", "[1]"] {
            assert!(serde_json::from_str::<Number>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn what_was_done_in_the_settings_is_a_value_or_an_action_never_both() {
        let line = |params: &str| {
            format!(r#"{{"jsonrpc":"2.0","method":"settings.used","params":{params}}}"#)
        };
        let read = |params: &str| match decode::<ShellToCore>(&line(params)) {
            Ok(ShellToCore::SettingsUsed(used)) => Some(used),
            Ok(other) => panic!("{other:?}"),
            Err(_) => None,
        };
        let used = read(r#"{"pane":"general","field":"notifications","value":false}"#).unwrap();
        assert_eq!((used.pane(), used.field()), ("general", "notifications"));
        assert_eq!(used.did(), &SettingsDid::Set(FormValue::Flag(false)));
        assert_eq!(
            read(r#"{"pane":"general","field":"theme","value":"light"}"#)
                .unwrap()
                .did(),
            &SettingsDid::Set(FormValue::Text("light".to_owned()))
        );
        for (number, value) in [("15", 15.0), ("0.5", 0.5), ("2.0", 2.0)] {
            let params = format!(r#"{{"pane":"storage","field":"cache_gb","value":{number}}}"#);
            assert_eq!(
                read(&params).unwrap().did(),
                &SettingsDid::Set(FormValue::Number(Number::new(value).unwrap()))
            );
        }
        assert_eq!(
            read(r#"{"pane":"accounts","field":"accounts","action":"add"}"#)
                .unwrap()
                .did(),
            &SettingsDid::Action {
                action: "add".to_owned(),
                row: None
            }
        );
        assert_eq!(
            read(r#"{"pane":"accounts","field":"accounts","action":"sign_in","row":"account_1"}"#)
                .unwrap()
                .did(),
            &SettingsDid::Action {
                action: "sign_in".to_owned(),
                row: Some("account_1".to_owned())
            }
        );

        // What is built is what is written, and what is read back.
        let set = SettingsUsed::set("general", "threads", FormValue::Flag(true)).unwrap();
        let sent = encode(&ShellToCore::SettingsUsed(set.clone())).unwrap();
        assert_eq!(
            sent,
            line(r#"{"pane":"general","field":"threads","value":true}"#)
        );
        assert_eq!(
            decode::<ShellToCore>(&sent).unwrap(),
            ShellToCore::SettingsUsed(set)
        );
        let asked = SettingsUsed::action("accounts", "accounts", "sign_in", Some("row_1")).unwrap();
        assert_eq!(
            encode(&ShellToCore::SettingsUsed(asked)).unwrap(),
            line(r#"{"pane":"accounts","field":"accounts","action":"sign_in","row":"row_1"}"#)
        );

        for bad in [
            r#"{"pane":"general","field":"theme"}"#,
            r#"{"pane":"general","field":"theme","value":"light","action":"add"}"#,
            r#"{"pane":"general","field":"theme","value":"light","row":"row_1"}"#,
            r#"{"pane":"general","field":"theme","row":"row_1"}"#,
            r#"{"pane":"General","field":"theme","value":"light"}"#,
            r#"{"pane":"general","field":"","value":"light"}"#,
            r#"{"pane":"general","field":"theme","value":null}"#,
            r#"{"pane":"general","field":"theme","value":["light"]}"#,
            r#"{"pane":"general","field":"theme","value":{"is":"light"}}"#,
            r#"{"pane":"accounts","field":"accounts","action":"Sign In"}"#,
            r#"{"pane":"accounts","field":"accounts","action":"sign_in","row":"Work"}"#,
            r#"{"field":"theme","value":"light"}"#,
            r#"{"pane":"general","field":"theme","value":"light","step":"general"}"#,
        ] {
            assert!(read(bad).is_none(), "{bad}");
        }
        assert!(SettingsUsed::set("general", "theme", FormValue::Text("x".repeat(4096))).is_some());
        assert!(SettingsUsed::set("general", "theme", FormValue::Text("x".repeat(4097))).is_none());
        assert!(SettingsUsed::set("General", "theme", FormValue::Flag(true)).is_none());
        assert!(SettingsUsed::action("accounts", "accounts", "add", Some("Row")).is_none());
        // Opening and closing the window say nothing more than that.
        for method in ["settings.opened", "settings.closed"] {
            let fine = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{{}}}}"#);
            assert!(decode::<ShellToCore>(&fine).is_ok());
            let extra =
                format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{{"pane":"general"}}}}"#);
            assert!(decode::<ShellToCore>(&extra).is_err());
        }
    }

    #[test]
    fn the_names_in_a_form_are_plain_ones() {
        for id in ["a", "smtp_host", "sha256", "trust_once"] {
            assert!(valid_form_id(id), "{id}");
        }
        let long = "a".repeat(MAX_FORM_ID + 1);
        for id in [
            "",
            "Name",
            "9lives",
            "_x",
            "a b",
            "a-b",
            "a.b",
            "é",
            long.as_str(),
        ] {
            assert!(!valid_form_id(id), "{id}");
        }
        assert!(valid_form_id(&"a".repeat(MAX_FORM_ID)));
    }

    #[test]
    fn a_secret_is_sent_and_read_but_never_printed() {
        let used = FormUsed::new(
            "password",
            Some("continue"),
            &[("name", "Example")],
            &[("password", "not-a-real-password")],
        )
        .unwrap();
        // It is there for the one place that uses it.
        assert_eq!(
            used.secret("password").map(Secret::reveal),
            Some("not-a-real-password")
        );
        assert_eq!(used.value("name"), Some("Example"));
        assert_eq!(used.value("password"), None);
        assert_eq!(used.secret("name"), None);
        assert_eq!((used.step(), used.button()), ("password", Some("continue")));
        // On the wire it is a string like any other, and comes back whole.
        let message = ShellToCore::FormUsed(used.clone());
        let line = encode(&message).unwrap();
        assert!(line.contains(r#""secrets":{"password":"not-a-real-password"}"#));
        assert_eq!(decode::<ShellToCore>(&line).unwrap(), message);

        // Printed any way there is, it is not there: by itself, in its
        // message, pretty or not.
        let secret = used.secret("password").unwrap();
        for printed in [
            format!("{secret:?}"),
            format!("{secret:#?}"),
            format!("{used:?}"),
            format!("{used:#?}"),
            format!("{message:?}"),
            format!("{message:#?}"),
        ] {
            assert!(!printed.contains("not-a-real-password"), "{printed}");
            assert!(printed.contains("Secret(…)"), "{printed}");
        }
        // What is not secret still prints, to be of use.
        assert!(format!("{used:?}").contains("Example"));
    }

    #[test]
    fn a_code_is_shown_and_sent_but_never_printed() {
        let code = |id: &str, value: &str| FormField::Code {
            id: id.to_owned(),
            label: "Code:".to_owned(),
            value: Secret::new(value),
            help: None,
        };
        let shown = FormShow::new(FormShowParams {
            step: "sign_in".to_owned(),
            title: "Sign In to Work".to_owned(),
            busy: Some("Quill is waiting.".to_owned()),
            fields: vec![
                code("page", "https://login.example.com/device"),
                code("code", "HJKL-MNPQ"),
            ],
            buttons: vec![button("cancel", Some(FormRole::Cancel))],
            ..FormShowParams::default()
        })
        .unwrap();
        // It is there for the one place that shows it, and is no value to
        // be typed over or the keyboard's to start in.
        let field = &shown.fields()[1];
        assert_eq!(field.code().map(Secret::reveal), Some("HJKL-MNPQ"));
        assert_eq!(field.value(), None);
        assert!(!field.focus());
        assert_eq!((field.id(), field.label()), ("code", "Code:"));
        assert_eq!(
            shown.fields()[0].code().map(Secret::reveal).map(str::len),
            Some(32)
        );
        // On the wire it is a string like any other, and comes back whole.
        let message = CoreToShell::FormShow(shown.clone());
        let line = encode(&message).unwrap();
        assert!(
            line.contains(r#"{"kind":"code","id":"code","label":"Code:","value":"HJKL-MNPQ"}"#),
            "{line}"
        );
        assert_eq!(decode::<CoreToShell>(&line).unwrap(), message);
        // Printed any way there is, neither the code nor the page is there.
        for printed in [
            format!("{field:?}"),
            format!("{shown:?}"),
            format!("{shown:#?}"),
            format!("{message:?}"),
            format!("{message:#?}"),
        ] {
            assert!(!printed.contains("HJKL"), "{printed}");
            assert!(!printed.contains("login.example.com"), "{printed}");
            assert!(printed.contains("Secret(…)"), "{printed}");
        }
        // It is only shown: it has no focus to be given, and it says what
        // it shows.
        for line in [
            r#"{"jsonrpc":"2.0","method":"form.show","params":{"step":"sign_in","title":"x","fields":[{"kind":"code","id":"code","label":"Code:","value":"HJKL-MNPQ","focus":true}],"buttons":[]}}"#,
            r#"{"jsonrpc":"2.0","method":"form.show","params":{"step":"sign_in","title":"x","fields":[{"kind":"code","id":"code","label":"Code:"}],"buttons":[]}}"#,
        ] {
            assert!(decode::<CoreToShell>(line).is_err(), "{line}");
        }
    }

    #[test]
    fn what_was_done_with_a_form_names_a_step_and_its_fields() {
        // A choice changed: no button.
        let changed = FormUsed::new("server", None, &[("type", "microsoft")], &[]).unwrap();
        assert_eq!(changed.button(), None);
        let line = encode(&ShellToCore::FormUsed(changed)).unwrap();
        assert!(!line.contains("button"), "{line}");
        assert!(line.contains(r#""secrets":{}"#), "{line}");

        assert!(FormUsed::new("The Server", None, &[], &[]).is_none());
        assert!(FormUsed::new("server", Some("Continue"), &[], &[]).is_none());
        assert!(FormUsed::new("server", Some(""), &[], &[]).is_none());
        assert!(FormUsed::new("server", None, &[("Name", "x")], &[]).is_none());
        assert!(FormUsed::new("server", None, &[], &[("pass word", "x")]).is_none());
        for line in [
            r#"{"jsonrpc":"2.0","method":"form.used","params":{"step":"server","values":{"Name":"x"},"secrets":{}}}"#,
            r#"{"jsonrpc":"2.0","method":"form.used","params":{"step":"server","button":null,"values":{},"secrets":{"a b":"x"}}}"#,
            r#"{"jsonrpc":"2.0","method":"form.used","params":{"step":"server","values":{}}}"#,
            r#"{"jsonrpc":"2.0","method":"form.used","params":{"step":"server","values":{},"secrets":{},"password":"x"}}"#,
        ] {
            assert!(decode::<ShellToCore>(line).is_err(), "{line}");
        }
    }

    #[test]
    fn optional_fields_are_left_out_rather_than_null() {
        let line = encode(&CoreToShell::CommandsList(CommandsList {
            commands: vec![CommandInfo {
                name: "nav.next".to_owned(),
                title: "Next".to_owned(),
                keys: vec!["j".to_owned()],
                menu: None,
                symbol: None,
            }],
        }))
        .unwrap();
        assert!(!line.contains("null"), "{line}");
        assert!(!line.contains("menu"), "{line}");
    }
}
