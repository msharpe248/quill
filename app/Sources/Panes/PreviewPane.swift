import AppKit
import QuickLookUI

/// A file for the Quick Look pane, and what to call it.
public struct PreviewItem: Equatable, Sendable {
    public var url: URL
    public var title: String

    public init(url: URL, title: String) {
        self.url = url
        self.title = title
    }
}

/// The files of a Quick Look pane and which of them is showing. Stepping
/// stops at both ends: it does not wrap round.
public struct PreviewDeck: Equatable, Sendable {
    public let items: [PreviewItem]
    public private(set) var index: Int

    /// `nil` unless `index` is one of the items.
    public init?(items: [PreviewItem], index: Int) {
        guard items.indices.contains(index) else { return nil }
        self.items = items
        self.index = index
    }

    public var current: PreviewItem { items[index] }

    /// "2 of 5" for the second of five; `nil` when there is only one, which
    /// needs no counting.
    public var position: String? {
        items.count > 1 ? "\(index + 1) of \(items.count)" : nil
    }

    /// Moves forwards (or, for a negative distance, backwards) as far as the
    /// ends allow. True when a different item is now the current one.
    @discardableResult
    public mutating func step(by distance: Int) -> Bool {
        let target = min(max(index + distance, 0), items.count - 1)
        guard target != index else { return false }
        index = target
        return true
    }
}

/// Quick Look in the second pane: one file at a time, with the arrow keys
/// stepping through the rest.
@MainActor
public final class PreviewPane: PaneView {
    public private(set) var deck: PreviewDeck
    private let preview: QLPreviewView

    public init(deck: PreviewDeck) {
        self.deck = deck
        // `.normal` always returns a view; the initialiser is failable only
        // for Objective-C's sake.
        preview = QLPreviewView(frame: .zero, style: .normal) ?? QLPreviewView()
        super.init(kind: .preview, title: deck.current.title, content: preview)
        showCurrent()
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// The file being shown.
    public var fileURL: URL { deck.current.url }

    /// The arrow keys step through the files when there are several, and
    /// are used up even at an end, so that the last file does not suddenly
    /// scroll instead. With one file they are Quick Look's own.
    override public func handleKey(_ key: PaneKey) -> Bool {
        guard deck.items.count > 1 else { return false }
        switch key {
        case .right, .down: step(by: 1)
        case .left, .up: step(by: -1)
        case .escape, .space: return false
        }
        return true
    }

    /// Shows other files in the same pane, as if it had been opened on them.
    public func show(_ deck: PreviewDeck) {
        self.deck = deck
        // The same path may hold something new: read it again.
        loaded = nil
        showCurrent()
    }

    private func step(by distance: Int) {
        if deck.step(by: distance) {
            showCurrent()
        }
    }

    private func showCurrent() {
        let item = deck.current
        setHeader(title: item.title, detail: deck.position ?? "")
        if isFile(item.url) {
            setMessage(nil)
            load()
        } else {
            setMessage("This file is no longer there to show.")
        }
    }

    /// The file Quick Look was last given.
    private(set) var loaded: URL?

    /// Quick Look is given a file only once the pane is in a window, which
    /// is where it comes to life.
    private func load() {
        guard window != nil, message == nil, loaded != deck.current.url else { return }
        loaded = deck.current.url
        preview.previewItem = deck.current.url as NSURL
    }

    override public func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        load()
    }

    override public var summary: String {
        let place = "item \(deck.index + 1) of \(deck.items.count), \(fileURL.lastPathComponent)"
        return message == nil ? place : "\(place), \(super.summary)"
    }
}
