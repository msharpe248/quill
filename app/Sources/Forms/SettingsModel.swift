import Bridge
import Foundation

/// What the view is to do about settings the core has just sent.
public struct SettingsUpdate: Equatable, Sendable {
    /// The panes themselves are others than before: their names, titles or
    /// symbols, or their order. The toolbar is made again.
    public let panesChanged: Bool
    /// The panes whose content is not what the window is showing, by name:
    /// each is drawn again. A pane the user has just changed, where the core
    /// then says the same, is not among them.
    public let redraw: Set<String>
    /// The pane on show, by name: the one that was, if the core still has
    /// it; otherwise the one the user last had; otherwise the first.
    public let pane: String?
}

/// Which pane of the settings window the user last had, by name. It is how
/// the window was left, not a setting, so the shell keeps it.
public protocol SettingsPaneMemory: AnyObject {
    var pane: String? { get set }
}

/// Kept in the app's defaults, from one run to the next.
public final class DefaultsSettingsPaneMemory: SettingsPaneMemory {
    public static let key = "SettingsPane"

    private let defaults: UserDefaults

    public init(defaults: UserDefaults) {
        self.defaults = defaults
    }

    public var pane: String? {
        get {
            // Anything there that could not be a pane's name is nothing.
            guard let stored = defaults.string(forKey: Self.key), FormID.isValid(stored) else { return nil }
            return stored
        }
        set {
            if let newValue, FormID.isValid(newValue) {
                defaults.set(newValue, forKey: Self.key)
            } else {
                defaults.removeObject(forKey: Self.key)
            }
        }
    }
}

/// Kept for the run and no longer: for a scripted run, which must leave the
/// user's own defaults as they were, and read nothing from them.
public final class TransientSettingsPaneMemory: SettingsPaneMemory {
    public var pane: String?

    public init(pane: String? = nil) {
        self.pane = pane
    }
}

/// The settings window, as the shell holds it: what the core last said to
/// show, with what the user has done to it since, which pane is on show, and
/// which row of each list is picked.
///
/// The core says which panes and which settings there are, every time; this
/// never adds one, keeps one the core has dropped, or decides what a setting
/// means. A change the user makes is put into this copy at once, as the
/// control already shows it, and sent to the core; the core answers with the
/// settings as they then stand, which is what is shown from then on. So a
/// change the core did not make is put back, and one it did make moves
/// nothing.
public struct SettingsModel: Equatable, Sendable {
    /// What the window shows: what the core last sent, with the user's own
    /// changes since. `nil` until the core has sent any.
    public private(set) var settings: SettingsShow?
    /// The pane on show, by name.
    public private(set) var pane: String?
    /// The row picked in each list, by the list's pane and name.
    private var picked: [String: String] = [:]

    public init() {}

    // MARK: - From the core

    /// Takes the settings from the core, in place of what there was.
    /// `remembered` is the pane the user last had, for when no pane is on
    /// show yet.
    public mutating func show(_ next: SettingsShow, remembered: String?) -> SettingsUpdate {
        let before = settings
        let outline: (SettingsShow?) -> [[String]] = { shown in
            (shown?.panes ?? []).map { [$0.id, $0.title, $0.symbol] }
        }
        var redraw: Set<String> = []
        for pane in next.panes where before?.panes.first(where: { $0.id == pane.id }) != pane {
            redraw.insert(pane.id)
        }
        // What is wrong is said on every pane.
        if before?.problem != next.problem {
            redraw.formUnion(next.panes.map(\.id))
        }
        settings = next
        let ids = next.panes.map(\.id)
        pane = [pane, remembered].compactMap { $0 }.first(where: ids.contains) ?? ids.first
        // A row that is no longer there is no longer picked.
        picked = picked.filter { key, row in
            next.panes.contains { pane in
                pane.fields.contains { field in
                    guard Self.key(pane.id, field.id) == key, case let .list(list) = field.kind else {
                        return false
                    }
                    return list.rows.contains { $0.id == row }
                }
            }
        }
        return SettingsUpdate(panesChanged: outline(before) != outline(next), redraw: redraw, pane: pane)
    }

