import Bridge
import Darwin
import Foundation

/// Reading a theme file: all of one that is small enough to be a theme, and
/// none of anything else.
public enum ThemeFile {
    public enum Problem: Error, Equatable, CustomStringConvertible {
        /// What was chosen is a folder, or something else that is not a
        /// file, or is not there.
        case notAFile
        /// The file is larger than a theme can be.
        case tooLarge
        /// The file could not be read.
        case unreadable(String)
        /// The file is not text.
        case notText

        public var description: String {
            switch self {
            case .notAFile: "that is not a file"
            case .tooLarge: "the file is larger than 64 KB, and no theme is"
            case let .unreadable(reason): "the file could not be read (\(reason))"
            case .notText: "the file is not text"
            }
        }
    }

    /// The text of a file of at most `ThemeSwatch.largest` bytes.
    public static func read(_ source: URL) throws -> String {
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
        let largest = ThemeSwatch.largest
        guard status.st_size <= off_t(largest) else { throw Problem.tooLarge }
        // One byte more than is allowed is enough to know a file that grew
        // since it was measured.
        var contents = Data()
        var buffer = [UInt8](repeating: 0, count: 16 * 1024)
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
        guard contents.count <= largest else { throw Problem.tooLarge }
        guard let text = String(data: contents, encoding: .utf8) else { throw Problem.notText }
        return text
    }
}

/// Copying a theme the user chose into the themes folder beside the
/// settings file, where the core finds it.
///
/// The shell does this, as it copies in a vimrc: the file chosen may be
/// anywhere, and inside the App Sandbox only the app itself is let read
/// what the user chose in a panel. The core is then told the theme's name,
/// reads the folder again, and decides for itself whether the file is a
/// theme it can use.
public struct ThemeImport: Equatable, Sendable {
    public enum Problem: Error, Equatable, CustomStringConvertible {
        case file(ThemeFile.Problem)
        /// The file names no background and foreground: it is not a theme
        /// written for Ghostty.
        case notATheme
        /// The file's name is none a theme can have.
        case badName(String)
        /// There is a theme of Quill's own under that name already, which
        /// would be found first.
        case taken(String)
        /// The themes folder is not a folder of Quill's own to write in.
        case notAFolder
        /// The copy could not be written.
        case unwritable(String)

        public var description: String {
            switch self {
            case let .file(problem): problem.description
            case .notATheme:
                "it is not a theme in Ghostty\u{2019}s format: it names no background and foreground colour"
            case let .badName(name):
                "\u{201C}\(name)\u{201D} cannot be a theme\u{2019}s name. Give the file a plainer name"
            case let .taken(name):
                "there is already a theme called \u{201C}\(name)\u{201D} in the themes folder, in \(name).toml"
            case .notAFolder: "the themes folder is not a folder"
            case let .unwritable(reason): "the copy could not be written (\(reason))"
            }
        }
    }

    /// The folder the copies go in.
    public let folder: URL

    public init(folder: URL) {
        self.folder = folder
    }

    /// The themes folder of a config folder.
    public init(configFolder: URL) {
        self.init(folder: configFolder.appendingPathComponent("themes", isDirectory: true))
    }

    /// What the theme in `source` will be called: its file's name, without
    /// a `.toml` it may have been given.
    public static func name(of source: URL) -> String {
        let name = source.lastPathComponent
        return name.lowercased().hasSuffix(".toml") ? String(name.dropLast(5)) : name
    }

    /// Copies the theme in `source` in, in place of one of the same name,
    /// and returns its name. The copy can be read and written by the user
    /// and nobody else. An earlier copy is whole until the new one is, and
    /// is left as it was if the new one is refused.
    @discardableResult
    public func importFile(_ source: URL) throws -> String {
        let text: String
        do {
            text = try ThemeFile.read(source)
        } catch let problem as ThemeFile.Problem {
            throw Problem.file(problem)
        }
        guard ThemeSwatch(text: text) != nil else { throw Problem.notATheme }
        let name = Self.name(of: source)
        guard ThemeName.isValid(name) else {
            // Said without whatever in it could not be shown.
            throw Problem.badName(String(name.unicodeScalars.filter { $0.properties.generalCategory != .control }))
        }
        var status = stat()
        if lstat(folder.appendingPathComponent("\(name).toml").path, &status) == 0 {
            throw Problem.taken(name)
        }
        try makeFolder()

        // Written to one side and moved into place, so that the core never
        // reads half a theme.
        let staged = folder.appendingPathComponent(".import-\(UUID().uuidString)")
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
            failure = Self.write(Data(text.utf8), to: descriptor)
        }
        if close(descriptor) != 0, failure == nil {
            failure = errno
        }
        // Onto the name itself: a link left there is replaced, not followed.
        if failure == nil, rename(staged.path, folder.appendingPathComponent(name).path) != 0 {
            failure = errno
        }
        if let failure {
            unlink(staged.path)
            throw Problem.unwritable(String(cString: strerror(failure)))
        }
        return name
    }

    /// Makes the themes folder if it is not there, the user's alone. One
    /// that is there is left as the user has it, and must be a folder: a
    /// link that leads somewhere else is not written through.
    private func makeFolder() throws {
        var status = stat()
        if lstat(folder.path, &status) == 0 {
            guard status.st_mode & S_IFMT == S_IFDIR else { throw Problem.notAFolder }
            return
        }
        do {
            try FileManager.default.createDirectory(
                at: folder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        } catch {
            throw Problem.unwritable(error.localizedDescription)
        }
        chmod(folder.path, 0o700)
    }

    /// Writes all of `contents`. Returns the error, if there was one.
    private static func write(_ contents: Data, to descriptor: Int32) -> Int32? {
        guard !contents.isEmpty else { return nil }
        return contents.withUnsafeBytes { buffer -> Int32? in
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
