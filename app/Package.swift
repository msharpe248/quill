// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "Quill",
    platforms: [
        .macOS(.v13)
    ],
    dependencies: [
        // libghostty's embedding API is unstable. Pin one exact release and
        // upgrade deliberately, one version at a time, in a change of its own.
        .package(url: "https://github.com/Lakr233/libghostty-spm.git", exact: "2.2.2026111302")
    ],
    targets: [
        // The only module that may import GhosttyKit or GhosttyTerminal.
        .target(
            name: "TerminalKit",
            dependencies: [
                .product(name: "GhosttyTerminal", package: "libghostty-spm")
            ]
        ),
        // The shell-core bridge: message types and the socket server. Knows
        // command names, never mail.
        .target(name: "Bridge"),
        // The native viewers of the second pane (Quick Look, AVKit, a web
        // view) and the file work done for the core. Knows files and titles,
        // never mail; takes from Bridge only the rule for which links open.
        .target(
            name: "Panes",
            dependencies: ["Bridge"]
        ),
        .executableTarget(
            name: "QuillApp",
            dependencies: ["TerminalKit", "Bridge", "Panes"]
        ),
        .testTarget(
            name: "BridgeTests",
            dependencies: ["Bridge"]
        ),
        .testTarget(
            name: "PanesTests",
            dependencies: ["Panes"]
        ),
    ],
    swiftLanguageModes: [.v6]
)
