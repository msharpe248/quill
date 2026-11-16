import Bridge
import Foundation
import Testing

@testable import Editor

struct VimLaunchTests {
    /// What the shell itself might have in its environment, none of which is
    /// Vim's business.
    private static let base: [String: String] = [
        "VIMINIT": "source /Users/robin/.vimrc",
        "EXINIT": "set shell=/bin/zsh",
        "GVIMINIT": "x",
        "MYVIMRC": "/Users/robin/.vimrc",
        "SHELL": "/bin/zsh",
        "PATH": "/opt/homebrew/bin:/usr/bin:/bin",
        "HOME": "/Users/robin",
        "TMPDIR": "/var/folders/xy/T/",
        "TERM": "dumb",
        "VIM": "/opt/homebrew/share/vim",
        "VIMRUNTIME": "/opt/homebrew/share/vim/vim92",
        "QUILL_DRAFT_DIR": "/somewhere/else",
        "QUILL_SHELL_SOCKET": "/tmp/q-1/bridge.sock",
        "XDG_CONFIG_HOME": "/Users/robin/.config",
        "EDITOR": "nvim",
        "USER": "robin",
        "LANG": "de_DE.UTF-8",
        "TZ": "Europe/Berlin",
    ]

    private static let container = "/Users/robin/Library/Containers/dev.quill.Quill/Data"

    private func makeLaunch(
        bundle: URL,
        path: String = container + "/tmp/quill-edit-812/message.quill",
        dir: String = container + "/tmp/quill-edit-812",
        vimrc: URL? = nil,
        base: [String: String] = base
    ) throws -> VimLaunch {
        VimLaunch(
            vim: try ShippedVim(inBundle: bundle),
            draft: try #require(PaneEditor(path: path, dir: dir, title: "Re: Q4 roadmap")),
            vimrc: vimrc,
            term: "xterm-256color",
            home: Self.container,
            temporaryDirectory: Self.container + "/tmp/",
            base: base
        )
    }

