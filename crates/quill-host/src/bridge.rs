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
    /// Show sanitized HTML in a web view pane.
    #[serde(rename = "pane.web")]
    PaneWeb(PaneWeb),
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
    /// Run Vim on a draft file, in a pane. Answered by `editor.exited`.
    #[serde(rename = "pane.editor")]
    PaneEditor(PaneEditor),
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
    /// Unread messages across all inboxes, for the Dock badge.
    pub unread: u32,
    /// The window title.
    pub title: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneWeb {
    /// The sanitized HTML. Files beside it in its folder may be loaded.
    pub path: AbsolutePath,
    pub title: String,
    /// The user has allowed this message to load remote content.
    pub remote: bool,
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

/// A draft file for the shell's Vim to edit, and the draft's own folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaneEditor {
    pub path: AbsolutePath,
    pub dir: AbsolutePath,
    pub title: String,
    /// Which Vim to run.
    pub vim: EditorVim,
}

/// Which Vim the shell runs: the one that comes with macOS, or the user's
/// own by way of the script they keep in the app's scripts folder. It is
/// one of these words: the shell is never given a path to a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorVim {
    #[default]
    System,
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
