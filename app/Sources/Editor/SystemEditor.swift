import Bridge
import Darwin
import Foundation

/// One of the editors that come with macOS: Vim, or nano (which on a Mac
/// is Pico). macOS has a third, mg, which is not here: it crashes as it
/// starts inside the sandbox.
///
/// These are the only programs outside the app's bundle that the shell
/// itself starts, and each is named here, by a constant: never by the core,
/// the config, the environment or the `PATH`. The App Sandbox lets an app
/// run what macOS itself ships in `/usr/bin`, and the editor so started is
/// inside the same sandbox as the app. An editor the user installed
/// themselves (Homebrew's Vim, say) cannot be started from the sandbox at
/// all; for their own Vim there is `UserEditorScript`.
///
/// Quill's plugin, which only Vim has a use for, comes from the app's
/// bundle.
public struct SystemEditor: Equatable, Sendable {
    /// Where macOS keeps each of them.
    public static func path(of kind: EditorKind) -> String? {
        switch kind {
        case .vim: "/usr/bin/vim"
        case .nano: "/usr/bin/nano"
        // Not one of macOS's: the user's own, by their script.
        case .script: nil
        }
    }

    /// Where what macOS ships lives. With every link followed, the program
    /// must still be in here.
    public static let systemRoot = "/usr"

    /// Why this Mac's own editor will not be run.
    public enum Problem: Error, Equatable, CustomStringConvertible {
        /// There is nothing where macOS keeps it.
        case missing(String)
        /// Something is there that is not a program that may be run.
        case unusable(String)
        /// It is a link to something that macOS did not put there.
        case notTheSystems(String)
        /// It is not one of the editors that come with macOS.
        case notOneOfThem(String)

        public var description: String {
            switch self {
            case let .missing(path): "there is no \(path) on this Mac"
            case let .unusable(path): "\(path) is not a program that can be run"
            case let .notTheSystems(path): "\(path) leads to a program that is not macOS's own"
            case let .notOneOfThem(name): "\(name) is not an editor that comes with macOS"
            }
        }
    }

    /// Which of them it is.
    public let kind: EditorKind
    /// The program.
    public let binary: URL
    /// Quill's side of writing a message in Vim, in the app's bundle. Only
    /// Vim has one.
    public let plugin: URL?

    /// This Mac's own `kind`, with the plugin in this bundle when it is
    /// Vim. Throws unless the program is there, is a program that may be
    /// run, and is the system's with every link followed, and the plugin is
    /// inside the bundle.
    public init(_ kind: EditorKind, inBundle bundle: URL) throws {
        guard let path = Self.path(of: kind) else {
            throw Problem.notOneOfThem(kind.rawValue)
        }
        try self.init(kind, inBundle: bundle, binary: path, within: Self.systemRoot)
    }

    /// As above, with the program and the folder it must be in named, for
    /// tests: nothing else calls this.
    init(_ kind: EditorKind, inBundle bundle: URL, binary path: String, within systemRoot: String) throws {
        guard kind != .script else {
            throw Problem.notOneOfThem(kind.rawValue)
        }
        self.kind = kind
        binary = URL(fileURLWithPath: path)
        guard let system = BundledFile.realPath(systemRoot), let real = BundledFile.realPath(path) else {
            throw Problem.missing(path)
        }
        guard real.hasPrefix(system.hasSuffix("/") ? system : system + "/") else {
            throw Problem.notTheSystems(path)
        }
        var status = stat()
        guard stat(real, &status) == 0, status.st_mode & S_IFMT == S_IFREG, access(real, X_OK) == 0 else {
            throw Problem.unusable(path)
        }
        plugin = kind == .vim ? try BundledFile.find(BundledFile.pluginPath, inBundle: bundle) : nil
    }
}

/// The editor a draft is to be edited in.
public enum EditorToRun: Equatable, Sendable {
    /// One that comes with macOS, inside the sandbox.
    case system(SystemEditor)
    /// Whichever Vim the user's own script starts, outside the sandbox.
    case script(UserEditorScript)

    /// The editor the core asked for, if this Mac has it. `scriptsFolder`
    /// is where the user's own script would be.
    public init(_ asked: EditorKind, inBundle bundle: URL, scriptsFolder: URL?) throws {
        switch asked {
        case .vim, .nano:
            self = .system(try SystemEditor(asked, inBundle: bundle))
        case .script:
            guard let scriptsFolder else {
                throw UserEditorScript.Problem.missing(UserEditorScript.name)
            }
            self = .script(try UserEditorScript(inScriptsFolder: scriptsFolder, bundle: bundle))
        }
    }

    /// Which editor this is.
    public var kind: EditorKind {
        switch self {
        case let .system(editor): editor.kind
        case .script: .script
        }
    }
}
