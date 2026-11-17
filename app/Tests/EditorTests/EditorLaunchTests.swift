import Bridge
import Foundation
import Testing

@testable import Editor

/// How an editor is started: the core's command as it came, in the user's
/// own environment, and nothing of the shell's making but where the draft is.
struct EditorLaunchTests {
    private let folder = "/Users/robin/Library/Caches/Quill/edit 812"

    private func request(_ argv: [String], editor: EditorKind = .vim) throws -> PaneEditor {
        try #require(PaneEditor(dir: folder, title: "Re: Q4 roadmap", editor: editor, argv: argv))
    }

    private let inherited = [
        "HOME": "/Users/robin", "USER": "robin", "SHELL": "/bin/zsh", "PATH": "/usr/bin:/bin:/opt/homebrew/bin",
        "LANG": "en_GB.UTF-8", "TERM": "dumb", "VIMINIT": "source /Users/robin/.config/vim/vimrc",
    ]

    @Test func theCommandIsRunAsTheCoreGaveIt() throws {
        let argv = [
            "/opt/homebrew/bin/nvim", "-N", "--cmd", "augroup QuillNoModelines | augroup END", "-c",
            "execute 'source' fnameescape($QUILL_DRAFT_DIR . '/quill.vim')", "--", folder + "/message.quill",
        ]
        let launch = EditorLaunch(request: try request(argv), term: "xterm-256color", base: inherited)
        #expect(launch.executable.path == "/opt/homebrew/bin/nvim")
        #expect(launch.arguments == Array(argv.dropFirst()))
        #expect(launch.argv == argv)
        // In the draft's own folder, spaces and all: a value, never parsed.
        #expect(launch.workingDirectory.path == folder)
    }

    @Test func theEditorIsTheUsersOwnInTheirOwnEnvironment() throws {
        let launch = EditorLaunch(
            request: try request(["/usr/bin/vim", folder + "/message.quill"]), term: "xterm-256color",
            base: inherited)
        // Their home, so their own vimrc; their shell, their language, and
        // whatever else they have set for their editor.
        for name in ["HOME", "USER", "SHELL", "PATH", "LANG", "VIMINIT"] {
            #expect(launch.environment[name] == inherited[name], "\(name)")
        }
        // The terminal is Quill's own, whatever the app was started on.
        #expect(launch.environment["TERM"] == "xterm-256color")
        #expect(launch.environment["COLORTERM"] == "truecolor")
        // And where the draft is, for Quill's plugin.
        #expect(EditorLaunch.draftFolderVariable == "QUILL_DRAFT_DIR")
        #expect(launch.environment["QUILL_DRAFT_DIR"] == folder)
        #expect(launch.environment.count == inherited.count + 2)
    }

    @Test func anEditorThatIsNotVimIsStartedTheSameWay() throws {
        let nano = EditorLaunch(
            request: try request(["/usr/bin/nano", "-t", "+1", folder + "/message.quill"], editor: .nano),
            term: "xterm-256color", base: [:])
        #expect(nano.argv == ["/usr/bin/nano", "-t", "+1", folder + "/message.quill"])
        #expect(nano.environment == ["TERM": "xterm-256color", "COLORTERM": "truecolor", "QUILL_DRAFT_DIR": folder])
    }
}

/// The `PATH` the app's programs are found by: the one it was started with,
/// and Homebrew's folders after it.
struct ProgramEnvironmentTests {
    private let both: (String) -> Bool = { _ in true }
    private let neither: (String) -> Bool = { _ in false }

    @Test func homebrewsFoldersComeAfterTheMacsOwnWhenTheyAreThere() {
        // As the Dock starts an app: the Mac's own PATH.
        #expect(
            ProgramEnvironment.path(inherited: "/usr/bin:/bin:/usr/sbin:/sbin", isFolder: both)
                == "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin")
        #expect(
            ProgramEnvironment.path(inherited: "/usr/bin:/bin:/usr/sbin:/sbin", isFolder: neither)
                == "/usr/bin:/bin:/usr/sbin:/sbin")
        // Only the one this Mac has.
        #expect(
            ProgramEnvironment.path(inherited: "/usr/bin:/bin", isFolder: { $0 == "/opt/homebrew/bin" })
                == "/usr/bin:/bin:/opt/homebrew/bin")
        // With none at all, or an empty one, the Mac's own stands in.
        #expect(ProgramEnvironment.path(inherited: nil, isFolder: neither) == ProgramEnvironment.systemPath)
        #expect(
            ProgramEnvironment.path(inherited: "", isFolder: both)
                == ProgramEnvironment.systemPath + ":/opt/homebrew/bin:/usr/local/bin")
    }

    @Test func aPathThatHasThemAlreadyIsLeftInItsOwnOrder() {
        // As a shell starts the app: the user's own order is theirs.
        let own = "/opt/homebrew/bin:/Users/robin/bin:/usr/local/bin:/usr/bin:/bin"
        #expect(ProgramEnvironment.path(inherited: own, isFolder: both) == own)
        #expect(
            ProgramEnvironment.path(inherited: "/opt/homebrew/bin:/usr/bin", isFolder: both)
                == "/opt/homebrew/bin:/usr/bin:/usr/local/bin")
    }

    @Test func theRestOfTheEnvironmentIsAsItWasWithALanguageWhereNoneWasSaid() {
        let inherited = ["HOME": "/Users/robin", "PATH": "/usr/bin:/bin", "EDITOR": "nvim"]
        let environment = ProgramEnvironment.environment(inherited: inherited, isFolder: neither)
        #expect(environment["HOME"] == "/Users/robin" && environment["EDITOR"] == "nvim")
        #expect(environment["PATH"] == "/usr/bin:/bin")
        #expect(environment["LANG"] == "en_US.UTF-8")
        // A language that was said is kept, either way of saying it.
        #expect(
            ProgramEnvironment.environment(inherited: ["LANG": "en_GB.UTF-8"], isFolder: neither)["LANG"]
                == "en_GB.UTF-8")
        #expect(ProgramEnvironment.environment(inherited: ["LC_ALL": "C"], isFolder: neither)["LANG"] == nil)
    }
}
