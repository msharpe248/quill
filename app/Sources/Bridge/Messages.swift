import Foundation

// The messages of the shell-core bridge, as Swift types.
//
// `schema/bridge.json` is the definition. These types, the Rust types in
// `crates/quill-host/src/bridge.rs` and the examples in `schema/examples` are
// all tested against it; change them together.

/// A message the core sends to the shell.
public enum CoreToShell: Equatable, Sendable {
    /// Every command, once at startup, in menu order.
    case commandsList([CommandInfo])
    /// The whole of what the shell mirrors, whenever any of it changes.
    case stateUpdate(StateUpdate)
    /// Show files in a Quick Look pane beside the terminal, one at a time.
    case panePreview(PanePreview)
    /// Play a video or audio file in a player pane.
    case panePlay(PaneFile)
    /// Make a PNG thumbnail of a file. Answered by `thumbnailMade`.
    case thumbnailMake(ThumbnailMake)
    /// Open a file in the application the Mac opens its kind with.
    case fileOpen(path: String)
    /// Ask where to save a copy of a file, and save it. Answered by
    /// `filesSaved`.
    case filesSave(FilesSave)
    /// Close the second pane and return focus to the terminal.
    case paneClose
    /// Open a web or mail address in the user's browser or mail program.
    case linkOpen(LinkOpen)
    /// Ask the user to choose files with the native open panel. What they
    /// choose goes back as `filesDropped`.
    case filesPick
    /// Put text on the system clipboard.
    case clipboardWrite(text: String)
    /// Show a notification, in place of any earlier one with the same id.
    /// What the user does with it goes back as `notifyUsed`.
    case notifyPost(NotifyPost)
    /// Take notifications down, shown or waiting.
    case notifyClear(NotifyClear)
    /// Check the spelling of some text. Answered by `spellChecked`.
    case spellCheck(SpellCheck)
    /// Ask what a misspelt word might have been meant to be. Answered by
    /// `spellSuggested`.
    case spellSuggest(SpellSuggest)
    /// Run Vim on a draft file, in a terminal pane of its own. Answered by `editorExited` when Vim ends, or at once if
    /// it could not be started.
    case paneEditor(PaneEditor)
    /// Show a form in a sheet on the window, in place of whatever form the
    /// sheet shows now. What the user does with it goes back as `formUsed`.
    case formShow(FormShow)
    /// Take the sheet down: the form is over.
    case formClose
    /// Show sanitized HTML in the reading pane: the web view that stands in
    /// the reader's place while web reading is on. It stays until
    /// `readingClose`, and never takes the keyboard by itself.
    case readingShow(ReadingShow)
    /// Scroll what the reading pane shows.
    case readingScroll(ScrollBy)
    /// Take the reading pane away: web reading is off.
    case readingClose
    /// Where the reading pane stands: how many columns the terminal keeps
    /// beside it.
    case readingPlace(ReadingPlace)
    /// What the settings window shows: every pane with its settings as they
    /// stand. What the user does there goes back as `settingsUsed`.
    case settingsShow(SettingsShow)
}

/// A message the shell sends to the core.
public enum ShellToCore: Equatable, Sendable {
    /// A menu item, toolbar button or notification action was used.
    case commandRun(name: String)
    /// The user chose files in the open panel or dropped them on the window.
    case filesDropped(FilesDropped)
    /// The answer to `thumbnailMake`.
    case thumbnailMade(id: Int, ok: Bool)
    /// The answer to `filesSave`.
    case filesSaved(name: String, saved: Bool)
    /// The user clicked a notification, or one of its actions.
    case notifyUsed(NotifyUsed)
    /// The Mac asked Quill to open a mail address.
    case urlOpen(UrlOpen)
    /// How many columns the terminal would have with the window to itself,
    /// whenever that changes.
    case windowResized(WindowResized)
    /// The answer to `spellCheck`.
    case spellChecked(SpellChecked)
    /// The answer to `spellSuggest`.
    case spellSuggested(SpellSuggested)
    /// The answer to `paneEditor`.
    case editorExited(EditorExited)
    /// A button of the form in the sheet was pressed, or one of its choices
    /// changed.
    case formUsed(FormUsed)
    /// The settings window was opened, or come back to: it wants
    /// `settingsShow`.
    case settingsOpened
    /// The settings window was closed.
    case settingsClosed
    /// A setting was changed in the settings window, or something was asked
    /// for from one of its lists.
    case settingsUsed(SettingsUsed)
    /// The app is quitting: flush and exit cleanly.
    case appWillQuit
}

public struct CommandInfo: Equatable, Sendable, Codable {
    /// The stable name, as used in `keys.toml` and `command.run`.
    public var name: String
    public var title: String
    /// Key sequences in Quill's notation, the preferred one first.
    public var keys: [String]
    /// The menu the command appears in, if any.
    public var menu: Menu?
    /// An SF Symbol name. A command with one may be shown in the toolbar.
    public var symbol: String?

    public init(name: String, title: String, keys: [String], menu: Menu? = nil, symbol: String? = nil) {
        self.name = name
        self.title = title
        self.keys = keys
        self.menu = menu
        self.symbol = symbol
    }

    static let allowedKeys: Set<String> = ["name", "title", "keys", "menu", "symbol"]
}

