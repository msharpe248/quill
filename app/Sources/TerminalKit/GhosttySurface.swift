import AppKit
import GhosttyTerminal

/// The libghostty implementation of `TerminalSurface`.
///
/// libghostty draws and handles input; the program runs on a PTY this module
/// owns (see `PTYProcess`). The two are joined by libghostty's host-managed
/// backend: bytes the program writes are handed to `session.receive`, and
/// bytes libghostty produces from key presses arrive in the session's `write`
/// closure and go to the PTY.
@MainActor
final class GhosttySurface: TerminalSurface {
    /// One controller for every surface in the app, as libghostty has one
    /// app with a surface for each terminal: the main one, and the editor
    /// pane beside it. Made with the first surface.
    private static var shared: TerminalController?
    /// How the shared controller was last told the terminals look.
    private static var sharedAppearance: TerminalAppearance?

    private let terminalView: DropAwareTerminalView
    private let controller: TerminalController
    private let session: InMemoryTerminalSession
    private let link: Link
    private let delegateProxy = DelegateProxy()
    private var pendingCommand: TerminalCommand?
    private var started = false

    var onExit: ((TerminalExit) -> Void)?
    private(set) var grid: TerminalGrid?
    var onGridChange: ((TerminalGrid) -> Void)?
    var onTitleChange: ((String) -> Void)? {
        get { delegateProxy.onTitleChange }
        set { delegateProxy.onTitleChange = newValue }
    }

    var onOpenLink: ((String) -> Void)? {
        get { delegateProxy.onOpenLink }
        set { delegateProxy.onOpenLink = newValue }
    }

    var onDropFiles: (([URL]) -> Void)? {
        get { terminalView.onDropFiles }
        set { terminalView.onDropFiles = newValue }
    }

    var view: NSView { terminalView }

    init(appearance: TerminalAppearance) {
        let link = Link()
        self.link = link
        session = InMemoryTerminalSession(
            write: { data in link.process?.write(data) },
            resize: { viewport in link.viewportChanged(viewport) },
            // The core repaints on every resize and only reads columns and rows.
            suppressesPixelOnlyResizes: true
        )
        if let shared = Self.shared {
            controller = shared
            // The surfaces look alike: the latest appearance is everyone's.
            // One that asks for what is already so changes nothing, and the
            // terminals that are running are not disturbed.
            if appearance != Self.sharedAppearance {
                _ = shared.setTerminalConfiguration(Self.configuration(for: appearance))
                Self.sharedAppearance = appearance
            }
        } else {
            controller = TerminalController(configuration: Self.configuration(for: appearance))
            Self.shared = controller
            Self.sharedAppearance = appearance
        }
        terminalView = DropAwareTerminalView(frame: .zero)
        terminalView.delegate = delegateProxy
        terminalView.controller = controller
        terminalView.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
        link.onFirstViewport = { [weak self] in
            Task { @MainActor in self?.startPendingCommand() }
        }
        delegateProxy.onMetrics = { [weak self] metrics in
            self?.measured(metrics)
        }
    }

    /// libghostty has measured the view: what the rest of the app may know
    /// of it is the grid, in cells, and the width of a cell.
    private func measured(_ metrics: TerminalGridMetrics) {
        guard metrics.columns > 0, metrics.rows > 0, metrics.cellWidthPixels > 0 else { return }
        let scale = terminalView.window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        let measured = TerminalGrid(
            columns: Int(metrics.columns), rows: Int(metrics.rows),
            cellWidthPixels: Int(metrics.cellWidthPixels), scale: scale)
        guard measured != grid else { return }
        grid = measured
        onGridChange?(measured)
    }

    func start(_ command: TerminalCommand) {
        guard !started, pendingCommand == nil else { return }
        pendingCommand = command
        // Wait for the first real grid size, so the program draws once at the
        // right size instead of at a guess and again after a resize.
        if link.viewport != nil {
            startPendingCommand()
        }
    }

    private func startPendingCommand() {
        guard !started, let command = pendingCommand, let viewport = link.viewport else { return }
        started = true
        pendingCommand = nil
        let session = session
        let startedAt = DispatchTime.now()
        let passOutput: @Sendable (Data) -> Void = { data in session.receive(data) }
        let passExit: @Sendable (TerminalExit) -> Void = { [weak self] exit in
            let elapsed = DispatchTime.now().uptimeNanoseconds - startedAt.uptimeNanoseconds
            let code: UInt32 = if case let .exited(code) = exit { UInt32(code) } else { 1 }
            session.finish(exitCode: code, runtimeMilliseconds: elapsed / 1_000_000)
            Task { @MainActor in self?.onExit?(exit) }
        }
        do {
            if command.userScript {
                link.process = try PTYUserScript(
                    command: command, size: viewport, onOutput: passOutput, onExit: passExit)
            } else {
                link.process = try PTYProcess(command: command, size: viewport, onOutput: passOutput, onExit: passExit)
            }
            // A resize may have landed between reading the viewport and now.
            if let latest = link.viewport, latest != viewport {
                link.process?.resize(latest)
            }
        } catch {
            onExit?(.failedToStart(reason: String(describing: error)))
        }
    }

