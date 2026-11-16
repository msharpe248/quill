import AppKit
import Bridge
import Forms

/// A form the core has the shell show, as a sheet on the main window.
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
    /// How many times the sheet has been opened, for a scripted run's dump.
    private(set) var timesOpened = 0

    /// How wide the sheet's content is, and the room around it, in points.
    private static let width: CGFloat = 500
    private static let margin: CGFloat = 20

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
        let update = model.show(form)
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
    }

    /// Takes the sheet down. Nothing typed in it is kept.
    func close() {
        model.close()
        // A field's own copy of what was typed goes with the field.
        for control in controls.values {
            control.stringValue = ""
        }
        controls = [:]
        buttons = [:]
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

        let title = NSTextField(labelWithString: form.title)
        title.font = .boldSystemFont(ofSize: NSFont.systemFontSize)
        title.lineBreakMode = .byTruncatingTail
        column.addArrangedSubview(title)
        if let note = form.note, !note.isEmpty {
            column.addArrangedSubview(Self.wrapping(note, width: inner, color: .secondaryLabelColor))
        }

        let rows = model.rows
        if !rows.isEmpty {
            let labels = rows.map { row -> NSTextField in
                let label = NSTextField(labelWithString: row.label)
                label.alignment = .right
                return label
            }
            // The labels' column is as wide as the longest of them, and
            // the fields have the rest.
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
            column.addArrangedSubview(grid)
        }

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
        return model.summary + ["sheet keys go to: \(focusedField ?? "no field")", opened]
    }
}