public enum Menu: String, Equatable, Sendable, Codable, CaseIterable {
    case app, file, edit, view, mailbox, message, window, help
}

public struct StateUpdate: Equatable, Sendable, Codable {
    /// Where the keyboard focus is inside the terminal UI.
    public var context: Context
    /// Names of the commands that would do something right now.
    public var enabled: [String]
    /// Names of the commands that switch something on and off, and have it
    /// on right now: their menu items are shown checked.
    public var checked: [String]
    /// Unread messages across all inboxes, for the Dock badge.
    public var unread: Int
    /// The window title.
    public var title: String

    public init(context: Context, enabled: [String], checked: [String] = [], unread: Int, title: String) {
        self.context = context
        self.enabled = enabled
        self.checked = checked
        self.unread = unread
        self.title = title
    }

    static let allowedKeys: Set<String> = ["context", "enabled", "checked", "unread", "title"]
}

public enum Context: String, Equatable, Sendable, Codable {
    case sidebar, list, reader, help, links, setup, certificate, folders, compose, palette, search
}

public struct LinkOpen: Equatable, Sendable, Codable {
    /// An http, https or mailto address. See `Links.isOpenable`.
    public var url: String

    public init(url: String) {
        self.url = url
    }

    static let allowedKeys: Set<String> = ["url"]
}

/// Which addresses the shell will open, whoever asks: the core over the
/// bridge, or the terminal reporting a click.
public enum Links {
    /// Only web pages and mail addresses, in printable ASCII. A message must
    /// not be able to open a file or start another program by naming some
    /// other scheme. This is the same rule as `quill_core::links::is_openable`.
    public static func isOpenable(_ url: String) -> Bool {
        let lower = url.lowercased()
        guard let scheme = ["https://", "http://", "mailto:"].first(where: lower.hasPrefix) else {
            return false
        }
        return url.utf8.count > scheme.utf8.count && url.utf8.allSatisfy { (0x21...0x7E).contains($0) }
    }

    /// The address as a URL, when it is one the shell opens.
    public static func openable(_ url: String) -> URL? {
        isOpenable(url) ? URL(string: url) : nil
    }
}

/// Files handed to the core: at least one, each by an absolute path the
/// core can read.
public struct FilesDropped: Equatable, Sendable, Codable {
    public var paths: [String]
    /// The files are copies the shell made for the core, which removes each
    /// once it has read it.
    public var temporary: Bool

    /// `nil` unless there is at least one path and every one is absolute.
    public init?(paths: [String], temporary: Bool) {
        guard !paths.isEmpty, paths.allSatisfy({ $0.hasPrefix("/") }) else { return nil }
        self.paths = paths
        self.temporary = temporary
    }

    static let allowedKeys: Set<String> = ["paths", "temporary"]
}

/// A file to show in a pane, by absolute path, and what to call it.
public struct PaneFile: Equatable, Sendable, Codable {
    public var path: String
    public var title: String

    /// `nil` unless the path is absolute.
    public init?(path: String, title: String) {
        guard path.hasPrefix("/") else { return nil }
        self.path = path
        self.title = title
    }

    static let allowedKeys: Set<String> = ["path", "title"]
}

/// Files for the Quick Look pane: at least one, and which to show first.
public struct PanePreview: Equatable, Sendable, Codable {
    public var items: [PaneFile]
    public var index: Int

    /// `nil` unless `index` is one of the items.
    public init?(items: [PaneFile], index: Int) {
        guard items.indices.contains(index) else { return nil }
        self.items = items
        self.index = index
    }

    static let allowedKeys: Set<String> = ["items", "index"]
}

/// A page for the reading pane, and what to say above it. The shell shows
/// the page under the web view's rules and the lines as plain text. It
/// knows nothing else about them.
public struct ReadingShow: Equatable, Sendable, Codable {
    /// The sanitized HTML. Files beside it in its folder may be loaded.
    public var path: String
    public var title: String
    /// The user has allowed this message to load remote content.
    public var remote: Bool
    /// Lines of plain text shown above the page, outside it, where the
    /// page cannot reach.
    public var header: [String]

    /// The most lines the header may have.
    public static let mostHeaderLines = 8
    /// The longest any one of them may be, in bytes.
    public static let longestHeaderLine = 2000

    static func isHeaderLine(_ line: String) -> Bool {
        (1...longestHeaderLine).contains(line.utf8.count)
            && !line.unicodeScalars.contains { $0.properties.generalCategory == .control }
    }

    /// `nil` unless the path is absolute and the header is at most eight
    /// lines, each one line of 1 to 2000 bytes.
    public init?(path: String, title: String, remote: Bool, header: [String] = []) {
        guard path.hasPrefix("/"), header.count <= Self.mostHeaderLines, header.allSatisfy(Self.isHeaderLine)
        else { return nil }
        self.path = path
        self.title = title
        self.remote = remote
        self.header = header
    }

    static let allowedKeys: Set<String> = ["path", "title", "remote", "header"]
}

/// The reading pane's place in the window, said in the terminal's own
/// measure: the terminal keeps `columns` columns and the reading pane has
/// the rest. The shell turns columns into points; why it is this many is
/// the core's business.
public struct ReadingPlace: Equatable, Sendable, Codable {
    public var columns: Int

    /// How many columns a terminal can have.
    public static let range = 1...65535