    func sendText(_ text: String) {
        _ = terminalView.paste(text: text)
    }

    func focus() {
        terminalView.window?.makeFirstResponder(terminalView)
    }

    func setAppearance(_ appearance: TerminalAppearance) {
        _ = controller.setTerminalConfiguration(Self.configuration(for: appearance))
        Self.sharedAppearance = appearance
    }

    func visibleText() -> String? {
        session.readViewportText()
    }

    func sendKeys(_ text: String) {
        link.process?.write(Data(text.utf8))
    }

    func terminate() {
        if let process = link.process {
            process.hangUp()
        } else {
            abandonStart()
        }
    }

    func kill() {
        if let process = link.process {
            process.endNow()
        } else {
            abandonStart()
        }
    }

    /// Gives up on a program that is waiting for the terminal's first size
    /// before it starts: whoever asked for it is waiting to hear how it
    /// ended, and it will not now begin.
    private func abandonStart() {
        guard !started, pendingCommand != nil else { return }
        started = true
        pendingCommand = nil
        onExit?(.failedToStart(reason: "stopped before it started"))
    }

    private static func configuration(for appearance: TerminalAppearance) -> TerminalConfiguration {
        TerminalConfiguration { builder in
            if let family = appearance.fontFamily {
                builder.withFontFamily(family)
            }
            builder.withFontSize(appearance.fontSize)
            builder.withBackground(appearance.background)
            builder.withForeground(appearance.foreground)
            builder.withWindowPaddingX(0)
            builder.withWindowPaddingY(0)
            // Ghostty's own shortcuts (cmd-c, cmd-a, cmd-z, cmd-enter and the
            // rest) would be taken here, before the app's menus or the
            // program could see them. The program inside is the whole app:
            // every key is its or the menus', so the terminal keeps none.
            builder.withCustom("keybind", "clear")
        }
    }
}

/// The terminal view, except that dropped files can be claimed by whoever
/// owns the surface instead of being pasted as paths. Text, links and
/// pictures without a file of their own drop as libghostty has them.
private final class DropAwareTerminalView: TerminalView {
    var onDropFiles: (([URL]) -> Void)?

    override func performDragOperation(_ sender: any NSDraggingInfo) -> Bool {
        let files =
            (sender.draggingPasteboard.readObjects(
                forClasses: [NSURL.self],
                options: [.urlReadingFileURLsOnly: true]
            ) as? [URL]) ?? []
        guard let onDropFiles, !files.isEmpty else {
            return super.performDragOperation(sender)
        }
        onDropFiles(files)
        return true
    }
}

/// What the session's callbacks, which run off the main thread, share with
/// the surface.
private final class Link: @unchecked Sendable {
    private let lock = NSLock()
    private var storedProcess: (any TerminalProgram)?
    private var storedViewport: PTYProcess.Size?
    private var firstViewportHandler: (@Sendable () -> Void)?

    var process: (any TerminalProgram)? {
        get { lock.withLock { storedProcess } }
        set { lock.withLock { storedProcess = newValue } }
    }

    var viewport: PTYProcess.Size? {
        lock.withLock { storedViewport }
    }

    var onFirstViewport: (@Sendable () -> Void)? {
        get { lock.withLock { firstViewportHandler } }
        set { lock.withLock { firstViewportHandler = newValue } }
    }

    func viewportChanged(_ viewport: InMemoryTerminalViewport) {
        guard viewport.columns > 0, viewport.rows > 0 else { return }
        let size = PTYProcess.Size(
            columns: viewport.columns,
            rows: viewport.rows,
            widthPixels: UInt16(clamping: viewport.widthPixels),
            heightPixels: UInt16(clamping: viewport.heightPixels)
        )
        let (process, first) = lock.withLock {
            let first = storedViewport == nil ? firstViewportHandler : nil
            storedViewport = size
            return (storedProcess, first)
        }
        process?.resize(size)
        first?()
    }
}

@MainActor
private final class DelegateProxy: TerminalSurfaceTitleDelegate, TerminalSurfaceOpenURLHandlingDelegate,
    TerminalSurfaceGridResizeDelegate
{
    var onTitleChange: ((String) -> Void)?
    var onOpenLink: ((String) -> Void)?
    var onMetrics: ((TerminalGridMetrics) -> Void)?

    /// On the main actor, as soon as the view has a new size: before the
    /// program inside has been told of it.
    func terminalDidResize(_ size: TerminalGridMetrics) {
        onMetrics?(size)
    }

    func terminalDidChangeTitle(_ title: String) {
        onTitleChange?(title)
    }

    /// Every clicked link is ours to decide. libghostty's own opener is never
    /// used: it would start `open` itself, outside the app's say.
    func terminalShouldHandleOpenURL(_ url: String, kind _: TerminalOpenURLKind) -> Bool {
        onOpenLink?(url)
        return true
    }
}
