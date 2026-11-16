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

    @Test func theTerminalIsGivenExactlyTheColumnsTheCoreAsksAtEveryWindowWidth() {
        for cells in Self.measures {
            for points in stride(from: 700, through: 3000, by: 37) {
                let width = CGFloat(points)
                let window = cells.columns(inWidth: width)
                let asked = Self.asked(window: window)
                let terminal = ReadingDivider.terminalWidth(
                    remembered: nil, asked: asked, width: width, divider: 1, cells: cells)
                // The terminal view, that wide, has those columns: what
                // the core draws in them is as wide as it said.
                #expect(cells.columns(inWidth: terminal) == asked, "\(cells) \(points)")
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
                terminal = ReadingDivider.terminalWidth(
                    remembered: nil, asked: Self.asked(window: window), width: width, divider: 1, cells: cells)
                seen.insert(terminal)
                // With the pane up the window's columns are still those,
                // though the terminal has fewer.
                #expect(cells.columns(inWidth: width) == window)
                #expect(cells.columns(inWidth: terminal) == Self.asked(window: window))
                // Off again: the terminal has the whole window back.
                terminal = width
            }
            #expect(seen.count == 1, "\(cells): the divider crept: \(seen.sorted())")

            // The window is resized with the pane up, and brought back.
            let place: (CGFloat) -> Int = { width in
                let window = cells.columns(inWidth: width)
                return ReadingDivider.columns(
                    remembered: nil, asked: Self.asked(window: window), width: width, divider: 1, cells: cells)
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
        #expect(ReadingDivider.columns(remembered: nil, asked: nil, width: 1180, divider: 1, cells: cells) == 73)
    }

    @Test func whereTheUserLeftTheDividerStandsInsteadUntilItIsPutBack() {
        let cells = CellMeasure(cellWidthPixels: 16, scale: 2)!
        let memory = TransientDividerMemory()
        let place = {
            ReadingDivider.columns(remembered: memory.columns, asked: 85, width: 1180, divider: 1, cells: cells)
        }
        #expect(place() == 85)

        // Dragged, and let go between two columns: it rests on the nearer.
        memory.columns = ReadingDivider.columns(draggedTo: 563, width: 1180, divider: 1, cells: cells)
        #expect(memory.columns == 70)
        #expect(place() == 70)
        #expect(
            ReadingDivider.terminalWidth(remembered: memory.columns, asked: 85, width: 1180, divider: 1, cells: cells)
                == 560)
        #expect(ReadingDivider.columns(draggedTo: 565, width: 1180, divider: 1, cells: cells) == 71)

        // Whatever the core says afterwards, as the window is resized.
        let wider = ReadingDivider.columns(
            remembered: memory.columns, asked: 99, width: 1400, divider: 1, cells: cells)
        #expect(wider == 70)

        // Put back: where the core says again.
        memory.columns = nil
        #expect(place() == 85)
    }

    @Test func neitherSideIsLeftWithoutRoom() {
        let cells = CellMeasure(cellWidthPixels: 16, scale: 2)!
        // Dragged all the way to either edge.
        #expect(ReadingDivider.columns(draggedTo: 0, width: 1180, divider: 1, cells: cells) == 24)
        let most = ReadingDivider.columns(draggedTo: 1180, width: 1180, divider: 1, cells: cells)
        #expect(most == 122)
        #expect(1180 - 1 - cells.width(forColumns: most) >= ReadingDivider.narrowestPane)

        // A place remembered from a wide window, in a narrow one: held to
        // what fits, and not forgotten.
        let memory = TransientDividerMemory(columns: 110)
        let narrow = ReadingDivider.columns(
            remembered: memory.columns, asked: 30, width: 600, divider: 1, cells: cells)
        #expect(narrow == 49)
        #expect(memory.columns == 110)
        #expect(
            ReadingDivider.columns(remembered: memory.columns, asked: 85, width: 1400, divider: 1, cells: cells) == 110)

        // A window with room for only one of the two keeps a terminal that
        // can be drawn in.
        #expect(ReadingDivider.columns(remembered: nil, asked: 24, width: 380, divider: 1, cells: cells) == 24)
        // And one narrower than that gives the terminal what there is.
        #expect(ReadingDivider.columns(remembered: nil, asked: 24, width: 160, divider: 1, cells: cells) == 20)
        #expect(ReadingDivider.columns(remembered: nil, asked: 24, width: 0, divider: 1, cells: cells) == 1)
    }

    @Test func theDividerIsRememberedFromOneRunToTheNextAndForgottenWhenPutBack() throws {
        // Defaults of the test's own, removed with it: never the app's.
        let suite = "dev.quill.tests.divider-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        #expect(DefaultsDividerMemory(defaults: defaults).columns == nil)
        DefaultsDividerMemory(defaults: defaults).columns = 70
        // Another run: another object over the same defaults.
        #expect(DefaultsDividerMemory(defaults: defaults).columns == 70)
        #expect(defaults.integer(forKey: DefaultsDividerMemory.key) == 70)

        // Put back: nothing is left behind.
        DefaultsDividerMemory(defaults: defaults).columns = nil
        #expect(DefaultsDividerMemory(defaults: defaults).columns == nil)
        #expect(defaults.object(forKey: DefaultsDividerMemory.key) == nil)

        // What is not a number of columns is not remembered, and what is
        // found there that is not one is nothing.
        let memory = DefaultsDividerMemory(defaults: defaults)
        memory.columns = 0
        #expect(defaults.object(forKey: DefaultsDividerMemory.key) == nil)
        for junk in ["seventy", -5, 0, 70000, 70.5, [70]] as [Any] {
            defaults.set(junk, forKey: DefaultsDividerMemory.key)
            #expect(memory.columns == nil, "\(junk)")
        }
    }

    @Test func aScriptedRunsDividerIsKeptNowhere() throws {
        let suite = "dev.quill.tests.divider-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        DefaultsDividerMemory(defaults: defaults).columns = 70

        // It starts with none, whatever the defaults hold, and what it is
        // given goes no further.
        let transient = TransientDividerMemory()
        #expect(transient.columns == nil)
        transient.columns = 90
        #expect(transient.columns == 90)
        #expect(DefaultsDividerMemory(defaults: defaults).columns == 70)
    }
}
