import AppKit
import Bridge
import Forms

/// A form the core has the shell show, as a sheet on a window: the steps of
/// adding an account, and every other question the core asks before going on
/// (a sign-in to wait for, a certificate, a time, a thing to confirm). Which
/// of them it is showing is nothing this knows. The window is the main one,
/// or the settings window when the question came up from there; the sheet
/// stays on the window it opened on until it is taken down.
///
/// The sheet is drawn from what the core sends and nothing else: its title,
/// what it says, its fields in the core's order and its buttons. It is drawn
/// again every time the core sends the form, which it does whenever any of
/// it changes. What a button does is the core's to decide: pressing one
/// sends what the fields hold, and the sheet stays as it is until the core
/// answers with another form or has it put away.
///
/// What the user has typed lives in `FormModel`, which has no window and is
/// tested without one. The exception is a secret field: what is typed there
/// is only ever in the field itself, until a button sends it, and then the
/// field is emptied.
@MainActor
final class FormSheet: NSObject, NSTextFieldDelegate {
    private(set) var model = FormModel()

    /// Where what the user did goes: to the core.
    private let send: (FormUsed) -> Void

    private var sheet: NSWindow?
    private weak var parent: NSWindow?
    /// The controls of the fields that can be typed in or chosen from, by
    /// field name.
    private var controls: [String: NSControl] = [:]
    private var buttons: [String: NSButton] = [:]
    /// The field a key was last recorded in, and when: a second key soon
    /// after the first is the second of a pair (`g g`), and not another
    /// key in its place.
    private var lastKey: (field: String, at: Date)?
    /// How long after one key a second still counts as following it.
    private static let pairWithin: TimeInterval = 1.0
    /// How many times the sheet has been opened, for a scripted run's dump.
    private(set) var timesOpened = 0

    /// How wide the sheet's content is, and the room around it, in points.
    private static let width: CGFloat = 500
    private static let margin: CGFloat = 20
    /// How large a code to be read off is drawn, in points.
    private static let codeSize: CGFloat = 18
    /// How large a well a colour is picked in is drawn.
    private static let wellSize = NSSize(width: 44, height: 24)

    init(send: @escaping (FormUsed) -> Void) {
        self.send = send
    }

    /// Whether the sheet is on screen.
    var isUp: Bool { sheet != nil }

    /// The sheet's own window, for a scripted run's picture of it.
    var window: NSWindow? { sheet }

    // MARK: - From the core

    /// Shows the form, in place of the one there was, opening the sheet on
    /// `parent` if it is not up.
    func show(_ form: FormShow, on parent: NSWindow?) {
        guard let parent else { return }
        // A form that differs only in the colours its wells hold is not
        // laid out again: a well a colour is being picked in stays the
        // well the colour panel is picking for.
        let coloursOnly = sheet != nil && model.differsOnlyInColors(from: form)
        let picking = controls.first { ($0.value as? NSColorWell)?.isActive == true }?.key
        let update = model.show(form)
        if coloursOnly {
            for row in model.rows {
                if case let .color(hex) = row.content, let well = controls[row.id] as? NSColorWell,
                    row.id != picking, let colour = Self.colour(hex)
                {
                    well.color = colour
                }
            }
            return
        }
        // What a secret field holds is kept across a redraw of the same
        // step by handing it from the old field to the new one, here and
        // nowhere else.
        var kept: [String: String] = [:]
        if update.sameStep {
            for (id, control) in controls where control is NSSecureTextField {
                kept[id] = control.stringValue
            }
        }
        // The field with the keyboard keeps it, unless the core says the
        // keyboard goes elsewhere.
        let hadFocus = focusedField
        let sheet = self.sheet ?? makeSheet()
        let content = build(keeping: kept)
        sheet.contentView = content.view
        resize(sheet, to: content.size)
        if self.sheet == nil {
            self.sheet = sheet
            self.parent = parent
            timesOpened += 1
            parent.beginSheet(sheet)
        }
        sheet.recalculateKeyViewLoop()
        if let focus = update.focus ?? hadFocus {
            focusField(focus)
        }
        // The colour panel goes on picking for the well it was picking
        // for, which is a new view now.
        if let picking, let well = controls[picking] as? NSColorWell {
            well.activate(true)
        }
    }

