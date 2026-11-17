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

    @Test func aKeyThatWasPressedIsWrittenAsQuillWritesKeys() {
        typealias Held = KeyNotation.Modifiers
        let written: (String, Held) -> String? = { KeyNotation.written(characters: $0, modifiers: $1) }
        // A letter or a sign is itself, and shift is part of what it is.
        #expect(written("j", []) == "j")
        #expect(written("G", [.shift]) == "G")
        #expect(written("?", [.shift]) == "?")
        #expect(written("-", []) == "-")
        // With a modifier it says which, in the order the core writes them.
        #expect(written("k", [.control]) == "ctrl-k")
        #expect(written("L", [.command, .shift]) == "cmd-L")
        #expect(written("x", [.command, .control, .option]) == "ctrl-alt-cmd-x")
        // A key with a name keeps shift as a modifier.
        #expect(written("\r", [.command]) == "cmd-enter")
        #expect(written("\t", []) == "tab")
        #expect(written("\u{19}", [.shift]) == "shift-tab")
        #expect(written("\u{1B}", []) == "esc")
        #expect(written(" ", []) == "space")
        #expect(written("\u{7F}", []) == "backspace")
        #expect(written("\u{F728}", []) == "delete")
        #expect(written("\u{F700}", [.shift]) == "shift-up")
        #expect(written("\u{F72D}", []) == "pagedown")
        #expect(written("\u{F704}", []) == "f1")
        #expect(written("\u{F70F}", [.command]) == "cmd-f12")
        // What is no key of Quill's is nothing: a modifier by itself, a
        // key with no name here.
        #expect(written("", [.shift]) == nil)
        #expect(written("ab", []) == nil)
        #expect(written("\u{F739}", []) == nil)
        // And what is written is read back as the same key.
        for (characters, held) in [("G", Held.shift), ("k", .control), ("\r", .command), ("\u{F700}", .shift)] {
            let text = written(characters, held)
            #expect(text.flatMap(KeyNotation.init) != nil, "\(String(describing: text))")
        }
    }
}
