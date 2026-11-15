import Darwin
import Foundation

/// The user's vimrc, as Quill keeps it: a copy in a folder of Quill's own.
///
/// Vim runs inside the app's sandbox and cannot read `~/.vimrc`. The user
/// chooses a file once, in an open panel, and the copy is what Vim is
/// started with from then on. Importing again replaces the copy.
public struct ImportedVimrc: Equatable, Sendable {
    public enum Problem: Error, Equatable, CustomStringConvertible {
        /// What was chosen is a folder, or something else that is not a
        /// file, or is not there.
        case notAFile
        /// The file is larger than `largest`.
        case tooLarge
        /// The file could not be read.
        case unreadable(String)
        /// The copy could not be written.
        case unwritable(String)

        public var description: String {
            switch self {
            case .notAFile: "that is not a file"
            case .tooLarge: "the file is larger than 1 MB"
            case let .unreadable(reason): "the file could not be read (\(reason))"
            case let .unwritable(reason): "the copy could not be written (\(reason))"
            }
        }
    }

    /// The largest vimrc taken, in bytes: 1 MB.
    public static let largest = 1_048_576

    /// The folder the copy is kept in.
    public let folder: URL

    public init(folder: URL) {
        self.folder = folder
    }

    /// The folder the copy goes in: `Quill/vim` in the app's Application
    /// Support folder, which inside the sandbox is in its container. A run
    /// whose core was pointed at another config folder, as a scratch run is,
    /// keeps its vimrc there too and leaves the user's own alone.
    public static func folder(applicationSupport: URL, configFolder: String?) -> URL {
        if let configFolder, configFolder.hasPrefix("/") {
            return URL(fileURLWithPath: configFolder, isDirectory: true)
                .appendingPathComponent("vim", isDirectory: true)
        }
        return
            applicationSupport
            .appendingPathComponent("Quill", isDirectory: true)
            .appendingPathComponent("vim", isDirectory: true)
    }

    /// Where the copy is, whether or not there is one.
    public var file: URL { folder.appendingPathComponent("vimrc") }

    /// The copy, if there is one: a file, and not a link to one.
    public var current: URL? {
        var status = stat()
        guard lstat(file.path, &status) == 0, status.st_mode & S_IFMT == S_IFREG else { return nil }
        return file
    }

    /// The size of the copy in bytes, if there is one.
    public var size: Int? {
        var status = stat()
        guard lstat(file.path, &status) == 0, status.st_mode & S_IFMT == S_IFREG else { return nil }
        return Int(status.st_size)
    }

    /// Copies `source` in, in place of any copy already there, and returns
    /// its size in bytes. The copy can be read and written by the user and
    /// nobody else. An earlier copy is whole until the new one is, and is
    /// left as it was if the new one is refused.
    @discardableResult
    public func importFile(_ source: URL) throws -> Int {
        let contents = try Self.read(source)
        do {
            try FileManager.default.createDirectory(
                at: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        } catch {
            throw Problem.unwritable(error.localizedDescription)
        }
        // The folder is Quill's own, whoever made it and however.
        chmod(folder.path, 0o700)

        // Written to one side and moved into place, so that Vim never
        // starts on half a vimrc.
        let staged = folder.appendingPathComponent(".vimrc-\(UUID().uuidString)")
        let descriptor = open(staged.path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0o600)
        guard descriptor >= 0 else {
            throw Problem.unwritable(String(cString: strerror(errno)))
        }
        var failure: Int32?
        // Exactly these permissions, whatever the process's mask.
        if fchmod(descriptor, 0o600) != 0 {
            failure = errno
        }
        if failure == nil {
            failure = Self.write(contents, to: descriptor)
        }
        if close(descriptor) != 0, failure == nil {
            failure = errno
        }
        if failure == nil, rename(staged.path, file.path) != 0 {
            failure = errno
        }
        if let failure {
            unlink(staged.path)
            throw Problem.unwritable(String(cString: strerror(failure)))
        }
        return contents.count
    }

    /// Removes the copy. Returns whether there was one.
    @discardableResult
    public func remove() throws -> Bool {
        if unlink(file.path) == 0 {
            return true
        }
        if errno == ENOENT {
            return false
        }
        throw Problem.unwritable(String(cString: strerror(errno)))
    }

    /// The whole of a file that is at most `largest` bytes.
    private static func read(_ source: URL) throws -> Data {
        guard source.isFileURL else { throw Problem.notAFile }
        // Not left waiting on a pipe, and nothing made the terminal of.
        let descriptor = open(source.path, O_RDONLY | O_NONBLOCK | O_NOCTTY)
        guard descriptor >= 0 else {
            throw errno == ENOENT ? Problem.notAFile : Problem.unreadable(String(cString: strerror(errno)))
        }
        defer { close(descriptor) }
        // Asked of the open file, so it is this file that is measured.
        var status = stat()
        guard fstat(descriptor, &status) == 0, status.st_mode & S_IFMT == S_IFREG else {
            throw Problem.notAFile
        }
        guard status.st_size <= off_t(largest) else {
            throw Problem.tooLarge
        }
        // One byte more than is allowed is enough to know a file that grew
        // since it was measured.
        var contents = Data()
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        while contents.count <= largest {
            let count = Darwin.read(descriptor, &buffer, buffer.count)
            if count > 0 {
                contents.append(contentsOf: buffer[..<count])
            } else if count == 0 {
                break
            } else if errno != EINTR {
                throw Problem.unreadable(String(cString: strerror(errno)))
            }
        }
        guard contents.count <= largest else {
            throw Problem.tooLarge
        }
        return contents
    }

    /// Writes all of `contents`. Returns the error, if there was one.
    private static func write(_ contents: Data, to descriptor: Int32) -> Int32? {
        contents.withUnsafeBytes { buffer -> Int32? in
            var offset = 0
            while offset < buffer.count {
                let written = Darwin.write(descriptor, buffer.baseAddress! + offset, buffer.count - offset)
                if written > 0 {
                    offset += written
                } else if written == -1, errno == EINTR {
                    continue
                } else {
                    return written == 0 ? EIO : errno
                }
            }
            return nil
        }
    }
}
