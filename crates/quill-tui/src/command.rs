//! The command registry. Every action is a named command; keys, the help
//! overlay, the status bar hints and (from M1) the app shell's menus all read
//! from here.

/// Where the user is. Decides which commands apply and what a key means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Sidebar,
    List,
    Reader,
    Help,
    /// Choosing one of a message's links to open.
    Links,
    /// Adding an account. Typing goes into the field with the cursor.
    Setup,
    /// Deciding about a server certificate that was not trusted.
    Certificate,
    /// Choosing a folder to move a message to. Typing narrows the list.
    Folders,
    /// Writing a message. Typing goes into the field with the cursor.
    Compose,
}

impl Context {
    pub fn title(self) -> &'static str {
        match self {
            Self::Sidebar => "Accounts",
            Self::List => "Message list",
            Self::Reader => "Reader",
            Self::Help => "Help",
            Self::Links => "Links",
            Self::Setup => "Add an account",
            Self::Certificate => "Certificate",
            Self::Folders => "Move to a folder",
            Self::Compose => "Writing a message",
        }
    }
}

const PANES: &[Context] = &[Context::Sidebar, Context::List, Context::Reader];
const EVERYWHERE: &[Context] = &[
    Context::Sidebar,
    Context::List,
    Context::Reader,
    Context::Help,
    Context::Links,
    Context::Setup,
    Context::Certificate,
    Context::Folders,
    Context::Compose,
];
/// The panes, and the form whose fields `tab` moves between.
const PANES_AND_FORMS: &[Context] = &[
    Context::Sidebar,
    Context::List,
    Context::Reader,
    Context::Setup,
    Context::Compose,
];
const COMPOSE: &[Context] = &[Context::Compose];
/// The panes, and the message being written in the reader's place.
const PANES_AND_COMPOSE: &[Context] = &[
    Context::Sidebar,
    Context::List,
    Context::Reader,
    Context::Compose,
];
/// Where a message is in view: commands on its body apply.
const MESSAGE: &[Context] = &[Context::List, Context::Reader];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandId {
    NavNext,
    NavPrev,
    NavTop,
    NavBottom,
    NavPageDown,
    NavPageUp,
    NavOpen,
    AppQuit,
    NavBack,
    FocusSidebar,
    FocusList,
    FocusReader,
    FocusNext,
    FocusPrev,
    ViewZoom,
    MessageToggleUnread,
    MessageToggleStar,
    MessageToggleHtml,
    MessageOpenLink,
    MessageArchive,
    MessageDelete,
    MessageMove,
    MessageCompose,
    MessageReply,
    MessageReplyAll,
    MessageForward,
    ComposeSend,
    ComposeSave,
    ComposeAttach,
    ComposeBcc,
    EditUndo,
    AccountAdd,
    DebugPreviewSample,
    DebugImageSample,
    HelpToggle,
}

/// How the help overlay groups commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Navigation,
    Panes,
    Message,
    General,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Navigation,
        Category::Panes,
        Category::Message,
        Category::General,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Panes => "Panes",
            Self::Message => "Message",
            Self::General => "General",
        }
    }
}

/// The app shell menu a command appears in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    App,
    File,
    Edit,
    View,
    Mailbox,
    Message,
    Help,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub id: CommandId,
    /// The stable name used in `keys.toml` and on the shell bridge.
    pub name: &'static str,
    /// What the help overlay and the menus call it.
    pub title: &'static str,
    pub category: Category,
    /// `None` keeps the command out of the menus (plain cursor movement).
    pub menu: Option<Menu>,
    /// An SF Symbol name, for commands worth a toolbar button in the app.
    pub symbol: Option<&'static str>,
    pub contexts: &'static [Context],
}

impl CommandSpec {
    pub fn applies_in(&self, context: Context) -> bool {
        self.contexts.contains(&context)
    }
}

/// Every command, in a fixed order. Where two commands share a key, the
/// earlier one wins in the contexts they share.
pub struct Registry {
    specs: Vec<CommandSpec>,
}

