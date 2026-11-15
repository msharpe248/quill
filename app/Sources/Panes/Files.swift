import Darwin
import Foundation

/// The app's temporary folder: the one place the shell and the core both
/// reach, and the only place the shell writes or opens a file because the
/// core asked.
public enum TemporaryFolder {
    public static var url: URL {
        URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
    }

    /// Whether `file` is inside `folder`, or would be once written.
    ///
    /// Links are followed first, so a link inside the folder that points out
    /// of it does not count, and neither does a path that climbs out with
    /// `..`. The file need not exist yet; the folder it is to go in must.
    public static func contains(_ file: URL, in folder: URL = url) -> Bool {
        guard file.isFileURL, let root = realPath(folder.path) else { return false }
        let name = file.lastPathComponent
        guard !name.isEmpty, name != ".", name != "..", name != "/" else { return false }
        let path: String
        if let whole = realPath(file.path) {
            path = whole
        } else if let parent = realPath(file.deletingLastPathComponent().path) {
            path = parent + "/" + name
        } else {
            return false
        }
        return path.hasPrefix(root.hasSuffix("/") ? root : root + "/")
    }

    /// The path with every link followed, if all of it exists.
    private static func realPath(_ path: String) -> String? {
        guard let resolved = realpath(path, nil) else { return nil }
        defer { free(resolved) }
        return String(cString: resolved)
    }
}

/// Saving a copy of a file where the user said.
public enum FileSaving {
    public enum SaveError: Error, Equatable {
        /// There is no file to copy.
        case nothingToCopy
        /// The place to save to is a folder.
        case destinationIsAFolder
    }

    /// Copies `source` to `destination`. A file already there is replaced,
    /// which the save panel has asked the user about, and is still whole if
    /// the copy fails part way. Touches the disk: not for the main thread.
    public static func copy(_ source: URL, to destination: URL) throws {
        let files = FileManager.default
        var isFolder: ObjCBool = false
        guard files.fileExists(atPath: source.path, isDirectory: &isFolder), !isFolder.boolValue else {
            throw SaveError.nothingToCopy
        }
        guard files.fileExists(atPath: destination.path, isDirectory: &isFolder) else {
            do {
                try files.copyItem(at: source, to: destination)
            } catch {
                // Nothing was there before, so whatever is there now is half
                // a copy.
                try? files.removeItem(at: destination)
                throw error
            }
            return
        }
        guard !isFolder.boolValue else {
            throw SaveError.destinationIsAFolder
        }
        // Copy to one side first and swap the two, so the old file is there
        // until the new one is complete. The system names a folder the app
        // may use for this, on the destination's volume, sandboxed or not.
        let staging = try files.url(
            for: .itemReplacementDirectory, in: .userDomainMask, appropriateFor: destination, create: true)
        defer { try? files.removeItem(at: staging) }
        let staged = staging.appendingPathComponent(destination.lastPathComponent)
        try files.copyItem(at: source, to: staged)
        _ = try files.replaceItemAt(destination, withItemAt: staged)
    }

    /// A name fit to offer in a save panel: the last part of whatever was
    /// given, without control characters, and never empty. The name comes
    /// from outside (a message named the file) and must not steer the panel
    /// to another folder.
    public static func offeredName(_ name: String) -> String {
        let last = name.split(whereSeparator: { $0 == "/" || $0 == ":" }).last.map(String.init) ?? ""
        let clean = String(
            last.unicodeScalars.filter { !CharacterSet.controlCharacters.contains($0) })
        let trimmed = clean.trimmingCharacters(in: .whitespaces)
        return trimmed.isEmpty || trimmed == "." || trimmed == ".." ? "Untitled" : trimmed
    }
}
