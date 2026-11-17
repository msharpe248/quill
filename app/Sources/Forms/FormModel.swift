import Bridge

/// One row of the sheet: a field as it is to be drawn now.
public struct FormRow: Equatable, Sendable {
    /// What is in the row.
    public enum Content: Equatable, Sendable {
        /// A text field holding this.
        case text(String)
        /// A field that hides what is typed. What it holds is the field's
        /// own business: nothing here ever has it.
        case secret
        /// A pop-up of these options, with the one of this name chosen.
        case choice(selected: String, options: [FormOption])
        /// Text that is only shown.
        case fixed(String)
        /// Text to read off and type somewhere else, shown large. Only the
        /// view that draws it reveals it.
        case code(FormSecret)
        /// A key, or keys one after another, taken by pressing them: what
        /// has been pressed, as Quill writes it, and empty when nothing
        /// has.
        case keys(String)
        /// A well holding this colour, `#rrggbb`, to pick another in.
        case color(String)
    }

    public let id: String
    public let label: String
    public let content: Content
    /// Said under the field, in small print.
    public let help: String?
    /// False while the core is busy: nothing is typed or chosen then.
    public let enabled: Bool
}

/// What the view is to do about a form the core has just sent.
public struct FormUpdate: Equatable, Sendable {
    /// The sheet was not up, and is to be opened.
    public let opens: Bool
    /// The form is at the step it was at before: what the user has typed
    /// in a secret field is theirs to keep.
    public let sameStep: Bool
    /// The field to give the keyboard to, when it is to move.
    public let focus: String?
}

/// The form in the sheet, as the shell holds it: what the core last said
/// to show, and what the user has typed and chosen since.
///
/// The core says which fields there are, every time; this never adds one,
/// keeps one the core has dropped, or decides what a field means. It only
/// remembers what is on screen, so that a form sent again keeps the user's
/// typing, and gathers it up for the core when a button is pressed.
///
/// What is typed in a secret field is never here. The view hands it over
/// at the moment a button is pressed, it goes into the one message, and
/// the view clears the field.
public struct FormModel: Equatable, Sendable {
    /// The form the core last sent; `nil` while the sheet is down.
    public private(set) var form: FormShow?
    /// What each field that can be typed in or chosen from holds on
    /// screen, by name. Never a secret field's.
    private var entered: [String: String] = [:]
    /// What the core last said each of them holds.
    private var told: [String: String] = [:]
    /// A button was pressed, and the core has yet to answer with a form
    /// that can be typed in.
    private var awaitingAnswer = false

    public init() {}

    public var isUp: Bool { form != nil }

    // MARK: - From the core

    /// Takes a form from the core, in place of the one there was.
    ///
    /// A field keeps what the user typed in it unless the form has moved
    /// to another step, or the core's own value for the field has changed
    /// since it last said: then the core's is shown.
    public mutating func show(_ next: FormShow) -> FormUpdate {
        let before = form
        let sameStep = before?.step == next.step
        var nextEntered: [String: String] = [:]
        var nextTold: [String: String] = [:]
        for field in next.fields {
            let core: String
            let allowed: [String]?
            switch field.kind {
            case let .text(value), let .keys(value), let .color(value):
                (core, allowed) = (value, nil)
            case let .choice(value, options):
                (core, allowed) = (value, options.map(\.id))
            // The last five are the settings window's, and are in no
            // form a sheet is given.
            case .secret, .fixed, .code, .toggle, .number, .list, .button, .font:
                continue
            }
            nextTold[field.id] = core
            if sameStep, told[field.id] == core, let typed = entered[field.id], allowed?.contains(typed) ?? true {
                nextEntered[field.id] = typed
            } else {
                nextEntered[field.id] = core
            }
        }
        // The keyboard goes where the core says when the form is new, at
        // a new step, or answering a button. A choice the user has just
        // changed moves nothing: they are still looking at it.
        var focus: String?
        if next.busy == nil {
            if !sameStep || awaitingAnswer || before?.busy != nil {
                focus = next.fields.first(where: \.focus)?.id ?? next.fields.first(where: \.isEditable)?.id
            }
            awaitingAnswer = false
        }
        form = next
        entered = nextEntered
        told = nextTold
        return FormUpdate(opens: before == nil, sameStep: sameStep, focus: focus)
    }

