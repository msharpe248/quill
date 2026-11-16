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
        // Notifications: the Mac's, and a recorder to stand in for them.
        // Knows ids, words and the names of buttons, never mail.
        .target(
            name: "Notifications",
            dependencies: ["Bridge"]
        ),
        // Spelling: the Mac's checker, asked for the core, and the sums that
        // turn its answers into the core's. Knows a text and where its
        // misspelt words are, never mail.
        .target(
            name: "Spelling",
            dependencies: ["Bridge"]
        ),
        // The Vim option: which Vim is run (the Mac's own, or the user's by
        // their script), how it is started, what the core is told when it
        // ends, and the user's imported vimrc. Knows a draft file and its folder, never mail, and
        // starts nothing itself.
        .target(
            name: "Editor",
            dependencies: ["Bridge"]
        ),
        // A form the core has the shell show in a sheet: which fields are
        // on it, what each holds as the user types, and what goes back to
        // the core when a button is pressed. And the settings window the
        // core describes: which panes and settings it has, what each
        // holds, and what goes back when one is changed. Knows names,
        // labels and values, never what a form or a setting is for, and
        // shows nothing itself.
        .target(
            name: "Forms",
            dependencies: ["Bridge"]
        ),
        // Themes written for Ghostty, as far as the shell goes with them:
        // finding the ones on this Mac, reading a file's few colours to
        // show beside its name, and copying the one the user chooses into
        // the themes folder. And whether the app's own windows are light or
        // dark. Knows names, files and colours, never what a theme's
        // colours are used for: that is the core's.
        .target(
            name: "Themes",
            dependencies: ["Bridge"]
        ),
        .executableTarget(
            name: "QuillApp",
            dependencies: [
                "TerminalKit", "Bridge", "Panes", "Notifications", "Spelling", "Editor", "Forms", "Themes",
            ]
        ),
        // What a terminal is told to look like, and nothing of libghostty:
        // these tests import TerminalKit and never the terminal itself.
        .testTarget(
            name: "TerminalKitTests",
            dependencies: ["TerminalKit"]
        ),
        .testTarget(
            name: "BridgeTests",
            dependencies: ["Bridge"]
        ),
        .testTarget(
            name: "PanesTests",
            dependencies: ["Panes"]
        ),
        .testTarget(
            name: "NotificationsTests",
            dependencies: ["Notifications"]
        ),
        .testTarget(
            name: "SpellingTests",
            dependencies: ["Spelling"]
        ),
        .testTarget(
            name: "EditorTests",
            dependencies: ["Editor"]
        ),
        .testTarget(
            name: "FormsTests",
            dependencies: ["Forms"]
        ),
        .testTarget(
            name: "ThemesTests",
            dependencies: ["Themes"]
        ),
    ],
    swiftLanguageModes: [.v6]
)
