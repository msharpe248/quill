import Bridge
import Testing

@testable import Forms

/// The rows of the window of keys, without a window: the core's list set
/// out heading by heading, and what is left of it for what is typed.
struct KeyRowsTests {
    private let list = KeysList(
        title: "Keyboard shortcuts",
        sections: [
            KeysList.Section(
                title: "Moving",
                keys: [
                    KeysList.Line(title: "Next item or line", keys: ["j", "down"]),
                    KeysList.Line(title: "First item", keys: ["g g", "home"]),
                ]),
            KeysList.Section(
                title: "Messages",
                keys: [
                    KeysList.Line(title: "Reply", keys: ["r"]),
                    KeysList.Line(title: "Archive", keys: ["e"]),
                ]),
            KeysList.Section(
                title: "Writing a message",
                keys: [
                    KeysList.Line(title: "Send", keys: ["cmd-enter", "ctrl-x s"]),
                    KeysList.Line(title: "Write in the editor", keys: ["cmd-shift-e"]),
                    KeysList.Line(title: "Send later", keys: []),
                ]),
        ])

    private func titles(_ rows: KeyRows) -> [String] {
        rows.rows.map { row in
            switch row {
            case let .heading(title): "[\(title)]"
            case let .key(title, _): title
            }
        }
    }

    @Test func theWholeListIsSetOutUnderTheCoresHeadingsInTheCoresOrder() {
        let rows = KeyRows(list)
        #expect(
            titles(rows) == [
                "[Moving]", "Next item or line", "First item", "[Messages]", "Reply", "Archive",
                "[Writing a message]", "Send", "Write in the editor", "Send later",
            ])
        #expect(rows.count == 7)
        // Keys are shown as a Mac writes them, the preferred one first, and
        // a command with none is still listed.
        #expect(rows.rows[1] == .key(title: "Next item or line", keys: ["j", "↓"]))
        #expect(rows.rows[2] == .key(title: "First item", keys: ["g g", "↖"]))
        #expect(rows.rows[7] == .key(title: "Send", keys: ["⌘↩", "⌃X s"]))
        #expect(rows.rows[8] == .key(title: "Write in the editor", keys: ["⇧⌘E"]))
        #expect(rows.rows[9] == .key(title: "Send later", keys: []))
        // Nothing but spaces typed is nothing typed.
        #expect(KeyRows(list, matching: "  ") == rows)
    }

    @Test func whatIsTypedIsFoundInWhatACommandIsCalledWhateverItsCase() {
        #expect(titles(KeyRows(list, matching: "send")) == ["[Writing a message]", "Send", "Send later"])
        #expect(titles(KeyRows(list, matching: "SEND LATER")) == ["[Writing a message]", "Send later"])
        #expect(titles(KeyRows(list, matching: "later send")) == ["[Writing a message]", "Send later"])
        #expect(KeyRows(list, matching: "send").count == 2)
        // A heading with nothing left under it is left out, and a search
        // that finds nothing leaves nothing.
        let none = KeyRows(list, matching: "calendar")
        #expect(none.rows.isEmpty && none.count == 0)
    }

    @Test func aKeyIsFoundAsQuillWritesItAndAsAMacDoes() {
        #expect(titles(KeyRows(list, matching: "cmd-shift-e")) == ["[Writing a message]", "Write in the editor"])
        #expect(titles(KeyRows(list, matching: "⇧⌘E")) == ["[Writing a message]", "Write in the editor"])
        #expect(titles(KeyRows(list, matching: "⌘")) == ["[Writing a message]", "Send", "Write in the editor"])
        #expect(titles(KeyRows(list, matching: "ctrl-x")) == ["[Writing a message]", "Send"])
        #expect(titles(KeyRows(list, matching: "down")) == ["[Moving]", "Next item or line"])
    }

    @Test func aHeadingThatHasWhatIsTypedKeepsAllThatIsUnderIt() {
        #expect(
            titles(KeyRows(list, matching: "writing")) == [
                "[Writing a message]", "Send", "Write in the editor", "Send later",
            ])
        // "message" is in one heading and in another's name.
        #expect(
            titles(KeyRows(list, matching: "message")) == [
                "[Messages]", "Reply", "Archive", "[Writing a message]", "Send", "Write in the editor",
                "Send later",
            ])
    }

    @Test func aKeyThatIsNotOneOfQuillsIsShownAsItCame() {
        #expect(KeyRows.shown("cmd-up") == "⌘↑")
        #expect(KeyRows.shown("K") == "K")
        #expect(KeyRows.shown("hyper-k") == "hyper-k")
        #expect(KeyRows.shown("") == "")
    }
}
