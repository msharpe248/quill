import AppKit

/// A program to run inside a terminal surface.
public struct TerminalCommand: Sendable, Equatable {
    public var executable: URL
    public var arguments: [String]
    /// The whole environment of the child. Nothing is inherited implicitly.
    public var environment: [String: String]
    public var workingDirectory: URL?
    /// Start the program on a terminal that is already in raw mode.
    ///
    /// A full-screen program normally does this for itself, but inside the
    /// App Sandbox a program may not change its terminal's modes. Only the
    /// side that owns the PTY may, and that is this module.
    public var rawTerminal: Bool

    public init(
        executable: URL,
        arguments: [String] = [],
        environment: [String: String] = [:],
        workingDirectory: URL? = nil,
        rawTerminal: Bool = false
    ) {
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.workingDirectory = workingDirectory
        self.rawTerminal = rawTerminal
    }
}

/// How a terminal surface looks. Colours are `#rrggbb`.
public struct TerminalAppearance: Sendable, Equatable {
    public var fontFamily: String?
    public var fontSize: Float
    public var background: String
    public var foreground: String

    public init(
        fontFamily: String? = nil,
        fontSize: Float = 13,
        background: String = "#1a1b26",
        foreground: String = "#c0caf5"
    ) {
        self.fontFamily = fontFamily
        self.fontSize = fontSize
        self.background = background
        self.foreground = foreground
    }
}

/// How the program in a surface ended.
public enum TerminalExit: Sendable, Equatable {
    case exited(code: Int32)
    case signaled(signal: Int32)
    case failedToStart(reason: String)
}

/// One terminal on screen running one program.
///
/// This protocol is the whole of what the rest of the app knows about the
/// terminal. Everything libghostty lives behind it, in this module.
@MainActor
public protocol TerminalSurface: AnyObject {
    /// The view to put in a window. The terminal's grid follows its size.
    var view: NSView { get }

    /// Called once, on the main actor, when the program ends.
    var onExit: ((TerminalExit) -> Void)? { get set }

    /// Called on the main actor when the program sets the terminal title.
    var onTitleChange: ((String) -> Void)? { get set }

    /// Called on the main actor when a link in the terminal is clicked, with
    /// the link's address as the terminal has it. Nothing is opened unless
    /// this is set and opens it.
    var onOpenLink: ((String) -> Void)? { get set }

    /// Starts the program. A surface runs one program in its lifetime.
    func start(_ command: TerminalCommand)

    /// Sends text to the program as a paste, not as key presses.
    func sendText(_ text: String)

    /// Gives the surface keyboard focus.
    func focus()

    func setAppearance(_ appearance: TerminalAppearance)

    /// The visible screen as text, one line per row. For tests and debugging.
    func visibleText() -> String?

    /// Asks the program to stop, as closing its window would.
    func terminate()
}

/// Creates a libghostty-backed surface.
@MainActor
public func makeTerminalSurface(appearance: TerminalAppearance = .init()) -> any TerminalSurface {
    GhosttySurface(appearance: appearance)
}
