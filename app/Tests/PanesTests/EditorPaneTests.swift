import AppKit
import Testing

@testable import Panes

@MainActor
struct EditorPaneTests {
    @Test func thePaneIsAFrameAroundATerminal() {
        let terminal = NSView()
        let pane = EditorPane(title: "Re: Q4 roadmap", terminal: terminal)
        #expect(pane.kind == .editor)
        #expect(pane.kind.rawValue == "editor")
        #expect(pane.title == "Re: Q4 roadmap")
        #expect(pane.detail.isEmpty)
        #expect(pane.message == nil)
        #expect(terminal.isDescendant(of: pane))
        // The keyboard goes to the terminal.
        #expect(pane.focusTarget === terminal)
        #expect(pane.summary == "running")
    }

    @Test func everyKeyBelongsToTheProgramInIt() {
        let pane = EditorPane(title: "Hello", terminal: NSView())
        // esc is Vim's, so the window must not take it to close the pane.
        #expect(pane.takesEveryKey)
        for key in PaneKey.allCases {
            #expect(!pane.handleKey(key))
        }
    }

    @Test func theViewersStillLeaveEscToTheWindow() {
        #expect(!PlayerPane(file: URL(fileURLWithPath: "/a.mov"), title: "a").takesEveryKey)
    }

    @Test func closingIsSaidInTheHeaderAndTheTitleKept() {
        let pane = EditorPane(title: "Hello", terminal: NSView())
        var asked = 0
        pane.onClose = { asked += 1 }
        #expect(!pane.isClosing)
        pane.setClosing()
        #expect(pane.isClosing)
        #expect(pane.title == "Hello")
        #expect(pane.detail == "saving and closing")
        #expect(pane.summary == "closing")
        // The pane never closes itself: it has only been told.
        #expect(asked == 0)
        #expect(pane.superview == nil)
    }
}