    /// The form is over: nothing typed in it is kept.
    public mutating func close() {
        self = FormModel()
    }

    // MARK: - What is drawn

    public var isBusy: Bool { form?.busy != nil }

    /// The fields, in the core's order, as they are to be drawn now.
    public var rows: [FormRow] {
        guard let form else { return [] }
        return form.fields.map { field in
            let content: FormRow.Content
            switch field.kind {
            case let .text(value):
                content = .text(entered[field.id] ?? value)
            case .secret:
                content = .secret
            case let .choice(value, options):
                content = .choice(selected: entered[field.id] ?? value, options: options)
            case let .fixed(value):
                content = .fixed(value)
            case let .code(value):
                content = .code(value)
            case let .keys(value):
                content = .keys(entered[field.id] ?? value)
            case let .color(value):
                content = .color(entered[field.id] ?? value)
            // The settings window's kinds: in no form a sheet is given.
            case .toggle, .number, .list, .button, .font:
                content = .fixed("")
            }
            return FormRow(
                id: field.id, label: field.label, content: content, help: field.help,
                enabled: form.busy == nil)
        }
    }

    // MARK: - From the user

    /// The user typed in a text field. Typing while the core is busy, or in
    /// anything that is not a text field of this form, is not taken.
    public mutating func type(_ text: String, in id: String) {
        guard let form, form.busy == nil, let field = form.fields.first(where: { $0.id == id }),
            case .text = field.kind
        else { return }
        entered[id] = text
    }

    /// The user chose an option of a pop-up. Returns what to tell the core,
    /// so that the fields that depend on the choice can change with it:
    /// everything the form holds, and no button. `nil` when nothing
    /// changed, or the option is not one of the field's.
    ///
    /// Secrets do not travel with a changed choice: only a button sends
    /// them.
    public mutating func choose(_ option: String, in id: String) -> FormUsed? {
        guard let form, form.busy == nil, let field = form.fields.first(where: { $0.id == id }),
            case let .choice(_, options) = field.kind, options.contains(where: { $0.id == option }),
            entered[id] != option
        else { return nil }
        entered[id] = option
        return FormUsed(step: form.step, button: nil, values: entered)
    }

    /// The user pressed a key, or keys one after another, in a field that
    /// records them. Returns what to tell the core, as for a choice that
    /// was changed: whether the key can be had is the core's to say, in
    /// the form it sends back. `nil` when nothing changed, or the field is
    /// not one that records keys.
    public mutating func record(_ keys: String, in id: String) -> FormUsed? {
        guard let form, form.busy == nil, let field = form.fields.first(where: { $0.id == id }),
            case .keys = field.kind, entered[id] != keys, keys.utf8.count <= FormField.mostKeysBytes
        else { return nil }
        entered[id] = keys
        return FormUsed(step: form.step, button: nil, values: entered)
    }

    /// The user picked a colour in a well. Returns what to tell the core,
    /// as for a choice that was changed, so that what is drawn in the
    /// colour can change with it. `nil` when nothing changed, the field is
    /// not a well, or what was picked is not a colour as the bridge says
    /// one.
    public mutating func pick(_ color: String, in id: String) -> FormUsed? {
        guard let form, form.busy == nil, let field = form.fields.first(where: { $0.id == id }),
            case .color = field.kind, entered[id] != color, StateUpdate.isColor(color)
        else { return nil }
        entered[id] = color
        return FormUsed(step: form.step, button: nil, values: entered)
    }

