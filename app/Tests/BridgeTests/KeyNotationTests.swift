import Testing

@testable import Bridge

@Suite struct KeyNotationTests {
    @Test func plainKeysBelongToTheTerminalNotToAMenu() {
        for text in ["j", "G", "?", "enter", "space", "ctrl-d", "shift-tab", "g g", "-"] {
            let notation = KeyNotation(text)
            #expect(notation != nil, "\(text)")
            #expect(notation?.menuShortcut == nil, "\(text) must not be a menu shortcut")
        }
    }

    @Test func commandChordsBecomeMenuShortcuts() throws {
        let enter = try #require(KeyNotation("cmd-enter")?.menuShortcut)
        #expect(enter.key == "\r")
        #expect(enter.modifiers == .command)

        let shifted = try #require(KeyNotation("cmd-shift-a")?.menuShortcut)
        #expect(shifted.key == "a")
        #expect(shifted.modifiers == [.command, .shift])

        // An upper-case letter means shift, as it does in the terminal.
        let upper = try #require(KeyNotation("cmd-K")?.menuShortcut)
        #expect(upper.key == "k")
        #expect(upper.modifiers == [.command, .shift])

        let minus = try #require(KeyNotation("cmd--")?.menuShortcut)
        #expect(minus.key == "-")
        // A sequence can never be one shortcut.
        #expect(KeyNotation("cmd-k cmd-j")?.menuShortcut == nil)
    }

    @Test func keysDisplayTheWayAMacWritesThem() {
        let expected = [
            "j": "j", "G": "G", "g g": "g g", "?": "?",
            "enter": "↩", "esc": "esc", "space": "space", "tab": "⇥",
            "shift-tab": "⇧⇥", "ctrl-d": "⌃D", "ctrl-c": "⌃C",
            "cmd-enter": "⌘↩", "cmd-shift-a": "⇧⌘A", "opt-left": "⌥←",
            "pagedown": "⇟", "ctrl--": "⌃-",
        ]
        for (text, display) in expected {
            #expect(KeyNotation(text)?.display == display, "\(text)")
        }
    }

    @Test func whatIsNotAKeyIsRefused() {
        #expect(KeyNotation("") == nil)
        #expect(KeyNotation("   ") == nil)
        #expect(KeyNotation("hyper-x") == nil)
        #expect(KeyNotation("ctrl-") == nil)
    }
}
