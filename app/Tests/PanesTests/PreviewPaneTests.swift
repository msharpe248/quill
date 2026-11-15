import AppKit
import Foundation
import Testing

@testable import Panes

@MainActor
struct PreviewPaneTests {
    private func items(_ count: Int, in scratch: Scratch) throws -> [PreviewItem] {
        try (1...count).map { number in
            PreviewItem(url: try scratch.write("File \(number)", to: "file-\(number).txt"), title: "File \(number)")
        }
    }

    @Test func theDeckStartsWhereItIsToldAndStopsAtBothEnds() throws {
        let scratch = try Scratch()
        var deck = try #require(PreviewDeck(items: try items(3, in: scratch), index: 1))
        #expect(deck.current.title == "File 2")
        #expect(deck.position == "2 of 3")

        var moved: Bool
        moved = deck.step(by: 1)
        #expect(moved)
        #expect(deck.index == 2)
        #expect(deck.position == "3 of 3")
        // The end: no further, and not round to the start.
        moved = deck.step(by: 1)
        #expect(!moved)
        #expect(deck.index == 2)

        moved = deck.step(by: -1)
        #expect(moved)
        moved = deck.step(by: -1)
        #expect(moved)
        #expect(deck.index == 0)
        #expect(deck.position == "1 of 3")
        moved = deck.step(by: -1)
        #expect(!moved)
        #expect(deck.index == 0)
        // A long stride stops at the end too.
        moved = deck.step(by: 10)
        #expect(moved)
        #expect(deck.index == 2)
    }

    @Test func aDeckNeedsItsIndexToBeOneOfItsItems() throws {
        let scratch = try Scratch()
        let three = try items(3, in: scratch)
        #expect(PreviewDeck(items: three, index: 3) == nil)
        #expect(PreviewDeck(items: three, index: -1) == nil)
        #expect(PreviewDeck(items: [], index: 0) == nil)
        #expect(PreviewDeck(items: three, index: 2) != nil)
    }

    @Test func oneItemShowsNoCounter() throws {
        let scratch = try Scratch()
        var deck = try #require(PreviewDeck(items: try items(1, in: scratch), index: 0))
        #expect(deck.position == nil)
        var moved = deck.step(by: 1)
        #expect(!moved)
        moved = deck.step(by: -1)
        #expect(!moved)

        let pane = PreviewPane(deck: deck)
        defer { pane.tearDown() }
        #expect(pane.title == "File 1")
        #expect(pane.detail == "")
        // With nothing to step to, the arrows are Quick Look's own.
        for key in PaneKey.allCases {
            #expect(!pane.handleKey(key))
        }
        #expect(pane.summary == "item 1 of 1, file-1.txt")
    }

    @Test func theArrowKeysStepThroughThePaneAndTheHeaderFollows() throws {
        let scratch = try Scratch()
        let deck = try #require(PreviewDeck(items: try items(3, in: scratch), index: 1))
        let pane = PreviewPane(deck: deck)
        defer { pane.tearDown() }
        #expect(pane.kind == .preview)
        #expect(pane.title == "File 2")
        #expect(pane.detail == "2 of 3")
        #expect(pane.fileURL.lastPathComponent == "file-2.txt")

        #expect(pane.handleKey(.right))
        #expect(pane.title == "File 3")
        #expect(pane.detail == "3 of 3")
        // At the end the key is still used up, and nothing moves.
        #expect(pane.handleKey(.right))
        #expect(pane.handleKey(.down))
        #expect(pane.detail == "3 of 3")
        #expect(pane.fileURL.lastPathComponent == "file-3.txt")

        #expect(pane.handleKey(.left))
        #expect(pane.handleKey(.up))
        #expect(pane.detail == "1 of 3")
        #expect(pane.handleKey(.left))
        #expect(pane.title == "File 1")
        #expect(pane.summary == "item 1 of 3, file-1.txt")

        // Not the pane's keys.
        #expect(!pane.handleKey(.space))
        #expect(!pane.handleKey(.escape))
    }

