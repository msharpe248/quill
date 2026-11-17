import AppKit
import Bridge
import Forms

/// The window of every key: what Help > Keyboard shortcuts brings up.
///
/// It is there to be glanced at while mail is read and written, so it is a
/// panel that floats over the main window, takes the keyboard only when its
/// search field is clicked, and stays until it is closed. Nothing about it
/// is modal.
///
/// What it lists is the core's (`keys.list`): the commands, what each is
/// called, under which headings, in what order. The window draws that, and
/// draws it again when the core says the keys have changed.
@MainActor
final class KeysWindowController: NSObject, NSWindowDelegate, NSTableViewDataSource, NSTableViewDelegate,
    NSSearchFieldDelegate
{
    /// Whether where the window was left is kept for the next run. A
    /// scripted run keeps nothing and reads nothing of the user's.
    private let keepsPlace: Bool
    /// The window it is first put beside.
    private let beside: () -> NSWindow?

    /// What the core last said the keys are.
    private var list: KeysList?
    private var shown = KeyRows(KeysList(title: "", sections: []))
    private var query = ""

    private var panel: NSPanel?
    private let search = NSSearchField()
    private let table = NSTableView()
    private let empty = NSTextField(labelWithString: "")
    /// How many times the window has been brought up, for a scripted run's
    /// dump.
    private(set) var timesShown = 0

    private static let size = NSSize(width: 400, height: 560)
    private static let smallest = NSSize(width: 300, height: 220)
    private static let rowHeight: CGFloat = 26
    private static let headingHeight: CGFloat = 30
    private static let column = NSUserInterfaceItemIdentifier("keys")

    init(keepsPlace: Bool, beside: @escaping () -> NSWindow?) {
        self.keepsPlace = keepsPlace
        self.beside = beside
    }

    /// Whether the window is on screen.
    var isOpen: Bool { panel?.isVisible ?? false }

    /// The window itself, for a scripted run's picture of it.
    var windowIfOpen: NSWindow? { isOpen ? panel : nil }

    // MARK: - What the core says

    /// The keys as they now are. A window that is open shows them at once.
    func set(_ list: KeysList) {
        guard list != self.list else { return }
        self.list = list
        panel?.title = list.title
        reload()
    }

    /// Brings the window up, or forward if it is up. The keyboard stays
    /// where it was: in the mail.
    func show() {
        guard let list else { return }
        let panel = self.panel ?? makePanel(titled: list.title)
        self.panel = panel
        if !panel.isVisible {
            timesShown += 1
            reload()
        }
        panel.orderFront(nil)
    }

    func close() {
        panel?.close()
    }

    // MARK: - The window

    private func makePanel(titled title: String) -> NSPanel {
        let panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: Self.size),
            styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: true)
        panel.title = title
        panel.isReleasedWhenClosed = false
        // Over the main window, and out of the way of its keys.
        panel.isFloatingPanel = true
        panel.becomesKeyOnlyIfNeeded = true
        panel.hidesOnDeactivate = true
        panel.collectionBehavior.insert(.fullScreenAuxiliary)
        panel.contentMinSize = Self.smallest
        panel.delegate = self
        panel.contentView = makeContent()
        place(panel)
        if keepsPlace {
            // Where it was left, if it has been up before.
            panel.setFrameAutosaveName("QuillKeysWindow")
        }
        return panel
    }

    /// Inside the top right corner of the main window, where it covers the
    /// message being read and not the list.
    private func place(_ panel: NSPanel) {
        guard let main = beside(), let screen = main.screen ?? NSScreen.main else {
            panel.center()
            return
        }
        let inset: CGFloat = 24
        let frame = panel.frame
        var origin = NSPoint(
            x: main.frame.maxX - frame.width - inset,
            y: main.frame.maxY - frame.height - inset - 52)
        let visible = screen.visibleFrame
        origin.x = min(max(origin.x, visible.minX), visible.maxX - frame.width)
        origin.y = min(max(origin.y, visible.minY), visible.maxY - frame.height)
        panel.setFrameOrigin(origin)
    }

    private func makeContent() -> NSView {
        search.placeholderString = "Search keys and commands"
        search.delegate = self
        search.sendsSearchStringImmediately = true
        search.setAccessibilityLabel("Search keys and commands")
        search.translatesAutoresizingMaskIntoConstraints = false

        let column = NSTableColumn(identifier: Self.column)
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .inset
        table.selectionHighlightStyle = .none
        table.allowsEmptySelection = true
        table.floatsGroupRows = true
        table.intercellSpacing = NSSize(width: 0, height: 0)
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        // The rows are read, not chosen: the keyboard has no use for the list.
        table.refusesFirstResponder = true
        table.dataSource = self
        table.delegate = self

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false

        empty.textColor = .secondaryLabelColor
        empty.alignment = .center
        empty.isHidden = true
        empty.translatesAutoresizingMaskIntoConstraints = false

        let content = NSView()
        content.addSubview(search)
        content.addSubview(scroll)
        content.addSubview(empty)
        NSLayoutConstraint.activate([
            search.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            search.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 12),
            search.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            scroll.topAnchor.constraint(equalTo: search.bottomAnchor, constant: 8),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            empty.centerXAnchor.constraint(equalTo: scroll.centerXAnchor),
            empty.topAnchor.constraint(equalTo: scroll.topAnchor, constant: 40),
            empty.leadingAnchor.constraint(greaterThanOrEqualTo: content.leadingAnchor, constant: 16),
        ])
        return content
    }

    /// The rows for what the core last said and what is typed.
    private func reload() {
        guard let list else { return }
        shown = KeyRows(list, matching: query)
        guard panel != nil else { return }
        table.reloadData()
        empty.stringValue = "No key or command has \u{201C}\(query)\u{201D} in it."
        empty.isHidden = !shown.rows.isEmpty
    }

    func windowWillClose(_: Notification) {
        // The next time it is brought up it is the whole list again.
        query = ""
        search.stringValue = ""
    }

    // MARK: - Searching

    func controlTextDidChange(_: Notification) {
        query = search.stringValue
        reload()
    }

    // MARK: - The list

    func numberOfRows(in _: NSTableView) -> Int {
        shown.rows.count
    }

    func tableView(_: NSTableView, isGroupRow row: Int) -> Bool {
        guard shown.rows.indices.contains(row), case .heading = shown.rows[row] else { return false }
        return true
    }

    func tableView(_: NSTableView, heightOfRow row: Int) -> CGFloat {
        guard shown.rows.indices.contains(row), case .heading = shown.rows[row] else { return Self.rowHeight }
        return Self.headingHeight
    }

    func tableView(_: NSTableView, shouldSelectRow _: Int) -> Bool {
        false
    }

    func tableView(_: NSTableView, viewFor _: NSTableColumn?, row: Int) -> NSView? {
        guard shown.rows.indices.contains(row) else { return nil }
        switch shown.rows[row] {
        case let .heading(title):
            let label = NSTextField(labelWithString: title)
            label.font = .systemFont(ofSize: NSFont.smallSystemFontSize, weight: .semibold)
            label.textColor = .secondaryLabelColor
            label.lineBreakMode = .byTruncatingTail
            let cell = NSTableCellView()
            cell.textField = label
            label.translatesAutoresizingMaskIntoConstraints = false
            cell.addSubview(label)
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
                label.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor, constant: -4),
                label.bottomAnchor.constraint(equalTo: cell.bottomAnchor, constant: -5),
            ])
            return cell
        case let .key(title, keys):
            return KeyRowView(title: title, keys: keys)
        }
    }

    // MARK: - For a scripted run

    /// Types into the search field, as the user would.
    func search(for text: String) -> Bool {
        guard isOpen else { return false }
        search.stringValue = text
        query = text
        reload()
        return true
    }

    /// The window in lines, for the dump: whether it is up, and every row
    /// it has.
    var summary: [String] {
        guard let panel, panel.isVisible else {
            return ["keys window: closed, brought up \(timesShown) times, list \(list == nil ? "not sent" : "sent")"]
        }
        let content = panel.contentRect(forFrameRect: panel.frame).size
        var lines = [
            "keys window: open, titled \(panel.title.debugDescription), brought up \(timesShown) times",
            "keys window size: \(Int(content.width)) x \(Int(content.height))",
            "keys window floats: \(panel.isFloatingPanel), has the keyboard: \(panel.isKeyWindow)",
            "keys search: \(query.debugDescription), \(shown.count) commands shown",
        ]
        for row in shown.rows {
            switch row {
            case let .heading(title): lines.append("keys heading: \(title)")
            case let .key(title, keys):
                lines.append("keys row: \(title) = \(keys.isEmpty ? "(none)" : keys.joined(separator: " | "))")
            }
        }
        return lines
    }
}

