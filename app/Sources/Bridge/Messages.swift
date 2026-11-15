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
    /// Show a file in a Quick Look pane beside the terminal.
    case panePreview(PanePreview)
    /// Close the second pane and return focus to the terminal.
    case paneClose
}

/// A message the shell sends to the core.
public enum ShellToCore: Equatable, Sendable {
    /// A menu item, toolbar button or notification action was used.
    case commandRun(name: String)
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
    case sidebar, list, reader, help
}

public struct PanePreview: Equatable, Sendable, Codable {
    /// Absolute path of a file the shell can read.
    public var path: String
    public var title: String

    public init(path: String, title: String) {
        self.path = path
        self.title = title
    }

    static let allowedKeys: Set<String> = ["path", "title"]
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
