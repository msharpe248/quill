import Darwin
import Foundation

/// A child process on a pseudo-terminal that this app owns.
///
/// libghostty can start a process itself, but on macOS it always goes through
/// `/usr/bin/login`, which the App Sandbox does not permit. Owning the PTY
/// also gives the shell the child's exit status and full say over its
/// environment.
final class PTYProcess: @unchecked Sendable {
    struct Size: Sendable, Equatable {
        var columns: UInt16
        var rows: UInt16
        var widthPixels: UInt16 = 0
        var heightPixels: UInt16 = 0
    }

    enum SpawnError: Error, CustomStringConvertible {
        case openPTY(errno: Int32)
        case spawn(errno: Int32)

        var description: String {
            switch self {
            case let .openPTY(code): "cannot open a pseudo-terminal: \(String(cString: strerror(code)))"
            case let .spawn(code): "cannot start the program: \(String(cString: strerror(code)))"
            }
        }
    }

    let pid: pid_t
    private let master: Int32
    private let readQueue = DispatchQueue(label: "quill.terminalkit.pty.read")
    private let writeQueue = DispatchQueue(label: "quill.terminalkit.pty.write")
    private let readSource: any DispatchSourceRead
    private let exitSource: any DispatchSourceProcess
    private let onOutput: @Sendable (Data) -> Void
    /// Whether the program has ended. Once it has, its process id and the
    /// PTY's descriptor may belong to something else, so nothing is sent to
    /// either. Read and written under `stateLock`.
    private var ended = false
    private let stateLock = NSLock()

