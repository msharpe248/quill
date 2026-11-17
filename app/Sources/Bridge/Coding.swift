import Foundation

/// Any JSON value. Used to check a message's shape before decoding it into
/// its type, and to compare messages in tests without caring about key order.
public enum JSONValue: Equatable, Sendable, Codable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            self = try .object(container.decode([String: JSONValue].self))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case let .bool(value): try container.encode(value)
        case let .number(value): try container.encode(value)
        case let .string(value): try container.encode(value)
        case let .array(value): try container.encode(value)
        case let .object(value): try container.encode(value)
        }
    }

    var object: [String: JSONValue]? {
        if case let .object(value) = self { value } else { nil }
    }
}

private let jsonRPCVersion = "2.0"
private let envelopeKeys: Set<String> = ["jsonrpc", "method", "params"]

/// The envelope of one message, checked: the right version, a method, an
/// object of params, and nothing else.
private struct Envelope {
    var method: String
    var params: JSONValue
    var paramKeys: Set<String>

    init(line: String) throws {
        let value: JSONValue
        do {
            value = try JSONDecoder().decode(JSONValue.self, from: Data(line.utf8))
        } catch {
            throw BridgeError.malformed("invalid JSON")
        }
        guard let object = value.object else {
            throw BridgeError.malformed("not an object")
        }
        guard case let .string(version)? = object["jsonrpc"] else {
            throw BridgeError.malformed("no jsonrpc version")
        }
        guard version == jsonRPCVersion else {
            throw BridgeError.version(version)
        }
        guard case let .string(method)? = object["method"] else {
            throw BridgeError.malformed("no method")
        }
        guard let params = object["params"], let paramObject = params.object else {
            throw BridgeError.malformed("\(method): params must be an object")
        }
        let extra = Set(object.keys).subtracting(envelopeKeys)
        guard extra.isEmpty else {
            throw BridgeError.malformed("unexpected \(extra.sorted().joined(separator: ", "))")
        }
        self.method = method
        self.params = params
        paramKeys = Set(paramObject.keys)
    }

    /// Decodes the params as `T`, refusing fields `T` does not have. Both
    /// ends ship together, so a mismatch is a bug to surface, not to ignore.
    func decode<T: Decodable>(_: T.Type, allowing allowed: Set<String>) throws -> T {
        try requireOnly(allowed)
        do {
            return try JSONDecoder().decode(T.self, from: JSONEncoder().encode(params))
        } catch {
            throw BridgeError.malformed("\(method): \(Self.describe(error))")
        }
    }

    /// An optional field is left out, never null.
    func requireNotNull(_ key: String) throws {
        if params.object?[key] == .null {
            throw BridgeError.malformed("\(method): \(key) is null")
        }
    }

    func requireOnly(_ allowed: Set<String>) throws {
        let extra = paramKeys.subtracting(allowed)
        guard extra.isEmpty else {
            throw BridgeError.malformed("\(method): unexpected \(extra.sorted().joined(separator: ", "))")
        }
    }

    private static func describe(_ error: any Error) -> String {
        switch error as? DecodingError {
        case let .keyNotFound(key, _): "missing \(key.stringValue)"
        case let .typeMismatch(_, context), let .valueNotFound(_, context), let .dataCorrupted(context):
            ([context.codingPath.map(\.stringValue).joined(separator: ".")] + [context.debugDescription])
                .filter { !$0.isEmpty }.joined(separator: ": ")
        default: String(describing: error)
        }
    }
}

private struct CommandsListParams: Codable {
    var commands: [CommandInfo]
}

private struct CommandRunParams: Codable {
    var name: String
}

/// `files.dropped` as it is on the wire, before its paths are checked.
private struct FilesDroppedParams: Codable {
    var paths: [String]
    var temporary: Bool
}

// The messages below as they are on the wire, before what they hold is
// checked: the types the rest of the app uses cannot hold a relative path,
// an index past the end or an empty name.

private struct PaneFileParams: Codable {
    var path: String
    var title: String
}

private struct PanePreviewParams: Codable {
    var items: [PaneFileParams]
    var index: Int
}

private struct ReadingShowParams: Codable {
    var path: String
    var title: String
    var remote: Bool
    var header: [String]
    var notice: ReadingNoticeParams?
}

private struct ReadingNoticeParams: Codable {
    var text: String
    var actions: [ReadingActionParams]
}

private struct ReadingActionParams: Codable {
    var command: String
    var title: String
}

/// The notice of a `reading.show`, checked as strictly as the message
/// itself: nothing in it that is not known, every button a command and a
/// title. `raw` is the notice as it came, to look for what is not known.
private func readingNotice(_ params: ReadingNoticeParams?, as raw: JSONValue?) throws -> ReadingNotice? {
    guard let params else { return nil }
    var keys = [(Set((raw?.object ?? [:]).keys), ReadingNotice.allowedKeys)]
    if case let .array(actions)? = raw?.object?["actions"] {
        keys += actions.map { (Set(($0.object ?? [:]).keys), ReadingAction.allowedKeys) }
    }
    for (found, allowed) in keys {
        let extra = found.subtracting(allowed)
        guard extra.isEmpty else {
            throw BridgeError.malformed("reading.show: unexpected \(extra.sorted().joined(separator: ", "))")
        }
    }
    let actions = try params.actions.map { action in
        guard let action = ReadingAction(command: action.command, title: action.title) else {
            throw BridgeError.malformed(
                "reading.show: a notice's action needs a command's name and a title of 1 to 64 bytes")
        }
        return action
    }
    guard let notice = ReadingNotice(text: params.text, actions: actions) else {
        throw BridgeError.malformed(
            "reading.show: a notice is one line of 1 to 2000 bytes with at most 3 actions")
    }
    return notice
}

