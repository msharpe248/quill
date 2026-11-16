import Darwin
import Foundation
import Testing

@testable import Themes

struct ThemeImportTests {
    private func importer(in scratch: Scratch) -> ThemeImport {
        ThemeImport(configFolder: scratch.file("config"))
    }

    @Test func theThemesFolderIsBesideTheSettingsFile() throws {
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        #expect(importer.folder.path == scratch.url.path + "/config/themes")
        // Looking makes nothing.
        #expect(!FileManager.default.fileExists(atPath: scratch.file("config").path))
    }

    @Test func aThemeIsCopiedInUnderItsFilesName() throws {
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        let source = try scratch.write(Written.night, to: "ghostty/Night Owl+")
        #expect(try importer.importFile(source) == "Night Owl+")
        let copy = importer.folder.appendingPathComponent("Night Owl+")
        #expect(try String(contentsOf: copy, encoding: .utf8) == Written.night)
        // A copy: the file chosen is as it was.
        #expect(try String(contentsOf: source, encoding: .utf8) == Written.night)
        // Nothing else is left in the folder.
        #expect(try scratch.names(in: importer.folder) == ["Night Owl+"])
        // A theme somebody called `.toml` is filed under its name without.
        let named = try scratch.write(Written.paper, to: "elsewhere/Dawn.toml")
        #expect(ThemeImport.name(of: named) == "Dawn")
        #expect(try importer.importFile(named) == "Dawn")
        #expect(try scratch.names(in: importer.folder) == ["Dawn", "Night Owl+"])
    }

    @Test func theCopyIsTheUsersAlone() throws {
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        // Whatever the file chosen allowed, and Ghostty's are open to all.
        let source = try scratch.write(Written.night, to: "ghostty/Night", permissions: 0o644)
        try importer.importFile(source)
        #expect(try scratch.permissions(of: importer.folder.appendingPathComponent("Night")) == 0o600)
        #expect(try scratch.permissions(of: importer.folder) == 0o700)
        // A read-only file is copied to one that can be replaced later.
        let locked = try scratch.write(Written.paper, to: "ghostty/Locked", permissions: 0o444)
        try importer.importFile(locked)
        #expect(try scratch.permissions(of: importer.folder.appendingPathComponent("Locked")) == 0o600)
    }

