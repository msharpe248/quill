import Bridge
import Foundation
import Testing

@testable import Editor

/// The user's own script for their own Vim, and the two files of Quill's
/// that go with it.
struct UserEditorScriptTests {
    /// A bundle with the helper in it, as `build-app.sh` makes.
    private func bundle(_ scratch: Scratch) throws -> URL {
        try scratch.write("#!/bin/sh\n", to: "Quill.app/\(UserEditorScript.helperPath)", permissions: 0o755)
        try scratch.write("\" plugin\n", to: "Quill.app/\(BundledFile.pluginPath)")
        return scratch.file("Quill.app")
    }

    /// The files as they are in the repository, which is where the bundle's
    /// copies come from.
    private static let resources = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Resources/vim")

    private func run(_ program: URL, _ arguments: [String]) throws -> (status: Int32, output: String) {
        let process = Process()
        process.executableURL = program
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        process.standardInput = FileHandle.nullDevice
        // As macOS starts a script of the user's: with no TERM.
        process.environment = ["PATH": "/usr/bin:/bin"]
        try process.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (process.terminationStatus, String(decoding: data, as: UTF8.self))
    }

    @Test func theScriptHasOneNameInOneFolder() throws {
        #expect(UserEditorScript.name == "editor-command.sh")
        let scratch = try Scratch()
        let bundle = try bundle(scratch)
        let folder = try scratch.folder("Application Scripts/dev.quill.Quill")
        try scratch.write(
            "#!/bin/sh\n", to: "Application Scripts/dev.quill.Quill/editor-command.sh", permissions: 0o755)
        let script = try UserEditorScript(inScriptsFolder: folder, bundle: bundle)
        #expect(script.script.path == folder.path + "/editor-command.sh")
        #expect(script.helper.path == bundle.path + "/Contents/Resources/vim/on-terminal")
        #expect(try VimToRun(.script, inBundle: bundle, scriptsFolder: folder) == .script(script))
    }