    /// Takes the sheet down. Nothing typed in it is kept.
    func close() {
        model.close()
        // A field's own copy of what was typed goes with the field, and
        // the colour panel with the well it was picking for.
        var wasPicking = false
        for control in controls.values {
            if let well = control as? NSColorWell {
                wasPicking = wasPicking || well.isActive
                well.deactivate()
            } else {
                control.stringValue = ""
            }
        }
        if wasPicking, NSColorPanel.sharedColorPanelExists {
            NSColorPanel.shared.orderOut(nil)
        }
        controls = [:]
        buttons = [:]
        lastKey = nil
        guard let sheet else { return }
        self.sheet = nil
        parent?.endSheet(sheet)
        sheet.orderOut(nil)
        parent = nil
    }

    // MARK: - Building

    private func makeSheet() -> NSWindow {
        let sheet = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 160), styleMask: [.titled],
            backing: .buffered, defer: true)
        sheet.isReleasedWhenClosed = false
        return sheet
    }

    /// The form laid out the way a Mac lays one out: a title, what it says
    /// in the secondary colour, labels on the right of their column beside
    /// the fields, what is wrong in red under them, and the buttons along
    /// the bottom on the right, with what is being waited for on the left.
    private func build(keeping kept: [String: String]) -> (view: NSView, size: NSSize) {
        controls = [:]
        buttons = [:]
        let inner = Self.width - 2 * Self.margin
        let column = NSStackView()
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 12
        column.edgeInsets = NSEdgeInsets(top: Self.margin, left: Self.margin, bottom: Self.margin, right: Self.margin)
        column.translatesAutoresizingMaskIntoConstraints = false
        guard let form = model.form else { return (column, NSSize(width: Self.width, height: 0)) }

        // A title can be a whole sentence (what saying yes will do), or
        // have an account's name in it: it takes the lines it needs.
        let title = Self.wrapping(
            form.title, width: inner, color: .labelColor, font: .boldSystemFont(ofSize: NSFont.systemFontSize))
        column.addArrangedSubview(title)
        if let note = form.note, !note.isEmpty {
            column.addArrangedSubview(Self.wrapping(note, width: inner, color: .secondaryLabelColor))
        }

        // Fields in a column of labels and controls, and each run of
        // colours two to a line: a theme has many.
        var run: [FormRow] = []
        var wells: [FormRow] = []
        for row in model.rows {
            if case .color = row.content {
                if !run.isEmpty {
                    column.addArrangedSubview(grid(of: run, width: inner, keeping: kept))
                    run = []
                }
                wells.append(row)
            } else {
                if !wells.isEmpty {
                    column.addArrangedSubview(wellGrid(of: wells, width: inner))
                    wells = []
                }
                run.append(row)
            }
        }
        if !run.isEmpty {
            column.addArrangedSubview(grid(of: run, width: inner, keeping: kept))
        }
        if !wells.isEmpty {
            column.addArrangedSubview(wellGrid(of: wells, width: inner))
        }
        return finish(column, form: form, inner: inner)
    }

    /// Fields with their labels in a column beside them.
    private func grid(of rows: [FormRow], width inner: CGFloat, keeping kept: [String: String]) -> NSView {
        let labels = rows.map { row -> NSTextField in
            let label = NSTextField(labelWithString: row.label)
            label.alignment = .right
            return label
        }
        // The labels' column is as wide as the longest of them, and the
        // fields have the rest.
        let spacing: CGFloat = 8
        let labelWidth = min(labels.map(\.intrinsicContentSize.width).max() ?? 0, inner / 2).rounded(.up)
        let fieldWidth = inner - labelWidth - spacing
        let grid = NSGridView()
        grid.rowSpacing = 8
        grid.columnSpacing = spacing
        grid.rowAlignment = .firstBaseline
        for (row, label) in zip(rows, labels) {
            let control = makeControl(for: row, width: fieldWidth, kept: kept[row.id])
            grid.addRow(with: [label, control])
            if let help = row.help, !help.isEmpty {
                let small = Self.wrapping(
                    help, width: fieldWidth, color: .secondaryLabelColor,
                    font: .systemFont(ofSize: NSFont.smallSystemFontSize))
                let added = grid.addRow(with: [NSGridCell.emptyContentView, small])
                // Close under the field it is about.
                added.topPadding = -4
            }
        }
        grid.column(at: 0).xPlacement = .trailing
        grid.column(at: 0).width = labelWidth
        grid.column(at: 1).xPlacement = .leading
        grid.column(at: 1).width = fieldWidth
        return grid
    }

    /// Colours to pick, two to a line: each a label and a well beside it.
    private func wellGrid(of rows: [FormRow], width inner: CGFloat) -> NSView {
        let spacing: CGFloat = 8
        let between: CGFloat = 24
        let labelWidth = ((inner - between) / 2 - spacing - Self.wellSize.width).rounded(.down)
        let grid = NSGridView()
        grid.rowSpacing = 8
        grid.columnSpacing = spacing
        grid.rowAlignment = .none
        grid.yPlacement = .center
        var index = 0
        while index < rows.count {
            var cells: [NSView] = []
            for row in rows[index..<min(index + 2, rows.count)] {
                let label = NSTextField(labelWithString: row.label)
                label.alignment = .right
                label.lineBreakMode = .byTruncatingTail
                cells.append(label)
                cells.append(makeControl(for: row, width: Self.wellSize.width, kept: nil))
            }
            while cells.count < 4 {
                cells.append(NSGridCell.emptyContentView)
            }
            grid.addRow(with: cells)
            index += 2
        }
        for column in [0, 2] {
            grid.column(at: column).xPlacement = .trailing
            grid.column(at: column).width = labelWidth
        }
        for column in [1, 3] {
            grid.column(at: column).xPlacement = .leading
            grid.column(at: column).width = Self.wellSize.width
        }
        grid.column(at: 1).trailingPadding = between - spacing
        return grid
    }

    /// The form's foot: what is wrong, what is being waited for, and the
    /// buttons; and the whole of it sized.
    private func finish(_ column: NSStackView, form: FormShow, inner: CGFloat) -> (view: NSView, size: NSSize) {
        if let error = form.error, !error.isEmpty {
            let label = Self.wrapping(error, width: inner, color: .systemRed)
            label.setAccessibilityLabel("Error: \(error)")
            column.addArrangedSubview(label)
        }

        let bottom = NSStackView()
        bottom.orientation = .horizontal
        bottom.alignment = .centerY
        bottom.spacing = 8
        if let busy = form.busy {
            let spinner = NSProgressIndicator()
            spinner.style = .spinning
            spinner.controlSize = .small
            spinner.isIndeterminate = true
            spinner.startAnimation(nil)
            bottom.addArrangedSubview(spinner)
            let waiting = NSTextField(labelWithString: busy)
            waiting.textColor = .secondaryLabelColor
            waiting.lineBreakMode = .byTruncatingTail
            // It gives way to the buttons, which keep their size.
            waiting.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            bottom.addArrangedSubview(waiting)
        }
        let gap = NSView()
        gap.setContentHuggingPriority(.init(1), for: .horizontal)
        bottom.addArrangedSubview(gap)
        for described in form.buttons {
            let button = NSButton(title: described.title, target: self, action: #selector(pressed(_:)))
            button.identifier = NSUserInterfaceItemIdentifier(described.id)
            button.isEnabled = described.enabled
            switch described.role {
            case .confirm: button.keyEquivalent = "\r"
            case .cancel: button.keyEquivalent = "\u{1B}"
            case nil: break
            }
            button.widthAnchor.constraint(greaterThanOrEqualToConstant: 84).isActive = true
            bottom.addArrangedSubview(button)
            buttons[described.id] = button
        }
        column.addArrangedSubview(bottom)
        column.setCustomSpacing(16, after: column.arrangedSubviews[column.arrangedSubviews.count - 2])

        let view = NSView()
        view.addSubview(column)
        NSLayoutConstraint.activate([
            column.widthAnchor.constraint(equalToConstant: Self.width),
            column.topAnchor.constraint(equalTo: view.topAnchor),
            column.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            bottom.widthAnchor.constraint(equalToConstant: inner),
        ])
        column.layoutSubtreeIfNeeded()
        return (view, NSSize(width: Self.width, height: column.fittingSize.height.rounded(.up)))
    }

    private func makeControl(for row: FormRow, width: CGFloat, kept: String?) -> NSView {
        let identifier = NSUserInterfaceItemIdentifier(row.id)
        switch row.content {
        case let .text(text):
            let field = NSTextField(string: text)
            field.identifier = identifier
            field.delegate = self
            field.isEnabled = row.enabled
            field.lineBreakMode = .byTruncatingTail
            field.cell?.usesSingleLineMode = true
            field.cell?.isScrollable = true
            field.widthAnchor.constraint(equalToConstant: width).isActive = true
            field.setAccessibilityLabel(row.label)
            controls[row.id] = field
            return field
        case .secret:
            let field = NSSecureTextField(string: kept ?? "")
            field.identifier = identifier
            field.delegate = self
            field.isEnabled = row.enabled
            field.widthAnchor.constraint(equalToConstant: width).isActive = true
            field.setAccessibilityLabel(row.label)
            controls[row.id] = field
            return field
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
            popup.isEnabled = row.enabled
            popup.setAccessibilityLabel(row.label)
            controls[row.id] = popup
            return popup
        case let .fixed(text):
            let label = Self.wrapping(text, width: width, color: .labelColor)
            label.identifier = identifier
            return label
        case let .color(hex):
            let well = NSColorWell(frame: NSRect(origin: .zero, size: Self.wellSize))
            well.identifier = identifier
            well.colorWellStyle = .default
            well.color = Self.colour(hex) ?? .black
            well.isEnabled = row.enabled
            well.target = self
            well.action = #selector(picked(_:))
            well.widthAnchor.constraint(equalToConstant: Self.wellSize.width).isActive = true
            well.heightAnchor.constraint(equalToConstant: Self.wellSize.height).isActive = true
            well.setAccessibilityLabel(row.label)
            controls[row.id] = well
            return well
        case let .keys(keys):
            let recorder = KeyRecorder()
            recorder.identifier = identifier
            recorder.stringValue = keys
            recorder.isEnabled = row.enabled
            recorder.widthAnchor.constraint(equalToConstant: width).isActive = true
            recorder.setAccessibilityLabel(row.label)
            let id = row.id
            recorder.onPress = { [weak self] key in self?.pressed(key: key, in: id) }
            recorder.onReturn = { [weak self] in _ = self?.click(.confirm) }
            recorder.onEscape = { [weak self] in _ = self?.click(.cancel) }
            controls[row.id] = recorder
            return recorder
        case let .code(code):
            // To be read off and typed somewhere else: large, and in
            // letters that are not taken for one another. Revealed here,
            // where it is drawn, and nowhere else.
            let label = Self.wrapping(
                code.revealed, width: width, color: .labelColor,
                font: .monospacedSystemFont(ofSize: Self.codeSize, weight: .medium))
            label.identifier = identifier
            return label
        }
    }

    /// Text that takes as many lines as it needs at `width`, and can be
    /// selected: an error to quote, a fingerprint to compare.
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

    /// Makes the sheet as tall as its content, keeping it where it is.
    private func resize(_ sheet: NSWindow, to size: NSSize) {
        let old = sheet.frame
        var frame = sheet.frameRect(forContentRect: NSRect(origin: .zero, size: size))
        guard frame.size != old.size else { return }
        frame.origin = NSPoint(x: old.midX - frame.width / 2, y: old.midY - frame.height / 2)
        sheet.setFrame(frame, display: true, animate: false)
    }

    // MARK: - The keyboard

    /// The name of the field the keyboard is in, if it is in one.
    private var focusedField: String? {
        guard let sheet, let responder = sheet.firstResponder as? NSView else { return nil }
        return controls.first { _, control in responder === control || responder.isDescendant(of: control) }?.key
    }

    private func focusField(_ id: String) {
        guard let sheet, let control = controls[id], control.isEnabled else { return }
        if sheet.makeFirstResponder(control), let field = control as? NSTextField {
            // The cursor at the end of what is there, not all of it
            // selected: the next key adds to it.
            field.currentEditor()?.selectedRange = NSRange(location: field.stringValue.utf16.count, length: 0)
        }
    }

    // MARK: - From the user

    func controlTextDidChange(_ notification: Notification) {
        // What is typed in a secret field stays in the field.
        guard let field = notification.object as? NSTextField, !(field is NSSecureTextField),
            let id = field.identifier?.rawValue
        else { return }
        model.type(field.stringValue, in: id)
    }

    /// Return in a field presses the button return presses, and esc the
    /// one esc presses, whichever field the keyboard is in. A field passes
    /// these on by itself only when it is the event being handled that
    /// brought them; asked here, they work however the key arrived.
    func control(_: NSControl, textView _: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)): click(.confirm)
        case #selector(NSResponder.cancelOperation(_:)): click(.cancel)
        default: false
        }
    }

    /// Clicks the button with this part to play, if the form has one that
    /// can be pressed now.
    private func click(_ role: FormRole) -> Bool {
        guard let described = model.form?.buttons.first(where: { $0.role == role }), described.enabled,
            let button = buttons[described.id]
        else { return false }
        button.performClick(nil)
        return true
    }

    @objc private func chose(_ sender: NSPopUpButton) {
        guard let id = sender.identifier?.rawValue, let option = sender.selectedItem?.representedObject as? String
        else { return }
        takeTyping()
        if let used = model.choose(option, in: id) {
            send(used)
        }
    }

    /// A colour was picked in a well: the core is told at once, and draws
    /// with it.
    @objc private func picked(_ sender: NSColorWell) {
        guard let id = sender.identifier?.rawValue, let hex = Self.hex(of: sender.color) else { return }
        pick(hex, in: id)
    }

    private func pick(_ hex: String, in id: String) {
        takeTyping()
        if let used = model.pick(hex, in: id) {
            send(used)
        }
    }

    /// The colour a `#rrggbb` names.
    private static func colour(_ hex: String) -> NSColor? {
        guard StateUpdate.isColor(hex), let value = UInt32(hex.dropFirst(), radix: 16) else { return nil }
        return NSColor(
            srgbRed: CGFloat(value >> 16 & 0xff) / 255, green: CGFloat(value >> 8 & 0xff) / 255,
            blue: CGFloat(value & 0xff) / 255, alpha: 1)
    }

    /// A colour as the bridge says one: `#rrggbb` in lower case.
    private static func hex(of colour: NSColor) -> String? {
        guard let colour = colour.usingColorSpace(.sRGB) else { return nil }
        let channel = { (part: CGFloat) in Int((min(max(part, 0), 1) * 255).rounded()) }
        return String(
            format: "#%02x%02x%02x", channel(colour.redComponent), channel(colour.greenComponent),
            channel(colour.blueComponent))
    }

    /// A key was pressed in a field that records keys. It is the field's
    /// key from now, or, pressed soon after another, the second of a
    /// pair. The core is told at once, and says in the form it sends back
    /// whether the key can be had.
    private func pressed(key: String, in id: String) {
        let now = Date()
        var keys = key
        if let lastKey, lastKey.field == id, now.timeIntervalSince(lastKey.at) < Self.pairWithin,
            let before = controls[id]?.stringValue, !before.isEmpty, !before.contains(" ")
        {
            keys = "\(before) \(key)"
        }
        lastKey = (id, now)
        record(keys, in: id)
    }

    /// Puts `keys` in a field that records keys and tells the core.
    private func record(_ keys: String, in id: String) {
        guard let used = model.record(keys, in: id) else { return }
        controls[id]?.stringValue = keys
        send(used)
    }

    @objc private func pressed(_ sender: NSButton) {
        guard let id = sender.identifier?.rawValue else { return }
        press(id)
    }

    /// Presses the button of this name, as a click on it does. False when
    /// the form has no such button, or it cannot be pressed now.
    @discardableResult
    func press(_ button: String) -> Bool {
        takeTyping()
        var secrets: [String: String] = [:]
        for (id, control) in controls where control is NSSecureTextField {
            secrets[id] = control.stringValue
        }
        guard let used = model.press(button, secrets: secrets) else { return false }
        send(used)
        // Sent, and not kept: what was typed in a secret field is the
        // core's now, and the field is empty.
        for control in controls.values where control is NSSecureTextField {
            control.stringValue = ""
        }
        return true
    }

    /// Makes sure the model has what every text field holds: typing still
    /// being composed has not been reported as a change yet.
    private func takeTyping() {
        for (id, control) in controls {
            if let field = control as? NSTextField, !(field is NSSecureTextField) {
                model.type(field.stringValue, in: id)
            }
        }
    }

    /// The field that records keys which has the keyboard, if one has: for
    /// a scripted run to press a real key in.
    var recordingField: NSView? {
        guard let sheet else { return nil }
        return sheet.firstResponder as? KeyRecorder
    }

    // MARK: - For a scripted run

    /// Puts `value` in the field of this name, as typing it or choosing it
    /// does: for a choice it is the option's name. False when the form has
    /// no such field, it takes no typing now, or the option is not one of
    /// its own.
    func set(_ id: String, to value: String) -> Bool {
        guard let control = controls[id], control.isEnabled else { return false }
        if let popup = control as? NSPopUpButton {
            guard let index = popup.itemArray.firstIndex(where: { $0.representedObject as? String == value }) else {
                return false
            }
            popup.selectItem(at: index)
            chose(popup)
        } else if control is NSSecureTextField {
            control.stringValue = value
        } else if control is KeyRecorder {
            // As pressing them does, all at once.
            record(value, in: id)
        } else if let well = control as? NSColorWell {
            // As picking it in the colour panel does, with no panel.
            guard let colour = Self.colour(value) else { return false }
            well.color = colour
            pick(value, in: id)
        } else {
            control.stringValue = value
            model.type(value, in: id)
        }
        return true
    }

    /// The sheet in lines of text, for the dump: what the model says of it,
    /// and which of its fields the keyboard is in. What a secret field
    /// holds is not in the model, and is not read here.
    var summary: [String] {
        let opened = "sheets opened: \(timesOpened)"
        guard isUp else { return model.summary + [opened] }
        return model.summary + [
            "sheet keys go to: \(focusedField ?? "no field")",
            "sheet is on the window titled: \(parent?.title.debugDescription ?? "none")", opened,
        ]
    }
}
