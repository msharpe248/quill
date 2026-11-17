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
    /// Choosing one of a message's attachments to view, open or save.
    Attachments,
    /// Adding an account. Typing goes into the field with the cursor.
    Setup,
    /// Deciding about a server certificate that was not trusted.
    Certificate,
    /// Waiting for the user to sign in to an account's provider.
    SignIn,
    /// Typing a time: when a snoozed message comes back, or when a
    /// message is sent.
    When,
    /// Being asked before unsubscribing from a mailing list.
    Unsubscribe,
    /// Being asked before an account is removed.
    RemoveAccount,
    /// Choosing a folder to move a message to. Typing narrows the list.
    Folders,
    /// Choosing a template to put into the message being written. Typing
    /// narrows the list.
    Templates,
    /// Choosing another spelling for a word of the message being written.
    Suggestions,
    /// Choosing one of the user's programs to give a message to. Typing
    /// narrows the list.
    Hooks,
    /// Writing a message. Typing goes into the field with the cursor.
    Compose,
    /// Typing what to search for. The list shows what is found so far.
    Search,
    /// The command palette: typing narrows what it offers.
    Palette,
}

impl Context {
    pub fn title(self) -> &'static str {
        match self {
            Self::Sidebar => "Accounts",
            Self::List => "Message list",
            Self::Reader => "Reader",
            Self::Help => "Help",
            Self::Links => "Links",
            Self::Attachments => "Attachments",
            Self::Setup => "Add an account",
            Self::Certificate => "Certificate",
            Self::SignIn => "Signing in",
            Self::When => "Choosing a time",
            Self::Unsubscribe => "Unsubscribe",
            Self::RemoveAccount => "Remove an account",
            Self::Folders => "Move to a folder",
            Self::Templates => "Insert a template",
            Self::Suggestions => "Spelling suggestions",
            Self::Hooks => "Run a hook",
            Self::Compose => "Writing a message",
            Self::Search => "Search",
            Self::Palette => "Command palette",
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
    Context::Attachments,
    Context::Setup,
    Context::Certificate,
    Context::SignIn,
    Context::When,
    Context::Unsubscribe,
    Context::RemoveAccount,
    Context::Folders,
    Context::Templates,
    Context::Suggestions,
    Context::Hooks,
    Context::Compose,
    Context::Search,
    Context::Palette,
];
/// Where there is something to move through: a pane, or a list drawn over
/// the panes. A question (a certificate, a sign-in, a time, unsubscribing)
/// has nothing to move in, and a key that moves must not reach what lies
/// behind it. Nor does a message being written: its own keys move its
/// cursor.
const LISTS: &[Context] = &[
    Context::Sidebar,
    Context::List,
    Context::Reader,
    Context::Help,
    Context::Links,
    Context::Attachments,
    Context::Folders,
    Context::Templates,
    Context::Suggestions,
    Context::Hooks,
    Context::Search,
    Context::Palette,
];
/// The same, and the form, whose fields the arrows step between.
const LISTS_AND_FORM: &[Context] = &[
    Context::Sidebar,
    Context::List,
    Context::Reader,
    Context::Help,
    Context::Links,
    Context::Attachments,
    Context::Setup,
    Context::Folders,
    Context::Templates,
    Context::Suggestions,
    Context::Hooks,
    Context::Search,
    Context::Palette,
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
const ATTACHMENTS: &[Context] = &[Context::Attachments];
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
    ViewSidebar,
    ViewZoom,
    ViewThreads,
    ViewWebReading,
    MessageToggleUnread,
    MessageToggleStar,
    MessageToggleHtml,
    MessageOpenLink,
    MessageArchive,
    MessageDelete,
    MessageMove,
    MessageSnooze,
    SelectToggle,
    SelectRange,
    SelectAll,
    SearchStart,
    PaletteOpen,
    MessageAttachments,
    MessageOpenInBrowser,
    MessageLoadRemote,
    SenderAlwaysRemote,
    MessageUnsubscribe,
    MessageHook,
    InviteAccept,
    InviteDecline,
    InviteTentative,
    AttachmentSave,
    AttachmentOpen,
    MessageCompose,
    MessageReply,
    MessageReplyAll,
    MessageForward,
    ComposeSend,
    ComposeSendLater,
    ComposeSave,
    ComposeAttach,
    ComposeBcc,
    ComposeTemplate,
    ComposeVim,
    ComposeFields,
    ComposeBody,
    ComposeTips,
    ComposeSpellSuggest,
    ComposeSpellNext,
    EditCopy,
    EditCut,
    EditSelectAll,
    EditUndo,
    AccountAdd,
    AccountSignIn,
    AccountRemove,
    MailboxSync,
    SidebarOpen,
    SidebarClose,
    HelpToggle,
    HelpKeys,
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
                    LISTS_AND_FORM,
                ),
                spec(
                    NavPrev,
                    "nav.prev",
                    "Previous item or line",
                    Navigation,
                    None,
                    LISTS_AND_FORM,
                ),
                spec(NavTop, "nav.top", "Go to the top", Navigation, None, LISTS),
                spec(
                    NavBottom,
                    "nav.bottom",
                    "Go to the bottom",
                    Navigation,
                    None,
                    LISTS,
                ),
                spec(
                    NavPageDown,
                    "nav.page_down",
                    "Page down",
                    Navigation,
                    None,
                    LISTS,
                ),
                spec(NavPageUp, "nav.page_up", "Page up", Navigation, None, LISTS),
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
                        Context::Attachments,
                        Context::Setup,
                        Context::When,
                        Context::Unsubscribe,
                        Context::Folders,
                        Context::Templates,
                        Context::Suggestions,
                        Context::Hooks,
                        Context::Search,
                        Context::Palette,
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
                        ViewSidebar,
                        "view.sidebar",
                        "Show the accounts pane",
                        Panes,
                        Some(Menu::View),
                        PANES,
                    ),
                    "sidebar.left",
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
                spec(
                    ViewThreads,
                    "view.threads",
                    "Group or ungroup conversations",
                    Panes,
                    Some(Menu::View),
                    PANES,
                ),
                // Only where the host has a web view pane: Quill's own
                // app. In a terminal it is never enabled.
                with_symbol(
                    spec(
                        ViewWebReading,
                        "view.web_reading",
                        "Web reading pane",
                        Panes,
                        Some(Menu::View),
                        PANES,
                    ),
                    "sidebar.right",
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
                        MessageSnooze,
                        "message.snooze",
                        "Snooze until a time",
                        Message,
                        Some(Menu::Message),
                        MESSAGE,
                    ),
                    "clock",
                ),
                spec(
                    SelectToggle,
                    "select.toggle",
                    "Select or unselect",
                    Message,
                    Some(Menu::Message),
                    &[Context::List],
                ),
                spec(
                    SelectRange,
                    "select.range",
                    "Select a range",
                    Message,
                    Some(Menu::Message),
                    &[Context::List],
                ),
                spec(
                    SelectAll,
                    "select.all",
                    "Select every message listed",
                    Message,
                    Some(Menu::Message),
                    &[Context::List],
                ),
                with_symbol(
                    spec(
                        SearchStart,
                        "search.start",
                        "Search",
                        General,
                        Some(Menu::Mailbox),
                        PANES,
                    ),
                    "magnifyingglass",
                ),
                with_symbol(
                    spec(
                        MailboxSync,
                        "mailbox.sync",
                        "Check for New Mail",
                        General,
                        Some(Menu::Mailbox),
                        PANES,
                    ),
                    "arrow.clockwise",
                ),
                // In the accounts pane only: elsewhere the keys they take
                // are free.
                spec(
                    SidebarOpen,
                    "sidebar.open",
                    "Show the account's folders",
                    Panes,
                    Some(Menu::Mailbox),
                    &[Context::Sidebar],
                ),
                spec(
                    SidebarClose,
                    "sidebar.close",
                    "Hide the account's folders",
                    Panes,
                    Some(Menu::Mailbox),
                    &[Context::Sidebar],
                ),
                spec(
                    PaletteOpen,
                    "palette.open",
                    "Command palette",
                    General,
                    Some(Menu::View),
                    PANES,
                ),
                spec(
                    MessageAttachments,
                    "message.attachments",
                    "List the attachments",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    MessageOpenInBrowser,
                    "message.open_in_browser",
                    "Open in the browser",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    MessageLoadRemote,
                    "message.load_remote",
                    "Load remote pictures for this message",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    SenderAlwaysRemote,
                    "sender.always_remote",
                    "Always load remote pictures from this sender",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    MessageUnsubscribe,
                    "message.unsubscribe",
                    "Unsubscribe from this list",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    MessageHook,
                    "message.hook",
                    "Run a hook on this message",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    InviteAccept,
                    "invite.accept",
                    "Accept the invitation",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    InviteDecline,
                    "invite.decline",
                    "Decline the invitation",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    InviteTentative,
                    "invite.tentative",
                    "Answer the invitation with maybe",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    AttachmentSave,
                    "attachment.save",
                    "Save the attachment",
                    Message,
                    Some(Menu::Message),
                    ATTACHMENTS,
                ),
                spec(
                    AttachmentOpen,
                    "attachment.open",
                    "Open the attachment in its application",
                    Message,
                    Some(Menu::Message),
                    ATTACHMENTS,
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
                    ComposeSendLater,
                    "compose.send_later",
                    "Send at a time",
                    Message,
                    Some(Menu::Message),
                    COMPOSE,
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
                    ComposeTemplate,
                    "compose.template",
                    "Insert a template",
                    Message,
                    Some(Menu::Message),
                    COMPOSE,
                ),
                spec(
                    ComposeVim,
                    "compose.vim",
                    "Write in Another Editor",
                    Message,
                    Some(Menu::Message),
                    COMPOSE,
                ),
                // The two keys below are the ends of the text in Quill's
                // own editor, and still are: see `State::run`.
                spec(
                    ComposeFields,
                    "compose.fields",
                    "Go to the fields above the text",
                    Message,
                    Some(Menu::Message),
                    COMPOSE,
                ),
                spec(
                    ComposeBody,
                    "compose.body",
                    "Go to the text",
                    Message,
                    Some(Menu::Message),
                    COMPOSE,
                ),
                spec(
                    ComposeTips,
                    "compose.tips",
                    "Show what the editor's keys do",
                    Message,
                    Some(Menu::Help),
                    COMPOSE,
                ),
                spec(
                    ComposeSpellSuggest,
                    "compose.spell_suggest",
                    "Spelling suggestions",
                    Message,
                    Some(Menu::Edit),
                    COMPOSE,
                ),
                spec(
                    ComposeSpellNext,
                    "compose.spell_next",
                    "Go to the next misspelling",
                    Message,
                    Some(Menu::Edit),
                    COMPOSE,
                ),
                // The three below have no menu items of their own. The app's
                // Copy, Cut and Select All run them while they are enabled,
                // and act on the terminal's own selection otherwise.
                spec(
                    EditCopy,
                    "edit.copy",
                    "Copy what is selected in the message being written",
                    General,
                    None,
                    COMPOSE,
                ),
                spec(
                    EditCut,
                    "edit.cut",
                    "Cut what is selected in the message being written",
                    General,
                    None,
                    COMPOSE,
                ),
                spec(
                    EditSelectAll,
                    "edit.select_all",
                    "Select all of the message being written",
                    General,
                    None,
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
                with_symbol(
                    spec(
                        AccountSignIn,
                        "account.sign_in",
                        "Sign in to an account",
                        General,
                        Some(Menu::File),
                        PANES,
                    ),
                    "person.badge.key",
                ),
                spec(
                    AccountRemove,
                    "account.remove",
                    "Remove this account\u{2026}",
                    General,
                    Some(Menu::File),
                    PANES,
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
                spec(
                    HelpKeys,
                    "help.keys",
                    "Keyboard shortcuts",
                    General,
                    Some(Menu::Help),
                    EVERYWHERE,
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
    /// Shown only while one of its commands would do something: for a
    /// command that, for many users, never has anything to act on.
    pub only_enabled: bool,
}

/// The hints the status bar shows in `context`, most useful first. The bar
/// shows as many as fit.
pub fn hints(context: Context) -> &'static [Hint] {
    use CommandId::*;
    const fn hint(commands: &'static [CommandId], label: &'static str) -> Hint {
        Hint {
            commands,
            label,
            only_enabled: false,
        }
    }
    const fn hint_while_enabled(commands: &'static [CommandId], label: &'static str) -> Hint {
        Hint {
            commands,
            label,
            only_enabled: true,
        }
    }
    const SIDEBAR: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "open"),
        // One or the other, and only on an account or one of its folders.
        hint_while_enabled(&[SidebarOpen], "show folders"),
        hint_while_enabled(&[SidebarClose], "hide folders"),
        hint(&[FocusNext], "list"),
        hint(&[ViewZoom], "zoom"),
        hint(&[HelpToggle], "help"),
        hint(&[AppQuit], "quit"),
    ];
    const LIST: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "open"),
        hint(&[SearchStart], "search"),
        hint(&[MessageCompose], "write"),
        hint_while_enabled(&[InviteAccept], "accept"),
        hint_while_enabled(&[InviteDecline], "decline"),
        hint(&[MessageReply], "reply"),
        hint(&[MessageArchive], "archive"),
        hint(&[MessageDelete], "delete"),
        hint(&[MessageToggleUnread], "unread"),
        hint(&[MessageToggleStar], "star"),
        hint(&[SelectToggle], "select"),
        hint_while_enabled(&[MessageHook], "hooks"),
        hint_while_enabled(&[ViewWebReading], "web/text"),
        hint_while_enabled(&[FocusNext], "message"),
        hint(&[ViewZoom], "zoom"),
        hint(&[HelpToggle], "help"),
        hint(&[AppQuit], "quit"),
    ];
    const READER: &[Hint] = &[
        hint(&[NavNext, NavPrev], "scroll"),
        hint(&[NavPageDown], "page"),
        hint(&[NavBack], "back"),
        hint_while_enabled(&[InviteAccept], "accept"),
        hint_while_enabled(&[InviteDecline], "decline"),
        hint_while_enabled(&[InviteTentative], "maybe"),
        hint(&[MessageReply], "reply"),
        hint(&[MessageArchive], "archive"),
        hint(&[MessageDelete], "delete"),
        hint(&[MessageAttachments], "files"),
        hint(&[MessageOpenLink], "links"),
        hint(&[MessageToggleHtml], "html/text"),
        hint(&[MessageOpenInBrowser], "browser"),
        hint_while_enabled(&[ViewWebReading], "web/text"),
        hint(&[MessageToggleUnread], "unread"),
        hint(&[MessageToggleStar], "star"),
        hint_while_enabled(&[MessageHook], "hooks"),
        hint(&[ViewZoom], "zoom"),
        hint(&[HelpToggle], "help"),
        hint(&[AppQuit], "quit"),
    ];
    const HELP: &[Hint] = &[
        hint(&[NavNext, NavPrev], "scroll"),
        hint(&[NavBack], "close"),
    ];
    const ATTACHED: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "view"),
        hint(&[AttachmentSave], "save"),
        hint(&[AttachmentOpen], "open in its app"),
        hint(&[NavBack], "close"),
    ];
    const SEARCHING: &[Hint] = &[
        hint(&[NavOpen], "to the results"),
        hint(&[NavBack], "stop searching"),
    ];
    const PALETTE: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "run"),
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
    const SIGNING_IN: &[Hint] = &[hint(&[NavBack], "give up")];
    const WHEN: &[Hint] = &[hint(&[NavOpen], "set"), hint(&[NavBack], "cancel")];
    const FOLDERS: &[Hint] = &[hint(&[NavOpen], "move here"), hint(&[NavBack], "cancel")];
    const UNSUBSCRIBING: &[Hint] = &[hint(&[NavOpen], "go ahead"), hint(&[NavBack], "cancel")];
    const REMOVING: &[Hint] = &[hint(&[NavBack], "keep the account")];
    const TEMPLATES: &[Hint] = &[hint(&[NavOpen], "insert"), hint(&[NavBack], "cancel")];
    const SUGGESTIONS: &[Hint] = &[
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "use this spelling"),
        hint(&[NavBack], "cancel"),
    ];
    const HOOKS: &[Hint] = &[hint(&[NavOpen], "run it"), hint(&[NavBack], "cancel")];
    const COMPOSING: &[Hint] = &[
        hint(&[ComposeSend], "send"),
        hint(&[ComposeSave], "save draft"),
        hint(&[ComposeAttach], "attach"),
        hint_while_enabled(&[ComposeTemplate], "template"),
        hint_while_enabled(&[ComposeSpellSuggest], "spelling"),
        hint(&[FocusNext], "next field"),
        hint(&[NavBack], "close"),
        // Last: the first to give way in a narrow window.
        hint_while_enabled(&[ComposeVim], "another editor"),
    ];
    match context {
        Context::Compose => COMPOSING,
        Context::Folders => FOLDERS,
        Context::Unsubscribe => UNSUBSCRIBING,
        Context::RemoveAccount => REMOVING,
        Context::Templates => TEMPLATES,
        Context::Suggestions => SUGGESTIONS,
        Context::Hooks => HOOKS,
        Context::Setup => SETUP,
        Context::Certificate => CERTIFICATE,
        Context::SignIn => SIGNING_IN,
        Context::When => WHEN,
        Context::Links => LINKS,
        Context::Search => SEARCHING,
        Context::Palette => PALETTE,
        Context::Attachments => ATTACHED,
        Context::Sidebar => SIDEBAR,
        Context::List => LIST,
        Context::Reader => READER,
        Context::Help => HELP,
    }
}

/// The hints the status bar shows while the message being written is in
/// Vim: what can still be asked for from here.
pub fn hints_in_vim() -> &'static [Hint] {
    use CommandId::*;
    const IN_VIM: &[Hint] = &[
        Hint {
            commands: &[ComposeSend],
            label: "send",
            only_enabled: true,
        },
        Hint {
            commands: &[ComposeSave],
            label: "save draft",
            only_enabled: true,
        },
        // Where the editor has the text alone and the fields are still
        // here: between the two, and to Quill's own editor.
        Hint {
            commands: &[ComposeFields],
            label: "fields",
            only_enabled: true,
        },
        Hint {
            commands: &[ComposeBody],
            label: "text",
            only_enabled: true,
        },
        Hint {
            commands: &[NavBack],
            // Shown as "close Vim", "close nano" and so on: the bottom
            // bar names the editor that has the message.
            label: "close the editor",
            only_enabled: true,
        },
        Hint {
            commands: &[ComposeVim],
            label: "Quill's editor",
            only_enabled: true,
        },
    ];
    IN_VIM
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
        for hint in hints_in_vim() {
            for id in hint.commands {
                assert!(registry.get(*id).unwrap().applies_in(Context::Compose));
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