    /// `nil` unless it is a number of columns a terminal can have.
    public init?(columns: Int) {
        guard Self.range.contains(columns) else { return nil }
        self.columns = columns
    }

    static let allowedKeys: Set<String> = ["columns"]
}

/// How many columns the terminal would have if it had the whole window:
/// what the window has room for, whatever stands beside the terminal now.
public struct WindowResized: Equatable, Sendable, Codable {
    public var columns: Int

    /// `nil` unless it is a number of columns a terminal can have.
    public init?(columns: Int) {
        guard ReadingPlace.range.contains(columns) else { return nil }
        self.columns = columns
    }

    static let allowedKeys: Set<String> = ["columns"]
}

/// How far to scroll the reading pane, and which way.
public enum ScrollBy: String, Equatable, Sendable, Codable, CaseIterable {
    case pageDown = "page_down"
    case pageUp = "page_up"
}

public struct ThumbnailMake: Equatable, Sendable, Codable {
    public var id: Int
    /// The file to picture.
    public var path: String
    /// Where to write the PNG.
    public var out: String
    /// The longest side wanted, in pixels.
    public var size: Int

    public static let sizes = 16...4096

    /// `nil` unless both paths are absolute, the id is not negative and
    /// the size is one that may be asked for.
    public init?(id: Int, path: String, out: String, size: Int) {
        guard id >= 0, path.hasPrefix("/"), out.hasPrefix("/"), Self.sizes.contains(size) else { return nil }
        self.id = id
        self.path = path
        self.out = out
        self.size = size
    }

    static let allowedKeys: Set<String> = ["id", "path", "out", "size"]
}

public struct FilesSave: Equatable, Sendable, Codable {
    /// The file to copy.
    public var path: String
    /// The name to offer in the panel.
    public var name: String

    /// `nil` unless the path is absolute and there is a name.
    public init?(path: String, name: String) {
        guard path.hasPrefix("/"), !name.isEmpty else { return nil }
        self.path = path
        self.name = name
    }

    static let allowedKeys: Set<String> = ["path", "name"]
}

/// A notification to show. The shell shows the words as plain text and hands
/// the id, and an action's name, back in `NotifyUsed`. It knows nothing else
/// about them.
public struct NotifyPost: Equatable, Sendable, Codable {
    /// The core's name for the notification.
    public var id: String
    public var title: String
    public var subtitle: String?
    public var body: String
    /// Buttons on the notification, in order. None is allowed.
    public var actions: [NotifyAction]

    /// The longest id, in bytes.
    public static let longestID = 128
    /// The most buttons a notification may carry.
    public static let mostActions = 4

    static func isID(_ id: String) -> Bool {
        (1...longestID).contains(id.utf8.count)
    }

    /// `nil` unless the id is 1 to 128 bytes, there is a title and there
    /// are at most four actions.
    public init?(id: String, title: String, subtitle: String? = nil, body: String, actions: [NotifyAction] = []) {
        guard Self.isID(id), !title.isEmpty, actions.count <= Self.mostActions else { return nil }
        self.id = id
        self.title = title
        self.subtitle = subtitle
        self.body = body
        self.actions = actions
    }

    static let allowedKeys: Set<String> = ["id", "title", "subtitle", "body", "actions"]
}

/// A button on a notification.
public struct NotifyAction: Equatable, Sendable, Codable {
    /// What comes back in `NotifyUsed`.
    public var name: String
    /// What the button says.
    public var title: String
    /// Using it brings Quill's window forward: what it starts needs the
    /// user there, as a reply does.
    public var foreground: Bool

    /// The longest name, in bytes.
    public static let longestName = 64

    static func isName(_ name: String) -> Bool {
        (1...longestName).contains(name.utf8.count)
    }

    /// `nil` unless the name is 1 to 64 bytes and there is a title.
    public init?(name: String, title: String, foreground: Bool = false) {
        guard Self.isName(name), !title.isEmpty else { return nil }
        self.name = name
        self.title = title
        self.foreground = foreground
    }

    static let allowedKeys: Set<String> = ["name", "title", "foreground"]

    private enum CodingKeys: String, CodingKey {
        case name, title, foreground
    }

    public init(from decoder: any Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        title = try values.decode(String.self, forKey: .title)
        foreground = try values.decodeIfPresent(Bool.self, forKey: .foreground) ?? false
    }

    /// `foreground` is written only when it is set, as the core writes it.
    public func encode(to encoder: any Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(name, forKey: .name)
        try values.encode(title, forKey: .title)
        if foreground {
            try values.encode(true, forKey: .foreground)
        }
    }
}

/// Notifications to take down: at least one, each by its id.
public struct NotifyClear: Equatable, Sendable, Codable {
    public var ids: [String]

    /// `nil` unless there is at least one id and each is 1 to 128 bytes.
    public init?(ids: [String]) {
        guard !ids.isEmpty, ids.allSatisfy(NotifyPost.isID) else { return nil }
        self.ids = ids
    }

    static let allowedKeys: Set<String> = ["ids"]
}

/// What the user did with a notification.
public struct NotifyUsed: Equatable, Sendable, Codable {
    public var id: String
    /// The name of the action chosen; `nil` when the notification itself
    /// was clicked.
    public var action: String?

