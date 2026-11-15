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
    private let terminalView: TerminalView
    private let controller: TerminalController
    private let session: InMemoryTerminalSession
    private let link: Link
    private let delegateProxy = DelegateProxy()
    private var pendingCommand: TerminalCommand?
    private var started = false

    var onExit: ((TerminalExit) -> Void)?
    var onTitleChange: ((String) -> Void)? {
        get { delegateProxy.onTitleChange }
        set { delegateProxy.onTitleChange = newValue }
    }

    var onOpenLink: ((String) -> Void)? {
        get { delegateProxy.onOpenLink }
        set { delegateProxy.onOpenLink = newValue }
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
        controller = TerminalController(configuration: Self.configuration(for: appearance))
        terminalView = TerminalView(frame: .zero)
        terminalView.delegate = delegateProxy
        terminalView.controller = controller
        terminalView.configuration = TerminalSurfaceOptions(backend: .inMemory(session))
        link.onFirstViewport = { [weak self] in
            Task { @MainActor in self?.startPendingCommand() }
        }
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
        do {
            link.process = try PTYProcess(
                command: command,
                size: viewport,
                onOutput: { data in session.receive(data) },
                onExit: { [weak self] exit in
                    let elapsed = DispatchTime.now().uptimeNanoseconds - startedAt.uptimeNanoseconds
                    let code: UInt32 = if case let .exited(code) = exit { UInt32(code) } else { 1 }
                    session.finish(exitCode: code, runtimeMilliseconds: elapsed / 1_000_000)
                    Task { @MainActor in self?.onExit?(exit) }
                }
            )
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
    }

    func visibleText() -> String? {
        session.readViewportText()
    }

    func terminate() {
        link.process?.hangUp()
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
        }
    }
}

/// What the session's callbacks, which run off the main thread, share with
/// the surface.
private final class Link: @unchecked Sendable {
    private let lock = NSLock()
    private var storedProcess: PTYProcess?
    private var storedViewport: PTYProcess.Size?
    private var firstViewportHandler: (@Sendable () -> Void)?

    var process: PTYProcess? {
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
private final class DelegateProxy: TerminalSurfaceTitleDelegate, TerminalSurfaceOpenURLHandlingDelegate {
    var onTitleChange: ((String) -> Void)?
    var onOpenLink: ((String) -> Void)?

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
