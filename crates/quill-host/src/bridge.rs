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

/// Whether `id` can name a step, field, option or button of a form: lower
/// case letters, digits and underscores, starting with a letter.
pub fn valid_form_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    id.len() <= MAX_FORM_ID
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Text that must not be shown or logged: what was typed into a secret
/// field of a form. `Debug` prints a placeholder, and there is no
/// `Display`.
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
/// core never sends one.
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
            if let FormField::Choice { value, options, .. } = field {
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
}

impl FormField {
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. }
            | Self::Secret { id, .. }
            | Self::Choice { id, .. }
            | Self::Static { id, .. } => id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Text { label, .. }
            | Self::Secret { label, .. }
            | Self::Choice { label, .. }
            | Self::Static { label, .. } => label,
        }
    }

    /// What the field holds, as the core has it. A secret has nothing to
    /// say.
    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Text { value, .. } | Self::Choice { value, .. } | Self::Static { value, .. } => {
                Some(value)
            }
            Self::Secret { .. } => None,
        }
    }

    /// Whether the keyboard starts in this field.
    pub fn focus(&self) -> bool {
        match self {
            Self::Text { focus, .. } | Self::Secret { focus, .. } | Self::Choice { focus, .. } => {
                *focus
            }
            Self::Static { .. } => false,
        }
    }

    /// Said under the field, in small print.
    pub fn help(&self) -> Option<&str> {
        match self {
            Self::Text { help, .. }
            | Self::Secret { help, .. }
            | Self::Choice { help, .. }
            | Self::Static { help, .. } => help.as_deref(),
        }
    }
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
