import Darwin
import Foundation

/// The Vim that ships inside the app, for writing a message in Vim.
///
/// It is looked for in the app's own bundle and nowhere else: not on the
/// `PATH`, not in `/usr/bin`. A build without a Vim in it has no Vim of its
/// own; the user can ask for the one that comes with macOS instead
/// (`SystemVim`), and that is the only other program the shell starts.
public struct ShippedVim: Equatable, Sendable {
    /// Why a bundle has no Vim that may be run.
    public enum Problem: Error, Equatable, CustomStringConvertible {
        /// Nothing is where this part of Vim goes.
        case missing(String)
        /// Something is there, but not what is needed: a folder where the
        /// program goes, or a program that may not be run.
        case unusable(String)
        /// The part is a link that leads out of the bundle.
        case outsideBundle(String)

        public var description: String {
            switch self {
            case let .missing(part): "\(part) is not in the app"
            case let .unusable(part): "\(part) is not what Vim needs there"
            case let .outsideBundle(part): "\(part) leads out of the app"
            }
        }
    }

    /// Where each part goes, from the top of the bundle.
    public static let binaryPath = "Contents/Helpers/vim"
    public static let homePath = "Contents/Resources/vim"
    public static let runtimePath = "Contents/Resources/vim/runtime"
    public static let pluginPath = "Contents/Resources/vim/quill.vim"

    /// The program.
    public let binary: URL
    /// What Vim calls `$VIM`: the folder its runtime and Quill's plugin are in.
    public let home: URL
    /// Vim's runtime files: `$VIMRUNTIME`.
    public let runtime: URL
    /// Quill's side of writing a message in Vim.
    public let plugin: URL

    /// The Vim in this bundle. Throws unless all of it is there: a program
    /// that may be run, the folder of its runtime files and the plugin, each
    /// of them, with every link followed, inside the bundle.
    public init(inBundle bundle: URL) throws {
        guard let root = Self.realPath(bundle.path) else {
            throw Problem.missing(Self.binaryPath)
        }
        binary = bundle.appendingPathComponent(Self.binaryPath)
        home = bundle.appendingPathComponent(Self.homePath, isDirectory: true)
        runtime = bundle.appendingPathComponent(Self.runtimePath, isDirectory: true)
        plugin = bundle.appendingPathComponent(Self.pluginPath)

        try Self.check(binary, Self.binaryPath, inside: root, isFolder: false)
        guard access(binary.path, X_OK) == 0 else {
            throw Problem.unusable(Self.binaryPath)
        }
        try Self.check(runtime, Self.runtimePath, inside: root, isFolder: true)
        try Self.check(plugin, Self.pluginPath, inside: root, isFolder: false)
    }

    /// Throws unless `part` is there, is a folder or a file as wanted, and
    /// is inside `root` once every link on the way to it has been followed.
    static func check(_ part: URL, _ name: String, inside root: String, isFolder: Bool) throws {
        guard let real = realPath(part.path) else {
            throw Problem.missing(name)
        }
        guard real.hasPrefix(root.hasSuffix("/") ? root : root + "/") else {
            throw Problem.outsideBundle(name)
        }
        var status = stat()
        guard stat(real, &status) == 0 else {
            throw Problem.missing(name)
        }
        let kind = status.st_mode & S_IFMT
        guard kind == (isFolder ? S_IFDIR : S_IFREG) else {
            throw Problem.unusable(name)
        }
    }

    /// The path with every link followed, if all of it exists.
    static func realPath(_ path: String) -> String? {
        guard let resolved = realpath(path, nil) else { return nil }
        defer { free(resolved) }
        return String(cString: resolved)
    }
}
