import Editor
import Foundation

/// Finds the Rust core, which ships inside the bundle, and says what
/// environment the programs the app starts are given: the core, and the
/// user's own editor.
enum CoreLocator {
    static let helperName = "quill"

    /// What the terminals in the app say they are.
    static let term = "xterm-256color"

    /// What the core exits with when the mailbox is open in another Quill,
    /// in a terminal: the same number as `ALREADY_OPEN_STATUS` in the core.
    static let alreadyOpenStatus: Int32 = 75

    static func coreURL(bundle: Bundle = .main) -> URL? {
        let helper = bundle.bundleURL
            .appendingPathComponent("Contents/Helpers", isDirectory: true)
            .appendingPathComponent(helperName)
        return FileManager.default.isExecutableFile(atPath: helper.path) ? helper : nil
    }

    /// The environment the core runs in. Built from scratch: the core gets
    /// what it needs and nothing the app happened to inherit.
    static func environment(extra: [String: String] = [:]) -> [String: String] {
        let inherited = ProcessInfo.processInfo.environment
        var environment: [String: String] = [
            "TERM": term,
            "COLORTERM": "truecolor",
            "TERM_PROGRAM": "Quill",
            // Where the core looks for the user's editor by its name: the
            // PATH the app was started with, and Homebrew's folders after.
            "PATH": ProgramEnvironment.path(
                inherited: inherited["PATH"], isFolder: ProgramEnvironment.folderExists),
            "HOME": NSHomeDirectory(),
            "TMPDIR": NSTemporaryDirectory(),
            "USER": NSUserName(),
            "LANG": inherited["LANG"] ?? "en_US.UTF-8",
        ]
        if let timezone = inherited["TZ"] {
            environment["TZ"] = timezone
        }
        environment.merge(extra) { _, new in new }
        return environment
    }

    /// The environment the user's own editor runs in: the app's own, as
    /// the user's session gave it, so that the editor finds their settings
    /// for it and their other programs. `extra` is what the run was told to
    /// give the programs it starts (`--core-env`), which is how a scripted
    /// run keeps an editor to a home folder of its own.
    static func programEnvironment(extra: [String: String] = [:]) -> [String: String] {
        var environment = ProgramEnvironment.environment(
            inherited: ProcessInfo.processInfo.environment, isFolder: ProgramEnvironment.folderExists)
        environment.merge(extra) { _, new in new }
        return environment
    }
}
