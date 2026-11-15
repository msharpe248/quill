import Foundation
import Testing

@testable import Editor

struct ImportedVimrcTests {
    private func store(in scratch: Scratch) -> ImportedVimrc {
        ImportedVimrc(folder: scratch.file("Application Support/Quill/vim"))
    }

    private func names(in folder: URL) throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: folder.path).sorted()
    }

    @Test func thereIsNoneUntilOneIsImported() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        #expect(store.current == nil)
        #expect(store.size == nil)
        #expect(store.file.path == scratch.url.path + "/Application Support/Quill/vim/vimrc")
        // Looking makes nothing.
        #expect(!FileManager.default.fileExists(atPath: scratch.file("Application Support").path))
    }

    @Test func aFileIsCopiedIntoQuillsOwnFolder() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        let source = try scratch.write("set number\nset expandtab\n", to: "home/.vimrc")
        #expect(try store.importFile(source) == 25)
        #expect(store.current == store.file)
        #expect(store.size == 25)
        #expect(try String(contentsOf: store.file, encoding: .utf8) == "set number\nset expandtab\n")
        // A copy: the user's file is as it was.
        #expect(try String(contentsOf: source, encoding: .utf8) == "set number\nset expandtab\n")
        #expect(try names(in: store.folder) == ["vimrc"])
    }

    @Test func theCopyIsTheUsersAlone() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        // Whatever the file chosen allowed.
        let source = try scratch.write("set number\n", to: "home/.vimrc", permissions: 0o666)
        try store.importFile(source)
        #expect(try scratch.permissions(of: store.file) == 0o600)
        #expect(try scratch.permissions(of: store.folder) == 0o700)

        // A read-only file is copied to one that can be replaced later.
        let locked = try scratch.write("set ruler\n", to: "home/locked", permissions: 0o444)
        try store.importFile(locked)
        #expect(try scratch.permissions(of: store.file) == 0o600)
        #expect(try String(contentsOf: store.file, encoding: .utf8) == "set ruler\n")
    }

    @Test func importingAgainReplacesTheCopy() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        try store.importFile(scratch.write("set number\n\" and a good deal more besides\n", to: "home/first"))
        // Open to all, as a copy made some other way might be.
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: store.file.path)
        #expect(try store.importFile(scratch.write("set ruler\n", to: "home/second")) == 10)
        #expect(try String(contentsOf: store.file, encoding: .utf8) == "set ruler\n")
        #expect(try scratch.permissions(of: store.file) == 0o600)
        // Nothing is left lying beside it.
        #expect(try names(in: store.folder) == ["vimrc"])
    }

    @Test func aFileOverOneMegabyteIsRefused() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        #expect(ImportedVimrc.largest == 1_048_576)

        // Exactly the limit is taken.
        let most = scratch.file("most")
        try Data(repeating: 0x22, count: ImportedVimrc.largest).write(to: most)
        #expect(try store.importFile(most) == 1_048_576)
        #expect(store.size == 1_048_576)

        // One byte more is not, and the copy already there is left alone.
        try store.importFile(scratch.write("set number\n", to: "small"))
        let tooMuch = scratch.file("too-much")
        try Data(repeating: 0x22, count: ImportedVimrc.largest + 1).write(to: tooMuch)
        #expect(throws: ImportedVimrc.Problem.tooLarge) { try store.importFile(tooMuch) }
        #expect(try String(contentsOf: store.file, encoding: .utf8) == "set number\n")
        #expect(try names(in: store.folder) == ["vimrc"])
    }

    @Test func aRefusedFirstImportLeavesNothingBehind() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        let tooMuch = scratch.file("too-much")
        try Data(repeating: 0x22, count: ImportedVimrc.largest + 1).write(to: tooMuch)
        #expect(throws: ImportedVimrc.Problem.tooLarge) { try store.importFile(tooMuch) }
        #expect(store.current == nil)
        #expect(!FileManager.default.fileExists(atPath: store.folder.path))
    }

    @Test func somethingThatHasNoEndIsNotRead() throws {
        // A device is not a file, however it is named; nothing is read
        // from it to find out.
        let scratch = try Scratch()
        let store = store(in: scratch)
        #expect(throws: ImportedVimrc.Problem.notAFile) {
            try store.importFile(URL(fileURLWithPath: "/dev/zero"))
        }
        try scratch.link("endless", to: URL(fileURLWithPath: "/dev/zero"))
        #expect(throws: ImportedVimrc.Problem.notAFile) { try store.importFile(scratch.file("endless")) }
        #expect(store.current == nil)
    }

    @Test func onlyAFileCanBeImported() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        #expect(throws: ImportedVimrc.Problem.notAFile) { try store.importFile(scratch.folder("a-folder")) }
        #expect(throws: ImportedVimrc.Problem.notAFile) { try store.importFile(scratch.file("not-there")) }
        #expect(throws: ImportedVimrc.Problem.notAFile) {
            try store.importFile(try #require(URL(string: "https://example.com/vimrc")))
        }
        let unreadable = try scratch.write("set number\n", to: "unreadable", permissions: 0o000)
        #expect(throws: ImportedVimrc.Problem.self) { try store.importFile(unreadable) }
        #expect(store.current == nil)
    }

    @Test func aLinkTheUserChoseIsFollowedToItsFile() throws {
        // Dotfiles are often links into a folder of them.
        let scratch = try Scratch()
        let store = store(in: scratch)
        try scratch.link("home/.vimrc", to: scratch.write("set number\n", to: "dotfiles/vimrc"))
        #expect(try store.importFile(scratch.file("home/.vimrc")) == 11)
        // What is kept is a file of its own, not a link.
        #expect(store.current == store.file)
        #expect(try String(contentsOf: store.file, encoding: .utf8) == "set number\n")
    }

    @Test func aLinkWhereTheCopyGoesIsNotACopyAndIsNotWrittenThrough() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        let elsewhere = try scratch.write("\" someone else's\n", to: "elsewhere/vimrc")
        try scratch.link("Application Support/Quill/vim/vimrc", to: elsewhere)
        // Vim is not started with it.
        #expect(store.current == nil)
        #expect(store.size == nil)
        // Importing puts a file where the link was, and leaves what the
        // link pointed at alone.
        try store.importFile(scratch.write("set number\n", to: "home/.vimrc"))
        #expect(store.current == store.file)
        #expect(try String(contentsOf: store.file, encoding: .utf8) == "set number\n")
        #expect(try String(contentsOf: elsewhere, encoding: .utf8) == "\" someone else's\n")
    }

    @Test func theCopyCanBeRemoved() throws {
        let scratch = try Scratch()
        let store = store(in: scratch)
        // Nothing to remove is not an error.
        #expect(try store.remove() == false)
        let source = try scratch.write("set number\n", to: "home/.vimrc")
        try store.importFile(source)
        #expect(try store.remove() == true)
        #expect(store.current == nil)
        #expect(try names(in: store.folder).isEmpty)
        #expect(try store.remove() == false)
        // The user's file was never Quill's to remove.
        #expect(FileManager.default.fileExists(atPath: source.path))
        // And one can be imported again.
        try store.importFile(source)
        #expect(store.current == store.file)
    }

    @Test func aPlaceThatCannotBeWrittenReportsFailure() throws {
        let scratch = try Scratch()
        let source = try scratch.write("set number\n", to: "home/.vimrc")
        // Something that is not a folder where the folder goes.
        try scratch.write("", to: "blocked")
        let blocked = ImportedVimrc(folder: scratch.file("blocked/vim"))
        #expect(throws: ImportedVimrc.Problem.self) { try blocked.importFile(source) }
        // A folder where the copy goes.
        let store = store(in: scratch)
        try scratch.folder("Application Support/Quill/vim/vimrc")
        #expect(throws: ImportedVimrc.Problem.self) { try store.importFile(source) }
        #expect(store.current == nil)
        #expect(try names(in: store.folder) == ["vimrc"])
    }

    @Test func theFolderIsQuillsOwnOrTheScratchRunsOwn() {
        let container = "/Users/robin/Library/Containers/dev.quill.Quill/Data"
        let support = URL(fileURLWithPath: container + "/Library/Application Support")
        #expect(
            ImportedVimrc.folder(applicationSupport: support, configFolder: nil).path
                == container + "/Library/Application Support/Quill/vim")
        // A run pointed at a scratch config folder keeps its vimrc there.
        #expect(
            ImportedVimrc.folder(applicationSupport: support, configFolder: "/tmp/scratch/config").path
                == "/tmp/scratch/config/vim")
        // Something that is not a place is not taken for one.
        #expect(
            ImportedVimrc.folder(applicationSupport: support, configFolder: "").path
                == support.path + "/Quill/vim")
        #expect(
            ImportedVimrc.folder(applicationSupport: support, configFolder: "relative/config").path
                == support.path + "/Quill/vim")
    }

    @Test func problemsSayWhatIsWrong() {
        #expect(ImportedVimrc.Problem.tooLarge.description == "the file is larger than 1 MB")
        #expect(ImportedVimrc.Problem.notAFile.description == "that is not a file")
    }
}