    @Test func vimIsStartedOnTheDraftWithItsOwnDefaults() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        let launch = try makeLaunch(bundle: bundle)
        #expect(launch.executable.path == bundle.path + "/Contents/Helpers/vim")
        #expect(
            launch.arguments == [
                "-N", "-u", "DEFAULTS", "-i", "NONE", "-n",
                "--cmd", "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END",
                "-c", "execute 'source' fnameescape($VIM . '/quill.vim')", "--",
                Self.container + "/tmp/quill-edit-812/message.quill",
            ])
        #expect(launch.argv == [bundle.path + "/Contents/Helpers/vim"] + launch.arguments)
        #expect(launch.workingDirectory.path == Self.container + "/tmp/quill-edit-812")
    }

    @Test func anImportedVimrcTakesThePlaceOfTheDefaults() throws {
        let scratch = try Scratch()
        let vimrc = URL(fileURLWithPath: Self.container + "/Library/Application Support/Quill/vim/vimrc")
        let launch = try makeLaunch(bundle: scratch.bundleWithVim(), vimrc: vimrc)
        #expect(
            launch.arguments == [
                "-N", "-u", Self.container + "/Library/Application Support/Quill/vim/vimrc", "-i", "NONE", "-n",
                "--cmd", "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END",
                "-c", "execute 'source' fnameescape($VIM . '/quill.vim')", "--",
                Self.container + "/tmp/quill-edit-812/message.quill",
            ])
    }

    @Test func theEnvironmentIsBuiltFromNothing() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        let launch = try makeLaunch(bundle: bundle)
        // All of it: nothing the shell had reaches Vim but the language and
        // the time zone.
        #expect(
            launch.environment == [
                "TERM": "xterm-256color",
                "HOME": Self.container,
                "TMPDIR": Self.container + "/tmp/",
                "SHELL": "/usr/bin/false",
                "VIM": bundle.path + "/Contents/Resources/vim",
                "VIMRUNTIME": bundle.path + "/Contents/Resources/vim/runtime",
                "QUILL_DRAFT_DIR": Self.container + "/tmp/quill-edit-812",
                "LANG": "de_DE.UTF-8",
                "TZ": "Europe/Berlin",
            ])
        for name in ["VIMINIT", "EXINIT", "GVIMINIT", "MYVIMRC", "PATH", "EDITOR", "XDG_CONFIG_HOME", "USER"] {
            #expect(launch.environment[name] == nil, "\(name)")
        }
    }

    @Test func onlyTheAllowListIsInherited() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        #expect(VimLaunch.inherited == ["LANG", "LC_ALL", "LC_CTYPE", "TZ"])
        // Every variable but those on the list is the same whatever the
        // shell had, down to having nothing at all.
        let full = try makeLaunch(bundle: bundle)
        let empty = try makeLaunch(bundle: bundle, base: [:])
        var expected = full.environment
        expected["LANG"] = "en_US.UTF-8"
        expected["TZ"] = nil
        #expect(empty.environment == expected)

        // A variable of the shell's is passed on only if it is on the list,
        // whatever it is called and whatever it holds.
        var hostile = Self.base
        for name in VimLaunch.inherited {
            hostile[name] = nil
        }
        for index in 0..<50 {
            hostile["VAR_\(index)"] = "/Users/robin/\(index)"
        }
        let none = try makeLaunch(bundle: bundle, base: hostile)
        #expect(none.environment == expected)

        var languages = Self.base
        languages["LC_ALL"] = "fr_FR.UTF-8"
        languages["LC_CTYPE"] = "UTF-8"
        let passed = try makeLaunch(bundle: bundle, base: languages)
        #expect(passed.environment["LC_ALL"] == "fr_FR.UTF-8")
        #expect(passed.environment["LC_CTYPE"] == "UTF-8")
        #expect(passed.environment.count == full.environment.count + 2)
    }

    @Test func noShellAndNoProgramOfTheUsers() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        let launch = try makeLaunch(bundle: bundle)
        // The program is the one in the bundle, started as itself.
        #expect(launch.argv[0].hasPrefix(bundle.path + "/"))
        #expect(!launch.argv.contains { $0.hasSuffix("/sh") || $0.hasSuffix("/zsh") })
        // One command before the vimrc and one after the draft is read, and
        // they are always the same two.
        #expect(launch.argv.filter { $0 == "--cmd" }.count == 1)
        #expect(launch.arguments[6...7] == ["--cmd", VimLaunch.noModelines])
        #expect(launch.argv.filter { $0 == "-c" || $0 == "-S" || $0.hasPrefix("+") }.count == 1)
        #expect(launch.arguments[8...9] == ["-c", VimLaunch.loadPlugin])
        // Vim is told the shell is a program that does nothing.
        #expect(launch.environment["SHELL"] == "/usr/bin/false")
        #expect(launch.environment["PATH"] == nil)
    }

    @Test func theDraftAndItsFolderAreOnlyEverAnArgumentAndAValue() throws {
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim()
        for dir in [
            "/tmp/quill edit 1",
            "/tmp/it's \"quoted\"",
            "/tmp/line\nbreak",
            "/tmp/$HOME `id` $(id) | q ; rm -rf %  # <cfile>",
            "/tmp/-c/+q",
            "/tmp/caf\u{E9} \u{1F4EC}",
        ] {
            let path = dir + "/message.quill"
            let launch = try makeLaunch(bundle: bundle, path: path, dir: dir)
            // The path is the last argument, whole and unchanged, after the
            // marker that says only files follow.
            #expect(launch.arguments.last == path, "\(dir.debugDescription)")
            #expect(launch.arguments.dropLast().last == "--")
            #expect(launch.arguments.count == 12)
            // No other argument has any of it in it.
            for argument in launch.arguments.dropLast() {
                #expect(!argument.contains(dir), "\(argument.debugDescription)")
            }
            // The folder is one variable's value, whole and unchanged, and
            // where Vim starts.
            #expect(launch.environment["QUILL_DRAFT_DIR"] == dir)
            #expect(launch.workingDirectory.path == dir)
            for (name, value) in launch.environment where name != "QUILL_DRAFT_DIR" {
                #expect(!value.contains(dir), "\(name)")
            }
            #expect(launch.environment.count == 9)
        }
    }

    @Test func aPathThatLooksLikeAnOptionIsStillAFile() throws {
        // Absolute paths cannot start with a dash, and `--` comes first
        // whatever they are.
        let scratch = try Scratch()
        let launch = try makeLaunch(bundle: scratch.bundleWithVim(), path: "/-u/NONE", dir: "/-u")
        #expect(Array(launch.arguments.suffix(2)) == ["--", "/-u/NONE"])
        #expect(launch.arguments[1...2] == ["-u", "DEFAULTS"])
    }

    @Test func thePlaceTheAppIsInstalledIsNeverPartOfACommand() throws {
        // The command that loads the plugin has no path in it: Vim reads
        // `$VIM` as a value and escapes it itself. So a space, a quote or a
        // percent sign in the folder the app is in is in a variable's value
        // and in no argument Vim parses.
        let scratch = try Scratch()
        let bundle = try scratch.bundleWithVim("My \"Apps\" 100%/Quill's.app")
        let launch = try makeLaunch(bundle: bundle)
        #expect(launch.arguments[8...9] == ["-c", "execute 'source' fnameescape($VIM . '/quill.vim')"])
        #expect(launch.arguments == (try makeLaunch(bundle: scratch.bundleWithVim("Plain.app"))).arguments)
        #expect(launch.environment["VIM"] == bundle.path + "/Contents/Resources/vim")
        #expect(launch.environment["VIMRUNTIME"] == bundle.path + "/Contents/Resources/vim/runtime")
        for argument in launch.arguments {
            #expect(!argument.contains(bundle.path))
        }
        #expect(launch.argv[0] == bundle.path + "/Contents/Helpers/vim")
        // The plugin that command names is the one that was found.
        #expect(try ShippedVim(inBundle: bundle).plugin.path == launch.environment["VIM"]! + "/quill.vim")
    }
}
