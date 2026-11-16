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
    /// Show sanitized HTML in a web view pane.
    case paneWeb(PaneWeb)
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
    /// Run the Vim that ships in the app on a draft file, in a terminal pane
    /// of its own. Answered by `editorExited` when Vim ends, or at once if
    /// it could not be started.
    case paneEditor(PaneEditor)
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
    /// The answer to `spellCheck`.
    case spellChecked(SpellChecked)
    /// The answer to `spellSuggest`.
    case spellSuggested(SpellSuggested)
    /// The answer to `paneEditor`.
    case editorExited(EditorExited)
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
    /// Unread messages across all inboxes, for the Dock badge.
    public var unread: Int
    /// The window title.
    public var title: String

    public init(context: Context, enabled: [String], unread: Int, title: String) {
        self.context = context
        self.enabled = enabled
        self.unread = unread
        self.title = title
    }

    static let allowedKeys: Set<String> = ["context", "enabled", "unread", "title"]
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

public struct PaneWeb: Equatable, Sendable, Codable {
    /// The sanitized HTML. Files beside it in its folder may be loaded.
    public var path: String
    public var title: String
    /// The user has allowed this message to load remote content.
    public var remote: Bool

    /// `nil` unless the path is absolute.
    public init?(path: String, title: String, remote: Bool) {
        guard path.hasPrefix("/") else { return nil }
        self.path = path
        self.title = title
        self.remote = remote
    }

    static let allowedKeys: Set<String> = ["path", "title", "remote"]
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

/// Which Vim the shell is to run. The core says which of the two; it never
/// names a program by its path.
public enum EditorVim: String, Equatable, Sendable, Codable {
    /// The one built into the app.
    case bundled
    /// The one that comes with macOS, which the user has asked for in
    /// their config.
    case system
    /// The user's own, by way of the script they keep in the app's scripts
    /// folder, which they have asked for in their config.
    case script
}

/// A draft file for Vim to edit, and the draft's own folder, where Vim and
/// the core leave each other notes. The shell passes the folder to Vim in
/// the environment and does nothing else with it.
public struct PaneEditor: Equatable, Sendable, Codable {
    /// The draft file.
    public let path: String
    /// The draft's own folder.
    public let dir: String
    public let title: String
    public let vim: EditorVim

    /// `nil` unless both paths are absolute.
    public init?(path: String, dir: String, title: String, vim: EditorVim = .bundled) {
        guard path.hasPrefix("/"), dir.hasPrefix("/") else { return nil }
        self.path = path
        self.dir = dir
        self.title = title
        self.vim = vim
    }

    static let allowedKeys: Set<String> = ["path", "dir", "title", "vim"]
}

/// How the editor ended.
public struct EditorExited: Equatable, Sendable, Codable {
    /// False when Vim could not be started: this build has none in it, or
    /// it would not start. `status` is then 0.
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
