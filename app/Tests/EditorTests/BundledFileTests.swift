import Foundation
import Testing

@testable import Editor

/// Quill's own files for Vim are taken from the app's bundle and from
/// nowhere else.
struct BundledFileTests {
    @Test func aFileInTheBundleIsFound() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithPlugin()
        #expect(BundledFile.pluginPath == "Contents/Resources/vim/quill.vim")
        #expect(
            try BundledFile.find(BundledFile.pluginPath, inBundle: bundle).path
                == bundle.path + "/Contents/Resources/vim/quill.vim")
    }

    @Test func nothingThereOrAFolderThereIsRefused() throws {
        let scratch = try Scratch()
        try scratch.folder("Quill.app/Contents/Resources")
        let bundle = scratch.file("Quill.app")
        #expect(throws: BundledFile.Problem.missing("Contents/Resources/vim/quill.vim")) {
            try BundledFile.find(BundledFile.pluginPath, inBundle: bundle)
        }
        try scratch.folder("Quill.app/\(BundledFile.pluginPath)")
        #expect(throws: BundledFile.Problem.unusable("Contents/Resources/vim/quill.vim")) {
            try BundledFile.find(BundledFile.pluginPath, inBundle: bundle)
        }
        // And a bundle that is not there has nothing in it.
        #expect(throws: BundledFile.Problem.missing("Contents/Resources/vim/quill.vim")) {
            try BundledFile.find(BundledFile.pluginPath, inBundle: scratch.file("Nothing.app"))
        }
    }

    @Test func aLinkThatLeadsOutOfTheBundleIsRefused() throws {
        let scratch = try Scratch()
        let elsewhere = try scratch.write("\" not ours\n", to: "elsewhere/quill.vim")
        try scratch.link("Quill.app/\(BundledFile.pluginPath)", to: elsewhere)
        #expect(throws: BundledFile.Problem.outsideBundle("Contents/Resources/vim/quill.vim")) {
            try BundledFile.find(BundledFile.pluginPath, inBundle: scratch.file("Quill.app"))
        }
        // Nor one that leaves by way of a folder that is a link.
        try scratch.write("\" not ours\n", to: "other/vim/quill.vim")
        try scratch.link("Linked.app/Contents/Resources", to: scratch.file("other"))
        #expect(throws: BundledFile.Problem.outsideBundle("Contents/Resources/vim/quill.vim")) {
            try BundledFile.find(BundledFile.pluginPath, inBundle: scratch.file("Linked.app"))
        }
        // A folder whose name only starts the same is not the bundle.
        try scratch.write("\" not ours\n", to: "Quill.app.backup/quill.vim")
        try scratch.link("Near.app/\(BundledFile.pluginPath)", to: scratch.file("Quill.app.backup/quill.vim"))
        try scratch.folder("Near.app")
        #expect(throws: BundledFile.Problem.outsideBundle("Contents/Resources/vim/quill.vim")) {
            try BundledFile.find(BundledFile.pluginPath, inBundle: scratch.file("Near.app"))
        }
    }

    @Test func aLinkThatStaysInsideTheBundleIsAllowed() throws {
        let scratch = try Scratch()
        let real = try scratch.write("\" plugin\n", to: "Quill.app/Contents/Resources/shared/quill.vim")
        try scratch.link("Quill.app/\(BundledFile.pluginPath)", to: real)
        #expect(throws: Never.self) {
            try BundledFile.find(BundledFile.pluginPath, inBundle: scratch.file("Quill.app"))
        }
    }

    @Test func aBundleReachedThroughALinkStillHasItsFiles() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithPlugin("Real.app")
        try scratch.link("Quill.app", to: bundle)
        #expect(
            try BundledFile.find(BundledFile.pluginPath, inBundle: scratch.file("Quill.app")).path
                == scratch.file("Quill.app").path + "/Contents/Resources/vim/quill.vim")
    }

    @Test func problemsSayWhatIsWrong() {
        #expect(BundledFile.Problem.missing("x").description == "x is not in the app")
        #expect(BundledFile.Problem.unusable("x").description == "x is not what Vim needs there")
        #expect(BundledFile.Problem.outsideBundle("x").description == "x leads out of the app")
    }
}
