import Foundation

/// The size of a terminal cell in the screen's own pixels, and how many of
/// those pixels there are to a point.
public struct CellSize: Equatable, Sendable {
    public var widthPixels: Int
    public var heightPixels: Int
    public var scale: CGFloat

    /// `nil` unless a cell has some width and height and a point some
    /// pixels.
    public init?(widthPixels: Int, heightPixels: Int, scale: CGFloat) {
        guard widthPixels > 0, heightPixels > 0, scale > 0, scale.isFinite else { return nil }
        self.widthPixels = widthPixels
        self.heightPixels = heightPixels
        self.scale = scale
    }
}

/// Where an editor's terminal goes when it is laid over another terminal,
/// over a rectangle of that terminal's cells.
///
/// The terminal underneath has no padding, so its cell at column `c`, row
/// `r` starts `c` cells from its left edge and `r` from its top. Both are
/// whole pixels, so the sums are done in pixels and are exact: the editor's
/// terminal is exactly as many cells wide and tall as it was given, and
/// every one of its cells lies over one of the terminal's.
public enum EditorOverlay {
    /// The frames for an editor given `columns` by `rows` cells at
    /// `column`, `row`, of which the top `covered` rows are not to be seen.
    ///
    /// - Returns: `shown`, the part to be seen, in points from the top left
    ///   of the terminal underneath, in a view whose y grows downwards; and
    ///   `whole`, the editor's own frame inside that part, whose top is
    ///   above it by the rows that are covered. A part with no height means
    ///   all of it is covered.
    public static func frames(
        column: Int, row: Int, columns: Int, rows: Int, covered: Int, cells: CellSize
    ) -> (shown: CGRect, whole: CGRect) {
        let hidden = min(max(covered, 0), max(rows, 0))
        let point = { (pixels: Int) in CGFloat(pixels) / cells.scale }
        let width = point(max(columns, 0) * cells.widthPixels)
        let height = point(max(rows, 0) * cells.heightPixels)
        let above = point(hidden * cells.heightPixels)
        let shown = CGRect(
            x: point(max(column, 0) * cells.widthPixels),
            y: point(max(row, 0) * cells.heightPixels) + above,
            width: width,
            height: height - above)
        let whole = CGRect(x: 0, y: -above, width: width, height: height)
        return (shown, whole)
    }
}