    /// The window was closed. What the core said is kept, to show at once
    /// when it is opened again, while the core reads the settings afresh.
    public mutating func close() {
        picked = [:]
    }

    // MARK: - What is drawn

    /// The panes, in the core's order.
    public var panes: [SettingsPane] { settings?.panes ?? [] }

    /// What is wrong, as the core says it.
    public var problem: String? { settings?.problem }

    /// Whether there is nothing to show but what is wrong: the settings
    /// cannot be changed from here.
    public var isUnreadable: Bool { settings.map { $0.panes.isEmpty } ?? false }

    /// The pane of this name.
    public func pane(named id: String) -> SettingsPane? {
        panes.first { $0.id == id }
    }

    /// The row picked in a list, if one is.
    public func pickedRow(in field: String, of pane: String) -> String? {
        picked[Self.key(pane, field)]
    }

    /// Whether an action of a list can be pressed now: it can be pressed at
    /// all, and if it is done to a row, a row that takes it is picked.
    public func canPress(_ action: String, in field: String, of pane: String) -> Bool {
        guard case let .list(list)? = settings?.field(field, in: pane)?.kind else { return false }
        return list.canPress(action, picked: pickedRow(in: field, of: pane))
    }

    // MARK: - From the user

    /// The user chose a pane. False when there is no such pane.
    public mutating func select(pane id: String) -> Bool {
        guard pane(named: id) != nil else { return false }
        pane = id
        return true
    }

    /// The user picked a row of a list, or none. A row the list has not got
    /// is none.
    public mutating func pick(row: String?, in field: String, of pane: String) {
        guard case let .list(list)? = settings?.field(field, in: pane)?.kind else { return }
        if let row, list.rows.contains(where: { $0.id == row }) {
            picked[Self.key(pane, field)] = row
        } else {
            picked[Self.key(pane, field)] = nil
        }
    }

    /// The user gave a field another value. Returns what to tell the core,
    /// or `nil` when the field takes no such value, or already has it: a
    /// choice that is none of the field's options, text for a toggle. A
    /// number is brought within the field's bounds first, and to the places
    /// it shows.
    ///
    /// What the window shows is changed at once, as the control has been.
    public mutating func set(_ value: SettingValue, in field: String, of pane: String) -> SettingsUsed? {
        guard let described = settings?.field(field, in: pane) else { return nil }
        let kind: FormField.Kind
        let sent: SettingValue
        switch (described.kind, value) {
        case let (.text(old), .text(new)):
            guard old != new else { return nil }
            (kind, sent) = (.text(value: new), value)
        case let (.choice(old, options), .text(new)):
            guard old != new, options.contains(where: { $0.id == new }) else { return nil }
            (kind, sent) = (.choice(value: new, options: options), value)
        case let (.toggle(title, old), .flag(new)):
            guard old != new else { return nil }
            (kind, sent) = (.toggle(title: title, value: new), value)
        case let (.number(number), .number(typed)):
            guard let taken = number.taking(typed), taken != number.value else { return nil }
            (kind, sent) = (.number(number.with(value: taken)), .number(taken))
        default:
            return nil
        }
        guard let used = SettingsUsed(pane: pane, field: field, did: .set(sent)) else { return nil }
        replace(field, in: pane, with: kind)
        return used
    }

    /// The user pressed an action of a list. Returns what to tell the core,
    /// or `nil` when it cannot be pressed now. An action done to a row is
    /// sent with the row picked, and any other with none.
    public func press(_ action: String, in field: String, of pane: String) -> SettingsUsed? {
        guard case let .list(list)? = settings?.field(field, in: pane)?.kind,
            canPress(action, in: field, of: pane),
            let described = list.actions.first(where: { $0.id == action })
        else { return nil }
        let row = described.row ? pickedRow(in: field, of: pane) : nil
        return SettingsUsed(pane: pane, field: field, did: .action(action, row: row))
    }