    @Test func aThemesFolderThatIsThereIsLeftAsTheUserHasIt() throws {
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        try scratch.write("name = \"mine\"\n", to: "config/themes/mine.toml")
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: importer.folder.path)
        try importer.importFile(scratch.write(Written.night, to: "ghostty/Night"))
        #expect(try scratch.permissions(of: importer.folder) == 0o755)
        #expect(try scratch.names(in: importer.folder) == ["Night", "mine.toml"])
    }

    @Test func importingAgainReplacesTheCopy() throws {
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        try importer.importFile(scratch.write(Written.night, to: "one/Dusk"))
        let copy = importer.folder.appendingPathComponent("Dusk")
        // Open to all, as a copy made some other way might be.
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: copy.path)
        #expect(try importer.importFile(scratch.write(Written.paper, to: "two/Dusk")) == "Dusk")
        #expect(try String(contentsOf: copy, encoding: .utf8) == Written.paper)
        #expect(try scratch.permissions(of: copy) == 0o600)
        #expect(try scratch.names(in: importer.folder) == ["Dusk"])
    }

    @Test func whatIsNotAThemeIsRefusedAndNothingIsCopied() throws {
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        try importer.importFile(scratch.write(Written.night, to: "ghostty/Kept"))
        func refuse(_ source: URL, _ problem: ThemeImport.Problem) {
            _ = #expect(throws: problem, "\(source.lastPathComponent)") { try importer.importFile(source) }
        }
        try refuse(scratch.write("Remember the milk.\n", to: "x/notes"), .notATheme)
        try refuse(scratch.write("", to: "x/empty"), .notATheme)
        try refuse(
            scratch.write("name = \"mine\"\n[colors]\ntext = \"#112233\"\n", to: "x/mine.toml"), .notATheme)
        try refuse(
            scratch.write(Written.night + String(repeating: "#\n", count: 40_000), to: "x/Huge"),
            .file(.tooLarge))
        refuse(scratch.file("x/gone"), .file(.notAFile))
        refuse(scratch.file("x"), .file(.notAFile))
        refuse(URL(string: "https://example.com/theme")!, .file(.notAFile))
        try Data([0xff, 0xfe, 0x00, 0x41]).write(to: scratch.file("x/binary"))
        refuse(scratch.file("x/binary"), .file(.notText))
        // A name no theme can have: hidden, or with something unseen in it.
        try refuse(scratch.write(Written.night, to: "x/.hidden"), .badName(".hidden"))
        try refuse(scratch.write(Written.night, to: "x/bell\u{7}name"), .badName("bellname"))
        try refuse(scratch.write(Written.night, to: "x/a:b"), .badName("a:b"))
        // One of Quill's own themes has the name already, and would be
        // found first.
        try scratch.write("name = \"taken\"\n", to: "config/themes/Taken.toml")
        try refuse(scratch.write(Written.night, to: "x/Taken"), .taken("Taken"))
        // The copy that was there is as it was, and nothing was added.
        #expect(try scratch.names(in: importer.folder) == ["Kept", "Taken.toml"])
        #expect(
            try String(contentsOf: importer.folder.appendingPathComponent("Kept"), encoding: .utf8)
                == Written.night)
    }

    @Test func aLinkIsNeverWrittenThrough() throws {
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        // A link where the copy goes is replaced, and what it led to is
        // left alone.
        let elsewhere = try scratch.write("Mine, and not a theme.\n", to: "elsewhere/diary")
        try FileManager.default.createDirectory(at: importer.folder, withIntermediateDirectories: true)
        let copy = importer.folder.appendingPathComponent("Night")
        try FileManager.default.createSymbolicLink(at: copy, withDestinationURL: elsewhere)
        try importer.importFile(scratch.write(Written.night, to: "ghostty/Night"))
        #expect(try String(contentsOf: elsewhere, encoding: .utf8) == "Mine, and not a theme.\n")
        var status = stat()
        #expect(lstat(copy.path, &status) == 0 && status.st_mode & S_IFMT == S_IFREG)
        #expect(try String(contentsOf: copy, encoding: .utf8) == Written.night)

        // A themes folder that is a link to somewhere else is not written
        // into at all.
        let other = try Scratch()
        let linked = ThemeImport(configFolder: other.file("config"))
        try FileManager.default.createDirectory(at: other.file("config"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: other.file("somewhere"), withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: linked.folder, withDestinationURL: other.file("somewhere"))
        #expect(throws: ThemeImport.Problem.notAFolder) {
            try linked.importFile(other.write(Written.night, to: "ghostty/Night"))
        }
        #expect(try other.names(in: other.file("somewhere")).isEmpty)
        // Nor a file where the folder should be.
        let third = try Scratch()
        try third.write("not a folder\n", to: "config/themes")
        #expect(throws: ThemeImport.Problem.notAFolder) {
            try ThemeImport(configFolder: third.file("config")).importFile(
                third.write(Written.night, to: "ghostty/Night"))
        }
    }

    @Test func aThemeIsReadThroughALinkToIt() throws {
        // What the user chose may be a link to the file, as an alias is.
        let scratch = try Scratch()
        let importer = importer(in: scratch)
        let real = try scratch.write(Written.paper, to: "real/Paper")
        let link = scratch.file("Linked")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: real)
        #expect(try importer.importFile(link) == "Linked")
        #expect(
            try String(contentsOf: importer.folder.appendingPathComponent("Linked"), encoding: .utf8)
                == Written.paper)
    }

    @Test func problemsSayWhatIsWrong() {
        #expect(ThemeImport.Problem.notATheme.description.contains("names no background and foreground colour"))
        #expect(ThemeImport.Problem.file(.tooLarge).description == "the file is larger than 64 KB, and no theme is")
        #expect(ThemeImport.Problem.file(.notAFile).description == "that is not a file")
        #expect(ThemeImport.Problem.taken("Dusk").description.hasSuffix("in Dusk.toml"))
        #expect(ThemeImport.Problem.badName(".x").description.contains("cannot be a theme\u{2019}s name"))
        #expect(ThemeImport.Problem.notAFolder.description == "the themes folder is not a folder")
        #expect(ThemeImport.Problem.unwritable("Permission denied").description.contains("Permission denied"))
    }
}
