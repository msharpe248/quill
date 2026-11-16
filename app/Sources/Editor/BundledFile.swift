import Darwin
import Foundation

/// The files of Quill's own that go with writing a message in Vim: the
/// plugin, and the helper a script of the user's is handed.
///
/// They are in the app's bundle and are looked for nowhere else. The app has
/// no Vim of its own inside it: the Vim is the one that comes with macOS
/// (`SystemEditor`) or the user's own by their script (`UserEditorScript`).
public enum BundledFile {
    /// Why a file of Quill's cannot be used.
    public enum Problem: Error, Equatable, CustomStringConvertible {
        /// Nothing is where the file goes.
        case missing(String)
        /// Something is there, but not what is needed: a folder where a
        /// file goes.
        case unusable(String)
        /// It is a link that leads out of the bundle.
        case outsideBundle(String)

        public var description: String {
            switch self {
            case let .missing(part): "\(part) is not in the app"
            case let .unusable(part): "\(part) is not what Vim needs there"
            case let .outsideBundle(part): "\(part) leads out of the app"
            }
        }
    }

    /// Quill's side of writing a message in Vim, from the top of the bundle.
    public static let pluginPath = "Contents/Resources/vim/quill.vim"

    /// The file at `path` in `bundle`. Throws unless it is there, is a file,
    /// and is inside the bundle once every link on the way to it has been
    /// followed.
    static func find(_ path: String, inBundle bundle: URL) throws -> URL {
        guard let root = realPath(bundle.path) else {
            throw Problem.missing(path)
        }
        let file = bundle.appendingPathComponent(path)
        guard let real = realPath(file.path) else {
            throw Problem.missing(path)
        }
        guard real.hasPrefix(root.hasSuffix("/") ? root : root + "/") else {
            throw Problem.outsideBundle(path)
        }
        var status = stat()
        guard stat(real, &status) == 0 else {
            throw Problem.missing(path)
        }
        guard status.st_mode & S_IFMT == S_IFREG else {
            throw Problem.unusable(path)
        }
        return file
    }

    /// The path with every link followed, if all of it exists.
    static func realPath(_ path: String) -> String? {
        guard let resolved = realpath(path, nil) else { return nil }
        defer { free(resolved) }
        return String(cString: resolved)
    }
}
