import Darwin
import Foundation

/// What a surface needs of the program running on its terminal.
protocol TerminalProgram: AnyObject, Sendable {
    /// Sends bytes to the program, as if typed.
    func write(_ data: Data)
    func resize(_ size: PTYProcess.Size)
    /// Asks the program to stop the way a closing terminal does.
    func hangUp()
    /// Ends the program without asking, as far as that can be done.
    func endNow()
}

extension PTYProcess: TerminalProgram {}

/// One of the user's own scripts on a pseudo-terminal that this app owns.
///
/// The script is in the app's scripts folder, where only the user can put
/// one, and it is macOS that runs it (`NSUserUnixTask`), outside the App
/// Sandbox: that is what the folder is for. So the app is not the script's
/// parent. It has no process id for it and cannot signal it, and neither
/// its environment nor the folder it starts in are the app's to choose. What
/// the app does have is the terminal. The script is given the terminal as
/// its input and output, and hears that the terminal has gone when this
/// side of it is closed.
final class PTYUserScript: TerminalProgram, @unchecked Sendable {
    private let master: Int32
    private let slave: Int32
    private let readQueue = DispatchQueue(label: "quill.terminalkit.script.read")
    private let writeQueue = DispatchQueue(label: "quill.terminalkit.script.write")
    private let readSource: any DispatchSourceRead
    private let onOutput: @Sendable (Data) -> Void
    private let onExit: @Sendable (TerminalExit) -> Void
    /// Once the terminal is closed its descriptor may be something else's,
    /// so nothing more is done with it. Read and written under `stateLock`.
    private var closed = false
    private var finished = false
    private var sawOutput = false
    private let stateLock = NSLock()

    /// Runs `command.executable`, a script in the app's scripts folder, with
    /// `command.arguments`, on a new PTY of `size`. The command's
    /// environment and working directory are not used: macOS decides both.
    /// `onOutput` and `onExit` are called on a private queue; `onExit` is
    /// called once.
    init(
        command: TerminalCommand,
        size: PTYProcess.Size,
        onOutput: @escaping @Sendable (Data) -> Void,
        onExit: @escaping @Sendable (TerminalExit) -> Void
    ) throws {
        // Before the terminal is made: a script that is not there, or not
        // in the scripts folder, costs nothing but the answer.
        let task = try NSUserUnixTask(url: command.executable)
        var masterSide: Int32 = -1
        var slaveSide: Int32 = -1
        var window = winsize(
            ws_row: size.rows,
            ws_col: size.columns,
            ws_xpixel: size.widthPixels,
            ws_ypixel: size.heightPixels
        )
        guard openpty(&masterSide, &slaveSide, nil, nil, &window) == 0 else {
            throw PTYProcess.SpawnError.openPTY(errno: errno)
        }
        // As in `PTYProcess`: only the master side may set the size from
        // inside the sandbox. The modes are left as they are. The script is
        // outside the sandbox and sets its own, as in any terminal.
        _ = ioctl(masterSide, TIOCSWINSZ, &window)
        let master = masterSide
        let slave = slaveSide
        self.master = master
        self.slave = slave
        self.onOutput = onOutput
        self.onExit = onExit
        readSource = DispatchSource.makeReadSource(fileDescriptor: master, queue: readQueue)
        readSource.setEventHandler { [self] in
            _ = readAvailable()
        }
        readSource.setCancelHandler { [writeQueue, master] in
            // Behind any write already on its way, as in `PTYProcess`.
            writeQueue.async {
                close(master)
            }
        }
        readSource.resume()

        // This side keeps its own descriptor for the terminal until the
        // script has ended, so that the terminal is never without one.
        let terminal = FileHandle(fileDescriptor: slave, closeOnDealloc: false)
        task.standardInput = terminal
        task.standardOutput = terminal
        task.standardError = terminal
        task.execute(withArguments: command.arguments) { [self] error in
            readQueue.async { [self] in
                // What the script wrote last is still in the terminal.
                while !isClosed, Self.isReadable(master), readAvailable() {}
                finish(Self.exit(from: error, sawOutput: stateLock.withLock { sawOutput }))
            }
        }
    }

