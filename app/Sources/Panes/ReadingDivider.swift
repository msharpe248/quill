import Foundation

/// The width of a terminal cell, and the sums that go between a width in
/// points and a number of columns.
///
/// A terminal view has as many columns as whole cells fit in its width in
/// pixels. A cell is a whole number of pixels wide, so the sums are done
/// in pixels and are exact; a point may be two pixels, and a cell an odd
/// number of them.
public struct CellMeasure: Equatable, Sendable {
    /// The width of one cell, in the screen's own pixels.
    public var cellWidthPixels: Int
    /// How many of those pixels there are to a point.
    public var scale: CGFloat

    /// `nil` unless a cell has some width and a point some pixels.
    public init?(cellWidthPixels: Int, scale: CGFloat) {
        guard cellWidthPixels > 0, scale > 0, scale.isFinite else { return nil }
        self.cellWidthPixels = cellWidthPixels
        self.scale = scale
    }

    /// How many columns a terminal view this many points wide has.
    public func columns(inWidth width: CGFloat) -> Int {
        guard width > 0, width.isFinite else { return 0 }
        return Int((width * scale).rounded(.down)) / cellWidthPixels
    }

    /// The narrowest width, in whole points, at which a terminal view has
    /// this many columns. Whole points, so that nothing between here and
    /// the screen rounds it down into one column fewer; the fraction of a
    /// cell that adds is always less than a cell.
    public func width(forColumns columns: Int) -> CGFloat {
        (CGFloat(max(columns, 0) * cellWidthPixels) / scale).rounded(.up)
    }

    /// The number of columns nearest to this width: where a divider let go
    /// of at this width comes to rest.
    public func nearestColumns(toWidth width: CGFloat) -> Int {
        guard width > 0, width.isFinite else { return 0 }
        return Int((width * scale / CGFloat(cellWidthPixels)).rounded())
    }
}

/// Where the divider between the terminal and the reading pane goes.
///
/// The place is a number of columns for the terminal to keep. The core
/// says how many (the accounts' and the list's, so that the pane has the
/// reader's place exactly); a place the user dragged the divider to stands
/// instead of that until they put it back. Nothing here knows why the core
/// says what it says.
public enum ReadingDivider {
    /// The fewest columns the terminal is left with.
    public static let fewestColumns = 24
    /// The least width the reading pane is left with, in points.
    public static let narrowestPane: CGFloat = 200

    /// How many columns the terminal keeps beside the reading pane.
    ///
    /// - Parameters:
    ///   - remembered: Where the user left the divider, if they moved it.
    ///   - asked: What the core said, if it has.
    ///   - width: The width there is for the terminal, the divider and the
    ///     pane together.
    ///   - divider: The divider's own thickness.
    public static func columns(
        remembered: Int?, asked: Int?, width: CGFloat, divider: CGFloat, cells: CellMeasure
    ) -> Int {
        let window = cells.columns(inWidth: width)
        // Before the core has said: half the window each.
        let wanted = remembered ?? asked ?? window / 2
        return held(wanted, width: width, divider: divider, cells: cells)
    }

    /// Where a divider the user let go of at this width comes to rest: on
    /// the nearest whole column that leaves both sides room.
    public static func columns(
        draggedTo terminalWidth: CGFloat, width: CGFloat, divider: CGFloat, cells: CellMeasure
    ) -> Int {
        held(cells.nearestColumns(toWidth: terminalWidth), width: width, divider: divider, cells: cells)
    }

    /// `wanted`, kept to what leaves the pane its least width and the
    /// terminal its fewest columns. In a window with room for only one of
    /// the two, the pane's width gives way: a terminal too narrow to draw
    /// in is no use to anyone.
    static func held(_ wanted: Int, width: CGFloat, divider: CGFloat, cells: CellMeasure) -> Int {
        let window = cells.columns(inWidth: width)
        let most = max(cells.columns(inWidth: width - divider - narrowestPane), min(fewestColumns, window), 1)
        let fewest = min(fewestColumns, most)
        return min(max(wanted, fewest), most)
    }

    /// The width to give the terminal for it to have that many columns.
    public static func terminalWidth(
        remembered: Int?, asked: Int?, width: CGFloat, divider: CGFloat, cells: CellMeasure
    ) -> CGFloat {
        cells.width(
            forColumns: columns(remembered: remembered, asked: asked, width: width, divider: divider, cells: cells))
    }
}

/// Where the user left the divider between the terminal and the reading
/// pane: how many columns the terminal keeps. `nil` while they have not
/// moved it, or have put it back.
///
/// It is how the window was left, not a setting, and it is counted in the
/// shell's own measure, so the shell keeps it, as it keeps the window's
/// size.
public protocol DividerMemory: AnyObject {
    var columns: Int? { get set }
}

/// Kept in the app's defaults, from one run to the next.
public final class DefaultsDividerMemory: DividerMemory {
    public static let key = "ReadingPaneTerminalColumns"

    private let defaults: UserDefaults

    public init(defaults: UserDefaults) {
        self.defaults = defaults
    }

    public var columns: Int? {
        get {
            // Anything there that is not a number of columns is nothing.
            guard let stored = defaults.object(forKey: Self.key) as? Int, (1...65535).contains(stored) else {
                return nil
            }
            return stored
        }
        set {
            if let newValue, (1...65535).contains(newValue) {
                defaults.set(newValue, forKey: Self.key)
            } else {
                defaults.removeObject(forKey: Self.key)
            }
        }
    }
}

/// Kept for the run and no longer: for a scripted run, which must leave
/// the user's own defaults as they were, and read nothing from them.
public final class TransientDividerMemory: DividerMemory {
    public var columns: Int?

    public init(columns: Int? = nil) {
        self.columns = columns
    }
}
