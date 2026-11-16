import Foundation

/// What the command line asks of the app. Everything here is for development
/// and tests; a user launching Quill from the Dock passes nothing.
struct LaunchOptions: Equatable {
    /// Run the core on the demo mailbox instead of the user's own mail:
    /// `--demo`. The demo holds invented mail in memory and writes nothing.
    var useDemoMailbox = false

    /// Extra environment variables for the core, from `--core-env KEY=VALUE`.
    var coreEnvironment: [String: String] = [:]

    /// Steps to perform once the window is up, from `--script`. See
    /// `DebugScript` for what a step can be.
    var script: String?

    /// After the script, write the terminal's text and the shell's own state
    /// to this file and quit. Lets a script check the app without screen
    /// recording permission.
    var dumpScreenTo: URL?

    /// Whether the app is being driven by a script or asked for a dump: a
    /// run that must leave nothing of the user's as it did not find it,
    /// their saved window and divider included.
    var isScripted: Bool { script != nil || dumpScreenTo != nil }

    init(arguments: [String]) {
        var rest = arguments.dropFirst().makeIterator()
        while let argument = rest.next() {
            switch argument {
            case "--demo":
                useDemoMailbox = true
            case "--core-env":
                if let pair = rest.next(), let equals = pair.firstIndex(of: "=") {
                    coreEnvironment[String(pair[..<equals])] = String(pair[pair.index(after: equals)...])
                }
            case "--script":
                script = rest.next()
            case "--dump-screen":
                dumpScreenTo = rest.next().map { URL(fileURLWithPath: $0) }
            default:
                // AppKit passes its own arguments (-NSDocumentRevisionsDebugMode
                // and the like). Ignore what is not ours.
                continue
            }
        }
    }
}
