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