private struct ReadingScrollParams: Codable {
    var by: String
}

private struct ColumnsParams: Codable {
    var columns: Int
}

private struct EditorPlaceParams: Codable {
    struct Cells: Codable {
        var column: Int
        var row: Int
        var columns: Int
        var rows: Int
        var covered: Int
    }

    var place: Cells?
    var keys: Bool

    init(_ place: EditorPlace) {
        self.place = place.cells.map {
            Cells(column: $0.column, row: $0.row, columns: $0.columns, rows: $0.rows, covered: $0.covered)
        }
        keys = place.keys
    }
}

/// The editor's place as the bridge says it: cells that a screen can have
/// and nothing beside them, or none, and never the keys with none.
private func checkedEditorPlace(_ params: EditorPlaceParams, as raw: JSONValue?) throws -> EditorPlace {
    guard let said = params.place else {
        guard !params.keys else {
            throw BridgeError.malformed("editor.place: an editor that is not on show cannot have the keys")
        }
        return .hidden
    }
    let extra = Set((raw?.object ?? [:]).keys).subtracting(EditorPlace.Cells.allowedKeys)
    guard extra.isEmpty else {
        throw BridgeError.malformed("editor.place: unexpected \(extra.sorted().joined(separator: ", "))")
    }
    guard
        let cells = EditorPlace.Cells(
            column: said.column, row: said.row, columns: said.columns, rows: said.rows, covered: said.covered)
    else {
        throw BridgeError.malformed("editor.place: a place is cells of the screen, at least one column and one row")
    }
    return EditorPlace(cells: cells, keys: params.keys)
}

private struct ThumbnailMakeParams: Codable {
    var id: Int
    var path: String
    var out: String
    var size: Int
}

private struct FileOpenParams: Codable {
    var path: String
}

private struct FilesSaveParams: Codable {
    var path: String
    var name: String
}

private struct ThumbnailMadeParams: Codable {
    var id: Int
    var ok: Bool
}

private struct FilesSavedParams: Codable {
    var name: String
    var saved: Bool
}

private struct ClipboardWriteParams: Codable {
    var text: String
}

private struct NotifyActionParams: Codable {
    var name: String
    var title: String
    /// Left out when false, as the core leaves it out.
    var foreground: Bool?
}

private struct NotifyPostParams: Codable {
    var id: String
    var title: String
    var subtitle: String?
    var body: String
    var actions: [NotifyActionParams]
}

private struct NotifyClearParams: Codable {
    var ids: [String]
}

private struct NotifyUsedParams: Codable {
    var id: String
    var action: String?
}

private struct UrlOpenParams: Codable {
    var url: String
}

private struct SpellCheckParams: Codable {
    var id: Int
    var text: String
}

private struct MisspeltParams: Codable {
    var start: Int
    var end: Int
}

private struct SpellCheckedParams: Codable {
    var id: Int
    var words: [MisspeltParams]
}

private struct SpellSuggestParams: Codable {
    var id: Int
    var word: String
}

private struct SpellSuggestedParams: Codable {
    var id: Int
    var words: [String]
}

private struct PaneEditorParams: Codable {
    var dir: String
    var title: String
    var editor: String
    var argv: [String]
}

private struct EditorExitedParams: Codable {
    var ran: Bool
    var status: Int
}

private struct AppearanceChangedParams: Codable {
    var dark: Bool
}

/// `themes.imported` as it is on the wire, before its name is checked.
private struct ThemesImportedParams: Codable {
    var name: String
}

private struct NoParams: Codable {}

private struct Outgoing<Params: Encodable>: Encodable {
    var jsonrpc = jsonRPCVersion
    var method: String
    var params: Params
}

/// One message as one line of JSON, without the newline.
private func encodeLine(method: String, params: some Encodable) -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
    // These types hold only strings, numbers that are numbers and arrays
    // of them, which always encode.
    let data = (try? encoder.encode(Outgoing(method: method, params: params))) ?? Data()
    return String(decoding: data, as: UTF8.self)
}

// The form messages are read from the JSON itself, and not through
// `Decodable`: a field's shape depends on its kind, and every object in
// them is checked for fields it should not have.

private extension JSONValue {
    var string: String? {
        if case let .string(value) = self { value } else { nil }
    }

    var bool: Bool? {
        if case let .bool(value) = self { value } else { nil }
    }

    var array: [JSONValue]? {
        if case let .array(value) = self { value } else { nil }
    }

    var number: Double? {
        if case let .number(value) = self { value } else { nil }
    }
}

/// One object of a form message, read a field at a time. What is wrong is
/// said by the field's name and never by what it held: a `form.used` holds
/// a password.
private struct FormObject {
    let method: String
    let fields: [String: JSONValue]

    init(_ value: JSONValue?, of method: String, allowing allowed: Set<String>) throws {
        guard let fields = value?.object else {
            throw BridgeError.malformed("\(method): not an object")
        }
        let extra = Set(fields.keys).subtracting(allowed)
        guard extra.isEmpty else {
            throw BridgeError.malformed("\(method): unexpected \(extra.sorted().joined(separator: ", "))")
        }
        self.method = method
        self.fields = fields
    }

