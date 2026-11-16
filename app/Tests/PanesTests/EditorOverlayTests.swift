import Foundation
import Testing

@testable import Panes

struct EditorOverlayTests {
    @Test func theEditorLiesExactlyOverTheCellsItWasGiven() throws {
        // Cells of 17 by 35 pixels, two pixels to a point: an odd number
        // of pixels, so a cell is not a whole number of points.
        let cells = try #require(CellSize(widthPixels: 17, heightPixels: 35, scale: 2))
        let frames = EditorOverlay.frames(column: 70, row: 7, columns: 76, rows: 30, covered: 0, cells: cells)
        #expect(frames.shown == CGRect(x: 595, y: 122.5, width: 646, height: 525))
        // Nothing is covered: the editor fills the part that is shown.
        #expect(frames.whole == CGRect(x: 0, y: 0, width: 646, height: 525))
        // As many pixels as its cells come to, so it has exactly that many.
        #expect(frames.whole.width * 2 == CGFloat(76 * 17))
        #expect(frames.whole.height * 2 == CGFloat(30 * 35))
    }

    @Test func rowsThatAreCoveredAreHiddenAndTheEditorIsNoSmallerForThem() throws {
        let cells = try #require(CellSize(widthPixels: 16, heightPixels: 34, scale: 2))
        let all = EditorOverlay.frames(column: 2, row: 6, columns: 40, rows: 20, covered: 0, cells: cells)
        let some = EditorOverlay.frames(column: 2, row: 6, columns: 40, rows: 20, covered: 3, cells: cells)
        // Three rows less is seen, from the top.
        #expect(some.shown.minY == all.shown.minY + 51)
        #expect(some.shown.maxY == all.shown.maxY)
        #expect(some.shown.height == all.shown.height - 51)
        // The editor keeps its size, and its bottom stays where it was.
        #expect(some.whole.size == all.whole.size)
        #expect(some.whole.minY == -51)
        #expect(some.shown.minY + some.whole.minY == all.shown.minY)
        // More rows covered than there are is all of them.
        let none = EditorOverlay.frames(column: 2, row: 6, columns: 40, rows: 20, covered: 99, cells: cells)
        #expect(none.shown.height == 0)
        #expect(none.whole.size == all.whole.size)
    }

    @Test func aCellHasSomeSize() {
        #expect(CellSize(widthPixels: 0, heightPixels: 34, scale: 2) == nil)
        #expect(CellSize(widthPixels: 16, heightPixels: 0, scale: 2) == nil)
        #expect(CellSize(widthPixels: 16, heightPixels: 34, scale: 0) == nil)
        #expect(CellSize(widthPixels: 16, heightPixels: 34, scale: .infinity) == nil)
        #expect(CellSize(widthPixels: 16, heightPixels: 34, scale: 1) != nil)
    }
}
