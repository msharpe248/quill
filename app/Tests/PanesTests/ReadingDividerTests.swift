import Foundation
import Testing

@testable import Panes

@Suite struct ReadingDividerTests {
    /// Cells as terminals have them: a whole number of pixels, on screens
    /// of one, two and three pixels to the point. Fifteen pixels at two to
    /// the point is a cell of seven and a half points.
    static let measures: [CellMeasure] = [
        (7, 1.0), (8, 1.0), (9, 1.0), (13, 1.0), (14, 2.0), (15, 2.0), (16, 2.0), (17, 2.0), (23, 2.0), (25, 3.0),
    ].map { CellMeasure(cellWidthPixels: $0.0, scale: $0.1)! }

    /// A stand-in for the core's rule: any rule will do, as long as it is
    /// asked afresh for every window width. Two fifths to the terminal.
    static func asked(window: Int) -> Int {
        max(window - window * 3 / 5, 24)
    }

    @Test func aCellHasSomeWidth() {
        #expect(CellMeasure(cellWidthPixels: 0, scale: 2) == nil)
        #expect(CellMeasure(cellWidthPixels: -8, scale: 2) == nil)
        #expect(CellMeasure(cellWidthPixels: 16, scale: 0) == nil)
        #expect(CellMeasure(cellWidthPixels: 16, scale: .infinity) == nil)
        #expect(CellMeasure(cellWidthPixels: 16, scale: 2) != nil)
    }

    @Test func aWidthForSomeColumnsHasExactlyThoseColumnsAndIsTheNarrowestThatDoes() {
        for cells in Self.measures {
            for columns in 1...500 {
                let width = cells.width(forColumns: columns)
                // Whole points: nothing rounds it afterwards.
                #expect(width == width.rounded(), "\(cells) \(columns)")
                #expect(cells.columns(inWidth: width) == columns, "\(cells) \(columns)")
                #expect(cells.columns(inWidth: width - 1) < columns, "\(cells) \(columns)")
            }
            #expect(cells.columns(inWidth: 0) == 0)
            #expect(cells.columns(inWidth: -40) == 0)
            #expect(cells.columns(inWidth: .nan) == 0)
            #expect(cells.width(forColumns: 0) == 0)
        }
        // A cell of seven and a half points: 57 columns need 427.5 points,
        // and are given 428.
        let odd = CellMeasure(cellWidthPixels: 15, scale: 2)!
        #expect(odd.width(forColumns: 57) == 428)
        #expect(odd.columns(inWidth: 428) == 57 && odd.columns(inWidth: 427) == 56)
    }

    /// What the core says for a window of this width: asked afresh, for
    /// the window as it is.
    static func said(_ width: CGFloat, _ cells: CellMeasure) -> ReadingDivider.Asked {
        let window = cells.columns(inWidth: width)
        return ReadingDivider.Asked(columns: asked(window: window), window: window)
    }

    @Test func theTerminalIsGivenExactlyTheColumnsTheCoreAsksAtEveryWindowWidth() {
        for cells in Self.measures {
            for points in stride(from: 700, through: 3000, by: 37) {
                let width = CGFloat(points)
                let said = Self.said(width, cells)
                let terminal = ReadingDivider.terminalWidth(asked: said, width: width, divider: 1, cells: cells)
                // The terminal view, that wide, has those columns: what
                // the core draws in them is as wide as it said.
                #expect(cells.columns(inWidth: terminal) == said.columns, "\(cells) \(points)")
                // And the pane has the rest, and room.
                #expect(width - 1 - terminal >= ReadingDivider.narrowestPane, "\(cells) \(points)")
            }
        }
    }