    /// `nil` unless the id is 1 to 128 bytes and the action's name, if
    /// there is one, 1 to 64.
    public init?(id: String, action: String? = nil) {
        guard NotifyPost.isID(id), action.map(NotifyAction.isName) ?? true else { return nil }
        self.id = id
        self.action = action
    }

    static let allowedKeys: Set<String> = ["id", "action"]
}

/// A mail address the Mac asked Quill to open: a `mailto:` link clicked in
/// another application. It cannot hold any other kind of address.
public struct UrlOpen: Equatable, Sendable, Codable {
    public var url: String

    /// The longest address passed on, in bytes.
    public static let longestURL = 8192

    private static let scheme = Array("mailto:".utf8)

    /// `nil` unless the address starts `mailto:`, in lower case, and is at
    /// most 8192 bytes.
    public init?(url: String) {
        guard url.utf8.starts(with: Self.scheme), url.utf8.count <= Self.longestURL else { return nil }
        self.url = url
    }

    /// An address as the Mac hands it over, where the scheme may be in
    /// either case: `MAILTO:robin@example.com` is passed on as
    /// `mailto:robin@example.com`. `nil` for every other scheme.
    public init?(handedOver address: String) {
        let bytes = address.utf8
        let lowered = bytes.prefix(Self.scheme.count).map { (0x41...0x5A).contains($0) ? $0 + 0x20 : $0 }
        guard lowered == Self.scheme else { return nil }
        // The scheme is ASCII, so what follows it starts on a character.
        self.init(url: "mailto:" + String(decoding: bytes.dropFirst(Self.scheme.count), as: UTF8.self))
    }

    static let allowedKeys: Set<String> = ["url"]
}

/// Text to have its spelling checked by the system's checker, in the user's
/// languages. Nothing is learned or remembered from it.
public struct SpellCheck: Equatable, Sendable, Codable {
    /// Comes back in `SpellChecked`.
    public let id: Int
    public let text: String

    /// The most text one check takes, in bytes.
    public static let longestText = 100_000

    /// `nil` unless the id is not negative and the text is at most 100000
    /// bytes.
    public init?(id: Int, text: String) {
        guard id >= 0, text.utf8.count <= Self.longestText else { return nil }
        self.id = id
        self.text = text
    }

    static let allowedKeys: Set<String> = ["id", "text"]
}

/// Where a misspelt word is: the byte offsets of its start and end in the
/// text that was checked, as UTF-8. Not UTF-16 units, and not characters.
public struct Misspelt: Equatable, Sendable, Codable {
    public let start: Int
    public let end: Int

    /// `nil` unless the word starts at or after the start of the text and
    /// ends after it starts.
    public init?(start: Int, end: Int) {
        guard start >= 0, start < end else { return nil }
        self.start = start
        self.end = end
    }

    static let allowedKeys: Set<String> = ["start", "end"]
}

/// The misspelt words of a text, in order and not overlapping. It cannot be
/// built holding anything else, so the core can use the offsets as given.
public struct SpellChecked: Equatable, Sendable, Codable {
    /// The id of the `SpellCheck` this answers.
    public let id: Int
    public let words: [Misspelt]

    /// The most misspelt words one answer names.
    public static let mostWords = 2000

    /// `nil` unless the id is not negative, there are at most 2000 words,
    /// and each starts at or after the end of the one before.
    public init?(id: Int, words: [Misspelt]) {
        guard id >= 0, words.count <= Self.mostWords else { return nil }
        var last = 0
        for word in words {
            guard word.start >= last else { return nil }
            last = word.end
        }
        self.id = id
        self.words = words
    }

    static let allowedKeys: Set<String> = ["id", "words"]
}

/// A misspelt word to have suggestions for.
public struct SpellSuggest: Equatable, Sendable, Codable {
    /// Comes back in `SpellSuggested`.
    public let id: Int
    public let word: String

    /// The longest word, asked about or suggested, in bytes.
    public static let longestWord = 200

    static func isWord(_ word: String) -> Bool {
        (1...longestWord).contains(word.utf8.count)
    }

    /// `nil` unless the id is not negative and the word is 1 to 200 bytes.
    public init?(id: Int, word: String) {
        guard id >= 0, Self.isWord(word) else { return nil }
        self.id = id
        self.word = word
    }

    static let allowedKeys: Set<String> = ["id", "word"]
}

/// Words to offer in place of a misspelt one, the likeliest first. None is
/// an answer too.
public struct SpellSuggested: Equatable, Sendable, Codable {
    /// The id of the `SpellSuggest` this answers.
    public let id: Int
    public let words: [String]

    /// The most suggestions one answer makes.
    public static let mostWords = 20

    /// `nil` unless the id is not negative and there are at most 20 words,
    /// each 1 to 200 bytes.
    public init?(id: Int, words: [String]) {
        guard id >= 0, words.count <= Self.mostWords, words.allSatisfy(SpellSuggest.isWord) else { return nil }
        self.id = id
        self.words = words
    }

    static let allowedKeys: Set<String> = ["id", "words"]
}

/// Which editor the shell is to run. The core says which of these; it never
/// names a program by its path.
public enum EditorKind: String, Equatable, Sendable, Codable, CaseIterable {
    /// The Vim that comes with macOS.
    case vim
    /// The nano that comes with macOS, which is Pico.
    case nano
    /// The user's own Vim, by way of the script they keep in the app's
    /// scripts folder, which they have asked for in their config.
    case script
}

