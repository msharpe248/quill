import Bridge
import Foundation
import Testing

@testable import Editor

struct EditorExitTests {
    @Test func aNormalExitIsStatusZero() {
        #expect(EditorExited(.exited(code: 0)) == EditorExited(ran: true, status: 0))
    }

    @Test func anExitWithAnErrorKeepsItsStatus() {
        // `:cq` leaves with 1; `:5cq` with 5.
        #expect(EditorExited(.exited(code: 1)) == EditorExited(ran: true, status: 1))
        #expect(EditorExited(.exited(code: 5)) == EditorExited(ran: true, status: 5))
        #expect(EditorExited(.exited(code: 255)) == EditorExited(ran: true, status: 255))
    }

    @Test func aSignalIsItsNumberBelowZero() {
        #expect(EditorExited(.signaled(signal: 1)) == EditorExited(ran: true, status: -1))
        #expect(EditorExited(.signaled(signal: 9)) == EditorExited(ran: true, status: -9))
        #expect(EditorExited(.signaled(signal: 11)) == EditorExited(ran: true, status: -11))
        #expect(EditorExited(.signaled(signal: 15)) == EditorExited(ran: true, status: -15))
    }

    @Test func whatIsReportedIsAlwaysInRange() {
        #expect(EditorExited(.exited(code: 256)) == EditorExited(ran: true, status: 255))
        #expect(EditorExited(.exited(code: .max)) == EditorExited(ran: true, status: 255))
        #expect(EditorExited(.exited(code: .min)) == EditorExited(ran: true, status: -255))
        #expect(EditorExited(.signaled(signal: 300)) == EditorExited(ran: true, status: -255))
        #expect(EditorExited(.signaled(signal: .max)) == EditorExited(ran: true, status: -255))
        // A signal never reads as a normal exit, whatever number arrives.
        #expect(EditorExited(.signaled(signal: 0)) == EditorExited(ran: true, status: -1))
        #expect(EditorExited(.signaled(signal: -9)) == EditorExited(ran: true, status: -9))
        for ending in [EditorEnding.exited(code: 70000), .signaled(signal: 70000), .failedToStart] {
            #expect(EditorExited.statuses.contains(EditorExited(ending).status))
        }
    }

    @Test func aVimThatNeverStartedDidNotRun() {
        #expect(EditorExited(.failedToStart) == EditorExited(ran: false, status: 0))
        #expect(EditorExited(.failedToStart) == .didNotRun)
    }
}

struct EditorSlotTests {
    @Test func nothingHasBeenAskedForAtFirst() {
        let slot = EditorSlot()
        #expect(slot.state == .none)
        #expect(slot.requests == 0)
        #expect(!slot.isRunning)
        #expect(slot.summary == "none")
    }

    @Test func aRequestStartsVimAndItsEndIsTheAnswer() {
        var slot = EditorSlot()
        #expect(slot.request(title: "Re: Q4 roadmap", available: true) == .start)
        #expect(slot.isRunning)
        #expect(slot.state == .running(title: "Re: Q4 roadmap"))
        #expect(slot.summary == #"running "Re: Q4 roadmap""#)
        #expect(slot.ended(.exited(code: 0)) == EditorExited(ran: true, status: 0))
        #expect(!slot.isRunning)
        #expect(slot.state == .exited(status: 0))
        #expect(slot.summary == "exited 0")
        #expect(slot.requests == 1)
    }

    @Test func onlyOneRunsAtATime() {
        var slot = EditorSlot()
        #expect(slot.request(title: "First", available: true) == .start)
        // A second is answered as not run, at once, and the first is not
        // disturbed.
        #expect(slot.request(title: "Second", available: true) == .refuse(.didNotRun))
        #expect(slot.request(title: "Third", available: false) == .refuse(.didNotRun))
        #expect(slot.state == .running(title: "First"))
        #expect(slot.requests == 3)
        // The first still gets its own answer when it ends.
        #expect(slot.ended(.exited(code: 1)) == EditorExited(ran: true, status: 1))
        #expect(slot.summary == "exited 1")
        // And then there is room again.
        #expect(slot.request(title: "Fourth", available: true) == .start)
        #expect(slot.state == .running(title: "Fourth"))
        #expect(slot.requests == 4)
    }

    @Test func aBuildWithNoVimAnswersAtOnce() {
        var slot = EditorSlot()
        #expect(slot.request(title: "Hello", available: false) == .refuse(EditorExited(ran: false, status: 0)!))
        #expect(!slot.isRunning)
        #expect(slot.state == .notAvailable)
        #expect(slot.summary == "not available")
        #expect(slot.requests == 1)
        // Nothing was started, so nothing ends.
        #expect(slot.ended(.exited(code: 0)) == nil)
        #expect(slot.state == .notAvailable)
    }

    @Test func everyStartedVimIsAnsweredExactlyOnce() {
        var slot = EditorSlot()
        #expect(slot.ended(.exited(code: 0)) == nil)
        #expect(slot.state == .none)
        _ = slot.request(title: "Hello", available: true)
        #expect(slot.ended(.signaled(signal: 1)) == EditorExited(ran: true, status: -1))
        #expect(slot.summary == "exited -1")
        // An ending heard twice is answered once.
        #expect(slot.ended(.signaled(signal: 1)) == nil)
        #expect(slot.ended(.exited(code: 0)) == nil)
        #expect(slot.summary == "exited -1")
    }

    @Test func aVimThatWouldNotStartIsAnsweredAsNotRun() {
        var slot = EditorSlot()
        _ = slot.request(title: "Hello", available: true)
        #expect(slot.ended(.failedToStart) == .didNotRun)
        #expect(slot.state == .failedToStart)
        #expect(slot.summary == "failed to start")
        #expect(slot.request(title: "Again", available: true) == .start)
    }

    @Test func aTitleIsShownAsItIs() {
        var slot = EditorSlot()
        _ = slot.request(title: "He said \"no\"\nand left", available: true)
        // On one line of the dump, whatever is in it.
        #expect(slot.summary == #"running "He said \"no\"\nand left""#)
        #expect(!slot.summary.contains("\n"))
    }
}

struct PoliteStopTests {
    @Test func vimIsAskedAsSomeoneAtTheKeyboardWould() {
        // esc on its own first.
        #expect(Array(PoliteStop.leaveMode.utf8) == [0x1B])
        // Then ctrl-\ ctrl-n, :wqa and return.
        #expect(Array(PoliteStop.saveAndQuit.utf8) == [0x1C, 0x0E] + Array(":wqa".utf8) + [0x0D])
    }

    @Test func nothingInItIsAVerdict() {
        // Closing the pane is not sending, postponing or discarding.
        for word in ["Quill", "send", "postpone", "discard", "!", "cq"] {
            #expect(!PoliteStop.saveAndQuit.contains(word))
        }
    }

    @Test func vimIsGivenTwoSecondsToGo() {
        #expect(PoliteStop.patience == 2)
        #expect(PoliteStop.pause > 0.1 && PoliteStop.pause < 0.5)
        #expect(PoliteStop.lastResort > 0)
    }
}
