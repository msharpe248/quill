//! The command registry. Every action is a named command; keys, the help
//! overlay, the status bar hints and (from M1) the app shell's menus all read
//! from here.

/// Where the user is. Decides which commands apply and what a key means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Sidebar,
    List,
    Reader,
    /// Selecting the words of the message being read with the keys: a
    /// cursor on the text, moved as a cursor is moved anywhere.
    Select,
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
    /// Naming a folder to make, and saying where: on the account's
    /// server, or on this Mac. Also naming one again, and being asked
    /// before a server's folder is deleted.
    NewFolder,
    /// Writing a rule, started from a message.
    NewRule,
    /// Pressing the key a command is to have, in the settings window of
    /// a host that has one.
    RecordKeys,
    /// Choosing a theme's colours, or being asked before one is deleted,
    /// in the settings window of a host that has one.
    EditTheme,
    /// Being asked before unsubscribing from a mailing list.
    Unsubscribe,
    /// Being asked before an account is removed.
    RemoveAccount,
    /// Typing the letter that stands for an account in lists.
    AccountLetter,
    /// The question asked before agents are let in over MCP.
    LetAgentsIn,
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
            Self::Select => "Selecting text with the keys",
            Self::Help => "Help",
            Self::Links => "Links",
            Self::Attachments => "Attachments",
            Self::Setup => "Add an account",
            Self::Certificate => "Certificate",
            Self::SignIn => "Signing in",
            Self::When => "Choosing a time",
            Self::NewFolder => "A folder made, renamed or deleted",
            Self::NewRule => "New rule",
            Self::RecordKeys => "Changing a key",
            Self::EditTheme => "Changing a theme",
            Self::Unsubscribe => "Unsubscribe",
            Self::RemoveAccount => "Remove an account",
            Self::AccountLetter => "An account\u{2019}s letter",
            Self::LetAgentsIn => "Letting agents in over MCP",
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
    Context::Select,
    Context::Help,
    Context::Links,
    Context::Attachments,
    Context::Setup,
    Context::Certificate,
    Context::SignIn,
    Context::When,
    Context::NewFolder,
    Context::NewRule,
    Context::RecordKeys,
    Context::EditTheme,
    Context::Unsubscribe,
    Context::RemoveAccount,
    Context::AccountLetter,
    Context::LetAgentsIn,
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
    Context::Select,
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
    WindowClose,
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
    ViewDarkPages,
    MessageToggleUnread,
    MessageToggleStar,
    MessageToggleHtml,
    MessageOpenLink,
    MessageArchive,
    MessageDelete,
    MessageMove,
    MessageCopy,
    MessageCopyText,
    MessageSave,
    MessagePutBack,
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
    TextSelect,
    EditUndo,
    AccountAdd,
    AccountSignIn,
    AccountRemove,
    AccountEdit,
    AccountLetter,
    FolderCreate,
    FolderRename,
    FolderRemove,
    MailboxSync,
    AgentsStop,
    CommandLineInstall,
    RulesToggle,
    RulesRun,
    RulesRunAll,
    RulesApply,
    RulesCreate,
    RulesNew,
    RulesEdit,
    RulesLog,
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
    /// Rules: a menu of their own, between Message and Help.
    Rules,
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
                        // The cursor that selects: what it has selected
                        // is copied.
                        Context::Select,
                        Context::Links,
                        Context::Attachments,
                        Context::Setup,
                        Context::When,
                        Context::NewFolder,
                        Context::NewRule,
                        Context::AccountLetter,
                        Context::Unsubscribe,
                        Context::Folders,
                        Context::Templates,
                        Context::Suggestions,
                        Context::Hooks,
                        Context::Search,
                        Context::Palette,
                    ],
                ),
                // Before nav.back, so that a key bound to both closes Quill
                // from a pane and only closes the help overlay.
                // In a terminal closing Quill is quitting it. In the app
                // it is closing the window, with Quill left running, as
                // the window's red button does.
                // No menu item for either: Close Window and Quit belong
                // to the shell, which has its own.
                spec(WindowClose, "window.close", "Close", General, None, PANES),
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
                // Likewise: it is the web pane's mail that is dark or not.
                spec(
                    ViewDarkPages,
                    "view.dark_pages",
                    "Dark backgrounds for messages",
                    Panes,
                    Some(Menu::View),
                    PANES,
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
                spec(
                    MessageCopy,
                    "message.copy",
                    "Copy to a folder",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                spec(
                    MessageCopyText,
                    "message.copy_text",
                    "Copy the message's text",
                    Message,
                    Some(Menu::Message),
                    &[Context::List, Context::Reader, Context::Select],
                ),
                spec(
                    MessageSave,
                    "message.save",
                    "Save as a file…",
                    Message,
                    Some(Menu::Message),
                    MESSAGE,
                ),
                with_symbol(
                    spec(
                        MessagePutBack,
                        "message.put_back",
                        "Put back",
                        Message,
                        Some(Menu::Message),
                        PANES,
                    ),
                    "arrow.uturn.backward",
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
                spec(
                    FolderCreate,
                    "folder.create",
                    "New folder\u{2026}",
                    General,
                    Some(Menu::Mailbox),
                    PANES,
                ),
                spec(
                    FolderRename,
                    "folder.rename",
                    "Rename this folder\u{2026}",
                    General,
                    Some(Menu::Mailbox),
                    &[Context::Sidebar],
                ),
                spec(
                    FolderRemove,
                    "folder.remove",
                    "Delete this folder\u{2026}",
                    General,
                    Some(Menu::Mailbox),
                    &[Context::Sidebar],
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
                        // The message being written, and in the Outbox
                        // the one the list is on.
                        PANES_AND_COMPOSE,
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
                // and act on the terminal's own selection otherwise. Copy
                // and Select All are the reader's too: the words of the
                // message on show, selected with the mouse.
                spec(
                    EditCopy,
                    "edit.copy",
                    "Copy what is selected",
                    General,
                    None,
                    PANES_AND_COMPOSE,
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
                    "Select all of the message",
                    General,
                    None,
                    &[Context::Compose, Context::Reader, Context::Select],
                ),
                // A cursor on the message's text, to select with the
                // keys. The keys that move it are its own, as in a field.
                spec(
                    TextSelect,
                    "text.select",
                    "Select text with the keys",
                    Message,
                    Some(Menu::Message),
                    &[Context::Reader, Context::Select],
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
                    AccountEdit,
                    "account.edit",
                    "Edit this account\u{2026}",
                    General,
                    Some(Menu::File),
                    PANES,
                ),
                spec(
                    AccountLetter,
                    "account.letter",
                    "Choose this account\u{2019}s letter\u{2026}",
                    General,
                    Some(Menu::File),
                    PANES,
                ),
                spec(
                    AccountRemove,
                    "account.remove",
                    "Remove this account\u{2026}",
                    General,
                    Some(Menu::File),
                    PANES,
                ),
                // In the app's own menu: it is about the program, not the
                // mail. No dialog follows it, so no ellipsis.
                spec(
                    CommandLineInstall,
                    "app.install_command_line",
                    "Install Command Line Tool",
                    General,
                    Some(Menu::App),
                    PANES,
                ),
                // Stops everything that works the mail from outside the
                // window, at once: for when an agent is doing what it
                // should not.
                spec(
                    AgentsStop,
                    "agents.stop",
                    "Stop Agents",
                    General,
                    Some(Menu::File),
                    PANES,
                ),
                // Rules, in a menu of their own. The switch first: its
                // item is checked while rules run by themselves.
                spec(
                    RulesToggle,
                    "rules.toggle",
                    "Rules On",
                    General,
                    Some(Menu::Rules),
                    PANES,
                ),
                spec(
                    RulesRun,
                    "rules.run",
                    "Run a Rule\u{2026}",
                    General,
                    Some(Menu::Rules),
                    PANES,
                ),
                spec(
                    RulesRunAll,
                    "rules.run_all",
                    "Run All Rules\u{2026}",
                    General,
                    Some(Menu::Rules),
                    PANES,
                ),
                // What the two above lead to: they show what would be
                // matched, and this does it.
                spec(
                    RulesApply,
                    "rules.apply",
                    "Run the Rule Shown",
                    General,
                    Some(Menu::Rules),
                    PANES,
                ),
                // A rule from nothing, and one begun from the message the
                // user is on: the same form.
                spec(
                    RulesCreate,
                    "rules.create",
                    "New Rule\u{2026}",
                    General,
                    Some(Menu::Rules),
                    PANES,
                ),
                spec(
                    RulesNew,
                    "rules.new",
                    "New Rule from This Message\u{2026}",
                    General,
                    Some(Menu::Rules),
                    MESSAGE,
                ),
                spec(
                    RulesEdit,
                    "rules.edit",
                    "Edit Rules",
                    General,
                    Some(Menu::Rules),
                    PANES,
                ),
                spec(
                    RulesLog,
                    "rules.log",
                    "What Rules Did",
                    General,
                    Some(Menu::Rules),
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
        // Only on a folder the user made, on a server or on this Mac.
        hint_while_enabled(&[FolderRename], "rename"),
        hint_while_enabled(&[FolderRemove], "delete folder"),
        hint(&[FocusNext], "list"),
        hint(&[ViewZoom], "zoom"),
        hint(&[HelpToggle], "help"),
        hint(&[WindowClose], "quit"),
    ];
    const LIST: &[Hint] = &[
        // First while a rule is being tried: it is what the list is
        // showing, and what it is waiting for.
        hint_while_enabled(&[RulesApply], "run the rule"),
        hint(&[NavNext, NavPrev], "move"),
        hint(&[NavOpen], "open"),
        hint(&[SearchStart], "search"),
        hint(&[MessageCompose], "write"),
        hint_while_enabled(&[ComposeSend], "send now"),
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
        hint(&[WindowClose], "quit"),
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
        hint(&[WindowClose], "quit"),
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
    const NEW_FOLDER: &[Hint] = &[hint(&[NavOpen], "make it"), hint(&[NavBack], "cancel")];
    const SELECTING: &[Hint] = &[
        hint(&[TextSelect], "select from here"),
        hint(&[NavOpen], "copy"),
        hint(&[NavBack], "done"),
    ];
    const LETTER: &[Hint] = &[hint(&[NavOpen], "use it"), hint(&[NavBack], "cancel")];
    const NEW_RULE: &[Hint] = &[
        hint(&[NavOpen], "save and try it"),
        hint(&[NavBack], "cancel"),
    ];
    const RECORDING: &[Hint] = &[hint(&[NavBack], "cancel")];
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
        Context::NewFolder => NEW_FOLDER,
        Context::AccountLetter => LETTER,
        Context::NewRule => NEW_RULE,
        // Asked in a sheet, like them: there is only leaving it.
        Context::RecordKeys | Context::EditTheme | Context::LetAgentsIn => RECORDING,
        Context::Links => LINKS,
        Context::Search => SEARCHING,
        Context::Palette => PALETTE,
        Context::Attachments => ATTACHED,
        Context::Sidebar => SIDEBAR,
        Context::List => LIST,
        Context::Reader => READER,
        Context::Select => SELECTING,
        Context::Help => HELP,
    }
}

/// The hints the status bar shows while a cursor selects with the keys:
/// what the key that began it does now says whether the selection has
/// begun.
pub fn hints_selecting(begun: bool) -> Vec<Hint> {
    use CommandId::*;
    let hint = |commands: &'static [CommandId], label: &'static str| Hint {
        commands,
        label,
        only_enabled: false,
    };
    vec![
        hint(
            &[TextSelect],
            if begun {
                "let go of it"
            } else {
                "select from here"
            },
        ),
        hint(&[NavOpen], "copy"),
        hint(&[NavBack], "done"),
    ]
}

/// The hints the status bar shows under the question about a folder:
/// what `answer` says the key that answers does (`make it`, `rename`,
/// `delete`).
pub fn hints_folder(answer: &'static str) -> Vec<Hint> {
    use CommandId::*;
    vec![
        Hint {
            commands: &[NavOpen],
            label: answer,
            only_enabled: false,
        },
        Hint {
            commands: &[NavBack],
            label: "cancel",
            only_enabled: false,
        },
    ]
}

/// The hints the status bar shows over the kinds of file a message can be
/// saved as: the list is the one hooks and rules are chosen from, and
/// choosing here runs nothing.
pub fn hints_saving_as() -> &'static [Hint] {
    use CommandId::*;
    const SAVING_AS: &[Hint] = &[
        Hint {
            commands: &[NavOpen],
            label: "save",
            only_enabled: false,
        },
        Hint {
            commands: &[NavBack],
            label: "cancel",
            only_enabled: false,
        },
    ];
    SAVING_AS
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
    fn close_comes_before_back_so_a_shared_key_closes_from_a_pane() {
        let registry = Registry::builtin();
        let position = |id| {
            registry
                .specs()
                .iter()
                .position(|spec| spec.id == id)
                .unwrap()
        };
        for id in [CommandId::WindowClose, CommandId::AppQuit] {
            assert!(position(id) < position(CommandId::NavBack));
            assert!(!registry.get(id).unwrap().applies_in(Context::Help));
        }
    }
}