    func string(_ key: String) throws -> String {
        guard let value = fields[key] else { throw BridgeError.malformed("\(method): missing \(key)") }
        guard let text = value.string else { throw BridgeError.malformed("\(method): \(key) is not a string") }
        return text
    }

    /// An optional field is left out, never null.
    func optionalString(_ key: String) throws -> String? {
        guard fields[key] != nil else { return nil }
        return try string(key)
    }

    func bool(_ key: String) throws -> Bool {
        guard let value = fields[key] else { throw BridgeError.malformed("\(method): missing \(key)") }
        guard let flag = value.bool else { throw BridgeError.malformed("\(method): \(key) is not true or false") }
        return flag
    }

    func optionalBool(_ key: String) throws -> Bool? {
        guard fields[key] != nil else { return nil }
        return try bool(key)
    }

    func array(_ key: String) throws -> [JSONValue] {
        guard let value = fields[key] else { throw BridgeError.malformed("\(method): missing \(key)") }
        guard let items = value.array else { throw BridgeError.malformed("\(method): \(key) is not a list") }
        return items
    }

    func number(_ key: String) throws -> Double {
        guard let value = fields[key] else { throw BridgeError.malformed("\(method): missing \(key)") }
        guard let number = value.number, number.isFinite else {
            throw BridgeError.malformed("\(method): \(key) is not a number")
        }
        return number
    }

    func optionalNumber(_ key: String) throws -> Double? {
        guard fields[key] != nil else { return nil }
        return try number(key)
    }

    /// A whole number.
    func whole(_ key: String) throws -> Int {
        let number = try number(key)
        guard number == number.rounded(), abs(number) < 1e9 else {
            throw BridgeError.malformed("\(method): \(key) is not a whole number")
        }
        return Int(number)
    }

    /// A list whose every item is a string.
    func stringList(_ key: String) throws -> [String] {
        try array(key).map { item in
            guard let text = item.string else {
                throw BridgeError.malformed("\(method): \(key) holds something that is not a string")
            }
            return text
        }
    }

    /// An object whose every value is a string.
    func strings(_ key: String) throws -> [String: String] {
        guard let value = fields[key] else { throw BridgeError.malformed("\(method): missing \(key)") }
        guard let object = value.object else { throw BridgeError.malformed("\(method): \(key) is not an object") }
        return try object.mapValues { value in
            guard let text = value.string else {
                throw BridgeError.malformed("\(method): \(key) holds something that is not a string")
            }
            return text
        }
    }
}

private extension FormField {
    /// The fields each kind may have.
    static func allowedKeys(for kind: String) -> Set<String>? {
        switch kind {
        case "text", "keys": ["kind", "id", "label", "value", "focus", "help"]
        case "secret": ["kind", "id", "label", "focus", "help"]
        case "choice": ["kind", "id", "label", "value", "options", "focus", "help"]
        case "static", "code": ["kind", "id", "label", "value", "help"]
        case "toggle": ["kind", "id", "label", "title", "value", "help"]
        case "number": FormNumber.allowedKeys
        case "list": FormList.allowedKeys
        case "button": ["kind", "id", "label", "title", "enabled", "help"]
        default: nil
        }
    }

    /// The kinds of field a sheet's form has, and the kinds the settings
    /// window has: a sheet has none that report as they change, and the
    /// settings have no secret, no code and no keys to record.
    static let sheetKinds = ["text", "secret", "choice", "static", "code", "keys"]
    static let settingsKinds = ["text", "choice", "static", "toggle", "number", "list", "button"]

    /// A field of the message called `method`, which has fields of these
    /// `kinds`: a form's, or a settings pane's.
    init(json: JSONValue, of method: String, kinds: [String]) throws {
        guard let name = json.object?["kind"]?.string, kinds.contains(name),
            let allowed = Self.allowedKeys(for: name)
        else {
            let last = kinds.last ?? ""
            throw BridgeError.malformed(
                "\(method): a field is \(kinds.dropLast().joined(separator: ", ")) or \(last)")
        }
        let object = try FormObject(json, of: method, allowing: allowed)
        let kind: Kind
        switch name {
        case "text":
            kind = try .text(value: object.string("value"))
        case "secret":
            kind = .secret
        case "choice":
            let options = try object.array("options").map { option in
                let option = try FormObject(option, of: method, allowing: FormOption.allowedKeys)
                return try FormOption(id: option.string("id"), title: option.string("title"))
            }
            kind = try .choice(value: object.string("value"), options: options)
        case "code":
            kind = try .code(value: FormSecret(object.string("value")))
        case "keys":
            kind = try .keys(value: object.string("value"))
        case "toggle":
            kind = try .toggle(title: object.string("title"), value: object.bool("value"))
        case "number":
            kind = try .number(
                FormNumber(
                    value: object.number("value"), min: object.number("min"), max: object.optionalNumber("max"),
                    step: object.number("step"), decimals: object.whole("decimals"),
                    unit: object.optionalString("unit")))
        case "list":
            let rows = try object.array("rows").map { row in
                let row = try FormObject(row, of: method, allowing: FormList.Row.allowedKeys)
                return try FormList.Row(
                    id: row.string("id"), title: row.string("title"), detail: row.optionalString("detail"),
                    actions: row.stringList("actions"))
            }
            let actions = try object.array("actions").map { action in
                let action = try FormObject(action, of: method, allowing: FormList.Action.allowedKeys)
                return try FormList.Action(
                    id: action.string("id"), title: action.string("title"), row: action.bool("row"),
                    enabled: action.bool("enabled"))
            }
            kind = try .list(
                FormList(
                    rows: rows, actions: actions, searchable: object.optionalBool("searchable") ?? false,
                    empty: object.optionalString("empty")))
        case "button":
            kind = try .button(title: object.string("title"), enabled: object.bool("enabled"))
        default:
            kind = try .fixed(value: object.string("value"))
        }
        try self.init(
            id: object.string("id"), label: object.string("label"), kind: kind,
            focus: object.optionalBool("focus") ?? false, help: object.optionalString("help"))
    }

