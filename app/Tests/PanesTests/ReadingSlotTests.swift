import Testing

@testable import Panes

@Suite struct ReadingSlotTests {
    @Test func theFirstPageIsShownAtOnceAndTheNextOnesTakeItsPlace() {
        var slot = ReadingSlot<String>()
        #expect(slot.page == nil)
        #expect(slot.show("one", over: .nothing) == .show("one"))
        #expect(slot.show("two", over: .reading) == .replace("two"))
        #expect(slot.page == "two")
    }

    @Test func aViewerKeepsThePlaceAndTheReadingPaneComesBackWhenItGoes() {
        var slot = ReadingSlot<String>()
        _ = slot.show("one", over: .nothing)
        // A viewer was opened over it. Pages asked for meanwhile wait.
        #expect(slot.show("two", over: .other) == .nothing)
        #expect(slot.show("three", over: .other) == .nothing)
        // The viewer closes: the latest page is what comes back.
        #expect(slot.vacated() == .show("three"))
    }

    @Test func closingTakesThePaneDownAndNothingComesBackAfterwards() {
        var slot = ReadingSlot<String>()
        _ = slot.show("one", over: .nothing)
        #expect(slot.close(over: .reading) == .close)
        #expect(slot.page == nil)
        #expect(slot.vacated() == .nothing)

        // Closed while a viewer has the place: the viewer stays, and no
        // reading pane follows it.
        _ = slot.show("two", over: .nothing)
        #expect(slot.close(over: .other) == .nothing)
        #expect(slot.vacated() == .nothing)
        // Closed with nothing up: nothing to do.
        #expect(slot.close(over: .nothing) == .nothing)
    }

    @Test func withNoReadingPaneAskedForAnEmptyPlaceStaysEmpty() {
        let slot = ReadingSlot<String>()
        #expect(slot.vacated() == .nothing)
    }
}
