import Bridge
import Foundation

/// How the shipped Vim is started on a draft: the program, its arguments,
/// the whole of its environment and the folder it starts in.
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

    /// What `-u` is given when the user has imported no vimrc: Vim's own
    /// defaults, and no file.
    public static let defaults = "DEFAULTS"

    /// The command that loads Quill's plugin once the draft is open, as the
    /// argument of `-c`. It is the same for every launch: it has no path in
    /// it. Vim reads where the plugin is from `$VIM` in its environment, as
    /// a value, and escapes it itself before using it as a file's name.
    ///
    /// `-S file` would be shorter, but Vim runs that as the command
    /// `source file` with the path written into it, where a space, a quote
    /// or a `%` in the name of the folder the app is installed in would be
    /// read as part of the command.
    public static let loadPlugin = "execute 'source' fnameescape($VIM . '/quill.vim')"

    /// A program that does nothing and fails. Vim is told this is the shell,
    /// so `:!` and `:shell` start nothing: no program is run from Vim.
    public static let noShell = "/usr/bin/false"

    /// What Vim takes from the shell's own environment, when it is there:
    /// the language and the time zone. Nothing else is passed down, so no
    /// `VIMINIT`, `EXINIT`, `SHELL` or `PATH` of the user's reaches Vim.
    public static let inherited = ["LANG", "LC_ALL", "LC_CTYPE", "TZ"]

    /// - Parameters:
    ///   - vim: the Vim in the app's bundle.
    ///   - draft: the file to edit and its folder, as the core named them.
    ///   - vimrc: the user's imported vimrc, if they have one.
    ///   - term: `TERM`, as the main terminal has it.
    ///   - home: the app's own home folder: inside the sandbox, its
    ///     container.
    ///   - temporaryDirectory: the app's own temporary folder.
    ///   - base: the shell's environment, from which only `inherited` is
    ///     taken.
    public init(
        vim: ShippedVim,
        draft: PaneEditor,
        vimrc: URL?,
        term: String,
        home: String,
        temporaryDirectory: String,
        base: [String: String]
    ) {
        executable = vim.binary
        arguments = [
            // A vimrc named with -u leaves Vim behaving as vi did, which no
            // vimrc written for Vim expects. -N says otherwise first.
            "-N",
            "-u", vimrc?.path ?? Self.defaults,
            // No viminfo file and no swap file: nothing is kept of a
            // message but the draft.
            "-i", "NONE",
            "-n",
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
        environment["VIM"] = vim.home.path
        environment["VIMRUNTIME"] = vim.runtime.path
        // The plugin reads the draft's folder from here.
        environment["QUILL_DRAFT_DIR"] = draft.dir
        self.environment = environment
        workingDirectory = URL(fileURLWithPath: draft.dir, isDirectory: true)
    }

    /// The program's name and its arguments, as it is started.
    public var argv: [String] { [executable.path] + arguments }
}
