import Darwin
import Foundation

@testable import Themes

/// A folder of the test's own inside the temporary folder, removed with it.
final class Scratch {
    let url: URL

    init() throws {
        let base = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true).resolvingSymlinksInPath()
        url = base.appendingPathComponent("quill-themes-\(UUID().uuidString.prefix(8))", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    deinit {
        try? FileManager.default.removeItem(at: url)
    }

    func file(_ name: String) -> URL {
        url.appendingPathComponent(name)
    }

    @discardableResult
    func write(_ text: String, to name: String, permissions: Int = 0o644) throws -> URL {
        let file = file(name)
        try FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data(text.utf8).write(to: file)
        try FileManager.default.setAttributes([.posixPermissions: permissions], ofItemAtPath: file.path)
        return file
    }

    func permissions(of file: URL) throws -> Int {
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        return (attributes[.posixPermissions] as? Int) ?? -1
    }

    func names(in folder: URL) throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted()
    }
}

/// Themes in Ghostty's format, written here: a dark one and a light one.
enum Written {
    static let night = """
        palette = 0=#1b1d2a
        palette = 1=#f0606a
        palette = 2=#6fd08c
        palette = 3=#e8c46a
        palette = 4=#7aa0f0
        palette = 5=#c58af0
        palette = 6=#6fd0e0
        palette = 7=#c8cce0
        background = #1e2030
        foreground = #d0d4ea
        cursor-color = #d0d4ea
        selection-background = #3a3f5c
        selection-foreground = #ffffff

        """
    static let paper = "background = #fbf6e8\nforeground = #5a6068\npalette = 4=#2a5db0\n"
}
