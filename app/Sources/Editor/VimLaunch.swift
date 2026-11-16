import Bridge
import Foundation

/// How Vim is started on a draft: the program, its arguments, the whole of
/// its environment and the folder it starts in.
///
/// The draft's path and its folder come from outside the shell. Each is an
/// argument or an environment value, whole: neither is ever written into a
/// command for Vim to parse, nor passed through a shell. The one command
/// Vim is given is a constant.
public struct VimLaunch: Equatable, Sendable {
    public let executable: URL
    /// The arguments after the program's own name.
    public let arguments: [String]
    /// Every variable Vim is given. It inherits no other.
    public let environment: [String: String]
    /// The draft's own folder.
    public let workingDirectory: URL
    /// The program is the user's own script, which macOS runs outside the
    /// sandbox. Its environment and the folder it starts in are then not
    /// the shell's to choose, and `environment` is empty.
    public let userScript: Bool

    /// What `-u` is given when the user has imported no vimrc: Vim's own
    /// defaults, and no file.
    public static let defaults = "DEFAULTS"

    /// The command that loads Quill's plugin once the draft is open, as the
    /// argument of `-c`, for the Vim that comes with macOS. It is the same
    /// for every launch: it has no path in it. Vim reads where the plugin is
    /// from a variable in its environment, as a value, and escapes it itself
    /// before using it as a file's name.
    ///
    /// `-S file` would be shorter, but Vim runs that as the command
    /// `source file` with the path written into it, where a space, a quote
    /// or a `%` in the name of the folder the app is installed in would be
    /// read as part of the command.
    public static let loadPlugin = "execute 'source' fnameescape($QUILL_VIM_PLUGIN)"

    /// The same for a Vim started by the user's own script. Nothing can be
    /// put in that Vim's environment, so the draft's folder is taken from
    /// the draft that Vim has just opened: the core puts a copy of the
    /// plugin beside it. Still a constant, with no path in it.
    public static let loadPluginBesideDraft =
        "let g:quill_dir = expand('%:p:h') | execute 'source' fnameescape(g:quill_dir . '/quill.vim')"

    /// Run before any file is read: modelines are off for every buffer.
    /// In a group of its own, because a vimrc that starts with a bare
    /// `autocmd!` clears every command that is in no group.
    public static let noModelines =
        "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END"

    /// A program that does nothing and fails. Vim is told this is the shell,
    /// so `:!` and `:shell` start nothing: no program is run from Vim.
    public static let noShell = "/usr/bin/false"

    /// What Vim takes from the shell's own environment, when it is there:
    /// the language and the time zone. Nothing else is passed down, so no
    /// `VIMINIT`, `EXINIT`, `SHELL` or `PATH` of the user's reaches Vim.
    public static let inherited = ["LANG", "LC_ALL", "LC_CTYPE", "TZ"]

    /// - Parameters:
    ///   - vim: the Vim to run: the one macOS has, or the user's own by
    ///     their script.
    ///   - draft: the file to edit and its folder, as the core named them.
    ///   - vimrc: the user's imported vimrc, if they have one.
    ///   - term: `TERM`, as the main terminal has it.
    ///   - home: the app's own home folder: inside the sandbox, its
    ///     container.
    ///   - temporaryDirectory: the app's own temporary folder.
    ///   - base: the shell's environment, from which only `inherited` is
    ///     taken.
    public init(
        vim: VimToRun,
        draft: PaneEditor,
        vimrc: URL?,
        term: String,
        home: String,
        temporaryDirectory: String,
        base: [String: String]
    ) {
        workingDirectory = URL(fileURLWithPath: draft.dir, isDirectory: true)
        let system: SystemVim
        switch vim {
        case let .system(found):
            system = found
        case let .script(script):
            // The user's own Vim, with their own vimrc, viminfo and swap
            // files: it is theirs, as it is in a terminal. What Quill asks
            // of it is what it asks there: modelines off for the draft, and
            // its plugin. The script is handed the helper first.
            executable = script.script
            arguments = [
                script.helper.path,
                "-N",
                "--cmd", Self.noModelines,
                "-c", Self.loadPluginBesideDraft,
                "--",
                draft.path,
            ]
            environment = [:]
            userScript = true
            return
        }
        executable = system.binary
        userScript = false
        arguments = [
            // A vimrc named with -u leaves Vim behaving as vi did, which no
            // vimrc written for Vim expects. -N says otherwise first.
            "-N",
            // Naming a vimrc, or none, also keeps Vim from reading the one
            // macOS has for everybody.
            "-u", vimrc?.path ?? Self.defaults,
            // No viminfo file and no swap file: nothing is kept of a
            // message but the draft.
            "-i", "NONE",
            "-n",
            // A message quotes other people's words, and a line of them
            // that looks like a modeline must not set options here,
            // whatever the vimrc says about modelines.
            "--cmd", Self.noModelines,
            "-c", Self.loadPlugin,
            // Whatever follows is a file, even if it were to start with a
            // dash or a plus.
            "--",
            draft.path,
        ]
        var environment: [String: String] = [:]
        for name in Self.inherited {
            environment[name] = base[name]
        }
        if environment["LANG"] == nil {
            environment["LANG"] = "en_US.UTF-8"
        }
        environment["TERM"] = term
        environment["HOME"] = home
        environment["TMPDIR"] = temporaryDirectory
        environment["SHELL"] = Self.noShell
        // Vim knows where its own files are, so `VIM` and `VIMRUNTIME` are
        // left for it to work out. Only the plugin is Quill's.
        environment["QUILL_VIM_PLUGIN"] = system.plugin.path
        // The plugin reads the draft's folder from here.
        environment["QUILL_DRAFT_DIR"] = draft.dir
        self.environment = environment
    }

    /// The program's name and its arguments, as it is started.
    public var argv: [String] { [executable.path] + arguments }
}
