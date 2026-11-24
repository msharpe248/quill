import AppKit
import Bridge
import Forms

/// The settings window: a window of its own with a toolbar of panes, as a
/// Mac's settings are, drawn from what the core describes.
///
/// Which panes there are, which settings each has, what each is called, what
/// it may be set to and what is said beside it all come from the core
/// (`settings.show`), every time any of it changes. Nothing here knows what a
/// setting means or where it is kept: a control is drawn for a field by the
/// field's kind, and when the user changes one the core is told which field
/// and what it was given (`settings.used`). There is no OK and no Apply: a
/// change is made as it is made, and the core answers with the settings as
/// they then stand, which is what the window shows from then on.
///
/// What the shell does know is that there is a settings file, as its menu
/// does: the button at the foot of a pane opens it, and so does the button
/// the window has in place of everything else when the core says the file
/// cannot be read for what it says.
///
/// What the window holds lives in `SettingsModel`, which has no window and is
/// tested without one.
@MainActor
final class SettingsWindowController: NSObject, NSWindowDelegate {
    private(set) var model = SettingsModel()

    /// Where what the user did goes: to the core.
    private let send: (ShellToCore) -> Void
    /// Which pane the user last had.
    private let memory: any SettingsPaneMemory
    /// Opens the settings file, as the menu's own item for it does.
    private let openFile: () -> Void
    /// Whether the window keeps its place from one run to the next. Not in
    /// a scripted run, which leaves the user's own saved window as it was.
    private let keepsPlace: Bool

    private var window: SettingsWindow?
    private let tabs = PaneTabs()
    /// The user asked for the window and the core has yet to say what goes
    /// in it.
    private var waitingToOpen = false
    /// The window was put on screen by this, a moment ago: its becoming the
    /// key window is not the user coming back to it.
    private var justPresented = false
    /// The controls of each pane, by the field's name in its pane
    /// (`pane.field`); a list's buttons by `pane.field.action`.
    private var controls: [String: NSControl] = [:]
    private var tables: [String: ListSource] = [:]
    /// How many times the window has been opened, for a scripted run's dump.
    private(set) var timesOpened = 0
    /// What the core was told, in order, for a scripted run's dump.
    private(set) var told: [String] = []

    /// How wide a pane is, and the room around what is in it, in points.
    private static let width: CGFloat = 620
    private static let margin: CGFloat = 20
    /// The name of the pane that stands in for all of them when the core
    /// has none to show.
    private static let unreadable = "unreadable"
    private static let fileButtonTitle = "Open Settings File\u{2026}"
    /// The tag of a button that is pressed, and is not a checkbox.
    private static let pushButton = 1
    /// What the terminal's own font is called among the fonts to choose.
    private static let ownFontTitle = "Quill\u{2019}s Own"

    init(
        memory: any SettingsPaneMemory, keepsPlace: Bool, send: @escaping (ShellToCore) -> Void,
        openFile: @escaping () -> Void
    ) {
        self.memory = memory
        self.keepsPlace = keepsPlace
        self.send = send
        self.openFile = openFile
        super.init()
        tabs.tabStyle = .toolbar
        // One pane is put in another's place as it is: they differ in
        // height, and the window is resized to the new one here.
        tabs.transitionOptions = []
        tabs.onSelect = { [weak self] pane in self?.selected(pane) }
    }

    /// Whether the window is on screen.
    var isOpen: Bool { window?.isVisible ?? false }

    /// The window itself, for a sheet to be put on and for a scripted run's
    /// picture of it.
    var windowIfOpen: NSWindow? { isOpen ? window : nil }

    // MARK: - Opening and closing

    /// The user asked for the settings. The core is asked what they are;
    /// the window comes up when it has said, or at once if it has said
    /// before, with what it said then until it says again.
    func open() {
        if let window, window.isVisible {
            window.makeKeyAndOrderFront(nil)
            tell(.settingsOpened)
            return
        }
        tell(.settingsOpened)
        if model.settings != nil {
            present()
        } else {
            waitingToOpen = true
        }
    }

    /// Opens the window on the pane of this name, or brings it forward
    /// on it: the core asked for that pane. A name no pane has leaves the
    /// window on the pane it would have opened on.
    func open(on pane: String) {
        // Where the window opens is the pane remembered: so it is
        // remembered first, for a window that is still to be made.
        memory.pane = pane
        open()
        _ = choose(pane: pane)
    }

    /// Takes the window down, as closing it does.
    func close() {
        waitingToOpen = false
        window?.close()
    }

    private func present() {
        waitingToOpen = false
        let window = self.window ?? makeWindow()
        self.window = window
        rebuildPanes()
        if !window.isVisible {
            timesOpened += 1
            justPresented = true
        }
        window.makeKeyAndOrderFront(nil)
    }

