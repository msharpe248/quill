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
                "commands.list.json", "pane.close.json", "pane.preview.json", "state.update.json",
                "url.open.json",
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
        #expect(examples.map(\.name) == ["app.will_quit.json", "command.run.json"])
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
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"pane.preview","params":{"path":"a.pdf","title":"a"}}"#)
        }
        #expect(throws: BridgeError.malformed("url.open: not an http, https or mailto address")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"url.open","params":{"url":"file:///etc/passwd"}}"#)
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
