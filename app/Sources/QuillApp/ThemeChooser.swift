import AppKit
import Themes

/// The chooser of themes to import: a sheet with the themes of a Ghostty
/// installed on this Mac, a field to search them by name, and the way to a
/// theme file kept anywhere else.
///
/// There are several hundred of them, which is more than a pop-up is any
/// use for: so a list, each row with the theme's name and a patch of its
/// colours, narrowed as the user types. Return imports the row picked, as a
/// double click does; esc cancels. The arrow keys move through the list
/// from the search field, where the keyboard starts.
///
/// The chooser only chooses. Copying the file in is the window's to do, and
/// what a theme is, the core's: the patch beside a name is read from the
/// file for the eye and for nothing else.
@MainActor
final class ThemeChooser: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSSearchFieldDelegate {
    /// Where themes are looked for.
    private let folders: [URL]
    /// The user chose this theme's file to import.
    private let onImport: (URL) -> Void
    /// The user asked to choose a file from somewhere else, having had the
    /// chooser on this window.
    private let onOther: (NSWindow?) -> Void

    /// Every theme found, and the ones the search and the scope leave.
    private var all: [ThemeSource] = []
    private var shown: [ThemeSource] = []
    /// What became of looking in each folder, once they have been looked in.
    private(set) var report: [ThemeFolderReport]?
    private var query = ""
    private var scope = ThemeScope.all

    private var sheet: NSWindow?
    private weak var parent: NSWindow?
    // The sheet's parts, made afresh each time it is opened.
    private var search = NSSearchField()
    private var scopes = NSSegmentedControl()
    private var table = NSTableView()
    private var status = NSTextField(labelWithString: "")
    private var empty = NSTextField(wrappingLabelWithString: "")
    private var importButton: NSButton?
    /// How many times the chooser has been opened, for a scripted run's dump.
    private(set) var timesOpened = 0

    private static let width: CGFloat = 520
    private static let margin: CGFloat = 20
    private static let listHeight: CGFloat = 300

    init(folders: [URL], onImport: @escaping (URL) -> Void, onOther: @escaping (NSWindow?) -> Void) {
        self.folders = folders
        self.onImport = onImport
        self.onOther = onOther
    }

    /// Whether the chooser is on screen.
    var isUp: Bool { sheet != nil }

    /// The chooser's own window, for a scripted run's picture of it.
    var window: NSWindow? { sheet }

    // MARK: - Opening and closing

