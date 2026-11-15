import AppKit
import Bridge
import TerminalKit

/// The main window: the terminal surface running the core, a toolbar built
/// from the core's commands, and a second pane for native viewers.
///
/// Nothing here knows about mail. What the window shows around the terminal
/// comes over the bridge as command names, a title and a count.
@MainActor
final class MainWindowController: NSWindowController, NSWindowDelegate, NSToolbarDelegate {
    let commands = CommandCenter()
    /// Called when the core has announced its commands, so the menu bar can
    /// be rebuilt around them.
    var onCommandsChanged: (() -> Void)?

    private let surface: any TerminalSurface
    private let options: LaunchOptions
    private let splitView = NSSplitView()
    private var bridge: BridgeServer?
    private(set) var previewPane: PreviewPane?
    private var keyMonitor: Any?

    /// Hands an address to the user's browser or mail program. A scripted
    /// run replaces it, so that a check opens nothing.
    var linkOpener: (URL) -> Void = { NSWorkspace.shared.open($0) }

    /// Puts text on the system clipboard. A scripted run replaces it, so
    /// that a check leaves the user's clipboard as it was.
    var clipboardWriter: (String) -> Void = { text in
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    /// Asks the user for files and hands them to the completion; an empty
    /// list means they chose none. A scripted run replaces it, so that a
    /// check never waits on a panel.
    var filePicker: (NSWindow?, @escaping ([URL]) -> Void) -> Void = { window, done in
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = true
        panel.resolvesAliases = true
        let finish: (NSApplication.ModalResponse) -> Void = { response in
            done(response == .OK ? panel.urls : [])
        }
        if let window {
            panel.beginSheetModal(for: window, completionHandler: finish)
        } else {
            finish(panel.runModal())
        }
    }

    private(set) var coreIsRunning = false
    /// The app asked the core to stop, so its exit is not a failure.
    private var stopping = false
    /// AppKit is waiting to hear that it may terminate.
    private var terminationPending = false

    init(options: LaunchOptions) {
        self.options = options
        surface = makeTerminalSurface()

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1180, height: 760),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Quill"
        window.minSize = NSSize(width: 480, height: 320)
        window.setFrameAutosaveName("QuillMainWindow")
        window.center()
        super.init(window: window)
        window.delegate = self

        splitView.isVertical = true
        splitView.dividerStyle = .thin
        splitView.addArrangedSubview(surface.view)
        window.contentView = splitView

        let toolbar = NSToolbar(identifier: "QuillMain")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        window.toolbar = toolbar
        window.toolbarStyle = .unifiedCompact

        commands.send = { [weak self] message in
            self?.bridge?.send(message)
        }
        surface.onExit = { [weak self] exit in
            self?.coreDidExit(exit)
        }
        // A link clicked in the terminal.
        surface.onOpenLink = { [weak self] address in
            self?.openLink(address)
        }
        // Files dropped on the terminal go to the core as files, which it
        // can read, not as paths, which inside the sandbox it cannot.
        surface.onDropFiles = { [weak self] files in
            self?.deliver(files: files)
        }
        // esc in the preview pane closes it. The terminal handles its own esc.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.keyCode == 53, previewHasFocus else { return event }
            closePreview()
            return nil
        }
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    override func showWindow(_ sender: Any?) {
        super.showWindow(sender)
        surface.focus()
        startCore()
        DebugScript.runIfAsked(options, on: self)
    }

    // MARK: - The core

    private func startCore() {
        guard let core = CoreLocator.coreURL() else {
            presentFailure("The Quill core is missing from the app bundle.")
            return
        }
        var environment = options.coreEnvironment
        do {
            // A short path: a socket's has little more than a hundred bytes.
            let directory = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("q-\(ProcessInfo.processInfo.processIdentifier)")
            let bridge = try BridgeServer(directory: directory)
            bridge.start(
                onMessage: { [weak self] in self?.handle($0) },
                onProblem: { NSLog("%@", $0) },
                onDisconnect: {}
            )
            self.bridge = bridge
            environment["QUILL_SHELL_SOCKET"] = bridge.socketPath
        } catch {
            // The core runs without menus rather than not at all, and says so.
            NSLog("Quill: no bridge: %@", String(describing: error))
        }

        var arguments = ["tui"]
        if options.useDemoMailbox {
            arguments.append("--demo")
        }
        coreIsRunning = true
        surface.start(
            TerminalCommand(
                executable: core,
                arguments: arguments,
                environment: CoreLocator.environment(extra: environment),
                rawTerminal: true
            ))
    }