/// A draft file for an editor to edit, and the draft's own folder, where Vim
/// and the core leave each other notes. The shell passes the folder to Vim
/// in the environment and does nothing else with it.
public struct PaneEditor: Equatable, Sendable, Codable {
    /// The draft file.
    public let path: String
    /// The draft's own folder.
    public let dir: String
    public let title: String
    public let editor: EditorKind
    /// The line of the file the message's body starts on, counting from 1:
    /// where an editor that has no plugin of Quill's is started.
    public let line: Int

    /// The lines a draft may be said to start its body on.
    public static let lines = 1...1_000_000

    /// `nil` unless both paths are absolute and the line is one there can
    /// be.
    public init?(path: String, dir: String, title: String, editor: EditorKind = .vim, line: Int = 1) {
        guard path.hasPrefix("/"), dir.hasPrefix("/"), Self.lines.contains(line) else { return nil }
        self.path = path
        self.dir = dir
        self.title = title
        self.editor = editor
        self.line = line
    }

    static let allowedKeys: Set<String> = ["path", "dir", "title", "editor", "line"]
}

/// How the editor ended.
public struct EditorExited: Equatable, Sendable, Codable {
    /// False when Vim could not be started: the one asked for is not
    /// there, or it would not start. `status` is then 0.
    public let ran: Bool
    /// What Vim exited with: 0 for a normal exit, anything else for `:cq`
    /// or a crash. A Vim ended by a signal is reported as the signal's
    /// number below zero.
    public let status: Int

    public static let statuses = -255...255

    /// The answer when Vim was not run.
    public static let didNotRun = EditorExited(unchecked: false, 0)

    /// `nil` unless the status is from -255 to 255.
    public init?(ran: Bool, status: Int) {
        guard Self.statuses.contains(status) else { return nil }
        self.ran = ran
        self.status = status
    }

    /// A Vim that ran and ended with this status, brought into the range
    /// that may be reported if it is outside it.
    public init(ranWithStatus status: Int) {
        self.init(unchecked: true, min(max(status, Self.statuses.lowerBound), Self.statuses.upperBound))
    }

    private init(unchecked ran: Bool, _ status: Int) {
        self.ran = ran
        self.status = status
    }

    static let allowedKeys: Set<String> = ["ran", "status"]
}

/// The names in a form: of its step, its fields, a choice's options and its
/// buttons.
public enum FormID {
    /// The longest a name may be, in bytes.
    public static let longest = 64

    /// Lower case letters, digits and underscores, starting with a letter.
    public static func isValid(_ id: String) -> Bool {
        let bytes = Array(id.utf8)
        guard let first = bytes.first, bytes.count <= longest, (0x61...0x7A).contains(first) else { return false }
        return bytes.allSatisfy { (0x61...0x7A).contains($0) || (0x30...0x39).contains($0) || $0 == 0x5F }
    }
}

/// One of the options of a choice: its name, and what the user reads.
public struct FormOption: Equatable, Sendable {
    public let id: String
    public let title: String

    public init(id: String, title: String) {
        self.id = id
        self.title = title
    }

    static let allowedKeys: Set<String> = ["id", "title"]
}

/// One field of a form.
public struct FormField: Equatable, Sendable {
    /// What a field is, and what it holds as the core has it.
    public enum Kind: Equatable, Sendable {
        /// A line of text to type.
        case text(value: String)
        /// A line of text that is not shown as it is typed: a password.
        /// The core says nothing of what it holds.
        case secret
        /// One of a few options, by its name.
        case choice(value: String, options: [FormOption])
        /// Text that is only shown.
        case fixed(value: String)
        /// Text shown to be read off, selected or copied, and typed
        /// somewhere else: a code to sign in with, the page to type it at.
        /// It is shown large, and nothing prints it.
        case code(value: FormSecret)
        /// Something that is on or off: a checkbox, with its own words.
        case toggle(title: String, value: Bool)
        /// A number within bounds.
        case number(FormNumber)
        /// Rows to pick one of, and things to do with it.
        case list(FormList)

        /// The kind's name on the bridge.
        public var name: String {
            switch self {
            case .text: "text"
            case .secret: "secret"
            case .choice: "choice"
            case .fixed: "static"
            case .code: "code"
            case .toggle: "toggle"
            case .number: "number"
            case .list: "list"
            }
        }
    }

    public let id: String
    public let label: String
    public let kind: Kind
    /// The keyboard starts in this field.
    public let focus: Bool
    /// Said under the field, in small print.
    public let help: String?

    public init(id: String, label: String, kind: Kind, focus: Bool = false, help: String? = nil) {
        self.id = id
        self.label = label
        self.kind = kind
        self.focus = focus
        self.help = help
    }

    /// What the field holds, as the core has it. A secret has nothing to
    /// say, and what a code field shows is the view's to reveal.
    public var value: String? {
        switch kind {
        case let .text(value), let .choice(value, _), let .fixed(value): value
        case .secret, .code, .toggle, .number, .list: nil
        }
    }

    /// Whether the user can change what it holds.
    public var isEditable: Bool {
        switch kind {
        case .text, .secret, .choice, .toggle, .number: true
        case .fixed, .code, .list: false
        }
    }