    var json: JSONValue {
        var object: [String: JSONValue] = ["kind": .string(kind.name), "id": .string(id), "label": .string(label)]
        switch kind {
        case let .text(value), let .fixed(value), let .keys(value):
            object["value"] = .string(value)
        case .secret:
            break
        // Written out here, on its way to be shown, and nowhere else.
        case let .code(value):
            object["value"] = .string(value.revealed)
        case let .choice(value, options):
            object["value"] = .string(value)
            object["options"] = .array(options.map { .object(["id": .string($0.id), "title": .string($0.title)]) })
        case let .toggle(title, value):
            object["title"] = .string(title)
            object["value"] = .bool(value)
        case let .number(number):
            object["value"] = .number(number.value)
            object["min"] = .number(number.min)
            object["step"] = .number(number.step)
            object["decimals"] = .number(Double(number.decimals))
            if let max = number.max {
                object["max"] = .number(max)
            }
            if let unit = number.unit {
                object["unit"] = .string(unit)
            }
        case let .list(list):
            object["rows"] = .array(
                list.rows.map { row in
                    var object: [String: JSONValue] = [
                        "id": .string(row.id), "title": .string(row.title),
                        "actions": .array(row.actions.map { .string($0) }),
                    ]
                    if let detail = row.detail {
                        object["detail"] = .string(detail)
                    }
                    return .object(object)
                })
            object["actions"] = .array(
                list.actions.map { action in
                    .object([
                        "id": .string(action.id), "title": .string(action.title), "row": .bool(action.row),
                        "enabled": .bool(action.enabled),
                    ])
                })
            // Left out when false, as the core leaves it out.
            if list.searchable {
                object["searchable"] = .bool(true)
            }
            if let empty = list.empty {
                object["empty"] = .string(empty)
            }
        case let .button(title, enabled):
            object["title"] = .string(title)
            object["enabled"] = .bool(enabled)
        }
        // Left out when false, as the core leaves it out.
        if focus {
            object["focus"] = .bool(true)
        }
        if let help {
            object["help"] = .string(help)
        }
        return .object(object)
    }
}

private extension FormShow {
    init(params: JSONValue) throws {
        let method = "form.show"
        let object = try FormObject(params, of: method, allowing: Self.allowedKeys)
        let fields = try object.array("fields").map {
            try FormField(json: $0, of: method, kinds: FormField.sheetKinds)
        }
        let buttons = try object.array("buttons").map { button in
            let button = try FormObject(button, of: method, allowing: FormButton.allowedKeys)
            var role: FormRole?
            if let name = try button.optionalString("role") {
                guard let known = FormRole(rawValue: name) else {
                    throw BridgeError.malformed("\(method): a button's role is default or cancel")
                }
                role = known
            }
            return try FormButton(
                id: button.string("id"), title: button.string("title"), role: role, enabled: button.bool("enabled"))
        }
        guard
            let form = try FormShow(
                step: object.string("step"), title: object.string("title"), note: object.optionalString("note"),
                error: object.optionalString("error"), busy: object.optionalString("busy"), fields: fields,
                buttons: buttons)
        else {
            throw BridgeError.malformed(
                "\(method): the step, fields and buttons need names of their own, a choice one of its options, "
                    + "and at most one field the focus")
        }
        self = form
    }

    var json: JSONValue {
        var object: [String: JSONValue] = [
            "step": .string(step), "title": .string(title), "fields": .array(fields.map(\.json)),
            "buttons": .array(
                buttons.map { button in
                    var object: [String: JSONValue] = [
                        "id": .string(button.id), "title": .string(button.title), "enabled": .bool(button.enabled),
                    ]
                    if let role = button.role {
                        object["role"] = .string(role.rawValue)
                    }
                    return .object(object)
                }),
        ]
        if let note {
            object["note"] = .string(note)
        }
        if let error {
            object["error"] = .string(error)
        }
        if let busy {
            object["busy"] = .string(busy)
        }
        return .object(object)
    }
}

private extension FormUsed {
    init(params: JSONValue) throws {
        let method = "form.used"
        let object = try FormObject(params, of: method, allowing: Self.allowedKeys)
        guard
            let used = try FormUsed(
                step: object.string("step"), button: object.optionalString("button"),
                values: object.strings("values"), secrets: object.strings("secrets").mapValues(FormSecret.init))
        else {
            throw BridgeError.malformed("\(method): the step, the button and the fields need names")
        }
        self = used
    }

    /// The one place a secret is written out: on its way to the core.
    var json: JSONValue {
        var object: [String: JSONValue] = [
            "step": .string(step),
            "values": .object(values.mapValues { .string($0) }),
            "secrets": .object(secrets.mapValues { .string($0.revealed) }),
        ]
        if let button {
            object["button"] = .string(button)
        }
        return .object(object)
    }
}