    @Test func switchingOnAndOffOverAndOverAndResizingInBetweenLeavesTheDividerWhereItWas() {
        for cells in Self.measures {
            let width: CGFloat = 1180
            // Off: the terminal has the window. On: it has what the core
            // asks for a window of that many columns.
            var seen: Set<CGFloat> = []
            var terminal = width
            for _ in 0..<40 {
                // With the terminal alone in the window, its columns are
                // the window's: what the core is told, and answers for.
                let window = cells.columns(inWidth: terminal)
                let said = ReadingDivider.Asked(columns: Self.asked(window: window), window: window)
                terminal = ReadingDivider.terminalWidth(asked: said, width: width, divider: 1, cells: cells)
                seen.insert(terminal)
                // With the pane up the window's columns are still those,
                // though the terminal has fewer.
                #expect(cells.columns(inWidth: width) == window)
                #expect(cells.columns(inWidth: terminal) == said.columns)
                // Off again: the terminal has the whole window back.
                terminal = width
            }
            #expect(seen.count == 1, "\(cells): the divider crept: \(seen.sorted())")

            // The window is resized with the pane up, and brought back.
            let place: (CGFloat) -> Int = { width in
                ReadingDivider.columns(asked: Self.said(width, cells), width: width, divider: 1, cells: cells)
            }
            let first = place(1180)
            for other in [1400, 900, 2200, 1181, 1179] as [CGFloat] {
                #expect(place(other) == Self.asked(window: cells.columns(inWidth: other)), "\(cells) \(other)")
                #expect(place(1180) == first, "\(cells) after \(other)")
            }
        }
    }

    @Test func untilTheCoreHasSaidTheTerminalHasHalfTheWindow() {
        let cells = CellMeasure(cellWidthPixels: 16, scale: 2)!
        #expect(ReadingDivider.columns(asked: nil, width: 1180, divider: 1, cells: cells) == 73)
    }

    @Test func aPlaceSaidForAWindowOfAnotherWidthIsNotGoneByAsItStands() {
        // Nine pixels to a cell: 2548 points have room for 283 columns.
        let cells = CellMeasure(cellWidthPixels: 9, scale: 1)!
        let now = ReadingDivider.Asked(columns: 155, window: 283)
        #expect(ReadingDivider.isCurrent(now, width: 2548, cells: cells))
        #expect(ReadingDivider.columns(asked: now, width: 2548, divider: 1, cells: cells) == 155)

        // The same words for a window that had room for 304: ten columns
        // too many for this one, if they were gone by. Until the core
        // says again, the terminal keeps the same share of this window.
        let stale = ReadingDivider.Asked(columns: 165, window: 304)
        #expect(!ReadingDivider.isCurrent(stale, width: 2548, cells: cells))
        #expect(ReadingDivider.columns(asked: stale, width: 2548, divider: 1, cells: cells) == 154)
        #expect(!ReadingDivider.isCurrent(nil, width: 2548, cells: cells))
    }

    @Test func aDividerLetGoOfRestsOnTheNearestColumn() {
        let cells = CellMeasure(cellWidthPixels: 16, scale: 2)!
        // Let go between two columns: it rests on the nearer, and that is
        // the number the core is told.
        #expect(ReadingDivider.columns(draggedTo: 563, width: 1180, divider: 1, cells: cells) == 70)
        #expect(ReadingDivider.columns(draggedTo: 565, width: 1180, divider: 1, cells: cells) == 71)
        // Once the core has said so, that is where it stands.
        let said = ReadingDivider.Asked(columns: 70, window: cells.columns(inWidth: 1180))
        #expect(ReadingDivider.terminalWidth(asked: said, width: 1180, divider: 1, cells: cells) == 560)
    }

    @Test func neitherSideIsLeftWithoutRoom() {
        let cells = CellMeasure(cellWidthPixels: 16, scale: 2)!
        // Dragged all the way to either edge.
        #expect(ReadingDivider.columns(draggedTo: 0, width: 1180, divider: 1, cells: cells) == 24)
        let most = ReadingDivider.columns(draggedTo: 1180, width: 1180, divider: 1, cells: cells)
        #expect(most == 122)
        #expect(1180 - 1 - cells.width(forColumns: most) >= ReadingDivider.narrowestPane)

        let said: (Int, CGFloat) -> ReadingDivider.Asked = { columns, width in
            ReadingDivider.Asked(columns: columns, window: cells.columns(inWidth: width))
        }
        // More than fits beside the pane: held to what does.
        #expect(ReadingDivider.columns(asked: said(110, 600), width: 600, divider: 1, cells: cells) == 49)
        // A window with room for only one of the two keeps a terminal that
        // can be drawn in.
        #expect(ReadingDivider.columns(asked: said(24, 380), width: 380, divider: 1, cells: cells) == 24)
        // And one narrower than that gives the terminal what there is.
        #expect(ReadingDivider.columns(asked: said(24, 160), width: 160, divider: 1, cells: cells) == 20)
        #expect(ReadingDivider.columns(asked: said(24, 0), width: 0, divider: 1, cells: cells) == 1)
    }
}
