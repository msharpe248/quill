import Darwin
import Foundation

/// The user's own script for writing a message in their own Vim
/// (`editor = "script"` under `[compose]`).
///
/// It lives in the app's scripts folder, `~/Library/Application
/// Scripts/<the app's id>/`. That folder is how macOS lets a sandboxed app
/// run something of the user's: the app can read it and not write to it, so
/// only the user can put a script there, and macOS runs the script outside
/// the sandbox when the app asks. The Vim it starts is therefore the user's
/// own in every way: their vimrc, their plugins, and no sandbox.
///
/// The shell names the script by this one fixed name in that one folder.
/// Neither the core, the config nor the environment can name another.
public struct UserEditorScript: Equatable, Sendable {
    /// Why there is no script to run.
    public enum Problem: Error, Equatable, CustomStringConvertible {
        /// The user has put no script in the folder.
        case missing(String)
        /// Something is there that is not a file that may be run.
        case unusable(String)

        public var description: String {
            switch self {
            case let .missing(path): "there is no \(path)"
            case let .unusable(path): "\(path) is not a file that can be run"
            }
        }
    }

    /// The script's name in the scripts folder.
    public static let name = "editor-command.sh"
    /// Quill's helper, which the script is handed: it makes the pane the
    /// terminal of the program it runs. From the top of the bundle.
    public static let helperPath = "Contents/Resources/vim/on-terminal"
    /// A script to start from, for the user to copy.
    public static let samplePath = "Contents/Resources/vim/editor-command.sh"

    /// The user's script.
    public let script: URL
    /// The helper in the app's bundle.
    public let helper: URL

    /// The script in `folder` and the helper in `bundle`. Throws unless the
    /// script is a file that may be run and the helper is inside the bundle.
    public init(inScriptsFolder folder: URL, bundle: URL) throws {
        script = folder.appendingPathComponent(Self.name)
        var status = stat()
        guard stat(script.path, &status) == 0 else {
            throw Problem.missing(script.path)
        }
        guard status.st_mode & S_IFMT == S_IFREG, access(script.path, X_OK) == 0 else {
            throw Problem.unusable(script.path)
        }
        helper = try BundledFile.find(Self.helperPath, inBundle: bundle)
    }
}
