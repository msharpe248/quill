import AppKit
import Bridge
import Notifications
import Panes
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
    /// The second pane, while one is showing. There is never more than one:
    /// showing another replaces it.
    private(set) var pane: PaneView?
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

    /// Opens a file in the application the user's Mac opens its kind with.
    /// A scripted run replaces it, so that a check starts no other program.
    var fileOpener: (URL) -> Void = { NSWorkspace.shared.open($0) }

    /// Asks the user where to save a file, offering a name, and hands the
    /// place to the completion; `nil` means they cancelled. The panel has
    /// already asked about replacing a file that is there. A scripted run
    /// replaces it, so that a check never waits on a panel.
    var savePanel: (NSWindow?, String, @escaping (URL?) -> Void) -> Void = { window, name, done in
        let panel = NSSavePanel()
        panel.nameFieldStringValue = name
        panel.canCreateDirectories = true
        let finish: (NSApplication.ModalResponse) -> Void = { response in
            done(response == .OK ? panel.url : nil)
        }
        if let window {
            panel.beginSheetModal(for: window, completionHandler: finish)
        } else {
            finish(panel.runModal())
        }
    }

    /// Shows the notifications the core asks for. A scripted run puts a
    /// recorder here, so that a check shows nothing and never asks the user
    /// whether Quill may. Otherwise the Mac's own are put here once the
    /// window is up.
    var notifier: (any Notifier)?

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
        // While a pane has the keyboard, esc closes it and the pane has
        // first call on the few keys it uses; every other key goes on to
        // the viewer inside it. The terminal handles its own keys. Only
        // this window's keys are looked at: a player gone full screen has a
        // window of its own, where esc means leave full screen.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.window === window, let pane, paneHasFocus,
                let key = PaneKey(event: event)
            else { return event }
            if key == .escape {
                closePane()
                return nil
            }
            return pane.handleKey(key) ? nil : event
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
        if notifier == nil {
            // Now, not when the first notification is posted: a click on
            // one left from an earlier run is only heard by an app that is
            // listening before it has finished launching. Nothing is asked
            // of the user until there is a notification to show.
            let system = SystemNotifier.forThisApp()
            system?.onResponse = { [weak self] id, actionIdentifier in
                self?.notificationUsed(id: id, actionIdentifier: actionIdentifier)
            }
            notifier = system
        }
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
            let items = preview.items.map { PreviewItem(url: URL(fileURLWithPath: $0.path), title: $0.title) }
            guard let deck = PreviewDeck(items: items, index: preview.index) else { break }
            if let showing = pane as? PreviewPane {
                // Other files in the pane that is there: nothing flickers.
                showing.show(deck)
                focus(showing)
            } else {
                show(PreviewPane(deck: deck))
            }
        case let .panePlay(file):
            show(PlayerPane(file: URL(fileURLWithPath: file.path), title: file.title))
        case let .paneWeb(page):
            show(
                WebPane(file: URL(fileURLWithPath: page.path), title: page.title, allowsRemoteContent: page.remote) {
                    [weak self] address in
                    // The same door every other link goes through.
                    self?.openLink(address.absoluteString)
                })
        case let .thumbnailMake(make):
            makeThumbnail(make)
        case let .fileOpen(path):
            openFile(URL(fileURLWithPath: path))
        case let .filesSave(save):
            saveCopy(of: URL(fileURLWithPath: save.path), offering: save.name)
        case .paneClose:
            closePane()
        case let .linkOpen(open):
            openLink(open.url)
        case let .clipboardWrite(text):
            clipboardWriter(text)
        case .filesPick:
            filePicker(window) { [weak self] files in
                self?.deliver(files: files)
                self?.surface.focus()
            }
        case let .notifyPost(notification):
            notifier?.post(notification)
        case let .notifyClear(clear):
            notifier?.clear(ids: clear.ids)
        }
    }

    /// Tells the core what the user did with a notification, given the
    /// Mac's name for it. A click on the notification itself also brings
    /// the window forward; a button does its work where the window is.
    /// Putting a notification away tells the core nothing.
    func notificationUsed(id: String, actionIdentifier: String) {
        guard let bridge, let used = NotificationResponse.used(id: id, actionIdentifier: actionIdentifier) else {
            return
        }
        if used.action == nil {
            bringForward()
        }
        note(used.action.map { "\(used.id) > \($0)" } ?? used.id, in: &notificationsUsed)
        bridge.send(.notifyUsed(used))
    }

    /// Passes on to the core the mail addresses among what the Mac asked
    /// Quill to open: a `mailto:` link clicked in another application.
    /// Nothing else is passed on, whatever it is. An address that arrives
    /// before the core has connected waits in the bridge until it has.
    func open(urls: [URL]) {
        guard let bridge else { return }
        for url in urls {
            guard let open = UrlOpen(handedOver: url.absoluteString) else {
                NSLog("Quill: left alone an address that is not a mailto address, or is too long")
                continue
            }
            bringForward()
            note(open.url, in: &urlsPassedOn)
            bridge.send(.urlOpen(open))
        }
    }

    private func bringForward() {
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
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

    /// Opens a file the core wrote out for the user in the application
    /// their Mac opens its kind with. Only a file in the app's temporary
    /// folder: the core has no business opening anything else, and a path
    /// that names something else is a mistake or worse.
    private func openFile(_ file: URL) {
        var isFolder: ObjCBool = false
        guard TemporaryFolder.contains(file),
            FileManager.default.fileExists(atPath: file.path, isDirectory: &isFolder), !isFolder.boolValue
        else {
            NSLog("Quill: did not open a file that is not in the app's temporary folder")
            return
        }
        fileOpener(file)
    }

    /// Makes the picture off the main thread and tells the core how it
    /// went. The answer is always sent: the core is waiting on it.
    private func makeThumbnail(_ make: ThumbnailMake) {
        Task { [weak self] in
            let made = await Thumbnails.make(
                file: URL(fileURLWithPath: make.path), out: URL(fileURLWithPath: make.out), size: make.size)
            guard let self else { return }
            if !made {
                NSLog("Quill: thumbnail %ld could not be made", make.id)
            }
            note("\(make.id) \(made ? "made" : "failed")", in: &thumbnailResults)
            bridge?.send(.thumbnailMade(id: make.id, ok: made))
        }
    }

    /// Asks the user where a copy of a file should go, puts it there, and
    /// tells the core what became of it. The answer is always sent.
    private func saveCopy(of file: URL, offering name: String) {
        savePanel(window, FileSaving.offeredName(name)) { [weak self] destination in
            guard let destination else {
                self?.finishSave(name: name, destination: nil, saved: false)
                return
            }
            Task { [weak self] in
                let saved = await Self.copy(file, to: destination)
                self?.finishSave(name: name, destination: destination, saved: saved)
            }
        }
    }

    /// Off the main thread: a big file takes a while to copy.
    private nonisolated static func copy(_ file: URL, to destination: URL) async -> Bool {
        do {
            try FileSaving.copy(file, to: destination)
            return true
        } catch {
            NSLog("Quill: a file could not be saved: %@", String(describing: error))
            return false
        }
    }

    private func finishSave(name: String, destination: URL?, saved: Bool) {
        if let destination {
            note("\(name) -> \(destination.path) (\(saved ? "saved" : "failed"))", in: &saveResults)
        } else {
            note("\(name) (cancelled)", in: &saveResults)
        }
        bridge?.send(.filesSaved(name: destination?.lastPathComponent ?? name, saved: saved))
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
        // Nothing goes on playing behind a closed window.
        pane?.tearDown()
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

    /// Puts a pane beside the terminal, in place of any pane already there,
    /// and gives it the keyboard.
    private func show(_ newPane: PaneView) {
        newPane.onClose = { [weak self] in self?.closePane() }
        // A pane that replaces another takes its place exactly; the first
        // pane leaves the terminal the larger share.
        var divider = splitView.bounds.width * 0.58
        if let old = pane {
            divider = surface.view.frame.width
            old.tearDown()
            old.removeFromSuperview()
        }
        pane = newPane
        splitView.addArrangedSubview(newPane)
        splitView.layoutSubtreeIfNeeded()
        splitView.setPosition(divider, ofDividerAt: 0)
        focus(newPane)
    }

    private func closePane() {
        guard let old = pane else { return }
        pane = nil
        old.tearDown()
        old.removeFromSuperview()
        surface.focus()
    }

    private func focus(_ pane: PaneView) {
        // A viewer that will not take the keyboard leaves it with the pane
        // around it, where esc still closes the pane.
        if window?.makeFirstResponder(pane.focusTarget) != true {
            window?.makeFirstResponder(pane)
        }
    }

    private var paneHasFocus: Bool {
        guard let pane, let responder = window?.firstResponder as? NSView else {
            return false
        }
        return responder.isDescendant(of: pane)
    }

    @objc func focusNextPane(_: Any?) {
        guard let pane else { return }
        if paneHasFocus {
            surface.focus()
        } else {
            focus(pane)
        }
    }

    /// With two panes, the previous one is the other one.
    @objc func focusPreviousPane(_ sender: Any?) {
        focusNextPane(sender)
    }

    @objc func closePane(_: Any?) {
        closePane()
    }

    // MARK: - Edit menu

    // The core edits text of its own inside the terminal, with a selection
    // the terminal view knows nothing about. While the core says one of
    // these commands applies, the menu item runs it there. Otherwise the
    // item does what it does in any Mac app, on the view with the focus:
    // text selected with the mouse in the terminal, or the second pane.

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
        if !paneHasFocus, commands.isEnabled(command) {
            commands.run(named: command)
        } else {
            NSApp.sendAction(native, to: nil, from: sender)
        }
    }

    // MARK: - For the debug script

    var terminalText: String { surface.visibleText() ?? "<no surface>" }

    /// What became of the latest `files.save` requests, in order.
    private(set) var saveResults: [String] = []

    /// What became of the latest `thumbnail.make` requests, in order.
    private(set) var thumbnailResults: [String] = []

    /// The notifications the user clicked, in order: `id`, or `id > action`
    /// for one of its buttons.
    private(set) var notificationsUsed: [String] = []

    /// The mail addresses passed on to the core, in order.
    private(set) var urlsPassedOn: [String] = []

    /// Keeps the last hundred: the app may run for weeks.
    private func note(_ line: String, in log: inout [String]) {
        log.append(line)
        log.removeFirst(max(log.count - 100, 0))
    }

    func typeIntoTerminal(_ text: String) {
        surface.sendText(text)
    }
}

extension MainWindowController: NSMenuItemValidation {
    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        switch item.action {
        case #selector(focusNextPane(_:)), #selector(focusPreviousPane(_:)), #selector(closePane(_:)):
            pane != nil
        default:
            true
        }
    }
}
