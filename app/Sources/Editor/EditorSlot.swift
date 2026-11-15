import Bridge
import Foundation

/// How the program in the editor pane ended.
public enum EditorEnding: Equatable, Sendable {
    /// It exited, with this status.
    case exited(code: Int32)
    /// A signal ended it.
    case signaled(signal: Int32)
    /// It never started.
    case failedToStart
}

public extension EditorExited {
    /// What the core is told of an ending: the exit status as it is, a
    /// signal as its number below zero, and a Vim that never started as not
    /// having run.
    init(_ ending: EditorEnding) {
        switch ending {
        case let .exited(code):
            self.init(ranWithStatus: Int(code))
        case let .signaled(signal):
            // A signal has a number above zero. Whatever arrives, the
            // answer is below zero: it must not read as a normal exit.
            self.init(ranWithStatus: -max(abs(Int(signal)), 1))
        case .failedToStart:
            self = .didNotRun
        }
    }
}

/// The one place there is for an editor, and what became of the last one.
///
/// Only one Vim runs at a time. A second request while one runs is answered
/// as not run, and the first is left alone. Every request gets exactly one
/// answer: at once when nothing is started, and otherwise when Vim ends.
public struct EditorSlot: Equatable, Sendable {
    public enum State: Equatable, Sendable {
        /// No editor has been asked for.
        case none
        /// Vim is running on the draft called this.
        case running(title: String)
        /// The last Vim ended, and this was reported.
        case exited(status: Int)
        /// The last request found no Vim in this build.
        case notAvailable
        /// The last Vim could not be started.
        case failedToStart
    }

    /// What to do about a request.
    public enum Answer: Equatable, Sendable {
        /// Start Vim. Tell the slot when it ends: `ended` has the answer.
        case start
        /// Start nothing, and send this now.
        case refuse(EditorExited)
    }

    public private(set) var state = State.none
    /// How many times an editor has been asked for, answered or not.
    public private(set) var requests = 0

    public init() {}

    public var isRunning: Bool {
        if case .running = state { true } else { false }
    }

    /// The core asked for an editor on the draft called `title`. `available`
    /// says whether this build has a Vim to run.
    public mutating func request(title: String, available: Bool) -> Answer {
        requests += 1
        if isRunning {
            // The one that is running stays as it is.
            return .refuse(.didNotRun)
        }
        guard available else {
            state = .notAvailable
            return .refuse(.didNotRun)
        }
        state = .running(title: title)
        return .start
    }

    /// The Vim that was started has ended. Returns what to tell the core,
    /// or `nil` when no Vim was running, so that nothing is answered twice.
    public mutating func ended(_ ending: EditorEnding) -> EditorExited? {
        guard isRunning else { return nil }
        let answer = EditorExited(ending)
        state = answer.ran ? .exited(status: answer.status) : .failedToStart
        return answer
    }

    /// The state in a few words, for a scripted run's dump.
    public var summary: String {
        switch state {
        case .none: "none"
        case let .running(title): "running \(title.debugDescription)"
        case let .exited(status): "exited \(status)"
        case .notAvailable: "not available"
        case .failedToStart: "failed to start"
        }
    }
}

/// Asking Vim to save and leave, as closing its pane does.
///
/// Closing the pane must not lose what was written, and it is not a verdict
/// on the message: nothing is written for Vim or the core to find. Vim is
/// asked as a person at the keyboard would ask it, and is ended only if it
/// does not go.
public enum PoliteStop {
    /// `esc`: out of insert mode, the command line or a half-typed command.
    public static let leaveMode = "\u{1B}"

    /// How long to leave `esc` on its own, in seconds, so that Vim does not
    /// take it for the start of a longer key.
    public static let pause = 0.2

    /// `ctrl-\ ctrl-n`, which is normal mode from wherever `esc` left Vim,
    /// then `:wqa` and return: write what has changed and quit.
    public static let saveAndQuit = "\u{1C}\u{0E}:wqa\r"

    /// How long Vim has to go after being asked, in seconds, before it is
    /// hung up on.
    public static let patience = 2.0

    /// How long it then has, in seconds, before it is killed. The core is
    /// waiting on an answer, and gets one.
    public static let lastResort = 1.0
}