    /// The user pressed a button. Returns what to tell the core, or `nil`
    /// when the field is no button or cannot be pressed now.
    public func pressButton(_ field: String, of pane: String) -> SettingsUsed? {
        guard case let .button(_, enabled)? = settings?.field(field, in: pane)?.kind, enabled else { return nil }
        return SettingsUsed(pane: pane, field: field, did: .action(SettingsUsed.buttonPress, row: nil))
    }

    private static func key(_ pane: String, _ field: String) -> String {
        "\(pane).\(field)"
    }

    /// Puts another value into the copy of the settings that is on show.
    private mutating func replace(_ field: String, in pane: String, with kind: FormField.Kind) {
        guard let shown = settings else { return }
        let panes = shown.panes.map { described -> SettingsPane in
            guard described.id == pane else { return described }
            let fields = described.fields.map { old in
                old.id == field ? FormField(id: old.id, label: old.label, kind: kind, help: old.help) : old
            }
            return SettingsPane(
                id: described.id, title: described.title, symbol: described.symbol, note: described.note,
                fields: fields, more: described.more)
        }
        // The same panes with one value another: still settings.
        if let changed = SettingsShow(panes: panes, problem: shown.problem) {
            settings = changed
        }
    }

    // MARK: - For a scripted run's dump

    /// The settings in lines of text: what is wrong, the pane on show, and
    /// each pane with each of its fields and what it holds.
    public var summary: [String] {
        guard let settings else { return ["settings: nothing from the core"] }
        var lines = ["settings problem: \(settings.problem?.debugDescription ?? "")"]
        lines.append("settings pane: \(pane ?? "none") of \(settings.panes.map(\.id).joined(separator: " "))")
        for described in settings.panes {
            lines.append(
                "settings \(described.id) \(described.title.debugDescription) [\(described.symbol)]"
                    + " note: \(described.note?.debugDescription ?? "")")
            for field in described.fields {
                let name = "\(described.id).\(field.id)"
                let shown: String
                switch field.kind {
                case let .text(text):
                    shown = "[text] = \(text.debugDescription)"
                case let .choice(selected, options):
                    shown = "[choice] = \(selected) (\(options.map(\.id).joined(separator: "|")))"
                case let .fixed(text):
                    shown = "[static] = \(text.debugDescription)"
                case let .toggle(title, value):
                    shown = "[toggle] = \(value) \(title.debugDescription)"
                case let .number(number):
                    let most = number.max.map(number.written) ?? "any"
                    shown =
                        "[number] = \(number.written(number.value)) (\(number.written(number.min)) to \(most)"
                        + ", \(number.unit ?? "no unit"))"
                case let .list(list):
                    let rows = list.rows.map { row in
                        let mark = pickedRow(in: field.id, of: described.id) == row.id ? "*" : ""
                        return "\(mark)\(row.title.debugDescription) \((row.detail ?? "").debugDescription)"
                    }
                    let actions = list.actions.map { action in
                        "\(canPress(action.id, in: field.id, of: described.id) ? "+" : "-")\(action.id)"
                    }
                    shown = "[list] rows: \(rows.joined(separator: "; ")) actions: \(actions.joined(separator: " "))"
                case let .button(title, enabled):
                    shown = "[button] \(title.debugDescription)\(enabled ? "" : " (off)")"
                // Settings have neither.
                case .secret, .code:
                    shown = "[\(field.kind.name)]"
                }
                lines.append("settings field \(name) \(shown)")
                if let help = field.help {
                    lines.append("settings help \(name) \(help.debugDescription)")
                }
            }
            if let more = described.more {
                lines.append("settings more \(described.id) \(more.debugDescription)")
            }
        }
        return lines
    }
}