private extension SettingsShow {
    init(params: JSONValue) throws {
        let method = "settings.show"
        let object = try FormObject(params, of: method, allowing: Self.allowedKeys)
        let panes = try object.array("panes").map { pane in
            let pane = try FormObject(pane, of: method, allowing: SettingsPane.allowedKeys)
            return try SettingsPane(
                id: pane.string("id"), title: pane.string("title"), symbol: pane.string("symbol"),
                note: pane.optionalString("note"),
                fields: pane.array("fields").map {
                    try FormField(json: $0, of: method, kinds: FormField.settingsKinds)
                },
                more: pane.optionalString("more"))
        }
        guard let settings = try SettingsShow(panes: panes, problem: object.optionalString("problem")) else {
            throw BridgeError.malformed(
                "\(method): there must be panes or a problem, each pane and field needs a name of its own, a "
                    + "pane a title and a symbol, no field has the focus, a choice is one of its options, a "
                    + "number within its bounds, and a list's rows and actions have names of their own")
        }
        self = settings
    }

    var json: JSONValue {
        var object: [String: JSONValue] = [
            "panes": .array(
                panes.map { pane in
                    var object: [String: JSONValue] = [
                        "id": .string(pane.id), "title": .string(pane.title), "symbol": .string(pane.symbol),
                        "fields": .array(pane.fields.map(\.json)),
                    ]
                    if let note = pane.note {
                        object["note"] = .string(note)
                    }
                    if let more = pane.more {
                        object["more"] = .string(more)
                    }
                    return .object(object)
                })
        ]
        if let problem {
            object["problem"] = .string(problem)
        }
        return .object(object)
    }
}

private extension SettingsUsed {
    init(params: JSONValue) throws {
        let method = "settings.used"
        let object = try FormObject(params, of: method, allowing: Self.allowedKeys)
        let did: Did
        switch (object.fields["value"], try object.optionalString("action")) {
        case let (value?, nil):
            guard object.fields["row"] == nil else {
                throw BridgeError.malformed("\(method): a row goes with an action")
            }
            switch value {
            case let .bool(flag): did = .set(.flag(flag))
            case let .number(number): did = .set(.number(number))
            case let .string(text): did = .set(.text(text))
            case .null, .array, .object:
                throw BridgeError.malformed("\(method): value is true or false, a number or text")
            }
        case let (nil, action?):
            did = try .action(action, row: object.optionalString("row"))
        default:
            throw BridgeError.malformed("\(method): a field is given a value, or one action is asked of a list")
        }
        guard let used = try SettingsUsed(pane: object.string("pane"), field: object.string("field"), did: did)
        else {
            throw BridgeError.malformed("\(method): the pane, the field, the action and the row need names")
        }
        self = used
    }

    var json: JSONValue {
        var object: [String: JSONValue] = ["pane": .string(pane), "field": .string(field)]
        switch did {
        case let .set(.flag(flag)): object["value"] = .bool(flag)
        case let .set(.number(number)): object["value"] = .number(number)
        case let .set(.text(text)): object["value"] = .string(text)
        case let .action(action, row):
            object["action"] = .string(action)
            if let row {
                object["row"] = .string(row)
            }
        }
        return .object(object)
    }
}

/// Refuses a field an object inside a message's params does not have.
private func requireOnly(_ allowed: Set<String>, in value: JSONValue, of method: String) throws {
    let extra = Set((value.object ?? [:]).keys).subtracting(allowed)
    guard extra.isEmpty else {
        throw BridgeError.malformed("\(method): unexpected \(extra.sorted().joined(separator: ", "))")
    }
}

