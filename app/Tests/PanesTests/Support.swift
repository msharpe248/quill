import AppKit
import Darwin
import Foundation
import Testing

/// A folder of the test's own inside the temporary folder, removed with it.
final class Scratch {
    let url: URL

    init() throws {
        url = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("quill-panes-\(UUID().uuidString.prefix(8))", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    deinit {
        try? FileManager.default.removeItem(at: url)
    }

    func file(_ name: String) -> URL {
        url.appendingPathComponent(name)
    }

    @discardableResult
    func write(_ text: String, to name: String) throws -> URL {
        let file = file(name)
        try Data(text.utf8).write(to: file)
        return file
    }
}

struct TimedOut: Error, CustomStringConvertible {
    let description: String
}

/// Waits, letting the main run loop turn, until the condition holds.
/// Throws if it does not within the time allowed.
@MainActor
func wait(
    upTo seconds: Double = 20, for what: String, until condition: @MainActor () -> Bool
) async throws {
    let deadline = Date().addingTimeInterval(seconds)
    while !condition() {
        guard Date() < deadline else {
            throw TimedOut(description: "timed out waiting for \(what)")
        }
        try await Task.sleep(for: .milliseconds(10))
    }
}

/// The same, for a condition that has to wait on something itself.
@MainActor
func wait(
    upTo seconds: Double = 20, for what: String, until condition: @MainActor () async -> Bool
) async throws {
    let deadline = Date().addingTimeInterval(seconds)
    while !(await condition()) {
        guard Date() < deadline else {
            throw TimedOut(description: "timed out waiting for \(what)")
        }
        try await Task.sleep(for: .milliseconds(20))
    }
}

/// Lets the given time pass while watching that the condition keeps
/// holding. Returns false as soon as it does not.
@MainActor
func holds(for seconds: Double, _ condition: @MainActor () -> Bool) async throws -> Bool {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        guard condition() else { return false }
        try await Task.sleep(for: .milliseconds(10))
    }
    return condition()
}

/// A web server on 127.0.0.1 that answers everything with an empty page and
/// remembers who called: every connection accepted, and every path asked for.
final class LocalServer: @unchecked Sendable {
    let port: UInt16
    private let listener: Int32
    private let lock = NSLock()
    private var accepted = 0
    private var asked: [String] = []

    init() throws {
        let listener = socket(AF_INET, SOCK_STREAM, 0)
        guard listener >= 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
        var yes: Int32 = 1
        setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &yes, socklen_t(MemoryLayout<Int32>.size))
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = 0
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        let size = socklen_t(MemoryLayout<sockaddr_in>.size)
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(listener, $0, size) }
        }
        guard bound == 0, listen(listener, 64) == 0 else {
            let code = errno
            close(listener)
            throw POSIXError(.init(rawValue: code) ?? .EIO)
        }
        var actual = sockaddr_in()
        var actualSize = size
        _ = withUnsafeMutablePointer(to: &actual) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(listener, $0, &actualSize) }
        }
        self.listener = listener
        port = UInt16(bigEndian: actual.sin_port)

        Thread.detachNewThread { [weak self] in
            while true {
                let connection = accept(listener, nil, nil)
                guard connection >= 0 else { return }
                guard let self else {
                    close(connection)
                    return
                }
                self.lock.withLock { self.accepted += 1 }
                DispatchQueue.global().async { self.answer(connection) }
            }
        }
    }

    deinit {
        stop()
    }

    func stop() {
        shutdown(listener, SHUT_RDWR)
        close(listener)
    }

    /// Connections accepted so far, whether or not anything was asked on them.
    var connections: Int { lock.withLock { accepted } }

    /// The paths asked for so far, in order.
    var paths: [String] { lock.withLock { asked } }

    func url(_ path: String) -> String {
        "http://127.0.0.1:\(port)\(path)"
    }

    private func answer(_ connection: Int32) {
        defer { close(connection) }
        var timeout = timeval(tv_sec: 2, tv_usec: 0)
        setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        var noSignal: Int32 = 1
        setsockopt(connection, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
        var request = [UInt8]()
        var buffer = [UInt8](repeating: 0, count: 2048)
        while !request.contains(UInt8(ascii: "\n")) {
            let count = read(connection, &buffer, buffer.count)
            guard count > 0 else { break }
            request.append(contentsOf: buffer[..<count])
        }
        // "GET /pixel.png HTTP/1.1"
        let line = String(
            decoding: request.prefix { $0 != UInt8(ascii: "\r") && $0 != UInt8(ascii: "\n") }, as: UTF8.self)
        let words = line.split(separator: " ")
        if words.count >= 2 {
            lock.withLock { asked.append(String(words[1])) }
        }
        let reply = Array("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".utf8)
        _ = reply.withUnsafeBytes { write(connection, $0.baseAddress, $0.count) }
    }
}