    /// Shows the chooser as a sheet on `parent`. The folders are looked in
    /// afresh every time, off the main thread: a theme added to Ghostty
    /// since is there, and the window is never kept waiting for a disk.
    func open(on parent: NSWindow?) {
        guard let parent, sheet == nil, parent.attachedSheet == nil else { return }
        query = ""
        scope = .all
        report = nil
        all = []
        shown = []
        let sheet = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 200), styleMask: [.titled],
            backing: .buffered, defer: true)
        sheet.isReleasedWhenClosed = false
        let content = build()
        sheet.contentView = content
        content.layoutSubtreeIfNeeded()
        sheet.setContentSize(content.fittingSize)
        self.sheet = sheet
        self.parent = parent
        timesOpened += 1
        refresh()
        parent.beginSheet(sheet)
        sheet.makeFirstResponder(search)

        let folders = folders
        Task.detached(priority: .userInitiated) { [weak self] in
            let found = GhosttyThemes.themes(in: folders)
            await MainActor.run {
                // For the chooser that asked, if it is still up.
                guard let self, self.sheet === sheet else { return }
                self.all = found.themes
                self.report = found.report
                self.refresh()
            }
        }
    }

    /// Takes the chooser down.
    func close() {
        guard let sheet else { return }
        self.sheet = nil
        parent?.endSheet(sheet)
        sheet.orderOut(nil)
        parent = nil
        importButton = nil
    }

    // MARK: - Building

    private func build() -> NSView {
        let inner = Self.width - 2 * Self.margin
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 12
        column.edgeInsets = NSEdgeInsets(top: Self.margin, left: Self.margin, bottom: Self.margin, right: Self.margin)
        column.translatesAutoresizingMaskIntoConstraints = false

        let title = NSTextField(labelWithString: "Import a Theme")
        title.font = .boldSystemFont(ofSize: NSFont.systemFontSize)
        column.addArrangedSubview(title)
        let note = NSTextField(
            wrappingLabelWithString:
                "Themes made for Ghostty that are on this Mac. The one you import is copied into "
                + "Quill\u{2019}s themes folder and chosen as the theme.")
        note.textColor = .secondaryLabelColor
        note.preferredMaxLayoutWidth = inner
        note.widthAnchor.constraint(equalToConstant: inner).isActive = true
        column.addArrangedSubview(note)

        search = NSSearchField()
        scopes = NSSegmentedControl(
            labels: ["All", "Dark", "Light"], trackingMode: .selectOne, target: self, action: #selector(scoped(_:)))
        table = NSTableView()
        status = NSTextField(labelWithString: "")
        empty = NSTextField(wrappingLabelWithString: "")
        search.placeholderString = "Search"
        search.delegate = self
        search.target = self
        search.action = #selector(searched(_:))
        search.sendsSearchStringImmediately = true
        search.stringValue = ""
        search.setAccessibilityLabel("Search themes")
        scopes.selectedSegment = 0
        scopes.setContentHuggingPriority(.required, for: .horizontal)
        scopes.setAccessibilityLabel("Which themes")
        let bar = NSStackView(views: [search, scopes])
        bar.orientation = .horizontal
        bar.spacing = 10
        bar.widthAnchor.constraint(equalToConstant: inner).isActive = true
        column.addArrangedSubview(bar)

        table.headerView = nil
        table.rowHeight = 30
        table.style = .inset
        table.allowsEmptySelection = true
        table.allowsMultipleSelection = false
        table.usesAlternatingRowBackgroundColors = false
        let only = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("theme"))
        only.width = inner - 24
        table.addTableColumn(only)
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(doubleClicked(_:))
        table.setAccessibilityLabel("Themes")
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.translatesAutoresizingMaskIntoConstraints = false
        // In a frame of its own, so that what is said when there is
        // nothing to list can stand in the middle of it.
        let frame = NSView()
        frame.addSubview(scroll)
        empty.alignment = .center
        empty.textColor = .secondaryLabelColor
        empty.preferredMaxLayoutWidth = inner - 80
        empty.translatesAutoresizingMaskIntoConstraints = false
        frame.addSubview(empty)
        NSLayoutConstraint.activate([
            frame.widthAnchor.constraint(equalToConstant: inner),
            frame.heightAnchor.constraint(equalToConstant: Self.listHeight),
            scroll.topAnchor.constraint(equalTo: frame.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: frame.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: frame.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: frame.trailingAnchor),
            empty.centerXAnchor.constraint(equalTo: frame.centerXAnchor),
            empty.centerYAnchor.constraint(equalTo: frame.centerYAnchor),
            empty.widthAnchor.constraint(lessThanOrEqualToConstant: inner - 80),
        ])
        column.addArrangedSubview(frame)

        status.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        status.textColor = .secondaryLabelColor
        column.addArrangedSubview(status)

        let other = NSButton(title: "Other\u{2026}", target: self, action: #selector(otherPressed(_:)))
        other.toolTip = "Choose a theme file from anywhere"
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelPressed(_:)))
        cancel.keyEquivalent = "\u{1b}"
        let importer = NSButton(title: "Import", target: self, action: #selector(importPressed(_:)))
        importer.keyEquivalent = "\r"
        importButton = importer
        let gap = NSView()
        gap.setContentHuggingPriority(.init(1), for: .horizontal)
        let buttons = NSStackView(views: [other, gap, cancel, importer])
        buttons.orientation = .horizontal
        buttons.spacing = 10
        buttons.widthAnchor.constraint(equalToConstant: inner).isActive = true
        column.addArrangedSubview(buttons)

        let view = NSView()
        view.addSubview(column)
        NSLayoutConstraint.activate([
            column.widthAnchor.constraint(equalToConstant: Self.width),
            column.topAnchor.constraint(equalTo: view.topAnchor),
            column.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            column.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            column.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        ])
        return view
    }

    // MARK: - What is listed

    /// The name of the theme picked, if one is.
    private var picked: ThemeSource? {
        shown.indices.contains(table.selectedRow) ? shown[table.selectedRow] : nil
    }

    /// Lists what the search and the scope leave, keeping the theme that
    /// was picked if it is still there, and otherwise picking the first:
    /// return then imports what the search found best.
    private func refresh() {
        let kept = picked?.name
        shown = GhosttyThemes.filter(all, by: query, scope: scope)
        table.reloadData()
        if let index = shown.firstIndex(where: { $0.name == kept }) ?? (shown.isEmpty ? nil : 0) {
            table.selectRowIndexes([index], byExtendingSelection: false)
            table.scrollRowToVisible(index)
        }
        if report == nil {
            empty.stringValue = "Looking for themes\u{2026}"
            status.stringValue = ""
        } else if all.isEmpty {
            empty.stringValue =
                "No themes made for Ghostty were found on this Mac. Choose Other\u{2026} to import a theme file "
                + "from anywhere."
            status.stringValue = ""
        } else {
            empty.stringValue = shown.isEmpty ? "No theme of that name." : ""
            status.stringValue =
                shown.count == all.count ? "\(all.count) themes" : "\(shown.count) of \(all.count) themes"
        }
        empty.isHidden = empty.stringValue.isEmpty
        importButton?.isEnabled = picked != nil
    }

    func numberOfRows(in _: NSTableView) -> Int {
        shown.count
    }

    func tableView(_: NSTableView, viewFor _: NSTableColumn?, row index: Int) -> NSView? {
        guard shown.indices.contains(index) else { return nil }
        let theme = shown[index]
        let patch = SwatchView(swatch: theme.swatch)
        patch.translatesAutoresizingMaskIntoConstraints = false
        let name = NSTextField(labelWithString: theme.name)
        name.lineBreakMode = .byTruncatingTail
        name.translatesAutoresizingMaskIntoConstraints = false
        let cell = NSTableCellView()
        cell.addSubview(patch)
        cell.addSubview(name)
        cell.textField = name
        NSLayoutConstraint.activate([
            patch.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
            patch.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            patch.widthAnchor.constraint(equalToConstant: SwatchView.size.width),
            patch.heightAnchor.constraint(equalToConstant: SwatchView.size.height),
            name.leadingAnchor.constraint(equalTo: patch.trailingAnchor, constant: 10),
            name.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor, constant: -4),
            name.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func tableViewSelectionDidChange(_: Notification) {
        importButton?.isEnabled = picked != nil
    }

    // MARK: - From the user

    @objc private func searched(_ sender: NSSearchField) {
        guard sender.stringValue != query else { return }
        query = sender.stringValue
        refresh()
    }

    func controlTextDidChange(_: Notification) {
        searched(search)
    }

    /// The arrow keys move through the list while the keyboard is in the
    /// search field, as they do in any Mac list with a search above it.
    func control(_: NSControl, textView _: NSTextView, doCommandBy selector: Selector) -> Bool {
        let step: Int
        switch selector {
        case #selector(NSResponder.moveDown(_:)): step = 1
        case #selector(NSResponder.moveUp(_:)): step = -1
        case #selector(NSResponder.scrollPageDown(_:)), #selector(NSResponder.pageDown(_:)): step = 9
        case #selector(NSResponder.scrollPageUp(_:)), #selector(NSResponder.pageUp(_:)): step = -9
        default: return false
        }
        guard !shown.isEmpty else { return true }
        let from = table.selectedRow < 0 ? (step > 0 ? -1 : shown.count) : table.selectedRow
        let to = min(max(from + step, 0), shown.count - 1)
        table.selectRowIndexes([to], byExtendingSelection: false)
        table.scrollRowToVisible(to)
        return true
    }

    @objc private func scoped(_ sender: NSSegmentedControl) {
        scope =
            ThemeScope.allCases.indices.contains(sender.selectedSegment)
            ? ThemeScope.allCases[sender.selectedSegment] : .all
        refresh()
    }

    @objc private func doubleClicked(_: Any?) {
        guard shown.indices.contains(table.clickedRow) else { return }
        importPressed(nil)
    }

    @objc private func importPressed(_: Any?) {
        guard let theme = picked else { return }
        close()
        onImport(theme.file)
    }

    @objc private func otherPressed(_: Any?) {
        let window = parent
        close()
        onOther(window)
    }

    @objc private func cancelPressed(_: Any?) {
        close()
    }

    // MARK: - For a scripted run

    /// Types into the search field, in place of what is there.
    func search(for text: String) -> Bool {
        guard isUp else { return false }
        search.stringValue = text
        searched(search)
        return true
    }

    /// Chooses which themes are listed: `all`, `dark` or `light`.
    func show(scope name: String) -> Bool {
        guard isUp, let wanted = ThemeScope(rawValue: name), let index = ThemeScope.allCases.firstIndex(of: wanted)
        else { return false }
        scopes.selectedSegment = index
        scoped(scopes)
        return true
    }

    /// Picks the row of the theme with this name, as a click on it does.
    func pick(named name: String) -> Bool {
        guard isUp, let index = shown.firstIndex(where: { $0.name == name }) else { return false }
        table.selectRowIndexes([index], byExtendingSelection: false)
        table.scrollRowToVisible(index)
        return true
    }

    /// Presses a button of the chooser: `import`, `other` or `cancel`.
    func press(_ button: String) -> Bool {
        guard isUp else { return false }
        switch button {
        case "import":
            guard picked != nil else { return false }
            importPressed(nil)
        case "other": otherPressed(nil)
        case "cancel": cancelPressed(nil)
        default: return false
        }
        return true
    }

    /// The chooser in lines of text, for the dump.
    var summary: [String] {
        var lines: [String] = []
        if isUp {
            let size = sheet?.contentView?.frame.size ?? .zero
            lines.append(
                "theme chooser: up, \(Int(size.width)) x \(Int(size.height)), search \(query.debugDescription), "
                    + "showing \(scope.rawValue), \(shown.count) of \(all.count) themes, "
                    + "picked \(picked?.name.debugDescription ?? "none"), "
                    + "import \(importButton?.isEnabled == true ? "enabled" : "disabled")")
            let first = shown.prefix(8).map { "\($0.name) (\($0.swatch.isDark ? "dark" : "light"))" }
            lines.append("theme chooser rows: \(first.joined(separator: " | "))")
            lines.append("theme chooser says: \(status.stringValue) \(empty.stringValue)")
            let responder = sheet?.firstResponder as? NSView
            let inSearch = responder.map { $0 === search || $0.isDescendant(of: search) } ?? false
            lines.append("theme chooser keys go to: \(inSearch ? "the search field" : "elsewhere")")
        } else {
            lines.append("theme chooser: closed")
        }
        lines.append("theme choosers opened: \(timesOpened)")
        if let report {
            lines.append("themes looked for in: \(report.map(\.summary).joined(separator: "; "))")
        }
        return lines
    }
}

/// A patch of a theme's colours: its background, with two letters in its
/// text colour and a dot of each colour its palette has for red, green,
/// yellow, blue, magenta and cyan.
@MainActor
final class SwatchView: NSView {
    static let size = NSSize(width: 124, height: 20)

    private let swatch: ThemeSwatch

    init(swatch: ThemeSwatch) {
        self.swatch = swatch
        super.init(frame: NSRect(origin: .zero, size: Self.size))
        setAccessibilityElement(true)
        setAccessibilityRole(.image)
        setAccessibilityLabel(swatch.isDark ? "A dark theme" : "A light theme")
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    private static func color(_ color: ThemeSwatch.Color) -> NSColor {
        NSColor(
            srgbRed: CGFloat(color.red) / 255, green: CGFloat(color.green) / 255, blue: CGFloat(color.blue) / 255,
            alpha: 1)
    }

    override func draw(_: NSRect) {
        let frame = bounds.insetBy(dx: 0.5, dy: 0.5)
        let shape = NSBezierPath(roundedRect: frame, xRadius: 5, yRadius: 5)
        Self.color(swatch.background).setFill()
        shape.fill()
        // A hairline, so that a patch the colour of the list is still seen.
        NSColor.separatorColor.setStroke()
        shape.lineWidth = 1
        shape.stroke()

        let letters = NSAttributedString(
            string: "Aa",
            attributes: [
                .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .medium),
                .foregroundColor: Self.color(swatch.foreground),
            ])
        let lettersSize = letters.size()
        letters.draw(at: NSPoint(x: 8, y: (bounds.height - lettersSize.height) / 2))

        let dot: CGFloat = 8
        var x = 8 + lettersSize.width + 10
        for accent in swatch.accents {
            Self.color(accent).setFill()
            NSBezierPath(ovalIn: NSRect(x: x, y: (bounds.height - dot) / 2, width: dot, height: dot)).fill()
            x += dot + 5
        }
    }
}
