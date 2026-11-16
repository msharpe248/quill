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
                "clipboard.write.json", "commands.list.json", "file.open.json", "files.pick.json",
                "files.save.json", "link.open.json", "notify.clear.json", "notify.post.json", "pane.close.json",
                "pane.editor.json", "pane.play.json", "pane.preview.json", "pane.web.json", "spell.check.json",
                "spell.suggest.json", "state.update.json", "thumbnail.make.json",
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
                "app.will_quit.json", "command.run.json", "editor.exited.json", "files.dropped.json",
                "files.saved.json", "notify.used.json", "spell.checked.json", "spell.suggested.json",
                "thumbnail.made.json", "url.open.json",
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
            "misspelt-word-with-no-length.json", "too-many-suggestions.json", "editor-on-a-relative-path.json",
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
        #expect(commands.count == 5)
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
                        unread: 12,
                        title: "Inbox (unified) - 12 unread"
                    )))
        #expect(try CoreToShell(line: examples["pane.close.json"]!) == .paneClose)
        #expect(try CoreToShell(line: examples["files.pick.json"]!) == .filesPick)
        #expect(
            try CoreToShell(line: examples["clipboard.write.json"]!)
                == .clipboardWrite(text: "Are you free on Thursday?\nThe usual place, 12:30."))
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
        guard case let .paneWeb(web) = try CoreToShell(line: examples["pane.web.json"]!) else {
            Issue.record("not a pane.web")
            return
        }
        #expect(!web.remote)
        #expect(web.path.hasSuffix("/message.html"))
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
        #expect(PaneWeb(path: "message.html", title: "", remote: true) == nil)
        #expect(ThumbnailMake(id: 1, path: "/a", out: "/b", size: 8) == nil)
        #expect(ThumbnailMake(id: -1, path: "/a", out: "/b", size: 64) == nil)
        #expect(FilesSave(path: "/a", name: "") == nil)

        let answers = try Dictionary(uniqueKeysWithValues: Examples.load("shell-to-core").map { ($0.name, $0.text) })
        #expect(try ShellToCore(line: answers["thumbnail.made.json"]!) == .thumbnailMade(id: 7, ok: true))
        #expect(
            try ShellToCore(line: answers["files.saved.json"]!) == .filesSaved(name: "roadmap.pdf", saved: true))
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
        let folder = "/Users/robin/Library/Containers/dev.quill.Quill/Data/tmp/quill-edit-812"
        #expect(
            try CoreToShell(line: examples["pane.editor.json"]!)
                == .paneEditor(
                    PaneEditor(
                        path: folder + "/message.quill", dir: folder, title: "Re: Q4 roadmap", editor: .vim, line: 7)!))

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

        // Paths and a title keep whatever is in them through the line.
        let awkward = PaneEditor(
            path: "/tmp/it's \"quoted\"/line\nbreak/caf\u{E9}.quill", dir: "/tmp/it's \"quoted\"/line\nbreak",
            title: "Re: \u{1F4EC} \"hello\"\n")!
        #expect(!CoreToShell.paneEditor(awkward).line.contains("\n"))
        #expect(try CoreToShell(line: CoreToShell.paneEditor(awkward).line) == .paneEditor(awkward))
        // No title is a title.
        #expect(
            CoreToShell.paneEditor(PaneEditor(path: "/d/m", dir: "/d", title: "")!).line
                == #"{"jsonrpc":"2.0","method":"pane.editor","params":{"dir":"/d","editor":"vim","line":1,"path":"/d/m","title":""}}"#
        )
        // Each editor is asked for by its word, and each word comes back
        // as the editor it was.
        #expect(EditorKind.allCases.map(\.rawValue) == ["vim", "nano", "script"])
        for kind in EditorKind.allCases {
            let asked = CoreToShell.paneEditor(PaneEditor(path: "/d/m", dir: "/d", title: "", editor: kind, line: 12)!)
            #expect(asked.line.contains(#""editor":"\#(kind.rawValue)""#))
            #expect(asked.line.contains(#""line":12"#))
            #expect(try CoreToShell(line: asked.line) == asked)
        }
    }

    @Test func theEditorHoldsOnlyWhatTheSchemaAllows() {
        // Both paths absolute.
        #expect(PaneEditor(path: "/d/message.quill", dir: "/d", title: "x") != nil)
        #expect(PaneEditor(path: "message.quill", dir: "/d", title: "x") == nil)
        #expect(PaneEditor(path: "/d/message.quill", dir: "d", title: "x") == nil)
        #expect(PaneEditor(path: "/d/message.quill", dir: "", title: "x") == nil)
        #expect(PaneEditor(path: "", dir: "/d", title: "x") == nil)
        #expect(PaneEditor(path: "~/message.quill", dir: "/d", title: "x") == nil)

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
        #expect(throws: BridgeError.malformed("pane.editor: the path and the dir must be absolute")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"m.quill","dir":"/d","title":"x","editor":"vim","line":1}}"#
            )
        }
        #expect(throws: BridgeError.malformed("pane.editor: the path and the dir must be absolute")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"d","title":"x","editor":"vim","line":1}}"#
            )
        }
        // Which editor is one of a few words. A program's path is not one,
        // nor an editor the app cannot run, nor leaving it out.
        for editor in [
            "/usr/bin/vim", "/opt/homebrew/bin/nvim", "Vim", "pico", "mg", "emacs", "system", "bundled",
            "editor-command.sh", "",
        ] {
            #expect(throws: BridgeError.malformed("pane.editor: editor is vim, nano or script, never a program")) {
                try CoreToShell(
                    line:
                        #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"/d","title":"x","editor":"\#(editor)","line":1}}"#
                )
            }
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing editor")) {
            try CoreToShell(
                line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"/d","title":"x"}}"#)
        }
        // The line the body starts on is a line there can be.
        for line in [0, -1, 1_000_001] {
            #expect(throws: BridgeError.malformed("pane.editor: line is from 1 to 1000000")) {
                try CoreToShell(
                    line:
                        #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"/d","title":"x","editor":"nano","line":\#(line)}}"#
                )
            }
            #expect(PaneEditor(path: "/d/m", dir: "/d", title: "x", editor: .nano, line: line) == nil)
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing line")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"/d","title":"x","editor":"nano"}}"#
            )
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing dir")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","title":"x"}}"#)
        }
        #expect(throws: BridgeError.malformed("pane.editor: missing title")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"/d"}}"#)
        }
        #expect(throws: BridgeError.malformed("pane.editor: unexpected vimrc")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"/d","title":"x","vimrc":"/etc/vimrc"}}"#
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
                line: #"{"jsonrpc":"2.0","method":"pane.editor","params":{"path":"/d/m","dir":"/d","title":"x"}}"#)
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

    @Test func optionalFieldsAreLeftOutRatherThanNull() {
        let line = CoreToShell.commandsList([CommandInfo(name: "nav.next", title: "Next", keys: ["j"])]).line
        #expect(!line.contains("null"))
        #expect(!line.contains("menu"))
    }
}
