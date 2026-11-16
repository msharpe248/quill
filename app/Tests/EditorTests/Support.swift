import Darwin
import Foundation

@testable import Editor

/// A folder of the test's own inside the temporary folder, removed with it.
final class Scratch {
    let url: URL

    init() throws {
        // With links followed: a bundle is compared with where its parts
        // really are, and the temporary folder is itself reached by a link.
        let base = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true).resolvingSymlinksInPath()
        url = base.appendingPathComponent("quill-editor-\(UUID().uuidString.prefix(8))", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    deinit {
        try? FileManager.default.removeItem(at: url)
    }

    func file(_ name: String) -> URL {
        url.appendingPathComponent(name)
    }

    @discardableResult
    func folder(_ name: String) throws -> URL {
        let folder = url.appendingPathComponent(name, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
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

    func link(_ name: String, to destination: URL) throws {
        let link = file(name)
        try FileManager.default.createDirectory(
            at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: destination)
    }

    /// Something shaped like an app bundle, called `name`, with what Quill
    /// puts in one for Vim: the plugin, and the helper a script is handed.
    func bundleWithPlugin(_ name: String = "Quill.app") throws -> URL {
        try write("\" plugin\n", to: "\(name)/\(BundledFile.pluginPath)")
        try write("#!/bin/sh\n", to: "\(name)/\(UserEditorScript.helperPath)", permissions: 0o755)
        return file(name)
    }

    /// Where the stand-in for the Mac's own Vim is: a file that may be run
    /// (it never is), in a folder standing in for `/usr`.
    var systemVimPath: String { file("usr/bin/vim").path }

    /// The Mac's own Vim, as far as a test has one, with the plugin from
    /// `bundle`.
    func systemVim(inBundle bundle: URL) throws -> SystemEditor {
        try write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o755)
        return try SystemEditor(.vim, inBundle: bundle, binary: systemVimPath, within: file("usr").path)
    }

    func permissions(of file: URL) throws -> Int {
        let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
        return (attributes[.posixPermissions] as? NSNumber)?.intValue ?? -1
    }
}
