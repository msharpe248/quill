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

/// How a terminal surface looks. Every surface in the app looks the same:
/// an appearance given to one is given to all.
///
/// `background` and `foreground` are the colours of a cell the program has
/// not painted and of text it has not coloured, as `#rrggbb`. Left out,
/// they are the terminal's own, which are light or dark as the Mac is.
public struct TerminalAppearance: Sendable, Equatable {
    public var fontFamily: String?
    public var fontSize: Float
    public var background: String?
    public var foreground: String?

    public init(
        fontFamily: String? = nil,
        fontSize: Float = 13,
        background: String? = nil,
        foreground: String? = nil
    ) {
        self.fontFamily = fontFamily
        self.fontSize = fontSize
        self.background = background
        self.foreground = foreground
    }

    /// Which of the terminal's two sets of colours goes with this
    /// appearance: the sixteen a program names (`red`, `bright-black`), the
    /// cursor and the selection.
    enum Palette: Equatable {
        /// The Mac's: light or dark as it is, and changing with it.
        case followsMac
        /// The ones made to be read on a dark background, or on a light
        /// one, whichever the Mac is in.
        case dark, light
    }

    /// The set that can be read on the background asked for. A background
    /// of the terminal's own is the Mac's, and so are its colours.
    var palette: Palette {
        guard let background = Self.channels(of: background) else { return .followsMac }
        // How bright the eye takes it for, from 0 to 255.
        let (red, green, blue) = (Double(background.red), Double(background.green), Double(background.blue))
        return 0.299 * red + 0.587 * green + 0.114 * blue < 128 ? .dark : .light
    }

    /// The background as it may be given to the terminal: `#rrggbb` and
    /// nothing else, whatever was handed in.
    var checkedBackground: String? { Self.channels(of: background) == nil ? nil : background }

    /// The foreground, likewise.
    var checkedForeground: String? { Self.channels(of: foreground) == nil ? nil : foreground }

    /// The red, green and blue of a `#rrggbb` colour, or nil for anything
    /// else.
    static func channels(of colour: String?) -> (red: UInt8, green: UInt8, blue: UInt8)? {
        guard let colour, colour.utf8.count == 7, colour.hasPrefix("#") else { return nil }
        let digits = colour.dropFirst()
        guard digits.allSatisfy(\.isHexDigit), let value = UInt32(digits, radix: 16) else { return nil }
        return (UInt8(value >> 16 & 0xff), UInt8(value >> 8 & 0xff), UInt8(value & 0xff))
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
