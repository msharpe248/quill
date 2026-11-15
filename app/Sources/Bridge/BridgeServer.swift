import Darwin
import Foundation

/// The shell's end of the bridge: a Unix domain socket in a private directory
/// that the core connects to.
///
/// One core connects in the server's lifetime. Messages from it are delivered
/// on the main actor; sending never blocks the caller.
public final class BridgeServer: @unchecked Sendable {
    public enum ServerError: Error, CustomStringConvertible {
        case pathTooLong(String)
        case system(String, errno: Int32)

        public var description: String {
            switch self {
            case let .pathTooLong(path): "the socket path is too long: \(path)"
            case let .system(what, code): "\(what): \(String(cString: strerror(code)))"
            }
        }
    }

    /// The path to give the core in `QUILL_SHELL_SOCKET`.
    public let socketPath: String

    private let directory: URL
    private let listener: Int32
    private let queue = DispatchQueue(label: "quill.bridge.read")
    private let writeQueue = DispatchQueue(label: "quill.bridge.write")
    private let lock = NSLock()
    private var connection: Int32 = -1
    private var pending: [String] = []
    private var closed = false

    /// Creates `directory` (owner-only) and listens on a socket inside it.
    public init(directory: URL) throws {
        self.directory = directory
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        let path = directory.appendingPathComponent("s").path
        socketPath = path

        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard path.utf8.count < capacity else {
            throw ServerError.pathTooLong(path)
        }
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.copyBytes(from: path.utf8)
        }

        listener = socket(AF_UNIX, SOCK_STREAM, 0)
        guard listener >= 0 else {
            throw ServerError.system("cannot create a socket", errno: errno)
        }
        unlink(path)
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(listener, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0 else {
            let code = errno
            Darwin.close(listener)
            throw ServerError.system("cannot bind \(path)", errno: code)
        }
        chmod(path, 0o600)
        guard listen(listener, 1) == 0 else {
            let code = errno
            Darwin.close(listener)
            throw ServerError.system("cannot listen on \(path)", errno: code)
        }
    }

    deinit {
        close()
    }

    /// Waits for the core to connect, then delivers what it sends.
    /// `onMessage` gets each message, `onProblem` each line that is not one,
    /// and `onDisconnect` is called once when the core goes away.
    public func start(
        onMessage: @escaping @MainActor (CoreToShell) -> Void,
        onProblem: @escaping @MainActor (String) -> Void,
        onDisconnect: @escaping @MainActor () -> Void
    ) {
        queue.async { [self] in
            var accepted: Int32 = -1
            repeat {
                accepted = accept(listener, nil, nil)
            } while accepted == -1 && errno == EINTR
            guard accepted >= 0 else { return }
            // A write to a core that has gone must fail, not kill the app.
            var on: Int32 = 1
            setsockopt(accepted, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
            let queued = lock.withLock {
                connection = accepted
                defer { pending.removeAll() }
                return pending
            }
            queued.forEach(write)

            var buffer = Data()
            var chunk = [UInt8](repeating: 0, count: 16 * 1024)
            while true {
                let count = read(accepted, &chunk, chunk.count)
                if count == -1, errno == EINTR { continue }
                guard count > 0 else { break }
                buffer.append(contentsOf: chunk[..<count])
                while let newline = buffer.firstIndex(of: UInt8(ascii: "\n")) {
                    let line = String(decoding: buffer[buffer.startIndex..<newline], as: UTF8.self)
                    buffer.removeSubrange(buffer.startIndex...newline)
                    guard !line.allSatisfy(\.isWhitespace) else { continue }
                    do {
                        let message = try CoreToShell(line: line)
                        DispatchQueue.main.async { MainActor.assumeIsolated { onMessage(message) } }
                    } catch {
                        let problem = "bridge: \(error)"
                        DispatchQueue.main.async { MainActor.assumeIsolated { onProblem(problem) } }
                    }
                }
            }
            DispatchQueue.main.async { MainActor.assumeIsolated { onDisconnect() } }
        }
    }

    /// Sends a message to the core. Before the core has connected, messages
    /// wait their turn.
    public func send(_ message: ShellToCore) {
        let line = message.line
        let connected = lock.withLock {
            if connection < 0, !closed {
                pending.append(line)
            }
            return connection >= 0
        }
        if connected {
            write(line)
        }
    }

    private func write(_ line: String) {
        writeQueue.async { [self] in
            let descriptor = lock.withLock { connection }
            guard descriptor >= 0 else { return }
            let bytes = Array((line + "\n").utf8)
            var offset = 0
            while offset < bytes.count {
                let written = bytes[offset...].withUnsafeBytes {
                    Darwin.write(descriptor, $0.baseAddress, $0.count)
                }
                if written > 0 {
                    offset += written
                } else if written == -1, errno == EINTR {
                    continue
                } else {
                    return
                }
            }
        }
    }

    /// Stops listening, drops the connection and removes the socket.
    public func close() {
        let (alreadyClosed, descriptor): (Bool, Int32) = lock.withLock {
            guard !closed else { return (true, -1) }
            closed = true
            let current = connection
            connection = -1
            return (false, current)
        }
        guard !alreadyClosed else { return }
        if descriptor >= 0 {
            shutdown(descriptor, SHUT_RDWR)
            Darwin.close(descriptor)
        }
        // Closing the listener makes a pending accept return.
        shutdown(listener, SHUT_RDWR)
        Darwin.close(listener)
        unlink(socketPath)
        try? FileManager.default.removeItem(at: directory)
    }
}