    func write(_ data: Data) {
        writeQueue.async { [self] in
            guard !isClosed else { return }
            data.withUnsafeBytes { buffer in
                var offset = 0
                while offset < buffer.count {
                    let written = Darwin.write(master, buffer.baseAddress! + offset, buffer.count - offset)
                    if written > 0 {
                        offset += written
                    } else if written == -1, errno == EINTR || errno == EAGAIN {
                        continue
                    } else {
                        return
                    }
                }
            }
        }
    }

    /// The kernel tells whoever has the terminal as their own about the new
    /// size. There is nobody here to tell directly.
    func resize(_ size: PTYProcess.Size) {
        var window = winsize(
            ws_row: size.rows,
            ws_col: size.columns,
            ws_xpixel: size.widthPixels,
            ws_ypixel: size.heightPixels
        )
        stateLock.withLock {
            guard !closed else { return }
            _ = ioctl(master, TIOCSWINSZ, &window)
        }
    }

    /// Closes this side of the terminal. A program that has it as its
    /// controlling terminal is hung up on by the kernel; any other finds
    /// nothing more to read. The script's end is still waited for.
    func hangUp() {
        closeTerminal()
    }

    /// There is no ending a program that is not this app's child. The
    /// terminal is closed, and whoever is waiting is told now that it is
    /// over: the script, if it ever ends, is not reported twice.
    func endNow() {
        closeTerminal()
        readQueue.async { [self] in
            finish(.signaled(signal: SIGKILL))
        }
    }

    private var isClosed: Bool {
        stateLock.withLock { closed }
    }

    private func closeTerminal() {
        let first = stateLock.withLock {
            let first = !closed
            closed = true
            return first
        }
        if first {
            readSource.cancel()
        }
    }

    /// On the read queue. Reports the end once.
    private func finish(_ exit: TerminalExit) {
        let first = stateLock.withLock {
            let first = !finished
            finished = true
            return first
        }
        guard first else { return }
        closeTerminal()
        close(slave)
        onExit(exit)
    }

    /// Reads once from the PTY. Returns `false` at end of file or on error.
    private func readAvailable() -> Bool {
        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        let count = read(master, &buffer, buffer.count)
        if count > 0 {
            stateLock.withLock { sawOutput = true }
            onOutput(Data(buffer[..<count]))
            return true
        }
        return count == -1 && errno == EINTR
    }

    private static func isReadable(_ descriptor: Int32) -> Bool {
        var poll = pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0)
        return Darwin.poll(&poll, 1, 0) > 0 && (poll.revents & Int16(POLLIN)) != 0
    }

    /// How the script ended, from what `NSUserUnixTask` says. It gives no
    /// status as a number: a script that exited with anything but 0 is an
    /// error whose reason names the status in words.
    static func exit(from error: (any Error)?, sawOutput: Bool) -> TerminalExit {
        guard let failure = error else { return .exited(code: 0) }
        let error = failure as NSError
        let reason = error.localizedFailureReason ?? ""
        if let status = statusNamed(in: reason) {
            return .exited(code: status)
        }
        // Something that drew on the terminal did run, however it ended.
        if sawOutput {
            return .exited(code: 1)
        }
        return .failedToStart(reason: reason.isEmpty ? error.localizedDescription : reason)
    }

    /// The last whole number in `reason`, as in "Script task exited with
    /// status 7", when it could be an exit status.
    static func statusNamed(in reason: String) -> Int32? {
        guard reason.contains("status") else { return nil }
        let digits = reason.split(whereSeparator: { !$0.isNumber }).last.map(String.init)
        guard let digits, let status = Int32(digits), (0...255).contains(status) else { return nil }
        return status
    }
}