impl Registry {
    pub fn builtin() -> Self {
        use Category::{General, Message, Navigation, Panes};
        use CommandId::*;
        let spec = |id, name, title, category, menu, contexts| CommandSpec {
            id,
            name,
            title,
            category,
            menu,
            symbol: None,
            contexts,
        };
        let with_symbol = |spec: CommandSpec, symbol| CommandSpec {
            symbol: Some(symbol),
            ..spec
        };
        Self {
            specs: vec![
                spec(
                    NavNext,
                    "nav.next",
                    "Next item or line",
                    Navigation,
                    None,
                    EVERYWHERE,
                ),
                spec(
                    NavPrev,
                    "nav.prev",
                    "Previous item or line",
                    Navigation,
                    None,
                    EVERYWHERE,
                ),
                spec(
                    NavTop,
                    "nav.top",
                    "Go to the top",
                    Navigation,
                    None,
                    EVERYWHERE,
                ),
                spec(
                    NavBottom,
                    "nav.bottom",
                    "Go to the bottom",
                    Navigation,
                    None,
                    EVERYWHERE,
                ),
                spec(
                    NavPageDown,
                    "nav.page_down",
                    "Page down",
                    Navigation,
                    None,
                    EVERYWHERE,
                ),
                spec(
                    NavPageUp,
                    "nav.page_up",
                    "Page up",
                    Navigation,
                    None,
                    EVERYWHERE,
                ),
                spec(
                    NavOpen,
                    "nav.open",
                    "Open",
                    Navigation,
                    Some(Menu::Message),
                    &[
                        Context::Sidebar,
                        Context::List,
                        Context::Links,
                        Context::Setup,
                        Context::Folders,
                    ],
                ),
                // Before nav.back, so that a key bound to both quits from a
                // pane and only closes the help overlay.
                // No menu item: in the app, Quit belongs to the shell, which asks the
                // core to stop over the bridge.
                spec(AppQuit, "app.quit", "Quit", General, None, PANES),
                spec(
                    NavBack,
                    "nav.back",
                    "Back, close or dismiss",
                    Navigation,
                    None,
                    EVERYWHERE,
                ),
                spec(
                    FocusSidebar,
                    "focus.sidebar",
                    "Focus the accounts pane",
                    Panes,
                    Some(Menu::View),
                    PANES,
                ),
                spec(
                    FocusList,
                    "focus.list",
                    "Focus the message list",
                    Panes,
                    Some(Menu::View),
                    PANES,
                ),
                spec(
                    FocusReader,
                    "focus.reader",
                    "Focus the reader",
                    Panes,
                    Some(Menu::View),
                    PANES,
                ),
                spec(
                    FocusNext,
                    "focus.next",
                    "Focus the next pane or field",
                    Panes,
                    Some(Menu::View),
                    PANES_AND_FORMS,
                ),
                spec(
                    FocusPrev,
                    "focus.prev",
                    "Focus the previous pane or field",
                    Panes,
                    Some(Menu::View),
                    PANES_AND_FORMS,
                ),
                with_symbol(
                    spec(
                        ViewZoom,
                        "view.zoom",
                        "Zoom the focused pane",
                        Panes,
                        Some(Menu::View),
                        PANES_AND_COMPOSE,
                    ),
                    "arrow.up.left.and.arrow.down.right",
                ),
                with_symbol(
                    spec(
                        MessageToggleUnread,
                        "message.toggle_unread",
                        "Mark read or unread",
                        Message,
                        Some(Menu::Message),
                        &[Context::List, Context::Reader],
                    ),
                    "envelope.badge",
                ),
                with_symbol(
                    spec(
                        MessageToggleStar,
                        "message.toggle_star",
                        "Star or unstar",
                        Message,
                        Some(Menu::Message),
                        &[Context::List, Context::Reader],
                    ),
                    "star",
                ),
                with_symbol(
                    spec(
                        MessageToggleHtml,
                        "message.toggle_html",
                        "Show as HTML or as plain text",
                        Message,
                        Some(Menu::Message),
                        MESSAGE,
                    ),
                    "doc.plaintext",
                ),
                with_symbol(
                    spec(
                        MessageOpenLink,
                        "message.open_link",
                        "Open a link",
                        Message,
                        Some(Menu::Message),
                        MESSAGE,
                    ),
                    "link",
                ),
                with_symbol(
                    spec(
                        MessageArchive,
                        "message.archive",
                        "Archive",
                        Message,
                        Some(Menu::Message),
                        MESSAGE,
                    ),
                    "archivebox",
                ),
                with_symbol(
                    spec(
                        MessageDelete,
                        "message.delete",
                        "Delete",
                        Message,
                        Some(Menu::Message),
                        MESSAGE,
                    ),
                    "trash",
                ),
                with_symbol(
                    spec(
                        MessageMove,
                        "message.move",
                        "Move to a folder",
                        Message,
                        Some(Menu::Message),
                        MESSAGE,
                    ),
                    "folder",
                ),
                with_symbol(
                    spec(
                        MessageCompose,
                        "message.compose",
                        "Write a new message",
                        Message,
                        Some(Menu::File),
                        PANES,
                    ),
                    "square.and.pencil",
                ),
                with_symbol(
                    spec(
                        MessageReply,
                        "message.reply",
                        "Reply",
                        Message,
                        Some(Menu::Message),
                        MESSAGE,
                    ),
                    "arrowshape.turn.up.left",
                ),
                spec(
                    MessageReplyAll,
                    "message.reply_all",
                    "Reply to all",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    MessageForward,
                    "message.forward",
                    "Forward",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                with_symbol(
                    spec(
                        ComposeSend,
                        "compose.send",
                        "Send",
                        Message,
                        Some(Menu::Message),
                        COMPOSE,
                    ),
                    "paperplane",
                ),
                spec(
                    ComposeSave,
                    "compose.save",
                    "Save as a draft",
                    Message,
                    Some(Menu::File),
                    COMPOSE,
                ),
                with_symbol(
                    spec(
                        ComposeAttach,
                        "compose.attach",
                        "Attach a file",
                        Message,
                        Some(Menu::Message),
                        COMPOSE,
                    ),
                    "paperclip",
                ),
                spec(
                    ComposeBcc,
                    "compose.bcc",
                    "Add blind copies (Bcc)",
                    Message,
                    Some(Menu::Message),
                    COMPOSE,
                ),
                spec(
                    EditUndo,
                    "edit.undo",
                    "Undo the last move, deletion or send",
                    General,
                    Some(Menu::Edit),
                    PANES,
                ),
                with_symbol(
                    spec(
                        AccountAdd,
                        "account.add",
                        "Add an account",
                        General,
                        Some(Menu::File),
                        PANES,
                    ),
                    "person.crop.circle.badge.plus",
                ),
                // A stand-in until messages have attachments to preview (M4):
                // it exercises the Quick Look pane end to end.
                with_symbol(
                    spec(
                        DebugPreviewSample,
                        "debug.preview_sample",
                        "Preview a sample file",
                        General,
                        Some(Menu::View),
                        PANES,
                    ),
                    "eye",
                ),
                // Likewise a stand-in: it proves the terminal draws images.
                with_symbol(
                    spec(
                        DebugImageSample,
                        "debug.image_sample",
                        "Show a sample image",
                        General,
                        Some(Menu::View),
                        PANES,
                    ),
                    "photo",
                ),
                with_symbol(
                    spec(
                        HelpToggle,
                        "help.toggle",
                        "Help for this context",
                        General,
                        Some(Menu::Help),
                        EVERYWHERE,
                    ),
                    "questionmark.circle",
                ),
            ],
        }
    }

    pub fn specs(&self) -> &[CommandSpec] {
        &self.specs
    }

    pub fn get(&self, id: CommandId) -> Option<&CommandSpec> {
        self.specs.iter().find(|spec| spec.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&CommandSpec> {
        self.specs.iter().find(|spec| spec.name == name)
    }
}

/// One entry of the status bar: the keys of one or two commands and a label.
#[derive(Debug, Clone, Copy)]
pub struct Hint {
    pub commands: &'static [CommandId],
    pub label: &'static str,
}

/// The hints the status bar shows in `context`, most useful first. The bar
/// shows as many as fit.
pub fn hints(context: Context) -> &'static [Hint] {
    use CommandId::*;
    const fn hint(commands: &'static [CommandId], label: &'static str) -> Hint {
        Hint { commands, label }
    }
    const SIDEBAR: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "open"),
        hint(&[FocusNext], "next pane"),
        hint(&[ViewZoom], "zoom"),
        hint(&[HelpToggle], "help"),
        hint(&[AppQuit], "quit"),
    ];
    const LIST: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "open"),
        hint(&[MessageCompose], "write"),
        hint(&[MessageReply], "reply"),
        hint(&[MessageArchive], "archive"),
        hint(&[MessageDelete], "delete"),
        hint(&[MessageToggleUnread], "unread"),
        hint(&[MessageToggleStar], "star"),
        hint(&[FocusNext], "next pane"),
        hint(&[ViewZoom], "zoom"),
        hint(&[HelpToggle], "help"),
        hint(&[AppQuit], "quit"),
    ];
    const READER: &[Hint] = &[
        hint(&[NavNext, NavPrev], "scroll"),
        hint(&[NavPageDown], "page"),
        hint(&[NavBack], "back"),
        hint(&[MessageReply], "reply"),
        hint(&[MessageArchive], "archive"),
        hint(&[MessageDelete], "delete"),
        hint(&[MessageOpenLink], "links"),
        hint(&[MessageToggleHtml], "html/text"),
        hint(&[MessageToggleUnread], "unread"),
        hint(&[MessageToggleStar], "star"),
        hint(&[ViewZoom], "zoom"),
        hint(&[HelpToggle], "help"),
        hint(&[AppQuit], "quit"),
    ];
    const HELP: &[Hint] = &[
        hint(&[NavNext, NavPrev], "scroll"),
        hint(&[NavBack], "close"),
    ];
    const LINKS: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "open in browser"),
        hint(&[NavBack], "close"),
    ];
    const SETUP: &[Hint] = &[
        hint(&[NavOpen], "continue"),
        hint(&[FocusNext], "next field"),
        hint(&[NavBack], "back"),
    ];
    const CERTIFICATE: &[Hint] = &[hint(&[NavBack], "reject")];
    const FOLDERS: &[Hint] = &[hint(&[NavOpen], "move here"), hint(&[NavBack], "cancel")];
    const COMPOSING: &[Hint] = &[
        hint(&[ComposeSend], "send"),
        hint(&[ComposeSave], "save draft"),
        hint(&[ComposeAttach], "attach"),
        hint(&[FocusNext], "next field"),
        hint(&[NavBack], "close"),
    ];
    match context {
        Context::Compose => COMPOSING,
        Context::Folders => FOLDERS,
        Context::Setup => SETUP,
        Context::Certificate => CERTIFICATE,
        Context::Links => LINKS,
        Context::Sidebar => SIDEBAR,
        Context::List => LIST,
        Context::Reader => READER,
        Context::Help => HELP,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn command_ids_and_names_are_unique() {
        let registry = Registry::builtin();
        let ids: HashSet<_> = registry.specs().iter().map(|spec| spec.id).collect();
        let names: HashSet<_> = registry.specs().iter().map(|spec| spec.name).collect();
        assert_eq!(ids.len(), registry.specs().len());
        assert_eq!(names.len(), registry.specs().len());
    }

    #[test]
    fn names_are_dotted_lowercase() {
        for spec in Registry::builtin().specs() {
            assert!(spec.name.contains('.'), "{}", spec.name);
            assert!(
                spec.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "{}",
                spec.name
            );
            assert!(!spec.contexts.is_empty(), "{}", spec.name);
        }
    }

    #[test]
    fn lookups_agree() {
        let registry = Registry::builtin();
        let spec = registry.by_name("view.zoom").unwrap();
        assert_eq!(spec.id, CommandId::ViewZoom);
        assert_eq!(registry.get(CommandId::ViewZoom).unwrap().name, "view.zoom");
        assert!(registry.by_name("no.such").is_none());
    }

    #[test]
    fn every_hint_names_commands_that_apply_in_its_context() {
        let registry = Registry::builtin();
        for context in EVERYWHERE {
            for hint in hints(*context) {
                for id in hint.commands {
                    let spec = registry.get(*id).unwrap();
                    assert!(spec.applies_in(*context), "{} in {context:?}", spec.name);
                }
            }
        }
    }

    #[test]
    fn quit_comes_before_back_so_a_shared_key_quits_from_a_pane() {
        let registry = Registry::builtin();
        let position = |id| {
            registry
                .specs()
                .iter()
                .position(|spec| spec.id == id)
                .unwrap()
        };
        assert!(position(CommandId::AppQuit) < position(CommandId::NavBack));
        assert!(
            !registry
                .get(CommandId::AppQuit)
                .unwrap()
                .applies_in(Context::Help)
        );
    }
}