    /// Starts `command` on a new PTY of `size`. `onOutput` and `onExit` are
    /// called on a private queue; `onExit` is called once, after the last
    /// output.
    init(
        command: TerminalCommand,
        size: Size,
        onOutput: @escaping @Sendable (Data) -> Void,
        onExit: @escaping @Sendable (TerminalExit) -> Void
    ) throws {
        var master: Int32 = -1
        var slave: Int32 = -1
        var window = winsize(
            ws_row: size.rows,
            ws_col: size.columns,
            ws_xpixel: size.widthPixels,
            ws_ypixel: size.heightPixels
        )
        guard openpty(&master, &slave, nil, nil, &window) == 0 else {
            throw SpawnError.openPTY(errno: errno)
        }
        defer { close(slave) }
        // The App Sandbox refuses every terminal "set" request made on the
        // slave side, silently in the case of `openpty`'s own attempt to
        // apply the size above. The master side is allowed, so the size and
        // modes are set again from here.
        _ = ioctl(master, TIOCSWINSZ, &window)
        if command.rawTerminal {
            var modes = termios()
            if tcgetattr(master, &modes) == 0 {
                cfmakeraw(&modes)
                tcsetattr(master, TCSANOW, &modes)
            }
        }
        guard let slaveName = ptsname(master) else {
            let code = errno
            close(master)
            throw SpawnError.openPTY(errno: code)
        }
        let slavePath = String(cString: slaveName)

        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        // The child is a session leader (SETSID below), so the first terminal
        // it opens becomes its controlling terminal.
        posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, slavePath, O_RDWR, 0)
        posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDOUT_FILENO)
        posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDERR_FILENO)
        if let directory = command.workingDirectory {
            posix_spawn_file_actions_addchdir_np(&actions, directory.path)
        }

        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        var defaultSignals = sigset_t()
        sigfillset(&defaultSignals)
        posix_spawnattr_setsigdefault(&attributes, &defaultSignals)
        var noSignals = sigset_t()
        sigemptyset(&noSignals)
        posix_spawnattr_setsigmask(&attributes, &noSignals)
        // CLOEXEC_DEFAULT closes every descriptor the file actions did not
        // set up, so the child never sees the master or anything else of ours.
        let flags =
            POSIX_SPAWN_SETSID | POSIX_SPAWN_CLOEXEC_DEFAULT
            | POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK
        posix_spawnattr_setflags(&attributes, Int16(flags))

        let arguments = [command.executable.path] + command.arguments
        let environment = command.environment.map { "\($0.key)=\($0.value)" }
        var pid: pid_t = 0
        let result = Self.withCStringArray(arguments) { argv in
            Self.withCStringArray(environment) { envp in
                posix_spawn(&pid, command.executable.path, &actions, &attributes, argv, envp)
            }
        }
        guard result == 0 else {
            close(master)
            throw SpawnError.spawn(errno: result)
        }

        self.pid = pid
        self.master = master
        self.onOutput = onOutput
        readSource = DispatchSource.makeReadSource(fileDescriptor: master, queue: readQueue)
        exitSource = DispatchSource.makeProcessSource(identifier: pid, eventMask: .exit, queue: readQueue)

        readSource.setEventHandler { [self] in
            _ = readAvailable()
        }
        readSource.setCancelHandler { [writeQueue, master] in
            // On the queue that writes, behind any write already on its
            // way: none may land on a descriptor that has been closed and
            // given to something else.
            writeQueue.async {
                close(master)
            }
        }
        exitSource.setEventHandler { [self] in
            // Before the program is reaped and its id can be used again.
            stateLock.withLock { ended = true }
            var status: Int32 = 0
            while waitpid(self.pid, &status, 0) == -1, errno == EINTR {}
            // Whatever the child wrote before it died is still in the PTY.
            while Self.isReadable(master), readAvailable() {}
            exitSource.cancel()
            readSource.cancel()
            let signal = status & 0x7F
            onExit(signal == 0 ? .exited(code: (status >> 8) & 0xFF) : .signaled(signal: signal))
        }
        readSource.resume()
        exitSource.resume()
    }

    /// Sends bytes to the program, as if typed.
    func write(_ data: Data) {
        writeQueue.async { [self] in
            guard !stateLock.withLock({ ended }) else { return }
            data.withUnsafeBytes { buffer in
                var offset = 0
                while offset < buffer.count {
                    let written = Darwin.write(master, buffer.baseAddress! + offset, buffer.count - offset)
                    if written > 0 {
                        offset += written
                    } else if written == -1, errno == EINTR || errno == EAGAIN {
                        continue
                    } else {
                        // The program has gone. Nothing useful to do with the rest.
                        return
                    }
                }
            }
        }
    }

    func resize(_ size: Size) {
        var window = winsize(
            ws_row: size.rows,
            ws_col: size.columns,
            ws_xpixel: size.widthPixels,
            ws_ypixel: size.heightPixels
        )
        stateLock.withLock {
            guard !ended else { return }
            _ = ioctl(master, TIOCSWINSZ, &window)
            // The kernel tells the terminal's foreground process group about
            // a new size. In the sandbox the child is not that group, having
            // been refused the terminal as its controlling one, so tell it
            // directly. A second SIGWINCH outside the sandbox is harmless.
            kill(pid, SIGWINCH)
        }
    }

    /// Asks the program to stop the way a closing terminal does.
    func hangUp() {
        send(SIGHUP)
    }

    /// Ends the program without asking.
    func endNow() {
        send(SIGKILL)
    }

    /// Sends a signal to the program, unless it has already ended.
    private func send(_ number: Int32) {
        stateLock.withLock {
            guard !ended else { return }
            kill(pid, number)
        }
    }

    /// Reads once from the PTY. Returns `false` at end of file or on error.
    private func readAvailable() -> Bool {
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        let count = read(master, &buffer, buffer.count)
        if count > 0 {
            onOutput(Data(buffer[..<count]))
            return true
        }
        return count == -1 && errno == EINTR
    }

    private static func isReadable(_ descriptor: Int32) -> Bool {
        var poll = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
        return Darwin.poll(&poll, 1, 0) > 0 && (poll.revents & Int16(POLLIN)) != 0
    }

    /// Calls `body` with a NULL-terminated array of C strings.
    private static func withCStringArray<Result>(
        _ strings: [String],
        _ body: ([UnsafeMutablePointer<CChar>?]) -> Result
    ) -> Result {
        let pointers = strings.map { strdup($0) }
        defer { pointers.forEach { free($0) } }
        return body(pointers + [nil])
    }
}