public extension CoreToShell {
    init(line: String) throws {
        let envelope = try Envelope(line: line)
        switch envelope.method {
        case "commands.list":
            let params = try envelope.decode(CommandsListParams.self, allowing: ["commands"])
            // The fields of each command are checked as strictly as the rest.
            if case let .array(commands)? = envelope.params.object?["commands"] {
                for command in commands {
                    let extra = Set((command.object ?? [:]).keys).subtracting(CommandInfo.allowedKeys)
                    guard extra.isEmpty else {
                        throw BridgeError.malformed(
                            "commands.list: unexpected \(extra.sorted().joined(separator: ", "))")
                    }
                }
            }
            self = .commandsList(params.commands)
        case "state.update":
            let state = try envelope.decode(StateUpdate.self, allowing: StateUpdate.allowedKeys)
            guard state.unread >= 0 else {
                throw BridgeError.malformed("state.update: unread is negative")
            }
            for (key, colour) in [("background", state.background), ("foreground", state.foreground)] {
                try envelope.requireNotNull(key)
                if let colour, !StateUpdate.isColor(colour) {
                    throw BridgeError.malformed("state.update: \(key) is not a #rrggbb colour")
                }
            }
            try envelope.requireNotNull("appearance")
            self = .stateUpdate(state)
        case "pane.preview":
            let params = try envelope.decode(PanePreviewParams.self, allowing: PanePreview.allowedKeys)
            // Each item is checked as strictly as the message itself.
            if case let .array(items)? = envelope.params.object?["items"] {
                for item in items {
                    let extra = Set((item.object ?? [:]).keys).subtracting(PaneFile.allowedKeys)
                    guard extra.isEmpty else {
                        throw BridgeError.malformed(
                            "pane.preview: unexpected \(extra.sorted().joined(separator: ", "))")
                    }
                }
            }
            let items = try params.items.map { item in
                guard let file = PaneFile(path: item.path, title: item.title) else {
                    throw BridgeError.malformed("pane.preview: path is not absolute")
                }
                return file
            }
            guard let preview = PanePreview(items: items, index: params.index) else {
                throw BridgeError.malformed("pane.preview: index is not one of the items")
            }
            self = .panePreview(preview)
        case "pane.play":
            let params = try envelope.decode(PaneFileParams.self, allowing: PaneFile.allowedKeys)
            guard let file = PaneFile(path: params.path, title: params.title) else {
                throw BridgeError.malformed("pane.play: path is not absolute")
            }
            self = .panePlay(file)
        case "thumbnail.make":
            let params = try envelope.decode(ThumbnailMakeParams.self, allowing: ThumbnailMake.allowedKeys)
            guard
                let make = ThumbnailMake(id: params.id, path: params.path, out: params.out, size: params.size)
            else {
                throw BridgeError.malformed("thumbnail.make: paths must be absolute and the size from 16 to 4096")
            }
            self = .thumbnailMake(make)
        case "file.open":
            let params = try envelope.decode(FileOpenParams.self, allowing: ["path"])
            guard params.path.hasPrefix("/") else {
                throw BridgeError.malformed("file.open: path is not absolute")
            }
            self = .fileOpen(path: params.path)
        case "files.save":
            let params = try envelope.decode(FilesSaveParams.self, allowing: FilesSave.allowedKeys)
            guard let save = FilesSave(path: params.path, name: params.name) else {
                throw BridgeError.malformed("files.save: the path must be absolute and the name not empty")
            }
            self = .filesSave(save)
        case "pane.close":
            try envelope.requireOnly([])
            self = .paneClose
        case "link.open":
            let open = try envelope.decode(LinkOpen.self, allowing: LinkOpen.allowedKeys)
            guard Links.isOpenable(open.url) else {
                throw BridgeError.malformed("link.open: not an http, https or mailto address")
            }
            self = .linkOpen(open)
        case "files.pick":
            try envelope.requireOnly([])
            self = .filesPick
        case "clipboard.write":
            let params = try envelope.decode(ClipboardWriteParams.self, allowing: ["text"])
            self = .clipboardWrite(text: params.text)
        case "notify.post":
            let params = try envelope.decode(NotifyPostParams.self, allowing: NotifyPost.allowedKeys)
            try envelope.requireNotNull("subtitle")
            // Each action is checked as strictly as the message itself.
            if case let .array(actions)? = envelope.params.object?["actions"] {
                for action in actions {
                    let extra = Set((action.object ?? [:]).keys).subtracting(NotifyAction.allowedKeys)
                    guard extra.isEmpty else {
                        throw BridgeError.malformed(
                            "notify.post: unexpected \(extra.sorted().joined(separator: ", "))")
                    }
                }
            }
            let actions = try params.actions.map { action in
                guard
                    let action = NotifyAction(
                        name: action.name, title: action.title, foreground: action.foreground ?? false)
                else {
                    throw BridgeError.malformed("notify.post: an action needs a title and a name of 1 to 64 bytes")
                }
                return action
            }
            guard
                let post = NotifyPost(
                    id: params.id, title: params.title, subtitle: params.subtitle, body: params.body,
                    actions: actions)
            else {
                throw BridgeError.malformed(
                    "notify.post: the id must be 1 to 128 bytes, the title not empty and the actions at most 4")
            }
            self = .notifyPost(post)
        case "notify.clear":
            let params = try envelope.decode(NotifyClearParams.self, allowing: NotifyClear.allowedKeys)
            guard let clear = NotifyClear(ids: params.ids) else {
                throw BridgeError.malformed("notify.clear: there must be an id, and each 1 to 128 bytes")
            }
            self = .notifyClear(clear)
        case "spell.check":
            let params = try envelope.decode(SpellCheckParams.self, allowing: SpellCheck.allowedKeys)
            guard let check = SpellCheck(id: params.id, text: params.text) else {
                throw BridgeError.malformed(
                    "spell.check: the id must not be negative and the text at most 100000 bytes")
            }
            self = .spellCheck(check)
        case "spell.suggest":
            let params = try envelope.decode(SpellSuggestParams.self, allowing: SpellSuggest.allowedKeys)
            guard let suggest = SpellSuggest(id: params.id, word: params.word) else {
                throw BridgeError.malformed("spell.suggest: the id must not be negative and the word 1 to 200 bytes")
            }
            self = .spellSuggest(suggest)
        case "pane.editor":
            let params = try envelope.decode(PaneEditorParams.self, allowing: PaneEditor.allowedKeys)
            guard let kind = EditorKind(rawValue: params.editor) else {
                throw BridgeError.malformed("pane.editor: editor is vim, nano or mg, never a program")
            }
            guard
                let request = PaneEditor(
                    dir: params.dir, title: params.title, editor: kind, argv: params.argv)
            else {
                throw BridgeError.malformed(
                    "pane.editor: the dir must be absolute and the argv a program by its absolute path")
            }
            self = .paneEditor(request)
        case "form.show":
            self = try .formShow(FormShow(params: envelope.params))
        case "form.close":
            try envelope.requireOnly([])
            self = .formClose
        case "reading.show":
            let params = try envelope.decode(ReadingShowParams.self, allowing: ReadingShow.allowedKeys)
            try envelope.requireNotNull("notice")
            let notice = try readingNotice(params.notice, as: envelope.params.object?["notice"])
            guard
                let show = ReadingShow(
                    path: params.path, title: params.title, remote: params.remote, header: params.header,
                    notice: notice)
            else {
                throw BridgeError.malformed(
                    "reading.show: the path must be absolute and the header at most 8 lines of 1 to 2000 bytes")
            }
            self = .readingShow(show)
        case "reading.scroll":
            let params = try envelope.decode(ReadingScrollParams.self, allowing: ["by"])
            guard let by = ScrollBy(rawValue: params.by) else {
                throw BridgeError.malformed("reading.scroll: by is page_down or page_up")
            }
            self = .readingScroll(by)
        case "reading.close":
            try envelope.requireOnly([])
            self = .readingClose
        case "reading.place":
            let params = try envelope.decode(ColumnsParams.self, allowing: ReadingPlace.allowedKeys)
            guard let place = ReadingPlace(columns: params.columns) else {
                throw BridgeError.malformed("reading.place: columns must be from 1 to 65535")
            }
            self = .readingPlace(place)
        case "editor.place":
            let params = try envelope.decode(EditorPlaceParams.self, allowing: EditorPlace.allowedKeys)
            try envelope.requireNotNull("place")
            self = try .editorPlace(checkedEditorPlace(params, as: envelope.params.object?["place"]))
        case "settings.show":
            self = try .settingsShow(SettingsShow(params: envelope.params))
        case "themes.pick":
            try envelope.requireOnly([])
            self = .themesPick
        case "keys.list":
            let list = try envelope.decode(KeysList.self, allowing: KeysList.allowedKeys)
            // The fields of each section and each line are checked as
            // strictly as the rest.
            if case let .array(sections)? = envelope.params.object?["sections"] {
                for section in sections {
                    try requireOnly(KeysList.Section.allowedKeys, in: section, of: "keys.list")
                    guard case let .array(lines)? = section.object?["keys"] else { continue }
                    for line in lines {
                        try requireOnly(KeysList.Line.allowedKeys, in: line, of: "keys.list")
                    }
                }
            }
            self = .keysList(list)
        case "keys.show":
            try envelope.requireOnly([])
            self = .keysShow
        default:
            throw BridgeError.unknownMethod(envelope.method)
        }
    }

