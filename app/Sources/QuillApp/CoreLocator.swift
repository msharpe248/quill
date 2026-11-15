import Foundation

/// Finds the Rust core. It ships inside the bundle, so it inherits the app's
/// sandbox; nothing outside the bundle is ever launched.
enum CoreLocator {
    static let helperName = "quill"

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
            "TERM": "xterm-256color",
            "COLORTERM": "truecolor",
            "TERM_PROGRAM": "Quill",
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            // In the sandbox this is the app's container.
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
}