    /// Whether `next` is the form on show but for the colours its wells
    /// hold: the view then has nothing to lay out again, and a well a
    /// colour is being picked in is left to its picking.
    public func differsOnlyInColors(from next: FormShow) -> Bool {
        guard let form, form.step == next.step, form.title == next.title, form.note == next.note,
            form.error == next.error, form.busy == next.busy, form.buttons == next.buttons,
            form.fields.count == next.fields.count
        else { return false }
        return zip(form.fields, next.fields).allSatisfy { old, new in
            guard old.id == new.id, old.label == new.label, old.help == new.help, old.focus == new.focus else {
                return false
            }
            if case .color = old.kind, case .color = new.kind { return true }
            return old.kind == new.kind
        }
    }

    /// The user pressed a button. Returns what to tell the core, or `nil`
    /// for a button the form does not have or that cannot be pressed now.
    ///
    /// `secrets` is what the view's secret fields hold at this moment, by
    /// field name; only those of this form's secret fields are sent. The
    /// button that esc presses puts the form away, and sends nothing that
    /// was typed with it, a secret least of all.
    public mutating func press(_ button: String, secrets: [String: String]) -> FormUsed? {
        guard let form, let pressed = form.buttons.first(where: { $0.id == button }), pressed.enabled else {
            return nil
        }
        var used: FormUsed?
        if pressed.role == .cancel {
            used = FormUsed(step: form.step, button: button, values: [:])
        } else {
            var sent: [String: FormSecret] = [:]
            for field in form.fields where field.kind == .secret {
                if let text = secrets[field.id] {
                    sent[field.id] = FormSecret(text)
                }
            }
            used = FormUsed(step: form.step, button: button, values: entered, secrets: sent)
        }
        if used != nil {
            awaitingAnswer = true
        }
        return used
    }

    // MARK: - For a scripted run's dump

    /// The sheet in lines of text: whether it is up, its step, what it
    /// says, and each field with what it holds on screen. A secret field
    /// is a fixed marker whatever it holds, or nothing, and so is a code.
    public var summary: [String] {
        guard let form else { return ["sheet: none"] }
        var lines = ["sheet: up", "sheet step: \(form.step)", "sheet title: \(form.title.debugDescription)"]
        lines.append("sheet note: \(form.note?.debugDescription ?? "")")
        lines.append("sheet error: \(form.error?.debugDescription ?? "")")
        lines.append("sheet busy: \(form.busy?.debugDescription ?? "")")
        for (row, field) in zip(rows, form.fields) {
            let shown: String
            switch row.content {
            case let .text(text):
                shown = "[text] = \(text.debugDescription)"
            case .secret:
                shown = "[secret] = \(Self.secretMarker)"
            case let .choice(selected, options):
                shown = "[choice] = \(selected) (\(options.map(\.id).joined(separator: "|")))"
            case let .fixed(text):
                shown = "[static] = \(text.debugDescription)"
            case let .code(code):
                // Its placeholder, whatever it shows.
                shown = "[code] = \(code)"
            case let .keys(keys):
                shown = "[keys] = \(keys.debugDescription)"
            case let .color(color):
                shown = "[color] = \(color)"
            }
            lines.append("sheet field \(row.id) \(shown)\(field.focus ? " (focus)" : "")")
        }
        let buttons = form.buttons.map { button in
            var notes: [String] = []
            switch button.role {
            case .confirm: notes.append("return")
            case .cancel: notes.append("esc")
            case nil: break
            }
            if !button.enabled {
                notes.append("disabled")
            }
            let noted = notes.isEmpty ? "" : " (\(notes.joined(separator: ", ")))"
            return "\(button.id) \(button.title.debugDescription)\(noted)"
        }
        lines.append("sheet buttons: \(buttons.joined(separator: " | "))")
        return lines
    }

    /// What a dump says in place of whatever a secret field holds.
    public static let secretMarker = "<secret>"
}