/// One command in the window of keys: what it does on the left, and its
/// keys on the right, each on a cap of its own.
private final class KeyRowView: NSTableCellView {
    init(title: String, keys: [String]) {
        super.init(frame: .zero)
        let label = NSTextField(labelWithString: title)
        label.lineBreakMode = .byTruncatingTail
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        label.setContentHuggingPriority(.defaultLow, for: .horizontal)
        label.toolTip = title
        textField = label

        let caps = NSStackView()
        caps.orientation = .horizontal
        caps.spacing = 5
        caps.setContentCompressionResistancePriority(.required, for: .horizontal)
        caps.setContentHuggingPriority(.required, for: .horizontal)
        if keys.isEmpty {
            let none = NSTextField(labelWithString: "no key")
            none.textColor = .tertiaryLabelColor
            none.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
            caps.addArrangedSubview(none)
        }
        for key in keys {
            caps.addArrangedSubview(Self.cap(key))
        }

        for view in [label, caps] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            addSubview(view)
        }
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            caps.leadingAnchor.constraint(greaterThanOrEqualTo: label.trailingAnchor, constant: 10),
            caps.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            caps.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        setAccessibilityLabel("\(title): \(keys.isEmpty ? "no key" : keys.joined(separator: ", "))")
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("not made from a nib")
    }

    /// A key on a cap: a rounded patch that follows light and dark.
    private static func cap(_ key: String) -> NSView {
        let label = NSTextField(labelWithString: key)
        // The system's own face: it has the Mac's signs for keys at a
        // size they can be told apart at.
        label.font = .systemFont(ofSize: 12, weight: .medium)
        label.textColor = .labelColor
        label.alignment = .center
        label.translatesAutoresizingMaskIntoConstraints = false
        let box = NSBox()
        box.boxType = .custom
        box.titlePosition = .noTitle
        box.cornerRadius = 4
        box.borderWidth = 1
        box.borderColor = .separatorColor
        box.fillColor = .quaternaryLabelColor
        box.contentViewMargins = .zero
        box.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 6),
            label.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -6),
            label.topAnchor.constraint(equalTo: box.topAnchor, constant: 2),
            label.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -2),
            // A cap for one letter is as wide as it is tall.
            label.widthAnchor.constraint(greaterThanOrEqualToConstant: 9),
        ])
        box.setContentCompressionResistancePriority(.required, for: .horizontal)
        return box
    }
}
