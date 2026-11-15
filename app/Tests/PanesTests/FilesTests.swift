import Foundation
import Testing

@testable import Panes

struct FileSavingTests {
    @Test func aCopyIsSavedUnderANewName() throws {
        let scratch = try Scratch()
        let source = try scratch.write("the report", to: "source.txt")
        let destination = scratch.file("Report.txt")
        try FileSaving.copy(source, to: destination)
        #expect(try String(contentsOf: destination, encoding: .utf8) == "the report")
        // A copy: the original is still there.
        #expect(try String(contentsOf: source, encoding: .utf8) == "the report")
    }

    @Test func anExistingFileIsReplaced() throws {
        let scratch = try Scratch()
        let source = try scratch.write("new", to: "source.txt")
        let destination = try scratch.write("old and rather longer", to: "Report.txt")
        try FileSaving.copy(source, to: destination)
        #expect(try String(contentsOf: destination, encoding: .utf8) == "new")
        // Nothing is left lying beside it.
        let names = try FileManager.default.contentsOfDirectory(atPath: scratch.url.path).sorted()
        #expect(names == ["Report.txt", "source.txt"])
    }

    @Test func aPlaceThatCannotBeWrittenReportsFailure() throws {
        let scratch = try Scratch()
        let source = try scratch.write("the report", to: "source.txt")
        // A folder that is not there.
        #expect(throws: (any Error).self) {
            try FileSaving.copy(source, to: scratch.file("no-such-folder/Report.txt"))
        }
        // A folder that may not be written in.
        let locked = scratch.file("locked")
        try FileManager.default.createDirectory(at: locked, withIntermediateDirectories: false)
        try FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: locked.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: locked.path) }
        #expect(throws: (any Error).self) {
            try FileSaving.copy(source, to: locked.appendingPathComponent("Report.txt"))
        }
        #expect(!FileManager.default.fileExists(atPath: locked.appendingPathComponent("Report.txt").path))
    }

    @Test func aFileThatCannotBeReplacedIsLeftAsItWas() throws {
        let scratch = try Scratch()
        let source = try scratch.write("new", to: "source.txt")
        let locked = scratch.file("locked")
        try FileManager.default.createDirectory(at: locked, withIntermediateDirectories: false)
        let destination = locked.appendingPathComponent("Report.txt")
        try Data("old".utf8).write(to: destination)
        try FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: locked.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: locked.path) }
        #expect(throws: (any Error).self) {
            try FileSaving.copy(source, to: destination)
        }
        #expect(try String(contentsOf: destination, encoding: .utf8) == "old")
    }

    @Test func thereMustBeAFileToCopyAndAFileToCopyTo() throws {
        let scratch = try Scratch()
        let source = try scratch.write("the report", to: "source.txt")
        #expect(throws: FileSaving.SaveError.nothingToCopy) {
            try FileSaving.copy(scratch.file("missing.txt"), to: scratch.file("Report.txt"))
        }
        #expect(throws: FileSaving.SaveError.nothingToCopy) {
            try FileSaving.copy(scratch.url, to: scratch.file("Report.txt"))
        }
        #expect(!FileManager.default.fileExists(atPath: scratch.file("Report.txt").path))
        #expect(throws: FileSaving.SaveError.destinationIsAFolder) {
            try FileSaving.copy(source, to: scratch.url)
        }
    }

    @Test func theNameOfferedIsOnlyEverAName() {
        #expect(FileSaving.offeredName("Report.pdf") == "Report.pdf")
        #expect(FileSaving.offeredName("../../Library/Report.pdf") == "Report.pdf")
        #expect(FileSaving.offeredName("/etc/passwd") == "passwd")
        #expect(FileSaving.offeredName("Re\u{0}port\n.pdf") == "Report.pdf")
        #expect(FileSaving.offeredName("  ") == "Untitled")
        #expect(FileSaving.offeredName("..") == "Untitled")
        #expect(FileSaving.offeredName("a/") == "a")
        #expect(FileSaving.offeredName("résumé 📎.pdf") == "résumé 📎.pdf")
    }
}

struct TemporaryFolderTests {
    @Test func aFileInsideCountsWhetherOrNotItIsThereYet() throws {
        let scratch = try Scratch()
        let existing = try scratch.write("x", to: "there.txt")
        #expect(TemporaryFolder.contains(existing))
        #expect(TemporaryFolder.contains(scratch.file("not-yet.png")))
        #expect(TemporaryFolder.contains(existing, in: scratch.url))
        #expect(TemporaryFolder.contains(scratch.file("not-yet.png"), in: scratch.url))
    }

    @Test func whatIsOutsideDoesNot() throws {
        let scratch = try Scratch()
        let other = try Scratch()
        let outside = try other.write("x", to: "outside.txt")
        #expect(!TemporaryFolder.contains(outside, in: scratch.url))
        #expect(!TemporaryFolder.contains(other.file("not-yet.png"), in: scratch.url))
        #expect(!TemporaryFolder.contains(URL(fileURLWithPath: "/etc/passwd")))
        #expect(!TemporaryFolder.contains(URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("x.png")))
        // The folder itself is not a file in it.
        #expect(!TemporaryFolder.contains(scratch.url, in: scratch.url))
        // A folder whose name only starts the same.
        #expect(!TemporaryFolder.contains(URL(fileURLWithPath: scratch.url.path + "-more/x.png"), in: scratch.url))
        // The folder to write in must be there.
        #expect(!TemporaryFolder.contains(scratch.file("no-such-folder/x.png"), in: scratch.url))
        #expect(!TemporaryFolder.contains(try #require(URL(string: "https://example.com/x.png"))))
    }

    @Test func climbingOutOrFollowingALinkOutDoesNot() throws {
        let scratch = try Scratch()
        let other = try Scratch()
        let outside = try other.write("x", to: "outside.txt")
        let climbing = URL(fileURLWithPath: scratch.url.path + "/../\(other.url.lastPathComponent)/outside.txt")
        #expect(!TemporaryFolder.contains(climbing, in: scratch.url))
        #expect(!TemporaryFolder.contains(URL(fileURLWithPath: scratch.url.path + "/sub/.."), in: scratch.url))

        // A link to a folder outside, and a link to a file outside.
        try FileManager.default.createSymbolicLink(at: scratch.file("way-out"), withDestinationURL: other.url)
        #expect(!TemporaryFolder.contains(scratch.file("way-out/outside.txt"), in: scratch.url))
        #expect(!TemporaryFolder.contains(scratch.file("way-out/not-yet.png"), in: scratch.url))
        try FileManager.default.createSymbolicLink(at: scratch.file("link.txt"), withDestinationURL: outside)
        #expect(!TemporaryFolder.contains(scratch.file("link.txt"), in: scratch.url))
        // A link that stays inside is fine.
        let inside = try scratch.write("x", to: "inside.txt")
        try FileManager.default.createSymbolicLink(at: scratch.file("near.txt"), withDestinationURL: inside)
        #expect(TemporaryFolder.contains(scratch.file("near.txt"), in: scratch.url))
    }
}
