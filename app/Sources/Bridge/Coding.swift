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
    // These types hold only strings, integers and arrays of them, which
    // always encode.
    let data = (try? encoder.encode(Outgoing(method: method, params: params))) ?? Data()
    return String(decoding: data, as: UTF8.self)
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
            self = .stateUpdate(state)
        case "pane.preview":
            let preview = try envelope.decode(PanePreview.self, allowing: PanePreview.allowedKeys)
            guard preview.path.hasPrefix("/") else {
                throw BridgeError.malformed("pane.preview: path is not absolute")
            }
            self = .panePreview(preview)
        case "pane.close":
            try envelope.requireOnly([])
            self = .paneClose
        case "url.open":
            let open = try envelope.decode(UrlOpen.self, allowing: UrlOpen.allowedKeys)
            guard Links.isOpenable(open.url) else {
                throw BridgeError.malformed("url.open: not an http, https or mailto address")
            }
            self = .urlOpen(open)
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
        case .paneClose:
            encodeLine(method: "pane.close", params: NoParams())
        case let .urlOpen(open):
            encodeLine(method: "url.open", params: open)
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
        case .appWillQuit:
            encodeLine(method: "app.will_quit", params: NoParams())
        }
    }
}
