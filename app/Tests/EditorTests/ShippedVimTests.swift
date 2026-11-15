import Foundation
import Testing

@testable import Editor

struct ShippedVimTests {
    @Test func aBundleWithAVimInItHasOne() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        let vim = try ShippedVim(inBundle: bundle)
        #expect(vim.binary.path == bundle.path + "/Contents/Helpers/vim")
        #expect(vim.home.path == bundle.path + "/Contents/Resources/vim")
        #expect(vim.runtime.path == bundle.path + "/Contents/Resources/vim/runtime")
        #expect(vim.plugin.path == bundle.path + "/Contents/Resources/vim/quill.vim")
    }

    @Test func aBundleWithoutOneHasNone() throws {
        let scratch = try Scratch()
        // As the app is built before `build-vim.sh` has been run: the
        // plugin is there, Vim is not.
        try scratch.write("\" plugin\n", to: "Quill.app/\(ShippedVim.pluginPath)")
        #expect(throws: ShippedVim.Problem.missing("Contents/Helpers/vim")) {
            try ShippedVim(inBundle: scratch.file("Quill.app"))
        }
        // No bundle at all.
        #expect(throws: ShippedVim.Problem.missing("Contents/Helpers/vim")) {
            try ShippedVim(inBundle: scratch.file("Nothing.app"))
        }
    }

    @Test func aVimSomewhereElseOnTheMacIsNeverUsed() throws {
        // Whatever this Mac has in /usr/bin or on the PATH, an empty bundle
        // has no Vim.
        let scratch = try Scratch()
        try scratch.folder("Quill.app/Contents/Helpers")
        try scratch.folder("Quill.app/Contents/Resources")
        #expect(throws: ShippedVim.Problem.self) {
            try ShippedVim(inBundle: scratch.file("Quill.app"))
        }
    }

    @Test func everyPartHasToBeThere() throws {
        for (missing, problem) in [
            (ShippedVim.runtimePath, ShippedVim.Problem.missing("Contents/Resources/vim/runtime")),
            (ShippedVim.pluginPath, ShippedVim.Problem.missing("Contents/Resources/vim/quill.vim")),
            (ShippedVim.binaryPath, ShippedVim.Problem.missing("Contents/Helpers/vim")),
        ] {
            let scratch = try Scratch()
            let bundle = try scratch.bundleWithVim()
            try FileManager.default.removeItem(at: bundle.appendingPathComponent(missing))
            #expect(throws: problem) { try ShippedVim(inBundle: bundle) }
        }
    }

    @Test func aFileThatMayNotBeRunIsNotAVim() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o644], ofItemAtPath: bundle.appendingPathComponent(ShippedVim.binaryPath).path)
        #expect(throws: ShippedVim.Problem.unusable("Contents/Helpers/vim")) {
            try ShippedVim(inBundle: bundle)
        }
    }

    @Test func theWrongKindOfThingIsNotAVim() throws {
        // A folder where the program goes. A folder can be "run" as far as
        // its permissions say, and is still not a program.
        var scratch = try Scratch()
        var bundle = try scratch.bundleWithVim()
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.binaryPath))
        try scratch.folder("Quill.app/\(ShippedVim.binaryPath)")
        #expect(throws: ShippedVim.Problem.unusable("Contents/Helpers/vim")) {
            try ShippedVim(inBundle: bundle)
        }

        // A file where the runtime folder goes.
        scratch = try Scratch()
        bundle = try scratch.bundleWithVim()
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.runtimePath))
        try scratch.write("", to: "Quill.app/\(ShippedVim.runtimePath)")
        #expect(throws: ShippedVim.Problem.unusable("Contents/Resources/vim/runtime")) {
            try ShippedVim(inBundle: bundle)
        }

        // A folder where the plugin goes.
        scratch = try Scratch()
        bundle = try scratch.bundleWithVim()
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.pluginPath))
        try scratch.folder("Quill.app/\(ShippedVim.pluginPath)")
        #expect(throws: ShippedVim.Problem.unusable("Contents/Resources/vim/quill.vim")) {
            try ShippedVim(inBundle: bundle)
        }
    }

    @Test func aLinkThatLeadsOutOfTheBundleIsRefused() throws {
        // The program is a link to one outside.
        var scratch = try Scratch()
        var bundle = try scratch.bundleWithVim()
        let outside = try scratch.write("#!/bin/sh\n", to: "elsewhere/vim", permissions: 0o755)
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.binaryPath))
        try scratch.link("Quill.app/\(ShippedVim.binaryPath)", to: outside)
        #expect(throws: ShippedVim.Problem.outsideBundle("Contents/Helpers/vim")) {
            try ShippedVim(inBundle: bundle)
        }

        // The folder the program is in is a link to one outside, so the
        // program is outside though nothing called `vim` is a link.
        scratch = try Scratch()
        bundle = try scratch.bundleWithVim()
        try scratch.write("#!/bin/sh\n", to: "elsewhere/vim", permissions: 0o755)
        try FileManager.default.removeItem(at: bundle.appendingPathComponent("Contents/Helpers"))
        try scratch.link("Quill.app/Contents/Helpers", to: scratch.file("elsewhere"))
        #expect(throws: ShippedVim.Problem.outsideBundle("Contents/Helpers/vim")) {
            try ShippedVim(inBundle: bundle)
        }

        // The runtime is a link to a folder outside.
        scratch = try Scratch()
        bundle = try scratch.bundleWithVim()
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.runtimePath))
        try scratch.link("Quill.app/\(ShippedVim.runtimePath)", to: scratch.folder("elsewhere/runtime"))
        #expect(throws: ShippedVim.Problem.outsideBundle("Contents/Resources/vim/runtime")) {
            try ShippedVim(inBundle: bundle)
        }

        // The plugin is a link to a file outside.
        scratch = try Scratch()
        bundle = try scratch.bundleWithVim()
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.pluginPath))
        try scratch.link(
            "Quill.app/\(ShippedVim.pluginPath)", to: scratch.write("\" other\n", to: "elsewhere/quill.vim"))
        #expect(throws: ShippedVim.Problem.outsideBundle("Contents/Resources/vim/quill.vim")) {
            try ShippedVim(inBundle: bundle)
        }

        // A bundle beside this one whose name starts the same is outside.
        scratch = try Scratch()
        bundle = try scratch.bundleWithVim()
        let neighbour = try scratch.write("#!/bin/sh\n", to: "Quill.app.old/vim", permissions: 0o755)
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.binaryPath))
        try scratch.link("Quill.app/\(ShippedVim.binaryPath)", to: neighbour)
        #expect(throws: ShippedVim.Problem.outsideBundle("Contents/Helpers/vim")) {
            try ShippedVim(inBundle: bundle)
        }
    }

    @Test func aLinkThatStaysInsideTheBundleIsAllowed() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        let real = try scratch.write("#!/bin/sh\n", to: "Quill.app/Contents/Resources/vim/bin/vim", permissions: 0o755)
        try FileManager.default.removeItem(at: bundle.appendingPathComponent(ShippedVim.binaryPath))
        try scratch.link("Quill.app/\(ShippedVim.binaryPath)", to: real)
        let vim = try ShippedVim(inBundle: bundle)
        // Named by where it goes in the bundle, not by where the link leads.
        #expect(vim.binary.path == bundle.path + "/Contents/Helpers/vim")
    }

    @Test func aBundleReachedThroughALinkStillHasItsVim() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim("Real.app")
        try scratch.link("Quill.app", to: bundle)
        let vim = try ShippedVim(inBundle: scratch.file("Quill.app"))
        #expect(vim.binary.path == scratch.file("Quill.app").path + "/Contents/Helpers/vim")
    }

    @Test func problemsSayWhatIsWrong() {
        #expect(
            ShippedVim.Problem.missing("Contents/Helpers/vim").description == "Contents/Helpers/vim is not in the app")
        #expect(
            ShippedVim.Problem.unusable("Contents/Helpers/vim").description
                == "Contents/Helpers/vim is not what Vim needs there")
        #expect(
            ShippedVim.Problem.outsideBundle("Contents/Helpers/vim").description
                == "Contents/Helpers/vim leads out of the app")
    }
}