    /// Whether the field is of a kind that says what the user did with it as
    /// they do it, with no button to wait for: the settings window's kinds,
    /// which a sheet has none of.
    public var reportsAsChanged: Bool {
        switch kind {
        case .toggle, .number, .list: true
        case .text, .secret, .choice, .fixed, .code: false
        }
    }

    /// Whether what the field holds hangs together: a choice is one of its
    /// options, a toggle says what it switches, a number is within its
    /// bounds, and the rows and actions of a list have names of their own.
    var isSound: Bool {
        switch kind {
        case let .choice(value, options):
            let ids = options.map(\.id)
            return ids.allSatisfy(FormID.isValid) && Set(ids).count == ids.count && ids.contains(value)
        case let .toggle(title, _):
            return !title.isEmpty
        case let .number(number):
            return number.isSound
        case let .list(list):
            return list.isSound
        case .text, .secret, .fixed, .code:
            return true
        }
    }
}

/// A number a form shows and takes: from `min` to `max`, both included, or
/// with no most when there is no `max`; shown with at most `decimals` places
/// after the point and stepped by `step`. `unit` is said after it.
public struct FormNumber: Equatable, Sendable {
    public let value: Double
    public let min: Double
    public let max: Double?
    public let step: Double
    public let decimals: Int
    public let unit: String?

    /// The most places after the point a number is shown with.
    public static let mostDecimals = 3

    public init(value: Double, min: Double, max: Double? = nil, step: Double, decimals: Int, unit: String? = nil) {
        self.value = value
        self.min = min
        self.max = max
        self.step = step
        self.decimals = decimals
        self.unit = unit
    }

    var isSound: Bool {
        guard value.isFinite, min.isFinite, step.isFinite, max?.isFinite ?? true else { return false }
        guard min <= value, step > 0, (0...Self.mostDecimals).contains(decimals) else { return false }
        if let max {
            return value <= max && min <= max
        }
        return true
    }

    /// `typed` as a number this field takes: brought within the bounds and
    /// to the places the field shows.
    public func taking(_ typed: Double) -> Double? {
        guard typed.isFinite else { return nil }
        let scale = pow(10, Double(decimals))
        let rounded = (typed * scale).rounded() / scale
        return Swift.min(Swift.max(rounded, min), max ?? .greatestFiniteMagnitude)
    }

    /// The number as the field writes it: no more places than it has, and
    /// none that are only noughts.
    public func written(_ number: Double) -> String {
        let scale = pow(10, Double(decimals))
        let rounded = (number * scale).rounded() / scale
        if rounded == rounded.rounded(), abs(rounded) < 1e15 {
            return String(Int64(rounded))
        }
        var text = String(format: "%.\(decimals)f", rounded)
        while text.hasSuffix("0") {
            text.removeLast()
        }
        return text
    }

    /// The same number with another value.
    public func with(value: Double) -> FormNumber {
        FormNumber(value: value, min: min, max: max, step: step, decimals: decimals, unit: unit)
    }

    static let allowedKeys: Set<String> = [
        "kind", "id", "label", "value", "min", "max", "step", "decimals", "unit", "help",
    ]
}

/// The rows and actions of a list in a form. `empty` is said in the rows'
/// place when there are none.
public struct FormList: Equatable, Sendable {
    /// One row: its name, what it is called, and what is said under that.
    /// `actions` names the list's actions that can be asked of this row.
    public struct Row: Equatable, Sendable {
        public let id: String
        public let title: String
        public let detail: String?
        public let actions: [String]

        public init(id: String, title: String, detail: String? = nil, actions: [String] = []) {
            self.id = id
            self.title = title
            self.detail = detail
            self.actions = actions
        }

        static let allowedKeys: Set<String> = ["id", "title", "detail", "actions"]
    }

    /// Something the list offers to do: a button under it. One with `row`
    /// is done to the row picked, and can be pressed only while a row that
    /// takes it is picked.
    public struct Action: Equatable, Sendable {
        public let id: String
        public let title: String
        public let row: Bool
        public let enabled: Bool

        public init(id: String, title: String, row: Bool, enabled: Bool = true) {
            self.id = id
            self.title = title
            self.row = row
            self.enabled = enabled
        }

        static let allowedKeys: Set<String> = ["id", "title", "row", "enabled"]
    }

    public let rows: [Row]
    public let actions: [Action]
    public let empty: String?

    /// The most rows a list may have, and the most actions.
    public static let mostRows = 200
    public static let mostActions = 6

    public init(rows: [Row], actions: [Action], empty: String? = nil) {
        self.rows = rows
        self.actions = actions
        self.empty = empty
    }

    var isSound: Bool {
        let actionIDs = actions.map(\.id)
        guard actions.count <= Self.mostActions, actionIDs.allSatisfy(FormID.isValid),
            Set(actionIDs).count == actionIDs.count, actions.allSatisfy({ !$0.title.isEmpty })
        else { return false }
        let rowIDs = rows.map(\.id)
        guard rows.count <= Self.mostRows, rowIDs.allSatisfy(FormID.isValid), Set(rowIDs).count == rowIDs.count
        else { return false }
        return rows.allSatisfy { row in !row.title.isEmpty && row.actions.allSatisfy(actionIDs.contains) }
    }