    @Test func aFileThatHasGoneIsSaidToBeGone() throws {
        let scratch = try Scratch()
        var files = try items(2, in: scratch)
        files[1].url = scratch.file("missing.pdf")
        let pane = PreviewPane(deck: try #require(PreviewDeck(items: files, index: 1)))
        defer { pane.tearDown() }
        #expect(pane.message != nil)
        // The pane takes the keyboard itself while it has no viewer to.
        #expect(pane.focusTarget === pane)
        // The others can still be reached.
        #expect(pane.handleKey(.left))
        #expect(pane.message == nil)
        #expect(pane.focusTarget !== pane)
    }

    // Quick Look itself, in a window that is never put on screen.
    @Test func quickLookIsGivenTheFileOnceThePaneIsInAWindow() throws {
        let scratch = try Scratch()
        let files = try items(3, in: scratch)
        let pane = PreviewPane(deck: try #require(PreviewDeck(items: files, index: 0)))
        #expect(pane.loaded == nil)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.borderless],
            backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.contentView = pane
        #expect(!window.isVisible)
        #expect(pane.loaded == files[0].url)
        #expect(pane.handleKey(.right))
        #expect(pane.loaded == files[1].url)
        // At the end nothing is loaded again.
        #expect(pane.handleKey(.right))
        #expect(pane.handleKey(.right))
        #expect(pane.loaded == files[2].url)

        // The core asks for other files while the pane is open.
        let others = try (1...2).map { number in
            PreviewItem(url: try scratch.write("Other", to: "other-\(number).txt"), title: "Other \(number)")
        }
        pane.show(try #require(PreviewDeck(items: others, index: 1)))
        #expect(pane.title == "Other 2")
        #expect(pane.detail == "2 of 2")
        #expect(pane.loaded == others[1].url)
        #expect(window.makeFirstResponder(pane.focusTarget))

        pane.tearDown()
        window.contentView = nil
    }

    @Test func theCloseButtonAsksToBeClosed() throws {
        let scratch = try Scratch()
        let pane = PreviewPane(deck: try #require(PreviewDeck(items: try items(1, in: scratch), index: 0)))
        defer { pane.tearDown() }
        var asked = 0
        pane.onClose = { asked += 1 }
        let button = try #require(Self.descendants(of: pane).compactMap { $0 as? NSButton }.first)
        #expect(button.accessibilityIdentifier() == "pane.close")
        button.performClick(nil)
        #expect(asked == 1)
    }

    static func descendants(of view: NSView) -> [NSView] {
        view.subviews.flatMap { [$0] + descendants(of: $0) }
    }
}

struct PaneKeyTests {
    @Test func onlyThePlainKeysArePaneKeys() {
        #expect(PaneKey(keyCode: 53) == .escape)
        #expect(PaneKey(keyCode: 49) == .space)
        #expect(PaneKey(keyCode: 123) == .left)
        #expect(PaneKey(keyCode: 124) == .right)
        #expect(PaneKey(keyCode: 125) == .down)
        #expect(PaneKey(keyCode: 126) == .up)
        // An arrow key always carries these two; they are not modifiers held.
        #expect(PaneKey(keyCode: 124, modifiers: [.numericPad, .function]) == .right)
        #expect(PaneKey(keyCode: 0) == nil)
        // cmd-right, shift-space and the like belong to someone else.
        #expect(PaneKey(keyCode: 124, modifiers: .command) == nil)
        #expect(PaneKey(keyCode: 49, modifiers: .shift) == nil)
        #expect(PaneKey(keyCode: 53, modifiers: .option) == nil)
        #expect(PaneKey(keyCode: 123, modifiers: .control) == nil)
    }

    @Test func aKeyEventIsReadByItsKeyCode() throws {
        let down = try #require(
            NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: [.numericPad, .function], timestamp: 0,
                windowNumber: 0, context: nil, characters: "\u{F703}", charactersIgnoringModifiers: "\u{F703}",
                isARepeat: false, keyCode: 124))
        #expect(PaneKey(event: down) == .right)
        let up = try #require(
            NSEvent.keyEvent(
                with: .keyUp, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
                characters: " ", charactersIgnoringModifiers: " ", isARepeat: false, keyCode: 49))
        #expect(PaneKey(event: up) == nil)
    }
}
