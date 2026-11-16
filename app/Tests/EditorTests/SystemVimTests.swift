import Bridge
import Foundation
import Testing

@testable import Editor

/// The Vim that comes with macOS: the one program outside the bundle that
/// the shell will start, and only that one.
struct SystemVimTests {
    private static let container = "/Users/robin/Library/Containers/dev.quill.Quill/Data"

    /// A bundle with the plugin in it and no Vim, as `build-app.sh` makes
    /// when `build-vim.sh` has not been run.
    private func bundleWithoutVim(_ scratch: Scratch) throws -> URL {
        try scratch.write("\" plugin\n", to: "Quill.app/\(ShippedVim.pluginPath)")
        return scratch.file("Quill.app")
    }

    @Test func theProgramIsAConstantInTheSystemsOwnFolder() {
        #expect(SystemVim.path == "/usr/bin/vim")
        #expect(SystemVim.systemRoot == "/usr")
    }

    @Test func aProgramInTheSystemsFolderWithThePluginInTheBundleIsTaken() throws {
        let scratch = try Scratch()
        let bundle = try bundleWithoutVim(scratch)
        let program = try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o755)
        let vim = try SystemVim(inBundle: bundle, binary: program.path, within: scratch.file("usr").path)
        #expect(vim.binary.path == program.path)
        #expect(vim.plugin.path == bundle.path + "/Contents/Resources/vim/quill.vim")
        // The bundle has no Vim of its own, and that is still so.
        #expect(throws: ShippedVim.Problem.missing("Contents/Helpers/vim")) { try ShippedVim(inBundle: bundle) }
        #expect(throws: ShippedVim.Problem.self) { try VimToRun(.bundled, inBundle: bundle) }
    }

    @Test func nothingThereAFolderOrAFileThatMayNotBeRunIsRefused() throws {
        let scratch = try Scratch()
        let bundle = try bundleWithoutVim(scratch)
        let root = scratch.file("usr").path
        try scratch.folder("usr/bin")
        let path = scratch.file("usr/bin/vim").path
        #expect(throws: SystemVim.Problem.missing(path)) {
            try SystemVim(inBundle: bundle, binary: path, within: root)
        }
        try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o644)
        #expect(throws: SystemVim.Problem.unusable(path)) {
            try SystemVim(inBundle: bundle, binary: path, within: root)
        }
        let folder = scratch.file("usr/bin/vi").path
        try scratch.folder("usr/bin/vi")
        #expect(throws: SystemVim.Problem.unusable(folder)) {
            try SystemVim(inBundle: bundle, binary: folder, within: root)
        }
    }

    @Test func aLinkToAProgramThatIsNotTheSystemsIsRefused() throws {
        let scratch = try Scratch()
        let bundle = try bundleWithoutVim(scratch)
        let root = scratch.file("usr").path
        try scratch.folder("usr/bin")
        let other = try scratch.write("#!/bin/sh\n", to: "opt/homebrew/bin/vim", permissions: 0o755)
        try scratch.link("usr/bin/vim", to: other)
        let path = scratch.file("usr/bin/vim").path
        #expect(throws: SystemVim.Problem.notTheSystems(path)) {
            try SystemVim(inBundle: bundle, binary: path, within: root)
        }
        // A folder whose name only starts the same is not inside it.
        let beside = try scratch.write("#!/bin/sh\n", to: "usr-local/bin/vim", permissions: 0o755)
        #expect(throws: SystemVim.Problem.notTheSystems(beside.path)) {
            try SystemVim(inBundle: bundle, binary: beside.path, within: root)
        }
        // A link that stays inside it is the system's own arrangement.
        let real = try scratch.write("#!/bin/sh\n", to: "usr/libexec/vim-9", permissions: 0o755)
        try scratch.link("usr/bin/vim9", to: real)
        #expect(throws: Never.self) {
            try SystemVim(inBundle: bundle, binary: scratch.file("usr/bin/vim9").path, within: root)
        }
    }

    @Test func withoutThePluginInTheBundleThereIsNothingToRun() throws {
        let scratch = try Scratch()
        let program = try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o755)
        let root = scratch.file("usr").path
        try scratch.folder("Quill.app/Contents")
        #expect(throws: ShippedVim.Problem.missing("Contents/Resources/vim/quill.vim")) {
            try SystemVim(inBundle: scratch.file("Quill.app"), binary: program.path, within: root)
        }
        // Nor with a plugin that is a link to a file somewhere else.
        let elsewhere = try scratch.write("\" not ours\n", to: "elsewhere/quill.vim")
        try scratch.link("Quill.app/\(ShippedVim.pluginPath)", to: elsewhere)
        #expect(throws: ShippedVim.Problem.outsideBundle("Contents/Resources/vim/quill.vim")) {
            try SystemVim(inBundle: scratch.file("Quill.app"), binary: program.path, within: root)
        }
    }

    @Test func itIsStartedAsTheShippedOneIsButFindsItsOwnFiles() throws {
        let scratch = try Scratch()
        let bundle = try bundleWithoutVim(scratch)
        let program = try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o755)
        let vim = try SystemVim(inBundle: bundle, binary: program.path, within: scratch.file("usr").path)
        let dir = Self.container + "/tmp/quill-view-1/edit-1"
        let launch = VimLaunch(
            vim: .system(vim),
            draft: try #require(PaneEditor(path: dir + "/message.quill", dir: dir, title: "Lunch", vim: .system)),
            vimrc: nil,
            term: "xterm-256color",
            home: Self.container,
            temporaryDirectory: Self.container + "/tmp/",
            base: [
                "VIM": "/opt/homebrew/share/vim", "VIMRUNTIME": "/opt/homebrew/share/vim/vim92",
                "VIMINIT": "source /Users/robin/.vimrc", "SHELL": "/bin/zsh", "PATH": "/opt/homebrew/bin:/usr/bin",
                "HOME": "/Users/robin", "QUILL_VIM_PLUGIN": "/somewhere/else.vim", "LANG": "de_DE.UTF-8",
            ]
        )
        #expect(launch.executable.path == program.path)
        #expect(
            launch.arguments == [
                "-N", "-u", "DEFAULTS", "-i", "NONE", "-n",
                "--cmd", "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END",
                "-c", "execute 'source' fnameescape($QUILL_VIM_PLUGIN)", "--",
                dir + "/message.quill",
            ])
        // Its own files are its own to find: no VIM, no VIMRUNTIME. The
        // plugin is Quill's, from the bundle, and nothing else of the
        // shell's environment gets through.
        #expect(
            launch.environment == [
                "LANG": "de_DE.UTF-8",
                "TERM": "xterm-256color",
                "HOME": Self.container,
                "TMPDIR": Self.container + "/tmp/",
                "SHELL": "/usr/bin/false",
                "QUILL_VIM_PLUGIN": bundle.path + "/Contents/Resources/vim/quill.vim",
                "QUILL_DRAFT_DIR": dir,
            ])
        #expect(launch.workingDirectory.path == dir)
    }

    @Test func problemsSayWhatIsWrong() {
        #expect(SystemVim.Problem.missing("/usr/bin/vim").description == "there is no /usr/bin/vim on this Mac")
        #expect(
            SystemVim.Problem.unusable("/usr/bin/vim").description == "/usr/bin/vim is not a program that can be run")
        #expect(
            SystemVim.Problem.notTheSystems("/usr/bin/vim").description
                == "/usr/bin/vim leads to a program that is not macOS's own")
    }
}
