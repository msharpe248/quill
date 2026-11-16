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
    /// The program is one of the user's own scripts, in the app's scripts
    /// folder, and macOS is asked to run it there, outside the App Sandbox.
    ///
    /// Nothing but a file in that folder can be run this way, and only the
    /// user can put one there. Its environment and the folder it starts in
    /// are macOS's to decide: `environment` and `workingDirectory` are not
    /// used. It is outside the sandbox, so it sets its terminal's modes
    /// itself and `rawTerminal` is not used either.
    public var userScript: Bool

    public init(
        executable: URL,
        arguments: [String] = [],
        environment: [String: String] = [:],
        workingDirectory: URL? = nil,
        rawTerminal: Bool = false,
        userScript: Bool = false
    ) {
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
        self.workingDirectory = workingDirectory
        self.rawTerminal = rawTerminal
        self.userScript = userScript
    }
}

/// How a terminal surface looks. Colours are `#rrggbb`. Every surface in the
/// app looks the same: an appearance given to one is given to all.
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

/// A terminal's grid: how many cells it has, and how wide one of them is.
public struct TerminalGrid: Equatable, Sendable {
    public var columns: Int
    public var rows: Int
    /// The width of one cell, in the screen's own pixels: a whole number.
    public var cellWidthPixels: Int
    /// How many of those pixels there are to a point.
    public var scale: CGFloat

    public init(columns: Int, rows: Int, cellWidthPixels: Int, scale: CGFloat) {
        self.columns = columns
        self.rows = rows
        self.cellWidthPixels = cellWidthPixels
        self.scale = scale
    }
}

/// One terminal on screen running one program.
///
/// This protocol is the whole of what the rest of the app knows about the
/// terminal. Everything libghostty lives behind it, in this module.
@MainActor
public protocol TerminalSurface: AnyObject {
    /// The view to put in a window. The terminal's grid follows its size.
    var view: NSView { get }

    /// The terminal's grid as it is now, once the view has been measured.
    var grid: TerminalGrid? { get }

    /// Called on the main actor when the grid changes: the view was
    /// resized, or its cells were (another font size, another screen).
    var onGridChange: ((TerminalGrid) -> Void)? { get set }

    /// Called once, on the main actor, when the program ends.
    var onExit: ((TerminalExit) -> Void)? { get set }

    /// Called on the main actor when the program sets the terminal title.
    var onTitleChange: ((String) -> Void)? { get set }

    /// Called on the main actor when a link in the terminal is clicked, with
    /// the link's address as the terminal has it. Nothing is opened unless
    /// this is set and opens it.
    var onOpenLink: ((String) -> Void)? { get set }

    /// Called on the main actor when files are dropped on the terminal.
    /// While it is unset a dropped file is pasted as its path, as a
    /// terminal does.
    var onDropFiles: (([URL]) -> Void)? { get set }

    /// Starts the program. A surface runs one program in its lifetime.
    func start(_ command: TerminalCommand)

    /// Sends text to the program as a paste, not as key presses.
    func sendText(_ text: String)

    /// Sends text to the program as if it were typed: its bytes as they
    /// are, with nothing put around them to mark a paste, so that a program
    /// reads `esc` as the key. Nothing is sent to a program that has not
    /// started or has ended.
    func sendKeys(_ text: String)

    /// Gives the surface keyboard focus.
    func focus()

    func setAppearance(_ appearance: TerminalAppearance)

    /// The visible screen as text, one line per row. For tests and debugging.
    func visibleText() -> String?

    /// Asks the program to stop, as closing its window would. A program
    /// still waiting to be started is not started, and is reported as
    /// having failed to start.
    func terminate()

    /// Ends the program at once. It is given no chance to save anything:
    /// for a program that was asked to stop and has not.
    func kill()
}

/// Creates a libghostty-backed surface.
@MainActor
public func makeTerminalSurface(appearance: TerminalAppearance = .init()) -> any TerminalSurface {
    GhosttySurface(appearance: appearance)
}