    var line: String {
        switch self {
        case let .commandsList(commands):
            encodeLine(method: "commands.list", params: CommandsListParams(commands: commands))
        case let .stateUpdate(state):
            encodeLine(method: "state.update", params: state)
        case let .panePreview(preview):
            encodeLine(method: "pane.preview", params: preview)
        case let .panePlay(file):
            encodeLine(method: "pane.play", params: file)
        case let .thumbnailMake(make):
            encodeLine(method: "thumbnail.make", params: make)
        case let .fileOpen(path):
            encodeLine(method: "file.open", params: FileOpenParams(path: path))
        case let .filesSave(save):
            encodeLine(method: "files.save", params: save)
        case .paneClose:
            encodeLine(method: "pane.close", params: NoParams())
        case let .linkOpen(open):
            encodeLine(method: "link.open", params: open)
        case .filesPick:
            encodeLine(method: "files.pick", params: NoParams())
        case let .clipboardWrite(text):
            encodeLine(method: "clipboard.write", params: ClipboardWriteParams(text: text))
        case let .notifyPost(post):
            encodeLine(method: "notify.post", params: post)
        case let .notifyClear(clear):
            encodeLine(method: "notify.clear", params: clear)
        case let .spellCheck(check):
            encodeLine(method: "spell.check", params: check)
        case let .spellSuggest(suggest):
            encodeLine(method: "spell.suggest", params: suggest)
        case let .paneEditor(editor):
            encodeLine(method: "pane.editor", params: editor)
        case let .formShow(form):
            encodeLine(method: "form.show", params: form.json)
        case .formClose:
            encodeLine(method: "form.close", params: NoParams())
        case let .readingShow(show):
            encodeLine(method: "reading.show", params: show)
        case let .readingScroll(by):
            encodeLine(method: "reading.scroll", params: ReadingScrollParams(by: by.rawValue))
        case .readingClose:
            encodeLine(method: "reading.close", params: NoParams())
        case let .readingPlace(place):
            encodeLine(method: "reading.place", params: place)
        case let .editorPlace(place):
            encodeLine(method: "editor.place", params: EditorPlaceParams(place))
        case let .settingsShow(settings):
            encodeLine(method: "settings.show", params: settings.json)
        case .themesPick:
            encodeLine(method: "themes.pick", params: NoParams())
        case let .keysList(list):
            encodeLine(method: "keys.list", params: list)
        case .keysShow:
            encodeLine(method: "keys.show", params: NoParams())
        }
    }
}

