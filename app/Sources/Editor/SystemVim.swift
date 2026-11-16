import Bridge
import Darwin
import Foundation

/// The Vim that comes with macOS: what the app writes a message in unless
/// the user has asked for their own (`vim = "system"` under `[compose]`,
/// which is also what it is when nothing is said).
///
/// It is the one program outside the app's bundle that the shell itself
/// starts, and it is named here, by a constant: never by the core, the
/// config, the environment or the `PATH`. The App Sandbox lets an app run
/// what macOS itself ships in `/usr/bin`, and the Vim so started is inside
/// the same sandbox as the app. A Vim the user installed themselves
/// (Homebrew's, say) cannot be started from the sandbox at all.
///
/// Quill's plugin still comes from the app's bundle.
public struct SystemVim: Equatable, Sendable {
    /// Where macOS keeps its Vim.
    public static let path = "/usr/bin/vim"
    /// Where what macOS ships lives. With every link followed, the program
    /// must still be in here.
    public static let systemRoot = "/usr"

    /// Why this Mac's own Vim will not be run.
    public enum Problem: Error, Equatable, CustomStringConvertible {
        /// There is nothing where macOS keeps its Vim.
        case missing(String)
        /// Something is there that is not a program that may be run.
        case unusable(String)
        /// It is a link to something that macOS did not put there.
        case notTheSystems(String)

        public var description: String {
            switch self {
            case let .missing(path): "there is no \(path) on this Mac"
            case let .unusable(path): "\(path) is not a program that can be run"
            case let .notTheSystems(path): "\(path) leads to a program that is not macOS's own"
            }
        }
    }

    /// The program.
    public let binary: URL
    /// Quill's side of writing a message in Vim, in the app's bundle.
    public let plugin: URL

    /// The Vim of this Mac and the plugin in this bundle. Throws unless the
    /// program is there, is a program that may be run, and is the system's
    /// with every link followed, and the plugin is inside the bundle.
    public init(inBundle bundle: URL) throws {
        try self.init(inBundle: bundle, binary: Self.path, within: Self.systemRoot)
    }

    /// As above, with the program and the folder it must be in named, for
    /// tests: nothing else calls this.
    init(inBundle bundle: URL, binary path: String, within systemRoot: String) throws {
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
        plugin = try BundledFile.find(BundledFile.pluginPath, inBundle: bundle)
    }
}

/// The Vim a draft is to be edited in.
public enum VimToRun: Equatable, Sendable {
    /// The one that comes with macOS, inside the sandbox.
    case system(SystemVim)
    /// Whichever one the user's own script starts, outside the sandbox.
    case script(UserEditorScript)

    /// The Vim the core asked for, if this app and this Mac have it.
    /// `scriptsFolder` is where the user's own script would be.
    public init(_ asked: EditorVim, inBundle bundle: URL, scriptsFolder: URL?) throws {
        switch asked {
        case .system: self = .system(try SystemVim(inBundle: bundle))
        case .script:
            guard let scriptsFolder else {
                throw UserEditorScript.Problem.missing(UserEditorScript.name)
            }
            self = .script(try UserEditorScript(inScriptsFolder: scriptsFolder, bundle: bundle))
        }
    }
}