    /// Whether the action of this name can be pressed with this row picked,
    /// or with none.
    public func canPress(_ action: String, picked row: String?) -> Bool {
        guard let described = actions.first(where: { $0.id == action }), described.enabled else { return false }
        guard described.row else { return true }
        guard let row, let picked = rows.first(where: { $0.id == row }) else { return false }
        return picked.actions.contains(action)
    }

    static let allowedKeys: Set<String> = ["kind", "id", "label", "rows", "actions", "empty", "help"]
}

/// The key that presses a button of a form.
public enum FormRole: String, Equatable, Sendable {
    /// Return.
    case confirm = "default"
    /// Esc.
    case cancel
}

/// A button of a form.
public struct FormButton: Equatable, Sendable {
    /// What goes back in `FormUsed`.
    public let id: String
    public let title: String
    /// Which key presses it, if one does.
    public let role: FormRole?
    /// Whether it can be pressed now.
    public let enabled: Bool

    public init(id: String, title: String, role: FormRole? = nil, enabled: Bool = true) {
        self.id = id
        self.title = title
        self.role = role
        self.enabled = enabled
    }

    static let allowedKeys: Set<String> = ["id", "title", "role", "enabled"]
}

/// A form to show in a sheet on the window: what it is called, what it says,
/// its fields in order and its buttons from left to right. The shell lays it
/// out and knows nothing of what it is for.
///
/// A sheet has no toggle, number or list: those say what the user did as
/// they do it, with no button to wait for, and are the settings window's.
public struct FormShow: Equatable, Sendable {
    /// Which step of the form this is. Goes back in `FormUsed`.
    public let step: String
    public let title: String
    /// Said above the fields.
    public let note: String?
    /// What is wrong, said below the fields.
    public let error: String?
    /// What the core is waiting for, in words. While it is there the
    /// fields take no typing.
    public let busy: String?
    public let fields: [FormField]
    public let buttons: [FormButton]

    /// The most fields a form may have, and the most buttons.
    public static let mostFields = 32
    public static let mostButtons = 6

    /// `nil` unless the step, the fields, the options and the buttons each
    /// have a name, no two fields or buttons share one, each choice is one
    /// of its options, at most one field has the focus, and at most one
    /// button is pressed by return and one by esc.
    public init?(
        step: String, title: String, note: String? = nil, error: String? = nil, busy: String? = nil,
        fields: [FormField], buttons: [FormButton]
    ) {
        guard FormID.isValid(step), !title.isEmpty, fields.count <= Self.mostFields,
            buttons.count <= Self.mostButtons
        else { return nil }
        guard fields.allSatisfy({ FormID.isValid($0.id) }), Set(fields.map(\.id)).count == fields.count,
            fields.filter(\.focus).count <= 1
        else { return nil }
        for field in fields {
            // Only a field that can be typed in or changed has the focus.
            if !field.isEditable, field.focus { return nil }
            // A sheet asks and waits for a button: it has none of the
            // kinds that report as they change.
            if field.reportsAsChanged || !field.isSound { return nil }
        }
        guard buttons.allSatisfy({ FormID.isValid($0.id) && !$0.title.isEmpty }),
            Set(buttons.map(\.id)).count == buttons.count,
            buttons.filter({ $0.role == .confirm }).count <= 1,
            buttons.filter({ $0.role == .cancel }).count <= 1
        else { return nil }
        self.step = step
        self.title = title
        self.note = note
        self.error = error
        self.busy = busy
        self.fields = fields
        self.buttons = buttons
    }

    static let allowedKeys: Set<String> = ["step", "title", "note", "error", "busy", "fields", "buttons"]
}

