import Foundation

/// What the command line asks of the app. Everything here is for development
/// and tests; a user launching Quill from the Dock passes nothing.
struct LaunchOptions: Equatable {
    /// Run the core on its real database instead of the demo mailbox.
    ///
    /// Until accounts exist (M2) the real database is empty, so the app shows
    /// the demo mailbox unless told otherwise.
    var useRealMailbox = false

    /// Extra environment variables for the core, from `--core-env KEY=VALUE`.
    var coreEnvironment: [String: String] = [:]

    /// Steps to perform once the window is up, from `--script`. See
    /// `DebugScript` for what a step can be.
    var script: String?

    /// After the script, write the terminal's text and the shell's own state
    /// to this file and quit. Lets a script check the app without screen
    /// recording permission.
    var dumpScreenTo: URL?

    init(arguments: [String]) {
        var rest = arguments.dropFirst().makeIterator()
        while let argument = rest.next() {
            switch argument {
            case "--no-demo":
                useRealMailbox = true
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
