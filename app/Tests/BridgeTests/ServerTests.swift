import Darwin
import Foundation
import Testing

@testable import Bridge

/// A scripted stand-in for the core: it connects to the server's socket and
/// the test plays the core's part by hand.
private final class FakeCore {
    let descriptor: Int32

    init(socketPath: String) throws {
        descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: socketPath.utf8) }
        let connected = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else {
            throw POSIXError(.init(rawValue: errno) ?? .EIO)
        }
        var timeout = timeval(tv_sec: 5, tv_usec: 0)
        setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    }

    deinit {
        close(descriptor)
    }

    func send(_ text: String) {
        let bytes = Array(text.utf8)
        _ = bytes.withUnsafeBytes { write(descriptor, $0.baseAddress, $0.count) }
    }

    /// Reads up to the next newline.
    func readLine() -> String? {
        var line = [UInt8]()
        var byte: UInt8 = 0
        while read(descriptor, &byte, 1) == 1 {
            if byte == UInt8(ascii: "\n") {
                return String(decoding: line, as: UTF8.self)
            }
            line.append(byte)
        }
        return nil
    }
}

/// What the server delivered, collected on the main actor.
@MainActor
private final class Inbox {
    var messages: [CoreToShell] = []
    var problems: [String] = []
    var disconnected = false

    func wait(until condition: () -> Bool) async throws {
        for _ in 0..<500 where !condition() {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(condition(), "timed out")
    }
}

private func temporaryDirectory() -> URL {
    // Short, because a socket path has little more than a hundred bytes.
    URL(fileURLWithPath: "/tmp").appendingPathComponent("qb-\(UUID().uuidString.prefix(8))")
}

@MainActor
private func startedServer() throws -> (BridgeServer, Inbox) {
    let server = try BridgeServer(directory: temporaryDirectory())
    let inbox = Inbox()
    server.start(
        onMessage: { inbox.messages.append($0) },
        onProblem: { inbox.problems.append($0) },
        onDisconnect: { inbox.disconnected = true }
    )
    return (server, inbox)
}

@Suite(.serialized) @MainActor struct ServerTests {
    @Test func theSocketIsPrivateToItsOwner() throws {
        let (server, _) = try startedServer()
        defer { server.close() }
        var info = stat()
        #expect(stat(server.socketPath, &info) == 0)
        #expect(info.st_mode & 0o777 == 0o600)
        let directory = (server.socketPath as NSString).deletingLastPathComponent
        #expect(stat(directory, &info) == 0)
        #expect(info.st_mode & 0o777 == 0o700)
    }

    @Test func whatTheCoreSendsIsDeliveredInOrder() async throws {
        let (server, inbox) = try startedServer()
        defer { server.close() }
        let core = try FakeCore(socketPath: server.socketPath)
        core.send(#"{"jsonrpc":"2.0","method":"pane.close","params":{}}"# + "\n\n")
        // Two messages in one write, and one split across two writes.
        core.send(
            #"{"jsonrpc":"2.0","method":"state.update","params":{"context":"list","enabled":[],"unread":3,"title":"Inbox"}}"#
                + "\n"
                + #"{"jsonrpc":"2.0","method":"pane.pre"#)
        core.send(#"view","params":{"path":"/tmp/a.pdf","title":"a.pdf"}}"# + "\n")
        try await inbox.wait { inbox.messages.count == 3 }
        #expect(
            inbox.messages == [
                .paneClose,
                .stateUpdate(StateUpdate(context: .list, enabled: [], unread: 3, title: "Inbox")),
                .panePreview(PanePreview(path: "/tmp/a.pdf", title: "a.pdf")),
            ])
        #expect(inbox.problems.isEmpty)
    }

    @Test func aLineThatIsNotAMessageIsReportedAndTheNextStillWorks() async throws {
        let (server, inbox) = try startedServer()
        defer { server.close() }
        let core = try FakeCore(socketPath: server.socketPath)
        core.send("this is not json\n" + #"{"jsonrpc":"2.0","method":"pane.close","params":{}}"# + "\n")
        try await inbox.wait { inbox.messages.count == 1 }
        #expect(inbox.problems == ["bridge: not a bridge message: invalid JSON"])
        #expect(inbox.messages == [.paneClose])
    }

    @Test func whatTheShellSendsArrivesAsOneLineEach() async throws {
        let (server, _) = try startedServer()
        defer { server.close() }
        // Sent before the core connects: it must wait, not be lost.
        server.send(.commandRun(name: "view.zoom"))
        let core = try FakeCore(socketPath: server.socketPath)
        #expect(core.readLine() == #"{"jsonrpc":"2.0","method":"command.run","params":{"name":"view.zoom"}}"#)
        server.send(.appWillQuit)
        #expect(core.readLine() == #"{"jsonrpc":"2.0","method":"app.will_quit","params":{}}"#)
    }

    @Test func theCoreGoingAwayIsReported() async throws {
        let (server, inbox) = try startedServer()
        defer { server.close() }
        do {
            let core = try FakeCore(socketPath: server.socketPath)
            core.send(#"{"jsonrpc":"2.0","method":"pane.close","params":{}}"# + "\n")
            try await inbox.wait { inbox.messages.count == 1 }
        }
        try await inbox.wait { inbox.disconnected }
        // Sending into the void is harmless.
        server.send(.appWillQuit)
    }

    @Test func closingRemovesTheSocketAndItsDirectory() throws {
        let (server, _) = try startedServer()
        let path = server.socketPath
        server.close()
        server.close()
        #expect(!FileManager.default.fileExists(atPath: path))
        #expect(!FileManager.default.fileExists(atPath: (path as NSString).deletingLastPathComponent))
    }

    @Test func aPathTooLongForASocketIsRefused() {
        let long = URL(fileURLWithPath: "/tmp").appendingPathComponent(String(repeating: "x", count: 120))
        #expect(throws: BridgeServer.ServerError.self) { try BridgeServer(directory: long) }
        try? FileManager.default.removeItem(at: long)
    }
}