    @Test func noScriptAFolderOrAFileThatMayNotBeRunIsRefused() throws {
        let scratch = try Scratch()
        let bundle = try bundle(scratch)
        let folder = try scratch.folder("scripts")
        let path = folder.path + "/editor-command.sh"
        #expect(throws: UserEditorScript.Problem.missing(path)) {
            try UserEditorScript(inScriptsFolder: folder, bundle: bundle)
        }
        // Another script in the folder is not this one.
        try scratch.write("#!/bin/sh\n", to: "scripts/other.sh", permissions: 0o755)
        #expect(throws: UserEditorScript.Problem.missing(path)) {
            try UserEditorScript(inScriptsFolder: folder, bundle: bundle)
        }
        try scratch.write("#!/bin/sh\n", to: "scripts/editor-command.sh", permissions: 0o644)
        #expect(throws: UserEditorScript.Problem.unusable(path)) {
            try UserEditorScript(inScriptsFolder: folder, bundle: bundle)
        }
        let other = try scratch.folder("folder")
        try scratch.folder("folder/editor-command.sh")
        #expect(throws: UserEditorScript.Problem.unusable(other.path + "/editor-command.sh")) {
            try UserEditorScript(inScriptsFolder: other, bundle: bundle)
        }
        // And with nowhere to look there is nothing to find.
        #expect(throws: UserEditorScript.Problem.self) {
            try VimToRun(.script, inBundle: bundle, scriptsFolder: nil)
        }
    }

    @Test func theHelperMustBeInTheBundle() throws {
        let scratch = try Scratch()
        let folder = try scratch.folder("scripts")
        try scratch.write("#!/bin/sh\n", to: "scripts/editor-command.sh", permissions: 0o755)
        try scratch.folder("Quill.app/Contents")
        #expect(throws: BundledFile.Problem.missing("Contents/Resources/vim/on-terminal")) {
            try UserEditorScript(inScriptsFolder: folder, bundle: scratch.file("Quill.app"))
        }
        let elsewhere = try scratch.write("#!/bin/sh\n", to: "elsewhere/on-terminal", permissions: 0o755)
        try scratch.link("Quill.app/\(UserEditorScript.helperPath)", to: elsewhere)
        #expect(throws: BundledFile.Problem.outsideBundle("Contents/Resources/vim/on-terminal")) {
            try UserEditorScript(inScriptsFolder: folder, bundle: scratch.file("Quill.app"))
        }
    }

    @Test func theScriptIsHandedTheHelperThenWhatVimIsStartedWith() throws {
        let scratch = try Scratch()
        let bundle = try bundle(scratch)
        let folder = try scratch.folder("scripts")
        try scratch.write("#!/bin/sh\n", to: "scripts/editor-command.sh", permissions: 0o755)
        let script = try UserEditorScript(inScriptsFolder: folder, bundle: bundle)
        let dir = "/Users/robin/Library/Containers/dev.quill.Quill/Data/tmp/quill-view-1/it's \"edit\" 1"
        let launch = VimLaunch(
            vim: .script(script),
            draft: try #require(PaneEditor(path: dir + "/message.quill", dir: dir, title: "Lunch", vim: .script)),
            // An imported vimrc is for a Vim inside the sandbox. This one
            // reads the user's own.
            vimrc: URL(fileURLWithPath: "/Users/robin/Library/Containers/dev.quill.Quill/Data/vim/vimrc"),
            term: "xterm-256color",
            home: "/Users/robin/Library/Containers/dev.quill.Quill/Data",
            temporaryDirectory: "/Users/robin/Library/Containers/dev.quill.Quill/Data/tmp/",
            base: ["VIMINIT": "x", "PATH": "/opt/homebrew/bin", "LANG": "de_DE.UTF-8"]
        )
        #expect(launch.userScript)
        #expect(launch.executable.path == folder.path + "/editor-command.sh")
        #expect(
            launch.arguments == [
                bundle.path + "/Contents/Resources/vim/on-terminal",
                "-N",
                "--cmd", "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END",
                "-c", "let g:quill_dir = expand('%:p:h') | execute 'source' fnameescape(g:quill_dir . '/quill.vim')",
                "--",
                dir + "/message.quill",
            ])
        // The draft's path is one whole argument and in no other, and no
        // command Vim is given has a path in it.
        for argument in launch.arguments.dropLast() {
            #expect(!argument.contains(dir))
        }
        // macOS decides the environment of a script it runs.
        #expect(launch.environment.isEmpty)
        // The Mac's own Vim is not run that way.
        let system = try scratch.systemVim(inBundle: bundle)
        #expect(
            !VimLaunch(
                vim: .system(system), draft: try #require(PaneEditor(path: "/d/m", dir: "/d", title: "")),
                vimrc: nil, term: "x", home: "/h", temporaryDirectory: "/t", base: [:]
            ).userScript)
    }

    @Test func theHelperRunsWhatItIsGivenAndSaysHowItEnded() throws {
        let helper = Self.resources.appendingPathComponent("on-terminal")
        // With no terminal to take, as here, the program still runs, with
        // its arguments whole and the TERM of Quill's panes.
        let said = try run(
            helper, ["/bin/sh", "-c", "printf '%s|%s|%s' \"$TERM\" \"$1\" \"$2\"", "sh", "a b", "it's \"x\""])
        #expect(said.output == "xterm-256color|a b|it's \"x\"")
        #expect(said.status == 0)
        #expect(try run(helper, ["/bin/sh", "-c", "exit 7"]).status == 7)
        #expect(try run(helper, ["/bin/sh", "-c", "kill -TERM $$"]).status == 128 + 15)
        let missing = try run(helper, ["/no/such/program"])
        #expect(missing.status == 127)
        #expect(missing.output.contains("cannot run /no/such/program"))
        let usage = try run(helper, [])
        #expect(usage.status == 64)
        #expect(usage.output.contains("usage: on-terminal"))
    }

    @Test func theSampleScriptPassesEverythingOnToAVim() throws {
        let sample = Self.resources.appendingPathComponent("editor-command.sh")
        #expect(FileManager.default.isExecutableFile(atPath: sample.path))
        // With `echo` standing in for the helper, what it would have run is
        // printed: a Vim by its full path, then the arguments as they came.
        let said = try run(sample, ["/bin/echo", "-N", "--", "/tmp/a draft/message.quill"])
        #expect(said.status == 0)
        let words = said.output.trimmingCharacters(in: .newlines)
        #expect(words.hasSuffix("/vim -N -- /tmp/a draft/message.quill"), "\(words)")
        #expect(words.hasPrefix("/"))
    }
}