public extension ShellToCore {
    init(line: String) throws {
        let envelope = try Envelope(line: line)
        switch envelope.method {
        case "command.run":
            let params = try envelope.decode(CommandRunParams.self, allowing: ["name"])
            guard !params.name.isEmpty else {
                throw BridgeError.malformed("command.run: name is empty")
            }
            self = .commandRun(name: params.name)
        case "files.dropped":
            let params = try envelope.decode(FilesDroppedParams.self, allowing: FilesDropped.allowedKeys)
            guard let files = FilesDropped(paths: params.paths, temporary: params.temporary) else {
                throw BridgeError.malformed("files.dropped: paths must be absolute, and there must be one")
            }
            self = .filesDropped(files)
        case "thumbnail.made":
            let params = try envelope.decode(ThumbnailMadeParams.self, allowing: ["id", "ok"])
            guard params.id >= 0 else {
                throw BridgeError.malformed("thumbnail.made: id is negative")
            }
            self = .thumbnailMade(id: params.id, ok: params.ok)
        case "files.saved":
            let params = try envelope.decode(FilesSavedParams.self, allowing: ["name", "saved"])
            self = .filesSaved(name: params.name, saved: params.saved)
        case "notify.used":
            let params = try envelope.decode(NotifyUsedParams.self, allowing: NotifyUsed.allowedKeys)
            try envelope.requireNotNull("action")
            guard let used = NotifyUsed(id: params.id, action: params.action) else {
                throw BridgeError.malformed(
                    "notify.used: the id must be 1 to 128 bytes and the action's name 1 to 64")
            }
            self = .notifyUsed(used)
        case "url.open":
            let params = try envelope.decode(UrlOpenParams.self, allowing: UrlOpen.allowedKeys)
            guard let open = UrlOpen(url: params.url) else {
                throw BridgeError.malformed("url.open: not a mailto address of at most 8192 bytes")
            }
            self = .urlOpen(open)
        case "window.resized":
            let params = try envelope.decode(ColumnsParams.self, allowing: WindowResized.allowedKeys)
            guard let resized = WindowResized(columns: params.columns) else {
                throw BridgeError.malformed("window.resized: columns must be from 1 to 65535")
            }
            self = .windowResized(resized)
        case "spell.checked":
            let params = try envelope.decode(SpellCheckedParams.self, allowing: SpellChecked.allowedKeys)
            // Each word is checked as strictly as the message itself.
            if case let .array(words)? = envelope.params.object?["words"] {
                for word in words {
                    let extra = Set((word.object ?? [:]).keys).subtracting(Misspelt.allowedKeys)
                    guard extra.isEmpty else {
                        throw BridgeError.malformed(
                            "spell.checked: unexpected \(extra.sorted().joined(separator: ", "))")
                    }
                }
            }
            let words = try params.words.map { word in
                guard let word = Misspelt(start: word.start, end: word.end) else {
                    throw BridgeError.malformed("spell.checked: a misspelt word must end after it starts")
                }
                return word
            }
            guard let checked = SpellChecked(id: params.id, words: words) else {
                throw BridgeError.malformed(
                    "spell.checked: the id must not be negative and the words at most 2000, in order")
            }
            self = .spellChecked(checked)
        case "spell.suggested":
            let params = try envelope.decode(SpellSuggestedParams.self, allowing: SpellSuggested.allowedKeys)
            guard let suggested = SpellSuggested(id: params.id, words: params.words) else {
                throw BridgeError.malformed(
                    "spell.suggested: the id must not be negative and the words at most 20, each 1 to 200 bytes")
            }
            self = .spellSuggested(suggested)
        case "editor.exited":
            let params = try envelope.decode(EditorExitedParams.self, allowing: EditorExited.allowedKeys)
            guard let exited = EditorExited(ran: params.ran, status: params.status) else {
                throw BridgeError.malformed("editor.exited: the status must be from -255 to 255")
            }
            self = .editorExited(exited)
        case "form.used":
            self = try .formUsed(FormUsed(params: envelope.params))
        case "settings.opened":
            try envelope.requireOnly([])
            self = .settingsOpened
        case "settings.closed":
            try envelope.requireOnly([])
            self = .settingsClosed
        case "settings.used":
            self = try .settingsUsed(SettingsUsed(params: envelope.params))
        case "appearance.changed":
            let params = try envelope.decode(AppearanceChangedParams.self, allowing: ["dark"])
            self = .appearanceChanged(dark: params.dark)
        case "themes.imported":
            let params = try envelope.decode(ThemesImportedParams.self, allowing: ThemesImported.allowedKeys)
            guard let imported = ThemesImported(name: params.name) else {
                throw BridgeError.malformed("themes.imported: not a name a theme can have")
            }
            self = .themesImported(imported)
        case "app.will_quit":
            try envelope.requireOnly([])
            self = .appWillQuit
        default:
            throw BridgeError.unknownMethod(envelope.method)
        }
    }

    var line: String {
        switch self {
        case let .commandRun(name):
            encodeLine(method: "command.run", params: CommandRunParams(name: name))
        case let .filesDropped(files):
            encodeLine(
                method: "files.dropped",
                params: FilesDroppedParams(paths: files.paths, temporary: files.temporary))
        case let .thumbnailMade(id, ok):
            encodeLine(method: "thumbnail.made", params: ThumbnailMadeParams(id: id, ok: ok))
        case let .filesSaved(name, saved):
            encodeLine(method: "files.saved", params: FilesSavedParams(name: name, saved: saved))
        case let .notifyUsed(used):
            encodeLine(method: "notify.used", params: used)
        case let .urlOpen(open):
            encodeLine(method: "url.open", params: open)
        case let .windowResized(resized):
            encodeLine(method: "window.resized", params: resized)
        case let .spellChecked(checked):
            encodeLine(method: "spell.checked", params: checked)
        case let .spellSuggested(suggested):
            encodeLine(method: "spell.suggested", params: suggested)
        case let .editorExited(exited):
            encodeLine(method: "editor.exited", params: exited)
        case let .formUsed(used):
            encodeLine(method: "form.used", params: used.json)
        case .settingsOpened:
            encodeLine(method: "settings.opened", params: NoParams())
        case .settingsClosed:
            encodeLine(method: "settings.closed", params: NoParams())
        case let .settingsUsed(used):
            encodeLine(method: "settings.used", params: used.json)
        case let .appearanceChanged(dark):
            encodeLine(method: "appearance.changed", params: AppearanceChangedParams(dark: dark))
        case let .themesImported(imported):
            encodeLine(method: "themes.imported", params: imported)
        case .appWillQuit:
            encodeLine(method: "app.will_quit", params: NoParams())
        }
    }
}
