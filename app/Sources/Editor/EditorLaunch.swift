import Bridge
import Foundation

/// How an editor is started on a draft: the program, its arguments, its
/// environment and the folder it starts in.
///
/// The command is the core's, whole (`pane.editor`): the user's own editor,
/// by its absolute path, with the arguments that kind of editor is given.
/// The shell adds nothing to it and takes nothing from it, and it is an
/// argv, never a line for a shell. The editor is the user's in every way:
/// it has the environment the app has, so it reads their own settings from
/// their own home folder.
public struct EditorLaunch: Equatable, Sendable {
    public let executable: URL
    /// The arguments after the program's own name.
    public let arguments: [String]
    /// Every variable the editor is given.
    public let environment: [String: String]
    /// The draft's own folder.
    public let workingDirectory: URL

    /// The variable that says where the draft's folder is. Quill's plugin
    /// for Vim is read from there, and reads it to find its notes.
    public static let draftFolderVariable = "QUILL_DRAFT_DIR"

    /// - Parameters:
    ///   - request: what the core asked for.
    ///   - term: `TERM`, as the main terminal has it.
    ///   - base: the environment programs the app starts are given
    ///     (`ProgramEnvironment`).
    public init(request: PaneEditor, term: String, base: [String: String]) {
        executable = URL(fileURLWithPath: request.program)
        arguments = request.arguments
        workingDirectory = URL(fileURLWithPath: request.dir, isDirectory: true)
        var environment = base
        environment["TERM"] = term
        environment["COLORTERM"] = "truecolor"
        environment[Self.draftFolderVariable] = request.dir
        self.environment = environment
    }

    /// The program's name and its arguments, as it is started.
    public var argv: [String] { [executable.path] + arguments }
}

/// The environment of the programs the app starts: the core, and through
/// it the user's own editor.
///
/// An app opened from the Dock or the Finder is not started by a shell, so
/// it has the Mac's own `PATH` and not the one the user's shell sets up.
/// Homebrew's two folders are put after it when they are there, so that a
/// program installed that way is found by its name: after, so that nothing
/// already found changes.
public enum ProgramEnvironment {
    /// The `PATH` every Mac has.
    public static let systemPath = "/usr/bin:/bin:/usr/sbin:/sbin"
    /// Where Homebrew puts programs, on Apple silicon and on Intel.
    public static let homebrewFolders = ["/opt/homebrew/bin", "/usr/local/bin"]

    /// `inherited` (or the Mac's own, when there is none) with each of
    /// Homebrew's folders that `isFolder` finds put after it, unless it is
    /// among them already.
    public static func path(inherited: String?, isFolder: (String) -> Bool) -> String {
        var folders = (inherited.flatMap { $0.isEmpty ? nil : $0 } ?? systemPath)
            .split(separator: ":", omittingEmptySubsequences: true).map(String.init)
        for folder in homebrewFolders where !folders.contains(folder) && isFolder(folder) {
            folders.append(folder)
        }
        return folders.joined(separator: ":")
    }

    /// What the app inherited, with the `PATH` above and a language where
    /// none was said: a terminal program with none takes its text for ASCII.
    public static func environment(inherited: [String: String], isFolder: (String) -> Bool) -> [String: String] {
        var environment = inherited
        environment["PATH"] = path(inherited: inherited["PATH"], isFolder: isFolder)
        if environment["LANG"] == nil, environment["LC_ALL"] == nil {
            environment["LANG"] = "en_US.UTF-8"
        }
        return environment
    }

    /// Whether there is a folder at `path`.
    public static func folderExists(_ path: String) -> Bool {
        var isFolder: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isFolder) && isFolder.boolValue
    }
}
