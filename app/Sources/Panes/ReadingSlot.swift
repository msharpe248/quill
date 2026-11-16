/// What has the second pane's place.
public enum PaneOccupant: Equatable, Sendable {
    /// Nothing: the terminal has the window to itself.
    case nothing
    /// The reading pane.
    case reading
    /// A viewer, a player, a web page asked for by itself, or the editor.
    case other
}

/// The one place there is for the reading pane, and what becomes of it as
/// the core asks for pages and other panes come and go.
///
/// The reading pane shares the second pane's place with every other pane.
/// A viewer the user opens takes the place while it is open, and the
/// reading pane comes back, on the page the core last asked for, when the
/// viewer goes. This decides; it shows nothing itself, and knows a page
/// only as something to hand back.
public struct ReadingSlot<Page: Equatable & Sendable>: Equatable, Sendable {
    /// What to do about the reading pane.
    public enum Step: Equatable, Sendable {
        /// Put a reading pane up on this page, now: nothing is in its
        /// place.
        case show(Page)
        /// A reading pane is on show: load this page out of sight, and put
        /// it in that pane's place once there is something to see in it,
        /// so that going from message to message does not flash.
        case replace(Page)
        /// Take the reading pane down.
        case close
        /// Nothing: another pane has the place, or there is nothing to do.
        case nothing
    }

    /// The page to read, while the core wants a reading pane at all.
    public private(set) var page: Page?

    public init() {}

    /// The core asked for a page in the reading pane.
    public mutating func show(_ page: Page, over occupant: PaneOccupant) -> Step {
        self.page = page
        switch occupant {
        case .nothing: return .show(page)
        case .reading: return .replace(page)
        // It waits its turn: the viewer was asked for, and stays.
        case .other: return .nothing
        }
    }

    /// The core asked for the reading pane to go.
    public mutating func close(over occupant: PaneOccupant) -> Step {
        page = nil
        return occupant == .reading ? .close : .nothing
    }

    /// The pane that had the place has gone and left it empty.
    public func vacated() -> Step {
        page.map(Step.show) ?? .nothing
    }
}