    private func makeWindow() -> SettingsWindow {
        // Not resizable by hand: it is as large as the pane on show.
        let window = SettingsWindow(
            contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 200), styleMask: [.titled, .closable],
            backing: .buffered, defer: true)
        window.isReleasedWhenClosed = false
        window.title = "Settings"
        window.toolbarStyle = .preference
        window.autorecalculatesKeyViewLoop = true
        window.contentViewController = tabs
        window.delegate = self
        window.center()
        if keepsPlace {
            window.setFrameAutosaveName("QuillSettingsWindow")
        }
        return window
    }

    func windowWillClose(_: Notification) {
        // A number still being typed is taken as typed, as it is when the
        // keyboard leaves the field any other way.
        window?.makeFirstResponder(nil)
        model.close()
        tell(.settingsClosed)
    }

    /// The user has come back to the window: the settings file may have
    /// been edited meanwhile, and a sheet that was on the window may have
    /// added an account. The core reads the settings afresh.
    func windowDidBecomeKey(_: Notification) {
        if justPresented {
            justPresented = false
            return
        }
        tell(.settingsOpened)
    }

    private func tell(_ message: ShellToCore) {
        switch message {
        case .settingsOpened: note("opened")
        case .settingsClosed: note("closed")
        case let .settingsUsed(used):
            switch used.did {
            case let .set(.flag(flag)): note("\(used.pane).\(used.field) = \(flag)")
            case let .set(.number(number)):
                // As it is written on the bridge: a whole number as one.
                let whole = number == number.rounded() && abs(number) < 1e15
                note("\(used.pane).\(used.field) = \(whole ? String(Int64(number)) : String(number))")
            case let .set(.text(text)): note("\(used.pane).\(used.field) = \(text.debugDescription)")
            case let .action(action, row): note("\(used.pane).\(used.field) > \(action)\(row.map { " \($0)" } ?? "")")
            }
        default: break
        }
        send(message)
    }

    private func note(_ line: String) {
        told.append(line)
        told.removeFirst(max(told.count - 100, 0))
    }

    // MARK: - From the core

    /// Shows the settings as the core now has them, in place of what there
    /// was. Only what differs from what is on show is drawn again.
    func show(_ settings: SettingsShow) {
        let update = model.show(settings, remembered: memory.pane)
        if waitingToOpen {
            present()
            return
        }
        guard window != nil else { return }
        // With no panes, what is wrong is all there is to draw.
        if update.panesChanged || model.isUnreadable {
            rebuildPanes()
        } else {
            for pane in update.redraw {
                redraw(pane)
            }
            fit()
        }
    }

    // MARK: - The panes

    /// The names of the panes the window has: the core's, or the one that
    /// stands in for them when the core has none.
    private var paneIDs: [String] {
        model.isUnreadable ? [Self.unreadable] : model.panes.map(\.id)
    }

    /// Makes the toolbar's panes again, from what the core has now.
    private func rebuildPanes() {
        controls = [:]
        tables = [:]
        tabs.replacing = true
        for item in tabs.tabViewItems {
            tabs.removeTabViewItem(item)
        }
        if model.isUnreadable {
            let controller = PaneController(pane: Self.unreadable)
            controller.title = "Settings"
            let item = NSTabViewItem(viewController: controller)
            item.identifier = Self.unreadable
            item.label = "Settings"
            item.image = NSImage(systemSymbolName: "exclamationmark.triangle", accessibilityDescription: nil)
            tabs.addTabViewItem(item)
            controller.setContent(buildUnreadable())
        } else {
            for pane in model.panes {
                let controller = PaneController(pane: pane.id)
                controller.title = pane.title
                let item = NSTabViewItem(viewController: controller)
                item.identifier = pane.id
                item.label = pane.title
                item.image =
                    NSImage(systemSymbolName: pane.symbol, accessibilityDescription: pane.title)
                    ?? NSImage(systemSymbolName: "gearshape", accessibilityDescription: pane.title)
                tabs.addTabViewItem(item)
                controller.setContent(build(pane))
            }
        }
        tabs.replacing = false
        // The toolbar is left on show even with the one pane that stands in
        // for the rest. Hiding it and showing it again resizes the window
        // with an animation of AppKit's own, in the middle of which the
        // panes being put back ask for another size: seen to end in an
        // exception, in a scripted run.
        let wanted = model.isUnreadable ? Self.unreadable : model.pane
        if let index = paneIDs.firstIndex(where: { $0 == wanted }) {
            tabs.selectedTabViewItemIndex = index
        }
        retitle()
        fit()
    }

    /// Draws one pane again, keeping the keyboard where it was.
    private func redraw(_ id: String) {
        guard let described = model.pane(named: id), let controller = controller(of: id) else { return }
        let focused = focusedControl
        for key in controls.keys where key.hasPrefix("\(id).") {
            controls[key] = nil
        }
        for key in tables.keys where key.hasPrefix("\(id).") {
            tables[key] = nil
        }
        controller.setContent(build(described))
        if let focused, focused.hasPrefix("\(id)."), let control = controls[focused], control.isEnabled {
            window?.makeFirstResponder(control)
        }
    }

    private func controller(of pane: String) -> PaneController? {
        tabs.tabViewItems.compactMap { $0.viewController as? PaneController }.first { $0.pane == pane }
    }

    /// The pane on show was chosen, by the user or as the window was made.
    private func selected(_ pane: String) {
        if pane != Self.unreadable, model.select(pane: pane) {
            memory.pane = pane
        }
        retitle()
        fit()
    }

    /// The window is called what the pane on show is called, as a Mac's
    /// settings window is.
    private func retitle() {
        let shown = tabs.tabView.selectedTabViewItem
        window?.title = shown?.label ?? "Settings"
    }

    /// Makes the window as large as the pane on show, keeping its top left
    /// corner where it is. At once, and never as an animation: an animated
    /// resize runs the event loop until it is done, and what the core sends
    /// meanwhile would be drawn into a window that is between two sizes.
    private func fit() {
        guard let window, let controller = tabs.tabView.selectedTabViewItem?.viewController as? PaneController
        else { return }
        let size = controller.contentSize
        guard size.height > 0 else { return }
        var frame = window.frameRect(forContentRect: NSRect(origin: .zero, size: size))
        let old = window.frame
        guard frame.size != old.size else { return }
        frame.origin = NSPoint(x: old.minX, y: old.maxY - frame.height)
        window.setFrame(frame, display: true, animate: false)
    }

    // MARK: - Building a pane

    /// A pane laid out the way a Mac lays its settings out: what it says in
    /// the secondary colour, labels on the right of their column beside the
    /// controls, a list across the whole width with its buttons under it,
    /// what is wrong in red, and at the foot what more the file can say,
    /// beside the button that opens it.
    private func build(_ pane: SettingsPane) -> (NSView, NSSize) {
        let inner = Self.width - 2 * Self.margin
        let column = Self.column()
        if let note = pane.note, !note.isEmpty {
            column.addArrangedSubview(Self.wrapping(note, width: inner, color: .secondaryLabelColor))
        }
        // Fields in a row of labels and controls, until a list, which has
        // the whole width to itself.
        var run: [FormField] = []
        let flush = { [self] in
            guard !run.isEmpty else { return }
            column.addArrangedSubview(grid(of: run, in: pane.id, width: inner))
            run = []
        }
        for field in pane.fields {
            if case let .list(list) = field.kind {
                flush()
                column.addArrangedSubview(listView(list, of: field, in: pane.id, width: inner))
            } else {
                run.append(field)
            }
        }
        flush()
        if let problem = model.problem, !problem.isEmpty {
            let label = Self.wrapping(problem, width: inner, color: .systemRed)
            label.setAccessibilityLabel("Error: \(problem)")
            column.addArrangedSubview(label)
        }
        if let more = pane.more, !more.isEmpty {
            let line = NSBox()
            line.boxType = .separator
            line.widthAnchor.constraint(equalToConstant: inner).isActive = true
            column.addArrangedSubview(line)
            let button = fileButton()
            let room = inner - button.intrinsicContentSize.width - 12
            let label = Self.wrapping(
                more, width: room, color: .secondaryLabelColor, font: .systemFont(ofSize: NSFont.smallSystemFontSize))
            let foot = NSStackView(views: [label, button])
            foot.orientation = .horizontal
            foot.alignment = .centerY
            foot.spacing = 12
            column.addArrangedSubview(foot)
        }
        return Self.sized(column)
    }

    /// What the window has in place of its panes when the core has none:
    /// what is wrong, and the way to the file. It is the one pane then, and
    /// the toolbar has the one button for it.
    private func buildUnreadable() -> (NSView, NSSize) {
        let inner = Self.width - 2 * Self.margin
        let column = Self.column()
        let problem = model.problem ?? ""
        let label = Self.wrapping(problem, width: inner, color: .labelColor)
        label.identifier = NSUserInterfaceItemIdentifier("\(Self.unreadable).problem")
        column.addArrangedSubview(label)
        let gap = NSView()
        gap.setContentHuggingPriority(.init(1), for: .horizontal)
        let foot = NSStackView(views: [gap, fileButton()])
        foot.orientation = .horizontal
        foot.widthAnchor.constraint(equalToConstant: inner).isActive = true
        column.addArrangedSubview(foot)
        return Self.sized(column)
    }

    private func fileButton() -> NSButton {
        let button = NSButton(title: Self.fileButtonTitle, target: self, action: #selector(openSettingsFile(_:)))
        button.setContentCompressionResistancePriority(.required, for: .horizontal)
        return button
    }

    private static func column() -> NSStackView {
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 14
        column.edgeInsets = NSEdgeInsets(top: margin, left: margin, bottom: margin, right: margin)
        column.translatesAutoresizingMaskIntoConstraints = false
        return column
    }

    /// A pane's content in a view of its own, and how large it is.
    private static func sized(_ column: NSStackView) -> (NSView, NSSize) {
        let view = NSView()
        view.addSubview(column)
        NSLayoutConstraint.activate([
            column.widthAnchor.constraint(equalToConstant: width),
            column.topAnchor.constraint(equalTo: view.topAnchor),
            column.leadingAnchor.constraint(equalTo: view.leadingAnchor),
        ])
        column.layoutSubtreeIfNeeded()
        return (view, NSSize(width: width, height: column.fittingSize.height.rounded(.up)))
    }

    /// Fields with their labels in a column beside them.
    private func grid(of fields: [FormField], in pane: String, width: CGFloat) -> NSView {
        let labels = fields.map { field -> NSTextField in
            let label = NSTextField(labelWithString: field.label)
            label.alignment = .right
            return label
        }
        let spacing: CGFloat = 8
        let labelWidth = min(labels.map(\.intrinsicContentSize.width).max() ?? 0, width / 2).rounded(.up)
        let fieldWidth = width - labelWidth - spacing
        let grid = NSGridView()
        grid.rowSpacing = 10
        grid.columnSpacing = spacing
        grid.rowAlignment = .firstBaseline
        for (field, label) in zip(fields, labels) {
            grid.addRow(with: [label, control(for: field, in: pane, width: fieldWidth)])
            if let help = field.help, !help.isEmpty {
                let small = Self.wrapping(
                    help, width: fieldWidth, color: .secondaryLabelColor,
                    font: .systemFont(ofSize: NSFont.smallSystemFontSize))
                let added = grid.addRow(with: [NSGridCell.emptyContentView, small])
                // Close under the control it is about.
                added.topPadding = -6
            }
        }
        grid.column(at: 0).xPlacement = .trailing
        grid.column(at: 0).width = labelWidth
        grid.column(at: 1).xPlacement = .leading
        grid.column(at: 1).width = fieldWidth
        return grid
    }

    private func control(for field: FormField, in pane: String, width: CGFloat) -> NSView {
        let key = "\(pane).\(field.id)"
        let identifier = NSUserInterfaceItemIdentifier(key)
        switch field.kind {
        case let .choice(selected, options):
            let popup = NSPopUpButton(frame: .zero, pullsDown: false)
            popup.identifier = identifier
            for option in options {
                popup.addItem(withTitle: option.title)
                popup.lastItem?.representedObject = option.id
            }
            if let index = options.firstIndex(where: { $0.id == selected }) {
                popup.selectItem(at: index)
            }
            popup.target = self
            popup.action = #selector(chose(_:))
            popup.setAccessibilityLabel(field.label)
            controls[key] = popup
            return popup
        case let .toggle(title, value):
            let box = NSButton(checkboxWithTitle: title, target: self, action: #selector(toggled(_:)))
            box.identifier = identifier
            box.state = value ? .on : .off
            controls[key] = box
            return box
        case let .number(number):
            let text = NSTextField(string: number.written(number.value))
            text.identifier = identifier
            text.alignment = .right
            text.target = self
            text.action = #selector(typedNumber(_:))
            text.cell?.usesSingleLineMode = true
            text.widthAnchor.constraint(equalToConstant: 84).isActive = true
            text.setAccessibilityLabel(field.label)
            controls[key] = text
            let stepper = NSStepper()
            stepper.identifier = identifier
            stepper.minValue = number.min
            stepper.maxValue = number.max ?? .greatestFiniteMagnitude
            stepper.increment = number.step
            stepper.valueWraps = false
            // One step to a click: each is told to the core, which answers.
            stepper.autorepeat = false
            stepper.doubleValue = number.value
            stepper.target = self
            stepper.action = #selector(stepped(_:))
            var views: [NSView] = [text, stepper]
            if let unit = number.unit, !unit.isEmpty {
                views.append(NSTextField(labelWithString: unit))
            }
            let row = NSStackView(views: views)
            row.orientation = .horizontal
            row.alignment = .firstBaseline
            row.spacing = 6
            return row
        case let .text(value):
            let text = NSTextField(string: value)
            text.identifier = identifier
            text.target = self
            text.action = #selector(typedText(_:))
            text.lineBreakMode = .byTruncatingTail
            text.cell?.usesSingleLineMode = true
            text.cell?.isScrollable = true
            text.widthAnchor.constraint(equalToConstant: min(width, 320)).isActive = true
            text.setAccessibilityLabel(field.label)
            controls[key] = text
            return text
        case let .fixed(value):
            let label = Self.wrapping(value, width: width, color: .labelColor)
            label.identifier = identifier
            return label
        case let .button(title, enabled):
            let button = NSButton(title: title, target: self, action: #selector(pressedButton(_:)))
            button.identifier = identifier
            button.isEnabled = enabled
            // Told apart from a checkbox, which is a button too.
            button.tag = Self.pushButton
            controls[key] = button
            return button
        case let .font(family):
            // The fonts on this Mac that a terminal can be drawn in, with
            // the terminal's own font first. A font the settings name that
            // is not on this Mac is shown as that, and stays chosen until
            // another is.
            let popup = NSPopUpButton(frame: .zero, pullsDown: false)
            popup.identifier = identifier
            popup.addItem(withTitle: Self.ownFontTitle)
            popup.lastItem?.representedObject = ""
            popup.menu?.addItem(.separator())
            let families = FixedPitchFonts.families
            if !family.isEmpty, !families.contains(family) {
                popup.addItem(withTitle: "\(family) (not on this Mac)")
                popup.lastItem?.representedObject = family
            }
            for name in families {
                popup.addItem(withTitle: name)
                popup.lastItem?.representedObject = name
            }
            if let index = popup.itemArray.firstIndex(where: { $0.representedObject as? String == family }) {
                popup.selectItem(at: index)
            }
            popup.target = self
            popup.action = #selector(chose(_:))
            popup.setAccessibilityLabel(field.label)
            controls[key] = popup
            return popup
        // Settings have no secret, no code and no colour to pick, and a
        // list is laid out by itself: the core sends none of these here.
        case .secret, .code, .keys, .color, .list:
            return NSView()
        }
    }

    /// A list across the pane: its rows, or what is said when it has none,
    /// its buttons under it, and what is said about it under those.
    private func listView(_ list: FormList, of field: FormField, in pane: String, width: CGFloat) -> NSView {
        let key = "\(pane).\(field.id)"
        let block = NSStackView()
        block.orientation = .vertical
        block.alignment = .leading
        block.spacing = 8
        if !field.label.isEmpty {
            block.addArrangedSubview(NSTextField(labelWithString: field.label))
        }
        // A list to be searched is a long one: it has more room, and a
        // line to type on over it.
        let height: CGFloat = list.searchable ? 300 : 148
        let source = ListSource(rows: model.shownRows(in: field.id, of: pane)) { [weak self] row in
            self?.picked(row, in: field.id, of: pane)
        }
        tables[key] = source
        if list.searchable {
            let search = NSSearchField()
            search.identifier = NSUserInterfaceItemIdentifier("\(key).search")
            search.stringValue = model.search(in: field.id, of: pane)
            search.placeholderString = "Search"
            search.sendsSearchStringImmediately = true
            search.target = self
            search.action = #selector(searched(_:))
            search.widthAnchor.constraint(equalToConstant: width).isActive = true
            block.addArrangedSubview(search)
        }
        let table = NSTableView()
        table.identifier = NSUserInterfaceItemIdentifier(key)
        table.headerView = nil
        table.rowHeight = 40
        table.style = .inset
        table.allowsEmptySelection = true
        table.allowsMultipleSelection = false
        let only = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("row"))
        only.width = width - 24
        table.addTableColumn(only)
        table.columnAutoresizingStyle = .lastColumnOnlyAutoresizingStyle
        table.dataSource = source
        table.delegate = source
        table.setAccessibilityLabel(field.label.isEmpty ? "List" : field.label)
        source.table = table
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.translatesAutoresizingMaskIntoConstraints = false
        // The rows in a frame of their own, so that what is said when there
        // are none can stand in the middle of it: a scroll view lays out
        // what is put inside it in its own way.
        let frame = NSView()
        frame.addSubview(scroll)
        NSLayoutConstraint.activate([
            frame.widthAnchor.constraint(equalToConstant: width),
            frame.heightAnchor.constraint(equalToConstant: height),
            scroll.topAnchor.constraint(equalTo: frame.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: frame.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: frame.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: frame.trailingAnchor),
        ])
        block.addArrangedSubview(frame)
        if list.rows.isEmpty, let empty = list.empty, !empty.isEmpty {
            let label = NSTextField(labelWithString: empty)
            label.textColor = .secondaryLabelColor
            label.translatesAutoresizingMaskIntoConstraints = false
            frame.addSubview(label)
            NSLayoutConstraint.activate([
                label.centerXAnchor.constraint(equalTo: frame.centerXAnchor),
                label.centerYAnchor.constraint(equalTo: frame.centerYAnchor),
            ])
        }
        // The row that was picked is still picked, if it is still there.
        if let row = model.pickedRow(in: field.id, of: pane),
            let index = source.rows.firstIndex(where: { $0.id == row })
        {
            source.quiet = true
            table.selectRowIndexes([index], byExtendingSelection: false)
            source.quiet = false
        }
        if !list.actions.isEmpty {
            let buttons = NSStackView()
            buttons.orientation = .horizontal
            buttons.spacing = 8
            for action in list.actions {
                let button = NSButton(title: action.title, target: self, action: #selector(pressed(_:)))
                let name = "\(key).\(action.id)"
                button.identifier = NSUserInterfaceItemIdentifier(name)
                button.isEnabled = model.canPress(action.id, in: field.id, of: pane)
                controls[name] = button
                buttons.addArrangedSubview(button)
            }
            block.addArrangedSubview(buttons)
        }
        if let help = field.help, !help.isEmpty {
            block.addArrangedSubview(
                Self.wrapping(
                    help, width: width, color: .secondaryLabelColor,
                    font: .systemFont(ofSize: NSFont.smallSystemFontSize)))
        }
        return block
    }

    /// Text that takes as many lines as it needs at `width`, and can be
    /// selected.
    private static func wrapping(
        _ text: String, width: CGFloat, color: NSColor, font: NSFont = .systemFont(ofSize: NSFont.systemFontSize)
    ) -> NSTextField {
        let label = NSTextField(wrappingLabelWithString: text)
        label.font = font
        label.textColor = color
        label.isSelectable = true
        label.preferredMaxLayoutWidth = width
        label.widthAnchor.constraint(equalToConstant: width).isActive = true
        return label
    }

    // MARK: - The keyboard

    /// The name of the control the keyboard is in, if it is in one.
    private var focusedControl: String? {
        guard let responder = window?.firstResponder as? NSView else { return nil }
        return controls.first { _, control in responder === control || responder.isDescendant(of: control) }?.key
    }

    // MARK: - From the user

    /// The pane and the field a control is for, from its name.
    private func place(of control: NSView) -> (pane: String, field: String)? {
        guard let parts = control.identifier?.rawValue.split(separator: ".").map(String.init), parts.count >= 2
        else { return nil }
        return (parts[0], parts[1])
    }

    /// Tells the core a field was given a value, if it was one the field
    /// takes and has not got. Otherwise the pane is drawn again, so that the
    /// control says what is so.
    private func set(_ value: SettingValue, by control: NSView) {
        guard let (pane, field) = place(of: control) else { return }
        if let used = model.set(value, in: field, of: pane) {
            tell(.settingsUsed(used))
        } else {
            redraw(pane)
        }
    }

    @objc private func chose(_ sender: NSPopUpButton) {
        guard let option = sender.selectedItem?.representedObject as? String else { return }
        set(.text(option), by: sender)
    }

    @objc private func toggled(_ sender: NSButton) {
        set(.flag(sender.state == .on), by: sender)
    }

    @objc private func typedText(_ sender: NSTextField) {
        set(.text(sender.stringValue), by: sender)
    }

    @objc private func typedNumber(_ sender: NSTextField) {
        // As it is typed here, with a comma for the point or without.
        let typed = sender.stringValue.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: ",", with: ".")
        guard let number = Double(typed), number.isFinite else {
            // Not a number: the field says again what the number is.
            if let (pane, _) = place(of: sender) {
                redraw(pane)
            }
            return
        }
        setNumber(number, by: sender)
    }

    @objc private func stepped(_ sender: NSStepper) {
        setNumber(sender.doubleValue, by: sender)
    }

    /// Gives a number field a number, as typing it or stepping to it does:
    /// brought within the field's bounds, and written in the field.
    private func setNumber(_ number: Double, by control: NSView) {
        guard let (pane, field) = place(of: control),
            case let .number(described)? = model.settings?.field(field, in: pane)?.kind
        else { return }
        if let used = model.set(.number(number), in: field, of: pane) {
            tell(.settingsUsed(used))
        }
        // What the field holds now, in the text and under the stepper.
        let now: Double
        if case let .number(held)? = model.settings?.field(field, in: pane)?.kind {
            now = held.value
        } else {
            now = described.value
        }
        if let text = controls["\(pane).\(field)"] as? NSTextField {
            text.stringValue = described.written(now)
            for sibling in text.superview?.subviews ?? [] {
                (sibling as? NSStepper)?.doubleValue = now
            }
        }
    }

    private func picked(_ row: String?, in field: String, of pane: String) {
        model.pick(row: row, in: field, of: pane)
        guard case let .list(list)? = model.settings?.field(field, in: pane)?.kind else { return }
        for action in list.actions {
            controls["\(pane).\(field).\(action.id)"]?.isEnabled = model.canPress(action.id, in: field, of: pane)
        }
    }

    @objc private func pressed(_ sender: NSButton) {
        guard let parts = sender.identifier?.rawValue.split(separator: ".").map(String.init), parts.count == 3,
            let used = model.press(parts[2], in: parts[1], of: parts[0])
        else { return }
        tell(.settingsUsed(used))
    }

    /// Something was typed over a list that can be searched: its rows are
    /// narrowed to those that have it, here and now. The core is not told:
    /// what is looked for is the window's own business, like the row picked.
    @objc private func searched(_ sender: NSSearchField) {
        guard let parts = sender.identifier?.rawValue.split(separator: ".").map(String.init), parts.count == 3
        else { return }
        search(sender.stringValue, in: parts[1], of: parts[0])
    }

    private func search(_ typed: String, in field: String, of pane: String) {
        let key = "\(pane).\(field)"
        guard let source = tables[key], let table = source.table else { return }
        model.search(typed, in: field, of: pane)
        source.quiet = true
        source.rows = model.shownRows(in: field, of: pane)
        table.reloadData()
        // The row picked stays picked while it is still on show.
        if let row = model.pickedRow(in: field, of: pane),
            let index = source.rows.firstIndex(where: { $0.id == row })
        {
            table.selectRowIndexes([index], byExtendingSelection: false)
            table.scrollRowToVisible(index)
        } else {
            table.deselectAll(nil)
        }
        source.quiet = false
        picked(model.pickedRow(in: field, of: pane), in: field, of: pane)
    }

    /// A button of the core's was pressed: the core is told which, and
    /// decides what that starts.
    @objc private func pressedButton(_ sender: NSButton) {
        guard let (pane, field) = place(of: sender), let used = model.pressButton(field, of: pane) else { return }
        tell(.settingsUsed(used))
    }

    @objc private func openSettingsFile(_: Any?) {
        openFile()
    }

    // MARK: - For a scripted run

    /// Chooses a pane, as a click on its toolbar button does. False when
    /// there is no such pane.
    func choose(pane: String) -> Bool {
        guard isOpen, let index = paneIDs.firstIndex(of: pane) else { return false }
        tabs.selectedTabViewItemIndex = index
        return true
    }

    /// Gives the control of a field (`pane.field`) a value, as the user
    /// does: the name of an option for a pop-up, `on` or `off` for a
    /// checkbox, what is typed for a field. False when there is no such
    /// control, or the value is not one it can be given.
    func set(_ name: String, to value: String) -> Bool {
        guard isOpen, let control = controls[name], control.isEnabled else { return false }
        if let popup = control as? NSPopUpButton {
            // By the option's name, or by what it is called: something
            // the user named (a theme) has a name here that is made up.
            guard
                let index = popup.itemArray.firstIndex(where: { $0.representedObject as? String == value })
                    ?? popup.itemArray.firstIndex(where: { $0.title == value })
            else {
                return false
            }
            popup.selectItem(at: index)
            chose(popup)
        } else if let field = control as? NSTextField {
            field.stringValue = value
            _ = field.sendAction(field.action, to: field.target)
        } else if let box = control as? NSButton, box.tag != Self.pushButton {
            guard ["on", "off"].contains(value) else { return false }
            box.state = value == "on" ? .on : .off
            toggled(box)
        } else {
            return false
        }
        return true
    }

    /// Clicks a number field's stepper (`pane.field`) up or down once.
    func step(_ name: String, up: Bool) -> Bool {
        guard isOpen, let text = controls[name] as? NSTextField,
            let stepper = text.superview?.subviews.compactMap({ $0 as? NSStepper }).first
        else { return false }
        stepper.doubleValue += up ? stepper.increment : -stepper.increment
        stepped(stepper)
        return true
    }

    /// Picks the row of a list (`pane.field`) that has this title, as a
    /// click on it does; an empty title picks none.
    /// Types over the list of this name (`pane.field`), in place of what
    /// is there. False when there is no such list to search.
    func search(_ name: String, for typed: String) -> Bool {
        let parts = name.split(separator: ".").map(String.init)
        guard isOpen, parts.count == 2, let field = controlsSearch(name) else { return false }
        field.stringValue = typed
        search(typed, in: parts[1], of: parts[0])
        return true
    }

    private func controlsSearch(_ name: String) -> NSSearchField? {
        guard let table = tables[name]?.table else { return nil }
        var view: NSView? = table
        // The search field is in the same block as the table's frame.
        while let current = view, !(current is NSStackView) {
            view = current.superview
        }
        return (view as? NSStackView)?.arrangedSubviews.compactMap { $0 as? NSSearchField }.first
    }

    func pick(_ name: String, rowTitled title: String) -> Bool {
        guard isOpen, let source = tables[name], let table = source.table else { return false }
        if title.isEmpty {
            table.deselectAll(nil)
            return true
        }
        guard let index = source.rows.firstIndex(where: { $0.title == title }) else { return false }
        table.selectRowIndexes([index], byExtendingSelection: false)
        return true
    }

    /// Presses a button of a list (`pane.field.action`), or a button that
    /// is a field of its own (`pane.field`). False when there is none, or
    /// it cannot be pressed now.
    func press(_ name: String) -> Bool {
        guard isOpen, let button = controls[name] as? NSButton, button.isEnabled,
            name.split(separator: ".").count == 3 || button.tag == Self.pushButton
        else { return false }
        button.performClick(nil)
        return true
    }

    /// Presses the button that opens the settings file.
    func pressFileButton() -> Bool {
        guard isOpen else { return false }
        openFile()
        return true
    }

    /// The window in lines of text, for the dump: whether it is open, what
    /// it is called and how large it is, where the keyboard is, what the
    /// core was told, and what the model says of the settings.
    var summary: [String] {
        var lines: [String] = []
        if let window, window.isVisible {
            let content = window.contentRect(forFrameRect: window.frame).size
            let toolbar = (window.toolbar?.items ?? []).map(\.label)
            lines.append("settings window: open, titled \(window.title.debugDescription)")
            lines.append("settings window size: \(Int(content.width)) x \(Int(content.height))")
            lines.append("settings window resizable: \(window.styleMask.contains(.resizable))")
            lines.append("settings toolbar: \(toolbar.joined(separator: " | "))")
            lines.append("settings keys go to: \(focusedControl ?? "no control")")
            // The controls as they stand, which is what a person sees:
            // each must agree with what the model says below.
            let drawn = controls.sorted { $0.key < $1.key }.map { name, control -> String in
                let shows: String
                if let popup = control as? NSPopUpButton {
                    shows = (popup.selectedItem?.representedObject as? String) ?? "?"
                } else if let field = control as? NSTextField {
                    shows = field.stringValue
                } else if let button = control as? NSButton {
                    shows =
                        name.split(separator: ".").count == 3 || button.tag == Self.pushButton
                        ? (button.isEnabled ? "enabled" : "disabled") : (button.state == .on ? "on" : "off")
                } else {
                    shows = "?"
                }
                return "\(name)=\(shows)"
            }
            lines.append("settings controls: \(drawn.joined(separator: " "))")
        } else {
            lines.append("settings window: closed")
        }
        lines.append("settings windows opened: \(timesOpened)")
        lines.append("settings pane remembered: \(memory.pane ?? "none")")
        lines.append("settings told the core: \(told.joined(separator: "; "))")
        if model.settings != nil {
            lines.append(contentsOf: model.summary)
        }
        return lines
    }
}

/// The settings window itself: esc and cmd-W close it, as they close any
/// Mac settings window.
final class SettingsWindow: NSWindow {
    override func cancelOperation(_: Any?) {
        performClose(nil)
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let held = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if event.type == .keyDown, held == .command, event.charactersIgnoringModifiers == "w" {
            performClose(nil)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}

/// The toolbar of panes. It says which pane was chosen.
@MainActor
private final class PaneTabs: NSTabViewController {
    var onSelect: ((String) -> Void)?
    /// The panes are being made again: what is chosen meanwhile is not the
    /// user's choice.
    var replacing = false

    override func tabView(_ tabView: NSTabView, willSelect tabViewItem: NSTabViewItem?) {
        // What is still being typed in the pane being left is taken as
        // typed.
        if !replacing {
            view.window?.makeFirstResponder(nil)
        }
        super.tabView(tabView, willSelect: tabViewItem)
    }

    override func tabView(_ tabView: NSTabView, didSelect tabViewItem: NSTabViewItem?) {
        super.tabView(tabView, didSelect: tabViewItem)
        guard !replacing, let pane = tabViewItem?.identifier as? String else { return }
        onSelect?(pane)
    }
}

/// One pane: a view that holds whatever was last built for it, at the top.
@MainActor
private final class PaneController: NSViewController {
    let pane: String
    private(set) var contentSize = NSSize.zero

    init(pane: String) {
        self.pane = pane
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override func loadView() {
        view = NSView()
    }

    func setContent(_ built: (view: NSView, size: NSSize)) {
        for old in view.subviews {
            old.removeFromSuperview()
        }
        built.view.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(built.view)
        NSLayoutConstraint.activate([
            built.view.topAnchor.constraint(equalTo: view.topAnchor),
            built.view.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            built.view.widthAnchor.constraint(equalToConstant: built.size.width),
            built.view.heightAnchor.constraint(equalToConstant: built.size.height),
        ])
        contentSize = built.size
        preferredContentSize = built.size
    }
}

/// The rows of a list, for its table, and word of which is picked.
@MainActor
private final class ListSource: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    /// The rows on show: all of the list's, or those a search has left.
    var rows: [FormList.Row]
    weak var table: NSTableView?
    /// A row is being picked by the window itself, to show what was picked
    /// before: nobody is told.
    var quiet = false
    private let onPick: (String?) -> Void

    init(rows: [FormList.Row], onPick: @escaping (String?) -> Void) {
        self.rows = rows
        self.onPick = onPick
    }

    func numberOfRows(in _: NSTableView) -> Int {
        rows.count
    }

    func tableView(_: NSTableView, viewFor _: NSTableColumn?, row index: Int) -> NSView? {
        guard rows.indices.contains(index) else { return nil }
        let row = rows[index]
        let title = NSTextField(labelWithString: row.title)
        title.lineBreakMode = .byTruncatingTail
        let detail = NSTextField(labelWithString: row.detail ?? "")
        detail.font = .systemFont(ofSize: NSFont.smallSystemFontSize)
        detail.textColor = .secondaryLabelColor
        detail.lineBreakMode = .byTruncatingTail
        let lines = NSStackView(views: [title, detail])
        lines.orientation = .vertical
        lines.alignment = .leading
        lines.spacing = 2
        lines.translatesAutoresizingMaskIntoConstraints = false
        let cell = NSTableCellView()
        cell.addSubview(lines)
        cell.textField = title
        NSLayoutConstraint.activate([
            lines.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
            lines.trailingAnchor.constraint(lessThanOrEqualTo: cell.trailingAnchor, constant: -4),
            lines.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }

    func tableViewSelectionDidChange(_: Notification) {
        guard !quiet, let table else { return }
        let index = table.selectedRow
        onPick(rows.indices.contains(index) ? rows[index].id : nil)
    }
}