/// What was typed into a secret field of a form: a password. Or what a code
/// field of a form shows: a code to sign in with, the page to type it at. It
/// is written on the bridge, a code is drawn in its field, and that is all.
/// However it is printed, by itself or inside whatever holds it, a
/// placeholder is printed in its place.
public struct FormSecret: Equatable, Sendable, CustomStringConvertible, CustomDebugStringConvertible,
    CustomReflectable
{
    private let text: String

    public init(_ text: String) {
        self.text = text
    }

    /// The text itself, for the one place that writes it on the bridge.
    public var revealed: String { text }

    public var description: String { "<secret>" }
    public var debugDescription: String { "<secret>" }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// What the user did with the form in the sheet: the button pressed, if one
/// was, and what the fields held at that moment. What the secret fields
/// held is apart from the rest, as `FormSecret`s, so that nothing prints it.
public struct FormUsed: Equatable, Sendable {
    /// The step the form was at, as `FormShow` named it.
    public let step: String
    /// The name of the button pressed; `nil` when a choice was changed and
    /// no button pressed.
    public let button: String?
    /// What the text fields and choices held, by field name.
    public let values: [String: String]
    /// What the secret fields held, by field name.
    public let secrets: [String: FormSecret]

    /// `nil` unless the step, the button and every field have a name, and
    /// there are no more fields than a form can have.
    public init?(step: String, button: String?, values: [String: String], secrets: [String: FormSecret] = [:]) {
        guard FormID.isValid(step), button.map(FormID.isValid) ?? true,
            values.keys.allSatisfy(FormID.isValid), secrets.keys.allSatisfy(FormID.isValid),
            values.count + secrets.count <= FormShow.mostFields
        else { return nil }
        self.step = step
        self.button = button
        self.values = values
        self.secrets = secrets
    }

    static let allowedKeys: Set<String> = ["step", "button", "values", "secrets"]
}

/// One pane of the settings window.
public struct SettingsPane: Equatable, Sendable {
    /// What goes back in `SettingsUsed`.
    public let id: String
    /// On the pane's toolbar button, and the window's title while the pane
    /// is on show.
    public let title: String
    /// The SF Symbol on the pane's toolbar button.
    public let symbol: String
    /// Said above the fields.
    public let note: String?
    public let fields: [FormField]
    /// Said at the foot of the pane, beside a button that opens the
    /// settings file: what more can be set there.
    public let more: String?

    public init(
        id: String, title: String, symbol: String, note: String? = nil, fields: [FormField], more: String? = nil
    ) {
        self.id = id
        self.title = title
        self.symbol = symbol
        self.note = note
        self.fields = fields
        self.more = more
    }

    /// Whether `symbol` could be the name of an SF Symbol: lower case
    /// letters, digits and dots.
    static func isSymbol(_ symbol: String) -> Bool {
        let bytes = Array(symbol.utf8)
        return !bytes.isEmpty && bytes.count <= FormID.longest
            && bytes.allSatisfy { (0x61...0x7A).contains($0) || (0x30...0x39).contains($0) || $0 == 0x2E }
    }

    static let allowedKeys: Set<String> = ["id", "title", "symbol", "note", "fields", "more"]
}

/// What the settings window shows: its panes, each with its settings as they
/// stand. The shell lays them out and knows nothing of what any setting
/// means.
///
/// `problem` is what is wrong. With no panes, the settings cannot be shown or
/// changed from here at all (the settings file has a mistake in it): the
/// window says so and offers the file. With panes, the last change was not
/// made, and this is why.
public struct SettingsShow: Equatable, Sendable {
    public let panes: [SettingsPane]
    public let problem: String?

    /// The most panes the window may have.
    public static let mostPanes = 12

    /// `nil` unless there is something to show, every pane and every field
    /// of a pane has a name of its own, each pane has a title and a symbol,
    /// there is no secret, no code and no field with the focus, and every
    /// field hangs together.
    public init?(panes: [SettingsPane], problem: String? = nil) {
        guard panes.count <= Self.mostPanes, !panes.isEmpty || !(problem ?? "").isEmpty else { return nil }
        guard panes.allSatisfy({ FormID.isValid($0.id) }), Set(panes.map(\.id)).count == panes.count else {
            return nil
        }
        for pane in panes {
            guard !pane.title.isEmpty, SettingsPane.isSymbol(pane.symbol),
                pane.fields.count <= FormShow.mostFields,
                pane.fields.allSatisfy({ FormID.isValid($0.id) }),
                Set(pane.fields.map(\.id)).count == pane.fields.count
            else { return nil }
            for field in pane.fields {
                switch field.kind {
                case .secret, .code: return nil
                case .text, .choice, .fixed, .toggle, .number, .list: break
                }
                if field.focus || !field.isSound { return nil }
            }
        }
        self.panes = panes
        self.problem = problem
    }

    /// The field of this name in the pane of this name.
    public func field(_ field: String, in pane: String) -> FormField? {
        panes.first { $0.id == pane }?.fields.first { $0.id == field }
    }

    static let allowedKeys: Set<String> = ["panes", "problem"]
}

/// What a field of the settings window was given: on or off for a toggle, a
/// number for a number, text for a text field, and for a choice the name of
/// the option.
public enum SettingValue: Equatable, Sendable {
    case flag(Bool)
    case number(Double)
    case text(String)
}

/// What the user did in the settings window: gave a field another value, or
/// asked for one of a list's actions, of the row picked if the action is
/// done to a row.
public struct SettingsUsed: Equatable, Sendable {
    public enum Did: Equatable, Sendable {
        /// The field was given this value.
        case set(SettingValue)
        /// This action of a list was asked for, of this row if of any.
        case action(String, row: String?)
    }

    public let pane: String
    public let field: String
    public let did: Did

    /// The longest text a setting may be given, in bytes.
    public static let longestText = 4096

    /// `nil` unless the pane, the field, the action and the row have names,
    /// a number is one, and text is no longer than a setting takes.
    public init?(pane: String, field: String, did: Did) {
        guard FormID.isValid(pane), FormID.isValid(field) else { return nil }
        switch did {
        case let .set(.number(number)):
            guard number.isFinite else { return nil }
        case let .set(.text(text)):
            guard text.utf8.count <= Self.longestText else { return nil }
        case .set(.flag):
            break
        case let .action(action, row):
            guard FormID.isValid(action), row.map(FormID.isValid) ?? true else { return nil }
        }
        self.pane = pane
        self.field = field
        self.did = did
    }

    static let allowedKeys: Set<String> = ["pane", "field", "value", "action", "row"]
}

public enum BridgeError: Error, Equatable, CustomStringConvertible {
    case malformed(String)
    case version(String)
    case unknownMethod(String)

    public var description: String {
        switch self {
        case let .malformed(reason): "not a bridge message: \(reason)"
        case let .version(version): "unsupported JSON-RPC version \"\(version)\""
        case let .unknownMethod(method): "unknown method \"\(method)\""
        }
    }
}
