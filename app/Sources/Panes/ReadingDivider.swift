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
/// reader's place exactly), and for a window of how many columns it
/// worked that out. Nothing else says: a divider the user drags is told
/// to the core, whose edge it is, and stays where the core then says.
/// Nothing here knows why the core says what it says.
public enum ReadingDivider {
    /// What the core said: the terminal keeps `columns` columns, in a
    /// window with room for `window` of them.
    public struct Asked: Equatable, Sendable {
        public var columns: Int
        public var window: Int

        public init(columns: Int, window: Int) {
            self.columns = columns
            self.window = window
        }
    }

    /// The fewest columns the terminal is left with.
    public static let fewestColumns = 24
    /// The least width the reading pane is left with, in points.
    public static let narrowestPane: CGFloat = 200

    /// Whether what the core said was worked out for the window as it is
    /// now: for as many columns as `width` has room for.
    public static func isCurrent(_ asked: Asked?, width: CGFloat, cells: CellMeasure) -> Bool {
        asked?.window == cells.columns(inWidth: width)
    }

    /// How many columns the terminal keeps beside the reading pane.
    ///
    /// What the core said for a window of another width is not gone by as
    /// it stands: until the core has said again, the terminal keeps the
    /// same share of the window as it is now.
    ///
    /// - Parameters:
    ///   - asked: What the core said, if it has.
    ///   - width: The width there is for the terminal, the divider and the
    ///     pane together.
    ///   - divider: The divider's own thickness.
    public static func columns(asked: Asked?, width: CGFloat, divider: CGFloat, cells: CellMeasure) -> Int {
        let window = cells.columns(inWidth: width)
        let wanted: Int
        if let asked, asked.window == window {
            wanted = asked.columns
        } else if let asked, asked.window > 0 {
            wanted = Int((Double(asked.columns) * Double(window) / Double(asked.window)).rounded())
        } else {
            // Before the core has said: half the window each.
            wanted = window / 2
        }
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
    public static func terminalWidth(asked: Asked?, width: CGFloat, divider: CGFloat, cells: CellMeasure) -> CGFloat {
        cells.width(forColumns: columns(asked: asked, width: width, divider: divider, cells: cells))
    }

    /// Where an older Quill kept a place of the shell's own for the
    /// divider. Nothing reads it now: it is taken out of the defaults.
    public static let formerDefaultsKey = "ReadingPaneTerminalColumns"
}
