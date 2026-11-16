import Bridge
import Foundation
import Testing

@testable import Editor

/// The editors that come with macOS: the only programs outside the bundle
/// that the shell itself will start.
struct SystemEditorTests {
    private static let container = "/Users/robin/Library/Containers/dev.quill.Quill/Data"

    /// A bundle with the plugin in it, as `build-app.sh` makes. There is no
    /// Vim in one.
    private func bundleWithoutVim(_ scratch: Scratch) throws -> URL {
        try scratch.write("\" plugin\n", to: "Quill.app/\(BundledFile.pluginPath)")
        return scratch.file("Quill.app")
    }

    @Test func eachProgramIsAConstantInTheSystemsOwnFolder() {
        #expect(SystemEditor.path(of: .vim) == "/usr/bin/vim")
        #expect(SystemEditor.path(of: .nano) == "/usr/bin/nano")
        // The user's own Vim is not one of macOS's, and is not found here.
        #expect(SystemEditor.path(of: .script) == nil)
        #expect(SystemEditor.systemRoot == "/usr")
        let scratch = try? Scratch()
        if let scratch, let bundle = try? bundleWithoutVim(scratch) {
            #expect(throws: SystemEditor.Problem.notOneOfThem("script")) {
                try SystemEditor(.script, inBundle: bundle)
            }
            #expect(throws: SystemEditor.Problem.notOneOfThem("script")) {
                try SystemEditor(.script, inBundle: bundle, binary: "/usr/bin/vim", within: "/usr")
            }
        }
    }

    @Test func nanoNeedsNoPluginAndIsStartedToSaveAsItLeaves() throws {
        let scratch = try Scratch()
        // Not even a plugin in the bundle: nano has no use for one.
        try scratch.folder("Quill.app/Contents")
        let program = try scratch.write("#!/bin/sh\n", to: "usr/bin/nano", permissions: 0o755)
        let nano = try SystemEditor(
            .nano, inBundle: scratch.file("Quill.app"), binary: program.path, within: scratch.file("usr").path)
        #expect(nano.kind == .nano)
        #expect(nano.plugin == nil)
        #expect(EditorToRun.system(nano).kind == .nano)
        let dir = Self.container + "/tmp/quill-view-1/it's \"edit\" 1"
        let launch = EditorLaunch(
            editor: .system(nano),
            draft: try #require(
                PaneEditor(path: dir + "/message.quill", dir: dir, title: "Lunch", editor: .nano, line: 7)),
            // An imported vimrc is Vim's and no business of nano's.
            vimrc: URL(fileURLWithPath: Self.container + "/vim/vimrc"),
            term: "xterm-256color",
            home: Self.container,
            temporaryDirectory: Self.container + "/tmp/",
            base: ["SHELL": "/bin/zsh", "PATH": "/opt/homebrew/bin:/usr/bin", "HOME": "/Users/robin", "TZ": "UTC"]
        )
        #expect(launch.executable.path == program.path)
        #expect(!launch.userScript)
        // Save on leaving without asking, start where the body does, and
        // the draft: whole, last, and never an option.
        #expect(launch.arguments == ["-t", "+7", dir + "/message.quill"])
        // The same environment built from nothing, without what is Vim's.
        #expect(
            launch.environment == [
                "LANG": "en_US.UTF-8",
                "TZ": "UTC",
                "TERM": "xterm-256color",
                "HOME": Self.container,
                "TMPDIR": Self.container + "/tmp/",
                "SHELL": "/usr/bin/false",
            ])
        #expect(launch.workingDirectory.path == dir)
    }

    @Test func aProgramInTheSystemsFolderWithThePluginInTheBundleIsTaken() throws {
        let scratch = try Scratch()
        let bundle = try bundleWithoutVim(scratch)
        let program = try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o755)
        let vim = try SystemEditor(.vim, inBundle: bundle, binary: program.path, within: scratch.file("usr").path)
        #expect(vim.kind == .vim)
        #expect(EditorToRun.system(vim).kind == .vim)
        #expect(vim.binary.path == program.path)
        #expect(vim.plugin?.path == bundle.path + "/Contents/Resources/vim/quill.vim")
    }

    @Test func nothingThereAFolderOrAFileThatMayNotBeRunIsRefused() throws {
        let scratch = try Scratch()
        let bundle = try bundleWithoutVim(scratch)
        let root = scratch.file("usr").path
        try scratch.folder("usr/bin")
        let path = scratch.file("usr/bin/vim").path
        #expect(throws: SystemEditor.Problem.missing(path)) {
            try SystemEditor(.vim, inBundle: bundle, binary: path, within: root)
        }
        try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o644)
        #expect(throws: SystemEditor.Problem.unusable(path)) {
            try SystemEditor(.vim, inBundle: bundle, binary: path, within: root)
        }
        let folder = scratch.file("usr/bin/vi").path
        try scratch.folder("usr/bin/vi")
        #expect(throws: SystemEditor.Problem.unusable(folder)) {
            try SystemEditor(.vim, inBundle: bundle, binary: folder, within: root)
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
        #expect(throws: SystemEditor.Problem.notTheSystems(path)) {
            try SystemEditor(.vim, inBundle: bundle, binary: path, within: root)
        }
        // A folder whose name only starts the same is not inside it.
        let beside = try scratch.write("#!/bin/sh\n", to: "usr-local/bin/vim", permissions: 0o755)
        #expect(throws: SystemEditor.Problem.notTheSystems(beside.path)) {
            try SystemEditor(.vim, inBundle: bundle, binary: beside.path, within: root)
        }
        // A link that stays inside it is the system's own arrangement.
        let real = try scratch.write("#!/bin/sh\n", to: "usr/libexec/vim-9", permissions: 0o755)
        try scratch.link("usr/bin/vim9", to: real)
        #expect(throws: Never.self) {
            try SystemEditor(.vim, inBundle: bundle, binary: scratch.file("usr/bin/vim9").path, within: root)
        }
    }

    @Test func withoutThePluginInTheBundleThereIsNothingToRun() throws {
        let scratch = try Scratch()
        let program = try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o755)
        let root = scratch.file("usr").path
        try scratch.folder("Quill.app/Contents")
        #expect(throws: BundledFile.Problem.missing("Contents/Resources/vim/quill.vim")) {
            try SystemEditor(.vim, inBundle: scratch.file("Quill.app"), binary: program.path, within: root)
        }
        // Nor with a plugin that is a link to a file somewhere else.
        let elsewhere = try scratch.write("\" not ours\n", to: "elsewhere/quill.vim")
        try scratch.link("Quill.app/\(BundledFile.pluginPath)", to: elsewhere)
        #expect(throws: BundledFile.Problem.outsideBundle("Contents/Resources/vim/quill.vim")) {
            try SystemEditor(.vim, inBundle: scratch.file("Quill.app"), binary: program.path, within: root)
        }
    }

    @Test func itIsStartedInsideTheSandboxAndFindsItsOwnFiles() throws {
        let scratch = try Scratch()
        let bundle = try bundleWithoutVim(scratch)
        let program = try scratch.write("#!/bin/sh\n", to: "usr/bin/vim", permissions: 0o755)
        let vim = try SystemEditor(.vim, inBundle: bundle, binary: program.path, within: scratch.file("usr").path)
        let dir = Self.container + "/tmp/quill-view-1/edit-1"
        let launch = EditorLaunch(
            editor: .system(vim),
            draft: try #require(PaneEditor(path: dir + "/message.quill", dir: dir, title: "Lunch", editor: .vim)),
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
        #expect(SystemEditor.Problem.missing("/usr/bin/vim").description == "there is no /usr/bin/vim on this Mac")
        #expect(
            SystemEditor.Problem.unusable("/usr/bin/vim").description == "/usr/bin/vim is not a program that can be run"
        )
        #expect(
            SystemEditor.Problem.notTheSystems("/usr/bin/vim").description
                == "/usr/bin/vim leads to a program that is not macOS's own")
    }
}