    private func handle(_ message: CoreToShell) {
        switch message {
        case let .commandsList(list):
            commands.setCommands(list)
            rebuildToolbar()
            onCommandsChanged?()
        case let .stateUpdate(state):
            commands.setEnabled(state.enabled)
            window?.title = state.title.isEmpty ? "Quill" : state.title
            window?.subtitle = state.unread > 0 ? "\(state.unread) unread" : ""
            NSApp.dockTile.badgeLabel = state.unread > 0 ? String(state.unread) : nil
        case let .panePreview(preview):
            showPreview(URL(fileURLWithPath: preview.path), title: preview.title)
        case .paneClose:
            closePreview()
        case let .linkOpen(open):
            openLink(open.url)
        case let .clipboardWrite(text):
            clipboardWriter(text)
        case .filesPick:
            filePicker(window) { [weak self] files in
                self?.deliver(files: files)
                self?.surface.focus()
            }
        }
    }

    /// Gives the core files the user chose or dropped.
    ///
    /// The user's choice lets this process read them; the core, a separate
    /// process in the same sandbox, was given no such leave. So each file is
    /// copied into the app's own temporary folder, which both can reach, and
    /// the core removes the copy once it has read it. What the files are for
    /// is the core's business.
    func deliver(files: [URL]) {
        let folder = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("files-\(UUID().uuidString.prefix(8))")
        var copies: [String] = []
        for file in files where file.isFileURL {
            var isDirectory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: file.path, isDirectory: &isDirectory),
                !isDirectory.boolValue
            else {
                NSLog("Quill: left out something that is not a file")
                continue
            }
            do {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                // Two files of one name from different folders both go.
                var copy = folder.appendingPathComponent(file.lastPathComponent)
                var attempt = 1
                while FileManager.default.fileExists(atPath: copy.path) {
                    attempt += 1
                    copy = folder.appendingPathComponent("\(attempt)")
                        .appendingPathComponent(file.lastPathComponent)
                    try FileManager.default.createDirectory(
                        at: copy.deletingLastPathComponent(), withIntermediateDirectories: true)
                }
                try FileManager.default.copyItem(at: file, to: copy)
                copies.append(copy.path)
            } catch {
                NSLog("Quill: a file could not be handed over: %@", String(describing: error))
            }
        }
        guard let message = FilesDropped(paths: copies, temporary: true) else { return }
        bridge?.send(.filesDropped(message))
    }

    /// Opens a web or mail address the way the user's Mac opens them.
    /// Whoever asks, the core or a click in the terminal, nothing else is
    /// opened: a message cannot name a file or another app's scheme.
    private func openLink(_ address: String) {
        guard let url = Links.openable(address) else {
            NSLog("Quill: did not open a link that is not an http, https or mailto address")
            return
        }
        linkOpener(url)
    }

    private func coreDidExit(_ exit: TerminalExit) {
        coreIsRunning = false
        bridge?.close()
        bridge = nil
        NSApp.dockTile.badgeLabel = nil
        if terminationPending {
            terminationPending = false
            NSApp.reply(toApplicationShouldTerminate: true)
            return
        }
        if stopping {
            NSApp.terminate(nil)
            return
        }
        switch exit {
        case .exited(code: 0):
            // The user quit from inside the terminal UI.
            NSApp.terminate(nil)
        case let .exited(code):
            presentFailure("The Quill core stopped unexpectedly (exit code \(code)).")
        case let .signaled(signal):
            presentFailure("The Quill core stopped unexpectedly (signal \(signal)).")
        case let .failedToStart(reason):
            presentFailure("The Quill core could not start: \(reason)")
        }
    }

    /// Asks the core to flush and exit. If it does not, it is hung up on.
    func stopCore() {
        guard coreIsRunning, !stopping else { return }
        stopping = true
        bridge?.send(.appWillQuit)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard let self, coreIsRunning else { return }
            surface.terminate()
        }
    }

    /// For `applicationShouldTerminate`: stops the core and reports whether
    /// the app must wait for it.
    func mustWaitForCoreBeforeTerminating() -> Bool {
        guard coreIsRunning else { return false }
        terminationPending = true
        stopCore()
        // Never hold the app open for a core that will not go.
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            guard let self, terminationPending else { return }
            terminationPending = false
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return true
    }

    private func presentFailure(_ message: String) {
        if options.dumpScreenTo != nil {
            // Running under a script: report and leave, never block on a
            // dialog. Whatever the core printed before it died is on screen.
            FileHandle.standardError.write(Data("quill: \(message)\n".utf8))
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [self] in
                DebugScript.dump(self, to: options.dumpScreenTo)
                exit(1)
            }
            return
        }
        let alert = NSAlert()
        alert.alertStyle = .critical
        alert.messageText = "Quill has to close"
        alert.informativeText = message
        alert.addButton(withTitle: "Quit")
        alert.runModal()
        NSApp.terminate(nil)
    }

    func windowWillClose(_: Notification) {
        stopCore()
    }

    // MARK: - Toolbar

    private func rebuildToolbar() {
        guard let toolbar = window?.toolbar else { return }
        while !toolbar.items.isEmpty {
            toolbar.removeItem(at: 0)
        }
        for (index, identifier) in toolbarDefaultItemIdentifiers(toolbar).enumerated() {
            toolbar.insertItem(withItemIdentifier: identifier, at: index)
        }
    }

    func toolbarDefaultItemIdentifiers(_: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.flexibleSpace] + commands.toolbarCommands.map { NSToolbarItem.Identifier($0.name) }
    }

    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar) + [.space]
    }

    func toolbar(
        _: NSToolbar,
        itemForItemIdentifier identifier: NSToolbarItem.Identifier,
        willBeInsertedIntoToolbar _: Bool
    ) -> NSToolbarItem? {
        commands.makeToolbarItem(for: identifier.rawValue)
    }

    // MARK: - Panes

    private func showPreview(_ url: URL, title: String) {
        let pane: PreviewPane
        if let existing = previewPane {
            pane = existing
        } else {
            pane = PreviewPane(frame: .zero)
            pane.onClose = { [weak self] in self?.closePreview() }
            previewPane = pane
            splitView.addArrangedSubview(pane)
            splitView.layoutSubtreeIfNeeded()
            // The terminal keeps the larger share.
            splitView.setPosition(splitView.bounds.width * 0.58, ofDividerAt: 0)
        }
        pane.show(url, title: title)
        window?.makeFirstResponder(pane.focusTarget)
    }

    private func closePreview() {
        guard let pane = previewPane else { return }
        previewPane = nil
        pane.removeFromSuperview()
        surface.focus()
    }

    private var previewHasFocus: Bool {
        guard let pane = previewPane, let responder = window?.firstResponder as? NSView else {
            return false
        }
        return responder.isDescendant(of: pane)
    }

    @objc func focusNextPane(_: Any?) {
        guard let pane = previewPane else { return }
        if previewHasFocus {
            surface.focus()
        } else {
            window?.makeFirstResponder(pane.focusTarget)
        }
    }

    /// With two panes, the previous one is the other one.
    @objc func focusPreviousPane(_ sender: Any?) {
        focusNextPane(sender)
    }

    @objc func closePreviewPane(_: Any?) {
        closePreview()
    }

    // MARK: - Edit menu

    // The core edits text of its own inside the terminal, with a selection
    // the terminal view knows nothing about. While the core says one of
    // these commands applies, the menu item runs it there. Otherwise the
    // item does what it does in any Mac app, on the view with the focus:
    // text selected with the mouse in the terminal, or the preview pane.

    @objc func editCut(_ sender: Any?) {
        runInCoreOrFocusedView("edit.cut", #selector(NSText.cut(_:)), sender)
    }

    @objc func editCopy(_ sender: Any?) {
        runInCoreOrFocusedView("edit.copy", #selector(NSText.copy(_:)), sender)
    }

    @objc func editSelectAll(_ sender: Any?) {
        runInCoreOrFocusedView("edit.select_all", #selector(NSText.selectAll(_:)), sender)
    }

    private func runInCoreOrFocusedView(_ command: String, _ native: Selector, _ sender: Any?) {
        if !previewHasFocus, commands.isEnabled(command) {
            commands.run(named: command)
        } else {
            NSApp.sendAction(native, to: nil, from: sender)
        }
    }

    // MARK: - For the debug script

    var terminalText: String { surface.visibleText() ?? "<no surface>" }

    func typeIntoTerminal(_ text: String) {
        surface.sendText(text)
    }
}

extension MainWindowController: NSMenuItemValidation {
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(focusNextPane(_:)), #selector(focusPreviousPane(_:)), #selector(closePreviewPane(_:)):
            previewPane != nil
        default:
            true
        }
    }
}
