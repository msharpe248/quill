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
                "files.save.json", "link.open.json", "pane.close.json", "pane.play.json", "pane.preview.json",
                "pane.web.json", "state.update.json", "thumbnail.make.json",
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
                "app.will_quit.json", "command.run.json", "files.dropped.json", "files.saved.json",
                "thumbnail.made.json",
            ])
        for (name, text) in examples {
            let message = try ShellToCore(line: text)
            #expect(try Examples.value(message.line) == Examples.value(text), "\(name) does not round-trip")
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
