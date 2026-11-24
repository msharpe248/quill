import Foundation
import Testing

@testable import Bridge

/// The examples in `schema/examples` are the contract the Rust core is tested
/// against too. If the two sides disagree, one of these suites fails.
private enum Examples {
    static let directory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // BridgeTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // app
        .deletingLastPathComponent()  // repository root
        .appendingPathComponent("schema/examples")

    static func load(_ group: String) throws -> [(name: String, text: String)] {
        let folder = directory.appendingPathComponent(group)
        let files = try FileManager.default.contentsOfDirectory(atPath: folder.path)
            .filter { $0.hasSuffix(".json") }
            .sorted()
        return try files.map { name in
            (name, try String(contentsOf: folder.appendingPathComponent(name), encoding: .utf8))
        }
    }

    static func value(_ text: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }
}

@Suite struct MessageTests {
    @Test func coreToShellExamplesRoundTrip() throws {
        let examples = try Examples.load("core-to-shell")
        #expect(
            examples.map(\.name) == [
                "clipboard.write.json", "commands.list.json", "editor.place.json", "file.open.json",
                "files.pick.json",
                "files.save.json", "form.close.json", "form.show.json", "keys.list.json", "keys.show.json",
                "link.open.json", "notify.clear.json", "notify.post.json", "pane.close.json",
                "pane.editor.json", "pane.play.json", "pane.preview.json",
                "reading.close.json", "reading.place.json", "reading.scroll.json", "reading.show.json",
                "rules.edit.json", "settings.show.json", "spell.check.json",
                "spell.suggest.json", "state.update.json", "themes.pick.json", "thumbnail.make.json",
                "window.close.json", "window.show.json",
            ])
        for (name, text) in examples {
            let message = try CoreToShell(line: text)
            let line = message.line
            #expect(!line.contains("\n"), "\(name) does not encode to one line")
            #expect(try Examples.value(line) == Examples.value(text), "\(name) does not round-trip")
            #expect(try CoreToShell(line: line) == message)
        }
    }

    @Test func shellToCoreExamplesRoundTrip() throws {
        let examples = try Examples.load("shell-to-core")
        #expect(
            examples.map(\.name) == [
                "app.will_quit.json", "appearance.changed.json", "command.run.json", "editor.exited.json",
                "files.dropped.json",
                "files.saved.json", "form.used.json", "notify.used.json", "reading.dragged.json",
                "settings.closed.json",
                "settings.opened.json", "settings.used.json", "spell.checked.json",
                "spell.suggested.json", "themes.imported.json", "thumbnail.made.json", "url.open.json",
                "window.resized.json",
            ])
        for (name, text) in examples {
            let message = try ShellToCore(line: text)
            let line = message.line
            #expect(!line.contains("\n"), "\(name) does not encode to one line")
            #expect(try Examples.value(line) == Examples.value(text), "\(name) does not round-trip")
            #expect(try ShellToCore(line: line) == message)
        }
    }

    @Test func aDirectionRefusesTheOtherDirectionsMessages() throws {
        for (name, text) in try Examples.load("shell-to-core") {
            #expect(throws: BridgeError.self, "\(name)") { try CoreToShell(line: text) }
        }
        for (name, text) in try Examples.load("core-to-shell") {
            #expect(throws: BridgeError.self, "\(name)") { try ShellToCore(line: text) }
        }
    }

    @Test func invalidExamplesAreRefusedByBothTypes() throws {
        let examples = try Examples.load("invalid")
        #expect(examples.count >= 6)
        for name in [
            "notification-with-no-id.json", "too-many-notification-actions.json", "url-that-is-not-mail.json",
            "misspelt-word-with-no-length.json", "too-many-suggestions.json", "editor-in-a-relative-folder.json",
            "editor-of-a-kind-there-is-not.json", "editor-named-by-its-path.json",
            "editor-program-found-by-its-name.json", "editor-with-no-program.json",
            "editor-command-as-a-line.json",
            "form-secret-sent-by-the-core.json", "form-choice-with-no-options.json",
            "form-code-with-the-focus.json", "form-code-with-nothing-to-show.json",
            "form-field-of-an-unknown-kind.json", "form-used-with-a-button-that-is-no-name.json",
            "form-used-with-no-secrets.json", "reading-header-too-long.json", "reading-scroll-sideways.json",
            "reading-notice-with-a-button-that-runs-nothing.json", "reading-notice-with-nothing-to-say.json",
            "reading-notice-with-too-many-buttons.json",
            "relative-reading-path.json", "reading-place-with-no-columns.json", "window-wider-than-a-terminal.json",
            "form-toggle-in-a-sheet.json", "settings-with-a-secret.json", "settings-with-nothing-to-show.json",
            "settings-number-that-steps-by-nothing.json", "settings-pane-with-no-symbol.json",
            "settings-list-action-with-no-title.json", "settings-field-with-the-focus.json",
            "settings-used-with-a-value-and-an-action.json", "settings-used-with-nothing-done.json",
            "settings-used-with-a-row-and-no-action.json", "settings-used-with-a-list-for-a-value.json",
            "state-with-a-colour-that-is-a-name.json", "state-with-an-appearance-of-its-own.json",
            "appearance-that-is-not-light-or-dark.json", "theme-imported-by-a-path.json",
            "theme-imported-with-a-hidden-name.json", "theme-imported-under-a-toml-name.json",
            "settings-button-with-nothing-on-it.json", "form-button-field-in-a-sheet.json",
            "editor-place-with-the-keys-and-nowhere-to-be.json", "editor-place-with-no-rows.json",
            "key-line-that-says-nothing.json", "reading-place-with-no-window.json",
            "reading-dragged-to-no-columns.json",
        ] {
            #expect(examples.contains { $0.name == name }, "\(name) is missing")
        }
        for (name, text) in examples {
            #expect(throws: BridgeError.self, "CoreToShell accepts \(name)") { try CoreToShell(line: text) }
            #expect(throws: BridgeError.self, "ShellToCore accepts \(name)") { try ShellToCore(line: text) }
        }
    }

    @Test func theExamplesDecodeToWhatTheySay() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        guard case let .commandsList(commands) = try CoreToShell(line: examples["commands.list.json"]!) else {
            Issue.record("not a commands.list")
            return
        }
        #expect(commands.count == 7)
        // Rules have a menu of their own.
        #expect(commands[4].menu == .rules)
        // Closing and quitting are two commands, and neither is in a menu.
        #expect(commands[5] == CommandInfo(name: "window.close", title: "Close", keys: ["q", "ctrl-c"]))
        #expect(commands[6] == CommandInfo(name: "app.quit", title: "Quit", keys: ["Q"]))
        #expect(commands[0] == CommandInfo(name: "nav.next", title: "Next item or line", keys: ["j", "down"]))
        #expect(commands[2].menu == .view)
        #expect(commands[2].symbol == "arrow.up.left.and.arrow.down.right")
        #expect(commands[3].keys.isEmpty)

        let state = try CoreToShell(line: examples["state.update.json"]!)
        #expect(
            state
                == .stateUpdate(
                    StateUpdate(
                        context: .list,
                        enabled: ["nav.next", "nav.open", "message.toggle_star", "view.zoom"],
                        checked: ["view.web_reading"],
                        unread: 12,
                        title: "Inbox (unified) - 12 unread",
                        background: "#1a1b26",
                        foreground: "#c0caf5",
                        appearance: .dark,
                        font: "JetBrains Mono",
                        fontSize: 13.5
                    )))
        #expect(try CoreToShell(line: examples["themes.pick.json"]!) == .themesPick)
        #expect(try CoreToShell(line: examples["pane.close.json"]!) == .paneClose)
        #expect(try CoreToShell(line: examples["files.pick.json"]!) == .filesPick)
        #expect(
            try CoreToShell(line: examples["clipboard.write.json"]!)
                == .clipboardWrite(text: "Are you free on Thursday?\nThe usual place, 12:30."))
    }

    @Test func theTerminalsColoursAreSixHexDigitsOrLeftOut() throws {
        func state(_ colours: String) -> String {
            #"{"jsonrpc":"2.0","method":"state.update","params":{"context":"list","enabled":[],"#
                + #""checked":[],"unread":0,"title":"Inbox"\#(colours)}}"#
        }
        // Left out: the terminals keep their own colours, and nothing is
        // said of them on the way back either.
        let plain = try CoreToShell(line: state(""))
        #expect(plain == .stateUpdate(StateUpdate(context: .list, enabled: [], unread: 0, title: "Inbox")))
        #expect(!plain.line.contains("background") && !plain.line.contains("foreground"))

        let both = try CoreToShell(line: state(##","background":"#fafafa","foreground":"#383a42""##))
        guard case let .stateUpdate(coloured) = both else {
            Issue.record("not a state.update")
            return
        }
        #expect(coloured.background == "#fafafa" && coloured.foreground == "#383a42")
        #expect(try CoreToShell(line: both.line) == both)
        // One without the other is said as it is.
        guard case let .stateUpdate(half) = try CoreToShell(line: state(##","background":"#000000""##)) else {
            Issue.record("not a state.update")
            return
        }
        #expect(half.background == "#000000" && half.foreground == nil)

        for bad in [
            #""blue""#, ##""#fff""##, ##""#1A1B26""##, #""1a1b26""#, ##""#1a1b2g""##, ##""#1a1b26ff""##, #""""#,
            #""default""#, "7", "null", ##"["#1a1b26"]"##, ##""#1a1b26\nkeybind = all""##,
        ] {
            #expect(throws: BridgeError.self, "background \(bad)") {
                try CoreToShell(line: state(#","background":\#(bad)"#))
            }
            #expect(throws: BridgeError.self, "foreground \(bad)") {
                try CoreToShell(line: state(#","foreground":\#(bad)"#))
            }
        }
    }

    @Test func filesForThePanesDecodeToWhatTheySay() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        let folder = "/Users/robin/Library/Containers/dev.quill.Quill/Data/tmp/quill-view-412"

        guard case let .panePreview(preview) = try CoreToShell(line: examples["pane.preview.json"]!) else {
            Issue.record("not a pane.preview")
            return
        }
        #expect(preview.index == 1)
        #expect(preview.items.map(\.title) == ["roadmap.pdf", "chart.png"])
        #expect(preview.items[1].path == folder + "/chart.png")

        #expect(
            try CoreToShell(line: examples["pane.play.json"]!)
                == .panePlay(PaneFile(path: folder + "/demo.mov", title: "demo.mov")!))
        #expect(
            try CoreToShell(line: examples["thumbnail.make.json"]!)
                == .thumbnailMake(
                    ThumbnailMake(id: 7, path: folder + "/demo.mov", out: folder + "/thumb-7.png", size: 640)!))
        #expect(try CoreToShell(line: examples["file.open.json"]!) == .fileOpen(path: folder + "/roadmap.pdf"))
        #expect(
            try CoreToShell(line: examples["files.save.json"]!)
                == .filesSave(FilesSave(path: folder + "/roadmap.pdf", name: "roadmap.pdf")!))

        // None of them can be built holding what the core would not send.
        #expect(PaneFile(path: "demo.mov", title: "demo") == nil)
        #expect(PanePreview(items: [], index: 0) == nil)
        #expect(ThumbnailMake(id: 1, path: "/a", out: "/b", size: 8) == nil)
        #expect(ThumbnailMake(id: -1, path: "/a", out: "/b", size: 64) == nil)
        #expect(FilesSave(path: "/a", name: "") == nil)

        let answers = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        #expect(try ShellToCore(line: answers["thumbnail.made.json"]!) == .thumbnailMade(id: 7, ok: true))
        #expect(
            try ShellToCore(line: answers["files.saved.json"]!) == .filesSaved(name: "roadmap.pdf", saved: true))
    }

    @Test func theReadingPanesMessagesDecodeToWhatTheySay() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        guard case let .readingShow(show) = try CoreToShell(line: examples["reading.show.json"]!) else {
            Issue.record("not a reading.show")
            return
        }
        #expect(!show.remote)
        #expect(show.title == "Issue 88: the autumn round-up")
        #expect(show.path.hasSuffix("/m88/page/message.html"))
        #expect(
            show.header == [
                "From: The Round-up <news@example.com>", "To: Robin Example <robin@example.com>",
                "Saturday 14 November 2026, 09:12",
            ])
        #expect(show.notice?.text == "2 remote pictures were left out.")
        #expect(show.notice?.actions.map(\.command) == ["message.load_remote", "sender.always_remote"])
        #expect(show.notice?.actions.map(\.title) == ["Load Pictures", "Always from This Sender"])
        // What is sent is what was read, and a page with nothing to say
        // about it says nothing of a notice.
        #expect(try CoreToShell(line: CoreToShell.readingShow(show).line) == .readingShow(show))
        let plain = try #require(ReadingShow(path: "/a", title: "", remote: false))
        #expect(!CoreToShell.readingShow(plain).line.contains("notice"))
        for bad in [
            #"null"#, #"{"text":"x"}"#, #"{"text":"","actions":[]}"#, #"{"text":"x","actions":[],"more":1}"#,
            #"{"text":"x","actions":[{"command":"Load","title":"Load"}]}"#,
            #"{"text":"x","actions":[{"command":"a.b","title":""}]}"#,
            #"{"text":"x","actions":[{"command":"a.b","title":"B","key":"i"}]}"#,
        ] {
            let line =
                #"{"jsonrpc":"2.0","method":"reading.show","params":{"path":"/a","title":"","remote":false,"header":[],"notice":"#
                + bad + "}}"
            #expect(throws: BridgeError.self, "\(bad)") { try CoreToShell(line: line) }
        }
        #expect(try CoreToShell(line: examples["reading.scroll.json"]!) == .readingScroll(.pageDown))
        #expect(try CoreToShell(line: examples["reading.close.json"]!) == .readingClose)
        #expect(CoreToShell.readingScroll(.pageUp).line.contains("\"by\":\"page_up\""))

        // It cannot be built holding what the core would not send.
        #expect(ReadingNotice(text: "Left out.") != nil)
        #expect(ReadingNotice(text: "") == nil)
        #expect(ReadingNotice(text: "two\nlines") == nil)
        let load = try #require(ReadingAction(command: "message.load_remote", title: "Load Pictures"))
        #expect(ReadingNotice(text: "Left out.", actions: Array(repeating: load, count: 3)) != nil)
        #expect(ReadingNotice(text: "Left out.", actions: Array(repeating: load, count: 4)) == nil)
        for name in ["", "load", "Message.load", "message.", ".load", "message.load-remote", "a b.c"] {
            #expect(ReadingAction(command: name, title: "Load") == nil, "\(name)")
        }
        #expect(ReadingAction(command: "a.b", title: "") == nil)
        #expect(ReadingAction(command: "a.b", title: String(repeating: "x", count: 64)) != nil)
        #expect(ReadingAction(command: "a.b", title: String(repeating: "x", count: 65)) == nil)
        #expect(ReadingShow(path: "message.html", title: "", remote: false) == nil)
        #expect(ReadingShow(path: "/a", title: "", remote: false) != nil)
        #expect(ReadingShow(path: "/a", title: "", remote: false, header: Array(repeating: "x", count: 8)) != nil)
        #expect(ReadingShow(path: "/a", title: "", remote: false, header: Array(repeating: "x", count: 9)) == nil)
        #expect(ReadingShow(path: "/a", title: "", remote: false, header: [""]) == nil)
        #expect(ReadingShow(path: "/a", title: "", remote: false, header: ["two\nlines"]) == nil)
        #expect(ReadingShow(path: "/a", title: "", remote: false, header: ["bell\u{7}"]) == nil)
        #expect(
            ReadingShow(path: "/a", title: "", remote: false, header: [String(repeating: "é", count: 1000)]) != nil)
        #expect(
            ReadingShow(path: "/a", title: "", remote: false, header: [String(repeating: "é", count: 1001)]) == nil)
        // A header is required, even an empty one, and nothing else is let in.
        let bare = #"{"jsonrpc":"2.0","method":"reading.show","params":{"path":"/a","title":"","remote":false}}"#
        #expect(throws: BridgeError.self) { try CoreToShell(line: bare) }
        let extra =
            #"{"jsonrpc":"2.0","method":"reading.show","params":{"path":"/a","title":"","remote":false,"header":[],"focus":true}}"#
        #expect(throws: BridgeError.self) { try CoreToShell(line: extra) }
        let closing = #"{"jsonrpc":"2.0","method":"reading.close","params":{"now":true}}"#
        #expect(throws: BridgeError.self) { try CoreToShell(line: closing) }
    }

    @Test func anEditorsPlaceIsCellsOfTheScreenOrNowhereAndNeverTheKeysWithoutAPlace() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        let cells = try #require(EditorPlace.Cells(column: 70, row: 7, columns: 76, rows: 30, covered: 2))
        #expect(
            try CoreToShell(line: examples["editor.place.json"]!)
                == .editorPlace(EditorPlace(cells: cells, keys: true)))
        let hidden = #"{"jsonrpc":"2.0","method":"editor.place","params":{"keys":false}}"#
        #expect(try CoreToShell(line: hidden) == .editorPlace(.hidden))
        #expect(try Examples.value(CoreToShell.editorPlace(.hidden).line) == Examples.value(hidden))
        #expect(EditorPlace.hidden.cells == nil && !EditorPlace.hidden.keys)

        // Nothing to run an editor in, or nowhere a screen has.
        #expect(EditorPlace.Cells(column: 70, row: 7, columns: 0, rows: 30, covered: 0) == nil)
        #expect(EditorPlace.Cells(column: 70, row: 7, columns: 76, rows: 0, covered: 0) == nil)
        #expect(EditorPlace.Cells(column: -1, row: 7, columns: 76, rows: 30, covered: 0) == nil)
        #expect(EditorPlace.Cells(column: 70, row: 7, columns: 65536, rows: 30, covered: 0) == nil)
        for bad in [
            #"{}"#, #"{"keys":true}"#, #"{"keys":"yes"}"#, #"{"keys":false,"place":null}"#,
            #"{"keys":false,"place":{"column":70,"row":7,"columns":0,"rows":30,"covered":0}}"#,
            #"{"keys":false,"place":{"column":70,"row":7,"columns":76,"rows":30}}"#,
            #"{"keys":false,"place":{"column":70.5,"row":7,"columns":76,"rows":30,"covered":0}}"#,
            #"{"keys":false,"place":{"column":70,"row":7,"columns":76,"rows":30,"covered":0,"points":9}}"#,
            #"{"keys":false,"hidden":true}"#,
        ] {
            let line = #"{"jsonrpc":"2.0","method":"editor.place","params":\#(bad)}"#
            #expect(throws: BridgeError.self, "\(bad)") { try CoreToShell(line: line) }
        }
    }

    @Test func theKeysAreAListUnderHeadingsAndTheWindowOfThemIsAskedForByItself() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        let list = KeysList(
            title: "Keyboard shortcuts",
            sections: [
                KeysList.Section(
                    title: "Moving",
                    keys: [
                        KeysList.Line(title: "Next message", keys: ["j", "down"]),
                        KeysList.Line(title: "First message", keys: ["g g"]),
                    ]),
                KeysList.Section(
                    title: "Writing a message",
                    keys: [
                        KeysList.Line(title: "Send", keys: ["cmd-enter", "ctrl-x s"]),
                        // A command no key is bound to is still listed.
                        KeysList.Line(title: "Send later", keys: []),
                    ]),
            ])
        #expect(try CoreToShell(line: examples["keys.list.json"]!) == .keysList(list))
        #expect(try CoreToShell(line: examples["keys.show.json"]!) == .keysShow)
        #expect(CoreToShell.keysShow.line == #"{"jsonrpc":"2.0","method":"keys.show","params":{}}"#)
        for bad in [
            #"{}"#, #"{"title":"Keys"}"#, #"{"sections":[]}"#, #"{"title":"Keys","sections":[],"front":true}"#,
            #"{"title":"Keys","sections":[{"title":"Moving"}]}"#,
            #"{"title":"Keys","sections":[{"title":"Moving","keys":[],"open":true}]}"#,
            #"{"title":"Keys","sections":[{"title":"Moving","keys":[{"title":"Next","keys":"j"}]}]}"#,
            #"{"title":"Keys","sections":[{"title":"Moving","keys":[{"title":"Next","keys":["j"],"name":"nav.next"}]}]}"#,
        ] {
            let line = #"{"jsonrpc":"2.0","method":"keys.list","params":\#(bad)}"#
            #expect(throws: BridgeError.self, "\(bad)") { try CoreToShell(line: line) }
        }
        #expect(throws: BridgeError.self) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"keys.show","params":{"front":true}}"#)
        }
        // Editing the rules is asked for with nothing said: no path.
        #expect(try CoreToShell(line: examples["rules.edit.json"]!) == .rulesEdit)
        #expect(CoreToShell.rulesEdit.line == #"{"jsonrpc":"2.0","method":"rules.edit","params":{}}"#)
        #expect(throws: BridgeError.self) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"rules.edit","params":{"path":"/etc/hosts"}}"#)
        }
        // Closing the window is asked for the same way, with nothing said.
        #expect(try CoreToShell(line: examples["window.close.json"]!) == .windowClose)
        #expect(CoreToShell.windowClose.line == #"{"jsonrpc":"2.0","method":"window.close","params":{}}"#)
        #expect(throws: BridgeError.self) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"window.close","params":{"quit":true}}"#)
        }
        // A window is brought forward: the main one with nothing said, or
        // the settings window on a pane, by a name a pane could have.
        #expect(try CoreToShell(line: examples["window.show.json"]!) == .windowShow(settings: "agents"))
        #expect(
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"window.show","params":{}}"#)
                == .windowShow(settings: nil))
        #expect(
            CoreToShell.windowShow(settings: nil).line
                == #"{"jsonrpc":"2.0","method":"window.show","params":{}}"#)
        #expect(throws: BridgeError.self) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"window.show","params":{"settings":"../etc"}}"#)
        }
        #expect(throws: BridgeError.self) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"window.show","params":{"front":true}}"#)
        }
    }

    @Test func thePlaceOfTheReadingPaneIsAWholeNumberOfColumnsBothWays() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        #expect(
            try CoreToShell(line: examples["reading.place.json"]!)
                == .readingPlace(ReadingPlace(columns: 78, window: 147)!))
        let answers = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        #expect(
            try ShellToCore(line: answers["window.resized.json"]!)
                == .windowResized(WindowResized(columns: 147)!))
        #expect(
            ShellToCore.windowResized(WindowResized(columns: 147)!).line
                == #"{"jsonrpc":"2.0","method":"window.resized","params":{"columns":147}}"#)

        // Neither can be built holding what the other side would refuse.
        #expect(ReadingPlace(columns: 0, window: 147) == nil && ReadingPlace(columns: -3, window: 147) == nil)
        #expect(ReadingPlace(columns: 65535, window: 65535) != nil)
        #expect(ReadingPlace(columns: 65536, window: 65536) == nil)
        // The terminal never keeps more columns than the window has.
        #expect(ReadingPlace(columns: 148, window: 147) == nil && ReadingPlace(columns: 78, window: 0) == nil)
        #expect(WindowResized(columns: 0) == nil && WindowResized(columns: 65536) == nil)
        #expect(WindowResized(columns: 1) != nil)
        for bad in [
            #"{"columns":0}"#, #"{"columns":78.5}"#, #"{"columns":"78"}"#, #"{}"#, #"{"columns":78,"points":624}"#,
        ] {
            let place = #"{"jsonrpc":"2.0","method":"reading.place","params":\#(bad)}"#
            #expect(throws: BridgeError.self, "\(bad)") { try CoreToShell(line: place) }
            let resized = #"{"jsonrpc":"2.0","method":"window.resized","params":\#(bad)}"#
            #expect(throws: BridgeError.self, "\(bad)") { try ShellToCore(line: resized) }
        }
        for bad in [
            #"{"columns":78}"#, #"{"window":147}"#, #"{"columns":148,"window":147}"#,
            #"{"columns":78,"window":0}"#, #"{"columns":78,"window":147.5}"#,
        ] {
            let place = #"{"jsonrpc":"2.0","method":"reading.place","params":\#(bad)}"#
            #expect(throws: BridgeError.self, "\(bad)") { try CoreToShell(line: place) }
        }
    }

    @Test func aDividerDraggedBesideTheReadingPaneIsColumnsOrPutBack() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        #expect(
            try ShellToCore(line: examples["reading.dragged.json"]!)
                == .readingDragged(ReadingDragged(columns: 90)!))
        #expect(
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"reading.dragged","params":{}}"#)
                == .readingDragged(.back))
        #expect(
            ShellToCore.readingDragged(ReadingDragged(columns: 90)!).line
                == #"{"jsonrpc":"2.0","method":"reading.dragged","params":{"columns":90}}"#)
        #expect(
            ShellToCore.readingDragged(.back).line == #"{"jsonrpc":"2.0","method":"reading.dragged","params":{}}"#)
        #expect(ReadingDragged(columns: 0) == nil && ReadingDragged(columns: 65536) == nil)
        for bad in [
            #"{"columns":0}"#, #"{"columns":90.5}"#, #"{"columns":"90"}"#, #"{"columns":null}"#,
            #"{"columns":90,"points":720}"#, #"{"points":720}"#,
        ] {
            let dragged = #"{"jsonrpc":"2.0","method":"reading.dragged","params":\#(bad)}"#
            #expect(throws: BridgeError.self, "\(bad)") { try ShellToCore(line: dragged) }
        }
    }

    @Test func droppedFilesAreAbsolutePathsAndAtLeastOne() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        guard case let .filesDropped(files) = try ShellToCore(line: examples["files.dropped.json"]!) else {
            Issue.record("not a files.dropped")
            return
        }
        #expect(files.paths.count == 2)
        #expect(files.paths[1].hasSuffix("/Q4 notes.txt"))
        #expect(files.temporary)

        // It cannot be built holding anything the core would refuse.
        #expect(FilesDropped(paths: [], temporary: true) == nil)
        #expect(FilesDropped(paths: ["notes.txt"], temporary: false) == nil)
        #expect(FilesDropped(paths: ["/tmp/a", "b"], temporary: false) == nil)
        #expect(FilesDropped(paths: ["/tmp/a b.txt"], temporary: false)?.paths == ["/tmp/a b.txt"])
    }

    @Test func notificationsDecodeToWhatTheySay() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        let actions = [
            NotifyAction(name: "archive", title: "Archive")!,
            NotifyAction(name: "read", title: "Mark as Read")!,
            // A reply is written in the window, so its button brings it forward.
            NotifyAction(name: "reply", title: "Reply", foreground: true)!,
        ]
        #expect(
            try CoreToShell(line: examples["notify.post.json"]!)
                == .notifyPost(
                    NotifyPost(
                        id: "message:4127", title: "Ana Ruiz", subtitle: "Work", body: "Q4 roadmap", actions: actions)!
                ))
        #expect(
            try CoreToShell(line: examples["notify.clear.json"]!)
                == .notifyClear(NotifyClear(ids: ["message:4127", "message:4128"])!))

        let answers = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        #expect(
            try ShellToCore(line: answers["notify.used.json"]!)
                == .notifyUsed(NotifyUsed(id: "message:4127", action: "archive")!))

        // With no subtitle, no buttons and no words below the title; and a
        // click on the notification itself, which names no action.
        let bare = try CoreToShell(
            line: #"{"jsonrpc":"2.0","method":"notify.post","params":{"id":"m","title":"Ana","body":"","actions":[]}}"#)
        #expect(bare == .notifyPost(NotifyPost(id: "m", title: "Ana", body: "")!))
        #expect(!bare.line.contains("subtitle"))
        let click = ShellToCore.notifyUsed(NotifyUsed(id: "message:4127")!).line
        #expect(click == #"{"jsonrpc":"2.0","method":"notify.used","params":{"id":"message:4127"}}"#)
        #expect(try ShellToCore(line: click) == .notifyUsed(NotifyUsed(id: "message:4127")!))
    }

    @Test func notificationsHoldOnlyWhatTheSchemaAllows() {
        let long = String(repeating: "a", count: 129)
        let action = NotifyAction(name: "archive", title: "Archive")!

        #expect(NotifyPost(id: "", title: "Ana", body: "") == nil)
        #expect(NotifyPost(id: long, title: "Ana", body: "") == nil)
        #expect(NotifyPost(id: String(long.dropLast()), title: "Ana", body: "") != nil)
        // The limit is in bytes: 43 of these letters are 129.
        #expect(NotifyPost(id: String(repeating: "\u{20AC}", count: 43), title: "Ana", body: "") == nil)
        #expect(NotifyPost(id: "m", title: "", body: "Q4 roadmap") == nil)
        #expect(NotifyPost(id: "m", title: "Ana", body: "", actions: Array(repeating: action, count: 4)) != nil)
        #expect(NotifyPost(id: "m", title: "Ana", body: "", actions: Array(repeating: action, count: 5)) == nil)

        #expect(NotifyAction(name: "", title: "Archive") == nil)
        #expect(NotifyAction(name: "archive", title: "") == nil)
        #expect(NotifyAction(name: String(repeating: "a", count: 64), title: "Archive") != nil)
        #expect(NotifyAction(name: String(repeating: "a", count: 65), title: "Archive") == nil)

        #expect(NotifyClear(ids: []) == nil)
        #expect(NotifyClear(ids: ["m", ""]) == nil)
        #expect(NotifyClear(ids: ["m", long]) == nil)
        #expect(NotifyClear(ids: ["m"])?.ids == ["m"])

        #expect(NotifyUsed(id: "") == nil)
        #expect(NotifyUsed(id: long) == nil)
        #expect(NotifyUsed(id: "m", action: "") == nil)
        #expect(NotifyUsed(id: "m", action: String(repeating: "a", count: 65)) == nil)
        #expect(NotifyUsed(id: "m", action: "archive")?.action == "archive")
    }

    @Test func onlyMailAddressesArePassedOn() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        let address = "mailto:ana.ruiz@example.com?subject=Q4%20roadmap&cc=kit@example.com"
        #expect(try ShellToCore(line: examples["url.open.json"]!) == .urlOpen(UrlOpen(url: address)!))

        // As the Mac hands one over, the scheme may be in either case. What
        // is passed on has it in lower case, and the rest as it came.
        #expect(UrlOpen(handedOver: address)?.url == address)
        #expect(
            UrlOpen(handedOver: "MAILTO:Ana.Ruiz@Example.com?Subject=Hi")?.url
                == "mailto:Ana.Ruiz@Example.com?Subject=Hi")
        #expect(UrlOpen(handedOver: "MailTo:robin@example.com")?.url == "mailto:robin@example.com")
        // The Mac hands over a URL; what is looked at is how it is written.
        let handed = URL(string: "MAILTO:ana.ruiz@example.com?subject=Q4%20roadmap&cc=kit@example.com")!
        #expect(UrlOpen(handedOver: handed.absoluteString)?.url == address)
        #expect(UrlOpen(handedOver: URL(fileURLWithPath: "/tmp/mailto:robin@example.com").absoluteString) == nil)
        // An address to nobody starts an empty message.
        #expect(UrlOpen(handedOver: "mailto:")?.url == "mailto:")
        #expect(UrlOpen(handedOver: "mailto:j\u{FC}rgen@example.com")?.url == "mailto:j\u{FC}rgen@example.com")
        for other in [
            "", "mailto", "mailt:robin@example.com", "https://example.com/", "http://example.com/mailto:robin",
            "file:///etc/passwd", "message://%3Cabc@example.com%3E", "x-apple-something://do",
            " mailto:robin@example.com", "mailto\u{1A}robin@example.com", "ma\u{130}lto:robin@example.com",
            "robin@example.com",
        ] {
            #expect(UrlOpen(handedOver: other) == nil, "\(other)")
            #expect(UrlOpen(url: other) == nil, "\(other)")
        }

        // The core takes only the lower case; the type holds nothing else.
        #expect(UrlOpen(url: "MAILTO:robin@example.com") == nil)

        // At most 8192 bytes, counted after the scheme is lowered.
        let longest = "mailto:" + String(repeating: "a", count: 8192 - 7)
        #expect(UrlOpen(url: longest) != nil)
        #expect(UrlOpen(url: longest + "a") == nil)
        #expect(UrlOpen(handedOver: "MAILTO:" + longest.dropFirst(7))?.url == longest)
        #expect(UrlOpen(handedOver: longest + "a") == nil)
        #expect(UrlOpen(handedOver: "mailto:" + String(repeating: "\u{FC}", count: 4093)) == nil)
    }

    @Test func spellingDecodesToWhatItSays() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        let text = "The agenda is atached. Thanks for the feedbak."
        #expect(try CoreToShell(line: examples["spell.check.json"]!) == .spellCheck(SpellCheck(id: 7, text: text)!))
        #expect(
            try CoreToShell(line: examples["spell.suggest.json"]!)
                == .spellSuggest(SpellSuggest(id: 8, word: "atached")!))

        let answers = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        guard case let .spellChecked(checked) = try ShellToCore(line: answers["spell.checked.json"]!) else {
            Issue.record("not a spell.checked")
            return
        }
        #expect(checked.id == 7)
        #expect(checked.words == [Misspelt(start: 14, end: 21)!, Misspelt(start: 38, end: 45)!])
        // The offsets are bytes of the text as UTF-8, which is what they cut.
        let bytes = Array(text.utf8)
        #expect(
            checked.words.map { String(decoding: bytes[$0.start..<$0.end], as: UTF8.self) } == ["atached", "feedbak"])
        #expect(
            try ShellToCore(line: answers["spell.suggested.json"]!)
                == .spellSuggested(SpellSuggested(id: 8, words: ["attached", "attaches", "attacked"])!))

        // Nothing to check, nothing misspelt and nothing to suggest are all
        // things to say.
        #expect(
            CoreToShell.spellCheck(SpellCheck(id: 0, text: "")!).line
                == #"{"jsonrpc":"2.0","method":"spell.check","params":{"id":0,"text":""}}"#)
        let none = ShellToCore.spellChecked(SpellChecked(id: 7, words: [])!).line
        #expect(none == #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7,"words":[]}}"#)
        #expect(try ShellToCore(line: none) == .spellChecked(SpellChecked(id: 7, words: [])!))
        let noIdea = ShellToCore.spellSuggested(SpellSuggested(id: 8, words: [])!).line
        #expect(noIdea == #"{"jsonrpc":"2.0","method":"spell.suggested","params":{"id":8,"words":[]}}"#)
        #expect(try ShellToCore(line: noIdea) == .spellSuggested(SpellSuggested(id: 8, words: [])!))
        #expect(
            ShellToCore.spellChecked(SpellChecked(id: 7, words: [Misspelt(start: 14, end: 21)!])!).line
                == #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7,"words":[{"end":21,"start":14}]}}"#)

        // Text keeps its line breaks and its letters through the line.
        let letters = SpellCheck(id: 3, text: "Gr\u{FC}\u{DF}e,\nJ\u{FC}rgen \u{1F44D}\u{1F3FD}")!
        #expect(!CoreToShell.spellCheck(letters).line.contains("\n"))
        #expect(try CoreToShell(line: CoreToShell.spellCheck(letters).line) == .spellCheck(letters))
    }

    @Test func spellingHoldsOnlyWhatTheSchemaAllows() {
        // At most 100000 bytes of text, however few characters that is.
        #expect(SpellCheck(id: 0, text: "") != nil)
        #expect(SpellCheck(id: -1, text: "word") == nil)
        #expect(SpellCheck(id: 1, text: String(repeating: "a", count: 100_000)) != nil)
        #expect(SpellCheck(id: 1, text: String(repeating: "a", count: 100_001)) == nil)
        #expect(SpellCheck(id: 1, text: String(repeating: "\u{20AC}", count: 33_333)) != nil)
        #expect(SpellCheck(id: 1, text: String(repeating: "\u{20AC}", count: 33_334)) == nil)

        // A word of 1 to 200 bytes.
        #expect(SpellSuggest(id: 1, word: "") == nil)
        #expect(SpellSuggest(id: -1, word: "atached") == nil)
        #expect(SpellSuggest(id: 1, word: String(repeating: "a", count: 200)) != nil)
        #expect(SpellSuggest(id: 1, word: String(repeating: "a", count: 201)) == nil)
        #expect(SpellSuggest(id: 1, word: String(repeating: "\u{FC}", count: 100)) != nil)
        #expect(SpellSuggest(id: 1, word: String(repeating: "\u{FC}", count: 101)) == nil)

        // A misspelt word ends after it starts, and starts in the text.
        #expect(Misspelt(start: 0, end: 1) != nil)
        #expect(Misspelt(start: 0, end: 0) == nil)
        #expect(Misspelt(start: 5, end: 5) == nil)
        #expect(Misspelt(start: 5, end: 4) == nil)
        #expect(Misspelt(start: -1, end: 4) == nil)

        // Misspelt words come in order and do not overlap. One may start
        // where the last ended.
        let first = Misspelt(start: 2, end: 6)!
        let second = Misspelt(start: 6, end: 9)!
        let inside = Misspelt(start: 5, end: 9)!
        #expect(SpellChecked(id: 1, words: [])?.words.isEmpty == true)
        #expect(SpellChecked(id: 1, words: [first, second])?.words == [first, second])
        #expect(SpellChecked(id: -1, words: [first]) == nil)
        #expect(SpellChecked(id: 1, words: [second, first]) == nil)
        #expect(SpellChecked(id: 1, words: [first, inside]) == nil)
        #expect(SpellChecked(id: 1, words: [first, first]) == nil)
        // At most 2000 of them.
        let many = (0..<2001).map { Misspelt(start: $0 * 3, end: $0 * 3 + 2)! }
        #expect(SpellChecked(id: 1, words: Array(many.prefix(2000)))?.words.count == 2000)
        #expect(SpellChecked(id: 1, words: many) == nil)

        // At most 20 suggestions, each 1 to 200 bytes.
        #expect(SpellSuggested(id: 1, words: [])?.words.isEmpty == true)
        #expect(SpellSuggested(id: -1, words: ["attached"]) == nil)
        #expect(SpellSuggested(id: 1, words: Array(repeating: "w", count: 20))?.words.count == 20)
        #expect(SpellSuggested(id: 1, words: Array(repeating: "w", count: 21)) == nil)
        #expect(SpellSuggested(id: 1, words: ["attached", ""]) == nil)
        #expect(SpellSuggested(id: 1, words: [String(repeating: "a", count: 200)]) != nil)
        #expect(SpellSuggested(id: 1, words: [String(repeating: "a", count: 201)]) == nil)
        #expect(SpellSuggested(id: 1, words: [String(repeating: "\u{FC}", count: 101)]) == nil)
    }

    @Test func spellingErrorsSayWhatIsWrong() {
        let longText = String(repeating: "a", count: 100_001)
        #expect(
            throws: BridgeError.malformed("spell.check: the id must not be negative and the text at most 100000 bytes")
        ) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"spell.check","params":{"id":1,"text":"\#(longText)"}}"#)
        }
        #expect(
            throws: BridgeError.malformed("spell.check: the id must not be negative and the text at most 100000 bytes")
        ) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"spell.check","params":{"id":-1,"text":"word"}}"#)
        }
        #expect(throws: BridgeError.malformed("spell.check: missing text")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"spell.check","params":{"id":1}}"#)
        }
        #expect(throws: BridgeError.malformed("spell.check: unexpected language")) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"spell.check","params":{"id":1,"text":"word","language":"en"}}"#)
        }
        #expect(throws: BridgeError.self) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"spell.check","params":{"id":1.5,"text":"word"}}"#)
        }
        #expect(throws: BridgeError.self) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"spell.check","params":{"id":"1","text":"word"}}"#)
        }
        #expect(
            throws: BridgeError.malformed("spell.suggest: the id must not be negative and the word 1 to 200 bytes")
        ) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"spell.suggest","params":{"id":8,"word":""}}"#)
        }
        #expect(throws: BridgeError.malformed("spell.checked: a misspelt word must end after it starts")) {
            try ShellToCore(
                line: #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7,"words":[{"start":0,"end":0}]}}"#)
        }
        #expect(throws: BridgeError.malformed("spell.checked: a misspelt word must end after it starts")) {
            try ShellToCore(
                line: #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7,"words":[{"start":-2,"end":3}]}}"#)
        }
        #expect(
            throws: BridgeError.malformed(
                "spell.checked: the id must not be negative and the words at most 2000, in order")
        ) {
            try ShellToCore(
                line:
                    #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7,"words":[{"start":4,"end":9},{"start":8,"end":12}]}}"#
            )
        }
        #expect(throws: BridgeError.malformed("spell.checked: unexpected word")) {
            try ShellToCore(
                line:
                    #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7,"words":[{"start":0,"end":4,"word":"teh"}]}}"#
            )
        }
        #expect(throws: BridgeError.malformed("spell.checked: missing words")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7}}"#)
        }
        #expect(
            throws: BridgeError.malformed(
                "spell.suggested: the id must not be negative and the words at most 20, each 1 to 200 bytes")
        ) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"spell.suggested","params":{"id":8,"words":[""]}}"#)
        }
    }

    @Test func theEditorDecodesToWhatItSays() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("core-to-shell").map { ($0.name, $0.text) })
        let folder = "/Users/robin/Library/Caches/Quill/edit-812"
        let asked = PaneEditor(
            dir: folder, title: "Re: Q4 roadmap", editor: .vim,
            argv: [
                "/opt/homebrew/bin/vim", "-N", "--cmd",
                "augroup QuillNoModelines | execute 'autocmd BufReadPre * setlocal nomodeline' | augroup END",
                "-c", "execute 'source' fnameescape($QUILL_DRAFT_DIR . '/quill.vim')", "--",
                folder + "/message.quill",
            ])!
        #expect(try CoreToShell(line: examples["pane.editor.json"]!) == .paneEditor(asked))
        #expect(asked.program == "/opt/homebrew/bin/vim")
        #expect(asked.arguments.first == "-N" && asked.arguments.last == folder + "/message.quill")

        let answers = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        #expect(
            try ShellToCore(line: answers["editor.exited.json"]!)
                == .editorExited(EditorExited(ran: true, status: 0)!))

        // A build with no Vim in it answers at once.
        #expect(
            ShellToCore.editorExited(.didNotRun).line
                == #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":false,"status":0}}"#)
        #expect(try ShellToCore(line: ShellToCore.editorExited(.didNotRun).line) == .editorExited(.didNotRun))
        // `:cq`, and a Vim ended by a signal.
        #expect(
            ShellToCore.editorExited(EditorExited(ranWithStatus: 1)).line
                == #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":1}}"#)
        let hungUp = ShellToCore.editorExited(EditorExited(ranWithStatus: -1))
        #expect(hungUp.line == #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":-1}}"#)
        #expect(try ShellToCore(line: hungUp.line) == hungUp)

        // A folder, an argument and a title keep whatever is in them
        // through the line: each is a value, never part of a command.
        let awkward = PaneEditor(
            dir: "/tmp/it's \"quoted\"/line\nbreak", title: "Re: \u{1F4EC} \"hello\"\n",
            argv: ["/opt/my vim/vim", "-c", "echo 'a b' | q", "/tmp/it's \"quoted\"/line\nbreak/caf\u{E9}.quill"])!
        #expect(!CoreToShell.paneEditor(awkward).line.contains("\n"))
        #expect(try CoreToShell(line: CoreToShell.paneEditor(awkward).line) == .paneEditor(awkward))
        // No title is a title.
        #expect(
            CoreToShell.paneEditor(PaneEditor(dir: "/d", title: "", argv: ["/usr/bin/vim", "/d/m"])!).line
                == #"{"jsonrpc":"2.0","method":"pane.editor","params":{"argv":["/usr/bin/vim","/d/m"],"dir":"/d","editor":"vim","title":""}}"#
        )
        // Each kind of editor is said by its word, and each word comes
        // back as the kind it was.
        #expect(EditorKind.allCases.map(\.rawValue) == ["vim", "nano", "mg"])
        for kind in EditorKind.allCases {
            let asked = CoreToShell.paneEditor(
                PaneEditor(dir: "/d", title: "", editor: kind, argv: ["/usr/bin/x", "+12", "/d/m"])!)
            #expect(asked.line.contains(#""editor":"\#(kind.rawValue)""#))
            #expect(try CoreToShell(line: asked.line) == asked)
        }
    }

    @Test func theEditorHoldsOnlyWhatTheSchemaAllows() {
        // The folder absolute, and the program named by its absolute path:
        // never a name to be looked for, and never nothing.
        let argv = ["/usr/bin/vim", "/d/message.quill"]
        #expect(PaneEditor(dir: "/d", title: "x", argv: argv) != nil)
        #expect(PaneEditor(dir: "d", title: "x", argv: argv) == nil)
        #expect(PaneEditor(dir: "", title: "x", argv: argv) == nil)
        #expect(PaneEditor(dir: "~/d", title: "x", argv: argv) == nil)
        #expect(PaneEditor(dir: "/d", title: "x", argv: []) == nil)
        #expect(PaneEditor(dir: "/d", title: "x", argv: ["vim", "/d/message.quill"]) == nil)
        #expect(PaneEditor(dir: "/d", title: "x", argv: ["./vim"]) == nil)
        // No more arguments, and none longer, than an editor is given, and
        // none that could end early as a C string.
        let most = PaneEditor.mostArguments
        #expect(PaneEditor(dir: "/d", title: "x", argv: ["/v"] + Array(repeating: "a", count: most - 1)) != nil)
        #expect(PaneEditor(dir: "/d", title: "x", argv: ["/v"] + Array(repeating: "a", count: most)) == nil)
        let longest = String(repeating: "a", count: PaneEditor.longestArgument)
        #expect(PaneEditor(dir: "/d", title: "x", argv: ["/v", longest]) != nil)
        #expect(PaneEditor(dir: "/d", title: "x", argv: ["/v", longest + "a"]) == nil)
        #expect(PaneEditor(dir: "/d", title: "x", argv: ["/v", "a\u{0}b"]) == nil)

        // A status from -255 to 255.
        #expect(EditorExited.statuses == -255...255)
        #expect(EditorExited(ran: true, status: 255)?.status == 255)
        #expect(EditorExited(ran: true, status: -255)?.status == -255)
        #expect(EditorExited(ran: true, status: 256) == nil)
        #expect(EditorExited(ran: true, status: -256) == nil)
        #expect(EditorExited(ran: false, status: 0) == .didNotRun)
        #expect(!EditorExited.didNotRun.ran)
        #expect(EditorExited.didNotRun.status == 0)
        // A status that is out of range is brought into it, not dropped:
        // the core is waiting on an answer.
        #expect(EditorExited(ranWithStatus: 0) == EditorExited(ran: true, status: 0))
        #expect(EditorExited(ranWithStatus: 4000) == EditorExited(ran: true, status: 255))
        #expect(EditorExited(ranWithStatus: -4000) == EditorExited(ran: true, status: -255))
    }

    @Test func editorErrorsSayWhatIsWrong() {
        let unusable = BridgeError.malformed(
            "pane.editor: the dir must be absolute and the argv a program by its absolute path")
        for bad in [
            #"{"dir":"d","title":"x","editor":"vim","argv":["/usr/bin/vim","/d/m"]}"#,
            #"{"dir":"/d","title":"x","editor":"vim","argv":["vim","/d/m"]}"#,
            #"{"dir":"/d","title":"x","editor":"vim","argv":[]}"#,
        ] {
            #expect(throws: unusable, "\(bad)") {
                try CoreToShell(line: #"{"jsonrpc":"2.0","method":"pane.editor","params":\#(bad)}"#)
            }
        }
        // What kind of editor it is is one of a few words. A program's
        // path is not one, nor leaving it out.
        for editor in ["/usr/bin/vim", "/opt/homebrew/bin/nvim", "Vim", "pico", "emacs", "script", ""] {
            #expect(throws: BridgeError.malformed("pane.editor: editor is vim, nano or mg, never a program")) {
                try CoreToShell(
                    line:
                        #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","title":"x","editor":"\#(editor)","argv":["/usr/bin/vim"]}}"#
                )
            }
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing editor")) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","title":"x","argv":["/v"]}}"#)
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing argv")) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","title":"x","editor":"vim"}}"#)
        }
        // The command is an argv, never a line.
        #expect(throws: BridgeError.self) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","title":"x","editor":"vim","argv":"/usr/bin/vim /d/m"}}"#
            )
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing dir")) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"title":"x","editor":"vim","argv":["/v"]}}"#)
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing title")) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","editor":"vim","argv":["/v"]}}"#)
        }
        #expect(throws: BridgeError.malformed("pane.editor: unexpected path")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","title":"x","editor":"vim","argv":["/v"],"path":"/d/m"}}"#
            )
        }
        #expect(throws: BridgeError.malformed("editor.exited: the status must be from -255 to 255")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":256}}"#)
        }
        #expect(throws: BridgeError.malformed("editor.exited: the status must be from -255 to 255")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":-256}}"#)
        }
        #expect(throws: BridgeError.malformed("editor.exited: missing status")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true}}"#)
        }
        #expect(throws: BridgeError.malformed("editor.exited: missing ran")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"status":0}}"#)
        }
        #expect(throws: BridgeError.malformed("editor.exited: unexpected signal")) {
            try ShellToCore(
                line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":0,"signal":1}}"#)
        }
        // Not a whole number, not a number, and not a yes or no.
        #expect(throws: BridgeError.self) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":1.5}}"#)
        }
        #expect(throws: BridgeError.self) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":"0"}}"#)
        }
        #expect(throws: BridgeError.self) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":1,"status":0}}"#)
        }
        // Each belongs to one direction only.
        #expect(throws: BridgeError.unknownMethod("editor.exited")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"editor.exited","params":{"ran":true,"status":0}}"#)
        }
        #expect(throws: BridgeError.unknownMethod("pane.editor")) {
            try ShellToCore(
                line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","title":"x"}}"#)
        }
    }

    @Test func errorsSayWhatIsWrong() {
        #expect(throws: BridgeError.version("1.0")) {
            try CoreToShell(line: #"{"jsonrpc":"1.0","method":"pane.close","params":{}}"#)
        }
        #expect(throws: BridgeError.unknownMethod("mail.delete")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"mail.delete","params":{}}"#)
        }
        #expect(throws: BridgeError.malformed("invalid JSON")) {
            try ShellToCore(line: "not json")
        }
        #expect(throws: BridgeError.malformed("pane.preview: path is not absolute")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.preview","params":{"items":[{"path":"a.pdf","title":"a"}],"index":0}}"#
            )
        }
        #expect(throws: BridgeError.malformed("pane.preview: index is not one of the items")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.preview","params":{"items":[{"path":"/a.pdf","title":"a"}],"index":1}}"#
            )
        }
        #expect(throws: BridgeError.malformed("pane.preview: unexpected size")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.preview","params":{"items":[{"path":"/a.pdf","title":"a","size":3}],"index":0}}"#
            )
        }
        #expect(throws: BridgeError.malformed("link.open: not an http, https or mailto address")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"link.open","params":{"url":"file:///etc/passwd"}}"#)
        }
        #expect(throws: BridgeError.malformed("url.open: not a mailto address of at most 8192 bytes")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"url.open","params":{"url":"MAILTO:robin@example.com"}}"#)
        }
        #expect(throws: BridgeError.malformed("notify.post: unexpected icon")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"notify.post","params":{"id":"m","title":"Ana","body":"","actions":[{"name":"a","title":"A","icon":"x"}]}}"#
            )
        }
        #expect(throws: BridgeError.malformed("notify.post: an action needs a title and a name of 1 to 64 bytes")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"notify.post","params":{"id":"m","title":"Ana","body":"","actions":[{"name":"","title":"A"}]}}"#
            )
        }
        #expect(throws: BridgeError.malformed("notify.post: subtitle is null")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"notify.post","params":{"id":"m","title":"Ana","subtitle":null,"body":"","actions":[]}}"#
            )
        }
        #expect(throws: BridgeError.malformed("notify.post: missing actions")) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"notify.post","params":{"id":"m","title":"Ana","body":""}}"#)
        }
        #expect(throws: BridgeError.malformed("notify.clear: there must be an id, and each 1 to 128 bytes")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"notify.clear","params":{"ids":[]}}"#)
        }
        #expect(throws: BridgeError.malformed("notify.used: action is null")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"notify.used","params":{"id":"m","action":null}}"#)
        }
        #expect(
            throws: BridgeError.malformed("notify.used: the id must be 1 to 128 bytes and the action's name 1 to 64")
        ) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"notify.used","params":{"id":"m","action":""}}"#)
        }
        #expect(throws: BridgeError.malformed("unexpected id")) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","id":1,"method":"app.will_quit","params":{}}"#)
        }
    }

    @Test func onlyWebAndMailAddressesAreOpenable() {
        for url in [
            "https://example.com/a?b=c&d=%20e#f", "HTTP://EXAMPLE.COM", "mailto:robin@example.com?subject=Hi",
        ] {
            #expect(Links.isOpenable(url), "\(url)")
            #expect(Links.openable(url) != nil, "\(url)")
        }
        for url in [
            "", "https://", "mailto:", "example.com", "/relative/path", "file:///etc/passwd",
            "javascript:alert(1)", "x-apple-something://do", "tel:+15555550100", "ftp://example.com/file",
            "https://example.com/a b", "https://example.com/\u{1B}]0;x\u{07}", "https://ex\u{430}mple.com/",
            " https://example.com/",
        ] {
            #expect(!Links.isOpenable(url), "\(url)")
            #expect(Links.openable(url) == nil, "\(url)")
        }
    }

    @Test func theAppearanceIsSaidWithTheStateAndLeftOutWhenItIsTheMacs() throws {
        func state(_ appearance: String) -> String {
            #"{"jsonrpc":"2.0","method":"state.update","params":{"checked":[],"context":"list","enabled":[],"title":"Inbox","unread":0\#(appearance)}}"#
        }
        guard case let .stateUpdate(plain) = try CoreToShell(line: state("")) else {
            Issue.record("not a state.update")
            return
        }
        // Left out: as the Mac is.
        #expect(plain.appearance == nil)
        #expect(try Examples.value(CoreToShell.stateUpdate(plain).line) == Examples.value(state("")))
        for (word, appearance) in [("light", Appearance.light), ("dark", .dark), ("system", .system)] {
            let line = state(#","appearance":"\#(word)""#)
            #expect(
                try CoreToShell(line: line)
                    == .stateUpdate(
                        StateUpdate(context: .list, enabled: [], unread: 0, title: "Inbox", appearance: appearance)))
            #expect(
                try Examples.value(
                    CoreToShell.stateUpdate(
                        StateUpdate(context: .list, enabled: [], unread: 0, title: "Inbox", appearance: appearance)
                    ).line) == Examples.value(line))
        }
        for bad in [#""Dark""#, #""auto""#, #""""#, "null", "true", "1"] {
            #expect(throws: BridgeError.self, "\(bad)") {
                try CoreToShell(line: state(#","appearance":\#(bad)"#))
            }
        }
    }

    @Test func theMacsAppearanceAndAnImportedThemeGoToTheCore() throws {
        let examples = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        #expect(try ShellToCore(line: examples["appearance.changed.json"]!) == .appearanceChanged(dark: true))
        #expect(
            try ShellToCore(line: examples["themes.imported.json"]!)
                == .themesImported(ThemesImported(name: "Dracula+ High Contrast")!))
        #expect(
            ShellToCore.appearanceChanged(dark: false).line
                == #"{"jsonrpc":"2.0","method":"appearance.changed","params":{"dark":false}}"#)
        #expect(
            ShellToCore.themesImported(ThemesImported(name: "Jack O' Lantern")!).line
                == #"{"jsonrpc":"2.0","method":"themes.imported","params":{"name":"Jack O' Lantern"}}"#)
        for bad in [
            #"{"jsonrpc":"2.0","method":"appearance.changed","params":{}}"#,
            #"{"jsonrpc":"2.0","method":"appearance.changed","params":{"dark":1}}"#,
            #"{"jsonrpc":"2.0","method":"appearance.changed","params":{"dark":true,"light":false}}"#,
            #"{"jsonrpc":"2.0","method":"themes.imported","params":{}}"#,
            #"{"jsonrpc":"2.0","method":"themes.imported","params":{"name":7}}"#,
            #"{"jsonrpc":"2.0","method":"themes.imported","params":{"name":"Dracula","file":"/tmp/x"}}"#,
            #"{"jsonrpc":"2.0","method":"themes.pick","params":{}}"#,
        ] {
            #expect(throws: BridgeError.self, "\(bad)") { try ShellToCore(line: bad) }
        }
        #expect(throws: BridgeError.self) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"themes.pick","params":{"folder":"/tmp"}}"#)
        }
    }

    @Test func aThemeIsNamedAsPeopleWriteAndNeverByAPath() {
        for name in [
            "Dracula", "3024 Day", "Dracula+ High Contrast", "Jack O' Lantern", "Builtin Tango (Light)", "v1.5",
            "x", "caf\u{e9}", String(repeating: "x", count: 100),
        ] {
            #expect(ThemeName.isValid(name), "\(name)")
            #expect(ThemesImported(name: name)?.name == name)
        }
        for name in [
            "", "../../config.toml", "a/b", "/etc/passwd", "a\\b", "a:b", ".hidden", "..", "Dracula.toml",
            "Dracula.TOML", " Dracula", "Dracula ", "two\nlines", "tab\there", "bell\u{7}", "del\u{7f}",
            "nbsp\u{a0}", String(repeating: "x", count: 101), String(repeating: "\u{e9}", count: 51),
        ] {
            #expect(!ThemeName.isValid(name), "\(name.debugDescription)")
            #expect(ThemesImported(name: name) == nil)
            // Nor does one arrive over the bridge.
            let line = ShellToCoreLine.imported(name)
            #expect(throws: BridgeError.self, "\(name.debugDescription)") { try ShellToCore(line: line) }
        }
    }

    @Test func optionalFieldsAreLeftOutRatherThanNull() {
        let line = CoreToShell.commandsList([CommandInfo(name: "nav.next", title: "Next", keys: ["j"])]).line
        #expect(!line.contains("null"))
        #expect(!line.contains("menu"))
    }
}

/// Lines as the shell would never write them, for the tests that check they
/// are refused.
enum ShellToCoreLine {
    static func imported(_ name: String) -> String {
        let quoted = String(decoding: (try? JSONEncoder().encode(name)) ?? Data(), as: UTF8.self)
        return #"{"jsonrpc":"2.0","method":"themes.imported","params":{"name":\#(quoted)}}"#
    }
}
