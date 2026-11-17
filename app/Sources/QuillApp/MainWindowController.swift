import AppKit
import Bridge
import Editor
import Forms
import Notifications
import Panes
import Spelling
import TerminalKit
import Themes

/// The main window: the terminal surface running the core, a toolbar built
/// from the core's commands, and a second pane for native viewers or for
/// Vim.
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
    /// The core's terminal, with an editor's laid over it while a message
    /// is being written in one.
    private let terminalStack: TerminalStack
    private let options: LaunchOptions
    private let splitView = DividerSplitView()
    private var bridge: BridgeServer?
    /// The second pane, while one is showing. There is never more than one:
    /// showing another replaces it.
    private(set) var pane: PaneView?
    private var keyMonitor: Any?

    /// The reading pane's turn in the second pane's place: the page the
    /// core last asked it to show, while it wants one shown at all.
    private(set) var readingSlot = ReadingSlot<ReadingShow>()
    /// A reading pane loading its page out of sight, to take the place of
    /// the one on show once there is something to see in it.
    private var incomingReading: WebPane?
    /// How many pages the reading pane has been asked to show.
    private(set) var readingRequests = 0
    /// How many columns the core says the terminal keeps beside the
    /// reading pane, once it has said.
    private(set) var readingColumnsAsked: Int?
    /// Where the user left the divider between the terminal and the
    /// reading pane. A scripted run keeps it for the run only, and reads
    /// and writes nothing of the user's.
    let dividerMemory: any DividerMemory
    /// The columns the core was last told the window has room for.
    private(set) var windowColumnsSent: Int?

    /// How the terminals look: the core's own, and the editor's beside it.
    /// The colours are the ones the core last said its theme has, so that
    /// an editor is not a light box beside a dark Quill.
    private var terminalAppearance = TerminalAppearance()

    /// The editor that has the text of the message being written, while
    /// one is running: in a terminal of its own, laid over the core's.
    private var editor: RunningEditor?
    /// Where the core last said the editor goes, and whether it has the
    /// keys. Kept for an editor that has yet to start.
    private(set) var editorPlace = EditorPlace.hidden
    private var mouseMonitor: Any?
    /// The one place there is for an editor, and what became of the last.
    private(set) var editorSlot = EditorSlot()
    /// What waits for the editor to end: the core being told to stop.
    private var afterEditor: (() -> Void)?
    /// The folder the core reads `config.toml` from.
    let configFolder: URL

    /// Shows a file or folder in the Finder. A scripted run replaces it, so
    /// that a check never brings the Finder forward.
    var revealer: (URL) -> Void = { NSWorkspace.shared.activateFileViewerSelecting([$0]) }

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

    /// Asks the user for a theme file kept anywhere, and hands it to the
    /// completion; `nil` means they thought better of it. A scripted run
    /// replaces it, so that a check never waits on a panel.
    var themeFilePicker: (NSWindow?, @escaping (URL?) -> Void) -> Void = { window, done in
        let panel = NSOpenPanel()
        panel.message = "Choose a theme made for Ghostty to copy into Quill"
        panel.prompt = "Import"
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.resolvesAliases = true
        // Ghostty keeps the user's own themes in a folder whose name
        // starts with a dot, in their own home folder and not the app's.
        panel.showsHiddenFiles = true
        if let home = GhosttyThemes.realHome() {
            let own = home.appendingPathComponent(".config/ghostty/themes", isDirectory: true)
            panel.directoryURL = own
        }
        let finish: (NSApplication.ModalResponse) -> Void = { response in
            done(response == .OK ? panel.url : nil)
        }
        if let window {
            panel.beginSheetModal(for: window, completionHandler: finish)
        } else {
            finish(panel.runModal())
        }
    }

    /// Tells the user that something they asked of the shell could not be
    /// done: a title and a sentence. A scripted run replaces it, so that a
    /// check never waits on an alert.
    var problemReporter: (NSWindow?, String, String) -> Void = { window, title, text in
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = title
        alert.informativeText = text
        if let window {
            alert.beginSheetModal(for: window)
        } else {
            alert.runModal()
        }
    }

    /// Shows the notifications the core asks for. A scripted run puts a
    /// recorder here, so that a check shows nothing and never asks the user
    /// whether Quill may. Otherwise the Mac's own are put here once the
    /// window is up.
    var notifier: (any Notifier)?

    /// Checks spelling for the core: the Mac's own checker, which shows
    /// nothing and changes nothing, so a scripted run may use it too. A
    /// check that must not depend on the Mac's dictionaries puts another
    /// here.
    var spellChecker: any SpellChecker {
        get { spelling.checker }
        set { spelling.checker = newValue }
    }

    /// Puts the core's questions to the checker in turn and sends every
    /// answer back.
    private(set) lazy var spelling = SpellingService(checker: SystemSpellChecker()) { [weak self] answer in
        self?.bridge?.send(answer)
    }

    /// The form the core has the window show in a sheet, while it has one:
    /// adding an account. What the user does with it goes back to the
    /// core, which decides what that means.
    private(set) lazy var formSheet = FormSheet { [weak self] used in
        self?.bridge?.send(.formUsed(used))
    }

    /// Which pane of the settings window the user last had. A scripted run
    /// keeps it for the run only, and reads and writes nothing of the
    /// user's.
    let settingsPaneMemory: any SettingsPaneMemory

    /// The settings window. What it shows is the core's to say, and what
    /// the user does in it goes back to the core, which decides what that
    /// means; the one thing it has of the shell's own is the way to the
    /// settings file.
    private(set) lazy var settingsWindow = SettingsWindowController(
        memory: settingsPaneMemory, keepsPlace: !options.isScripted,
        send: { [weak self] message in self?.bridge?.send(message) },
        openFile: { [weak self] in self?.editConfig(nil) })

    /// The window of every key, which Help > Keyboard shortcuts brings up
    /// and which stays up beside the mail.
    private(set) lazy var keysWindow = KeysWindowController(
        keepsPlace: !options.isScripted, beside: { [weak self] in self?.window })

    /// Copies a theme the user chose into the themes folder beside the
    /// settings file.
    let themeImport: ThemeImport

    /// The chooser of themes to import: the ones of a Ghostty on this Mac,
    /// and the way to a file kept anywhere else.
    private(set) lazy var themeChooser = ThemeChooser(
        folders: GhosttyThemes.folders(home: GhosttyThemes.realHome()),
        onImport: { [weak self] file in self?.importTheme(from: file) },
        onOther: { [weak self] window in
            guard let self else { return }
            themeFilePicker(window ?? self.window) { [weak self] file in
                guard let file else { return }
                self?.importTheme(from: file)
            }
        })

    /// Whether the app's own parts are light or dark: what the settings
    /// say, and in a scripted run what the script says the Mac is.
    private(set) var look = AppLook()
    /// What the core was last told the Mac is.
    private(set) var systemDarkTold: Bool?
    /// Word of the app's appearance changing: with the app left to the
    /// Mac, that is the Mac changing.
    private var appearanceObservation: NSKeyValueObservation?

    private(set) var coreIsRunning = false
    /// The app asked the core to stop, so its exit is not a failure.
    private var stopping = false
    /// AppKit is waiting to hear that it may terminate.
    private var terminationPending = false

    init(options: LaunchOptions) {
        self.options = options
        surface = makeTerminalSurface()
        terminalStack = TerminalStack(terminal: surface.view)
        dividerMemory =
            options.isScripted ? TransientDividerMemory() : DefaultsDividerMemory(defaults: .standard)
        settingsPaneMemory =
            options.isScripted ? TransientSettingsPaneMemory() : DefaultsSettingsPaneMemory(defaults: .standard)
        let support =
            FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
            .appendingPathComponent("Library/Application Support", isDirectory: true)
        // A run whose core was pointed at another config folder, as a
        // scratch run is, shows that one and leaves the user's own alone.
        if let folder = options.coreEnvironment["QUILL_CONFIG_DIR"], folder.hasPrefix("/") {
            configFolder = URL(fileURLWithPath: folder, isDirectory: true)
        } else {
            configFolder =
                support
                .appendingPathComponent("Quill", isDirectory: true)
                .appendingPathComponent("config", isDirectory: true)
        }
        themeImport = ThemeImport(configFolder: configFolder)
        // A scripted run never asks what the Mac looks like: it is light
        // until the script says otherwise, whatever the user has it as.
        if options.isScripted {
            look.pretended = false
        }

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1180, height: 760),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Quill"
        window.minSize = NSSize(width: 480, height: 320)
        // A scripted run starts at the size above every time, and leaves
        // the size the user's own window was last at as it was.
        if !options.isScripted {
            window.setFrameAutosaveName("QuillMainWindow")
        }
        window.center()
        super.init(window: window)
        window.delegate = self

        splitView.isVertical = true
        splitView.dividerStyle = .thin
        splitView.addArrangedSubview(terminalStack)
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
        // The terminal has another size, or its cells have: the core is
        // told how many columns the window has room for, and the reading
        // pane's divider is put where that many columns end.
        surface.onGridChange = { [weak self] _ in
            self?.reportWindowColumns()
            self?.placeReadingDivider()
            // The editor's place is said in cells: with cells of another
            // size it is somewhere else.
            self?.placeEditor()
        }
        splitView.onDividerMoved = { [weak self] in self?.dividerMoved() }
        splitView.onDividerDoubleClicked = { [weak self] in self?.dividerReset() }
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
        // the viewer inside it. The terminal handles its own keys, and so
        // does an editor laid over it, where esc is Vim's. Only this
        // window's keys are looked at: a player gone full screen has a
        // window of its own, where esc means leave full screen.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, event.window === window, let pane, paneHasFocus,
                let key = PaneKey(event: event)
            else { return event }
            if key == .escape {
                // The reading pane is not the user's to close: esc gives
                // the keys back to the terminal, where the list is.
                if isReadingPane {
                    focusTerminal()
                } else {
                    closePane()
                }
                return nil
            }
            return pane.handleKey(key) ? nil : event
        }
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// A click on a terminal while an editor is laid over the core's. Which
    /// of the two has the keys is the core's to say, and a click is how the
    /// user says it to the core. On the editor, when the core has the keys:
    /// the core is asked to go to the text, as its command does. On the
    /// core's terminal: the core hears the click itself, and whatever it
    /// makes of it, the keys end up where it last said.
    private func watchClicksOnTheTerminals() {
        mouseMonitor = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown) { [weak self] event in
            guard let self, event.window === window, editor != nil, window?.attachedSheet == nil else {
                return event
            }
            let point = terminalStack.convert(event.locationInWindow, from: nil)
            guard terminalStack.bounds.contains(point) else { return event }
            if terminalStack.editorIsShown(at: point) {
                if !editorPlace.keys {
                    commands.run(named: Self.goToTextCommand)
                }
            } else {
                DispatchQueue.main.async { [weak self] in self?.giveKeysAsTheCoreSaid() }
            }
            return event
        }
    }

    /// The core's command that puts the cursor in the text of the message
    /// being written: what a click on the editor means.
    static let goToTextCommand = "compose.body"

    override func showWindow(_ sender: Any?) {
        super.showWindow(sender)
        surface.focus()
        watchClicksOnTheTerminals()
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
            // What the Mac looks like, for a core whose settings say to
            // follow it: said now, and whenever it changes. And said in the
            // core's environment as well, so that its first frame is drawn
            // with the right theme and not corrected a moment later.
            systemDarkTold = nil
            applyLook()
            if let dark = systemDarkTold {
                environment["QUILL_SYSTEM_APPEARANCE"] = dark ? "dark" : "light"
            }
            appearanceObservation = NSApp.observe(\.effectiveAppearance) { [weak self] _, _ in
                Task { @MainActor in self?.reportSystemAppearance() }
            }
            // If the terminal has been measured already. It waits in the
            // bridge until the core has connected.
            reportWindowColumns()
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
            commands.setChecked(state.checked)
            window?.title = state.title.isEmpty ? "Quill" : state.title
            window?.subtitle = state.unread > 0 ? "\(state.unread) unread" : ""
            NSApp.dockTile.badgeLabel = state.unread > 0 ? String(state.unread) : nil
            setTerminalColours(background: state.background, foreground: state.foreground)
            setAppearance(state.appearance ?? .system)
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
        case let .thumbnailMake(make):
            makeThumbnail(make)
        case let .fileOpen(path):
            openFile(URL(fileURLWithPath: path))
        case let .filesSave(save):
            saveCopy(of: URL(fileURLWithPath: save.path), offering: save.name)
        case .paneClose:
            // An editor is what the core means while one is running: it
            // is asked to save and leave, and the core hears when it has.
            if editor != nil {
                stopEditor()
            } else {
                closePane()
            }
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
        case let .spellCheck(check):
            spelling.check(check)
        case let .spellSuggest(suggest):
            spelling.suggest(suggest)
        case let .paneEditor(request):
            openEditor(request)
        case let .formShow(form):
            formSheet.show(form, on: sheetWindow)
        case .formClose:
            formSheet.close()
        case let .settingsShow(settings):
            settingsWindow.show(settings)
        case .themesPick:
            showThemeChooser()
        case let .keysList(list):
            keysWindow.set(list)
        case .keysShow:
            keysWindow.show()
        case let .readingShow(page):
            showReading(page)
        case let .readingScroll(by):
            readingPane?.scroll(by)
        case .readingClose:
            closeReading()
        case let .readingPlace(place):
            readingColumnsAsked = place.columns
            placeReadingDivider()
        case let .editorPlace(place):
            editorPlace = place
            placeEditor()
        }
    }

    /// The window a sheet of the core's goes on: the settings window when
    /// it is the one in front, since what brought the question up was done
    /// there (adding an account, signing in to one), and otherwise the main
    /// window. A sheet that is up stays on the window it is on.
    private var sheetWindow: NSWindow? {
        guard let settings = settingsWindow.windowIfOpen else { return window }
        let front = NSApp.orderedWindows.first { $0 === settings || $0 === window }
        return front === settings ? settings : window
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
        // A form of the core's has nobody left to answer to, and nor has
        // the settings window.
        formSheet.close()
        settingsWindow.close()
        // There is nobody left to hand a draft back to. Vim is still asked
        // to save it and go, and is not left running behind the app.
        afterEditor = nil
        stopEditor()
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
        case .exited(code: CoreLocator.alreadyOpenStatus):
            presentFailure(
                "Quill is already open on this mailbox in a terminal. Quit it there, then open Quill again.")
        case let .exited(code):
            presentFailure("The Quill core stopped unexpectedly (exit code \(code)).")
        case let .signaled(signal):
            presentFailure("The Quill core stopped unexpectedly (signal \(signal)).")
        case let .failedToStart(reason):
            presentFailure("The Quill core could not start: \(reason)")
        }
    }

    /// Asks the core to flush and exit. If it does not, it is hung up on.
    ///
    /// A message being written in Vim is saved and handed back first: the
    /// core cannot take a draft back once it has gone.
    func stopCore() {
        guard coreIsRunning, !stopping else { return }
        stopping = true
        guard editor != nil else {
            askCoreToStop()
            return
        }
        afterEditor = { [weak self] in self?.askCoreToStop() }
        stopEditor()
    }

    private func askCoreToStop() {
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

    func windowDidResize(_: Notification) {
        reportWindowColumns()
        placeReadingDivider()
    }

    func windowDidChangeBackingProperties(_: Notification) {
        reportWindowColumns()
        placeReadingDivider()
    }

    func windowWillClose(_: Notification) {
        // The settings are the main window's: they go with it.
        settingsWindow.close()
        // Nothing goes on playing, or loading, behind a closed window.
        dropIncomingReading()
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
        // A page still loading for the reading pane has no pane to go
        // into now. The latest is loaded afresh when this one goes.
        dropIncomingReading()
        // A pane that replaces another takes its place exactly; the first
        // pane leaves the terminal the larger share.
        var divider = splitView.bounds.width * 0.58
        if let old = pane {
            divider = terminalStack.frame.width
            old.tearDown()
            old.removeFromSuperview()
        }
        pane = newPane
        splitView.addArrangedSubview(newPane)
        holdTheTerminal()
        splitView.layoutSubtreeIfNeeded()
        splitView.setPosition(divider, ofDividerAt: 0)
        focus(newPane)
    }

    /// As the window is resized the pane takes up the difference and the
    /// terminal keeps its width, and so its columns: what is drawn in it
    /// does not shift about on the way.
    private func holdTheTerminal() {
        guard splitView.arrangedSubviews.count == 2 else { return }
        splitView.setHoldingPriority(.defaultLow + 10, forSubviewAt: 0)
        splitView.setHoldingPriority(.defaultLow - 10, forSubviewAt: 1)
    }

    /// Closes the pane, whoever asks: its close button, the menu, `esc` in
    /// a viewer, or the core.
    private func closePane() {
        guard pane != nil else { return }
        // The reading pane goes when the core says web reading is off,
        // and by nothing else.
        if isReadingPane {
            return
        }
        removePane()
    }

    private func removePane() {
        guard let old = pane else { return }
        pane = nil
        old.tearDown()
        old.removeFromSuperview()
        focusTerminal()
        // The place is empty: the reading pane has it back, on the page
        // the core last asked for, and where the reading pane goes,
        // wherever the divider of the pane that has gone was left.
        if case let .show(page) = readingSlot.vacated() {
            place(makeReadingPane(page))
        }
    }

    // MARK: - The reading pane

    /// The reading pane, while it is the pane on show.
    private var readingPane: WebPane? { pane as? WebPane }

    private var isReadingPane: Bool { readingPane != nil }

    private var occupant: PaneOccupant {
        if pane == nil {
            return .nothing
        }
        return isReadingPane ? .reading : .other
    }

    /// How long a page is given to load out of sight before it is put on
    /// show anyway, still loading.
    private static let longestWaitForAPage = 0.4

    private func makeReadingPane(_ page: ReadingShow) -> WebPane {
        let pane = WebPane(
            file: URL(fileURLWithPath: page.path), title: page.title, allowsRemoteContent: page.remote,
            header: page.header, notice: page.notice
        ) { [weak self] address in
            // The same door every other link goes through.
            self?.openLink(address.absoluteString)
        }
        // A button of the notice runs its command, as a menu item does.
        // What the command is, and whether it still applies, is the
        // core's to say.
        pane.onNoticeButton = { [weak self] command in
            self?.commands.run(named: command)
        }
        return pane
    }

    /// Uses a button of the notice above the page in the reading pane, by
    /// the name of its command, as a click on it does. False when there
    /// is no such button on show.
    func pressReadingNotice(_ command: String) -> Bool {
        readingPane?.pressNoticeButton(command) ?? false
    }

    /// Shows a page in the reading pane, in place of the page it showed.
    /// The keyboard stays where it is: with the terminal, and its list.
    private func showReading(_ page: ReadingShow) {
        readingRequests += 1
        switch readingSlot.show(page, over: occupant) {
        case let .show(page):
            place(makeReadingPane(page))
        case let .replace(page):
            dropIncomingReading()
            let incoming = makeReadingPane(page)
            incomingReading = incoming
            guard incoming.phase != .failed else {
                swapInReading(incoming)
                return
            }
            incoming.onSettled = { [weak self, weak incoming] in
                guard let incoming else { return }
                self?.swapInReading(incoming)
            }
            Task { [weak self, weak incoming] in
                try? await Task.sleep(for: .seconds(Self.longestWaitForAPage))
                guard let incoming else { return }
                self?.swapInReading(incoming)
            }
        case .close, .nothing:
            // Another pane has the place. The page waits for it to go.
            break
        }
    }

    /// Puts a reading pane beside the terminal, in an empty place, without
    /// giving it the keyboard.
    private func place(_ newPane: WebPane) {
        pane = newPane
        splitView.addArrangedSubview(newPane)
        holdTheTerminal()
        splitView.layoutSubtreeIfNeeded()
        splitView.setPosition(readingTerminalWidth, ofDividerAt: 0)
    }

    // MARK: - The reading pane's divider

    /// The terminal's cells, once it has been measured.
    private var cells: CellMeasure? {
        surface.grid.flatMap { CellMeasure(cellWidthPixels: $0.cellWidthPixels, scale: $0.scale) }
    }

    /// How many columns the terminal would have with the window to itself.
    var windowColumns: Int? {
        cells?.columns(inWidth: splitView.bounds.width)
    }

    /// Tells the core how many columns the window has room for, when that
    /// has changed. The core lays out by it while the reading pane has
    /// part of the window, and says where the pane goes.
    private func reportWindowColumns() {
        guard let bridge, let columns = windowColumns, columns != windowColumnsSent,
            let resized = WindowResized(columns: columns)
        else { return }
        windowColumnsSent = columns
        bridge.send(.windowResized(resized))
    }

    /// The width the terminal has beside the reading pane: that of the
    /// columns the user left it, or else of those the core says. Half the
    /// window until the terminal has been measured.
    private var readingTerminalWidth: CGFloat {
        guard let cells else { return (splitView.bounds.width / 2).rounded() }
        return ReadingDivider.terminalWidth(
            remembered: dividerMemory.columns, asked: readingColumnsAsked, width: splitView.bounds.width,
            divider: splitView.dividerThickness, cells: cells)
    }

    /// Puts the divider where the reading pane's goes, if the reading pane
    /// is the pane on show and it is not there already. Not while the user
    /// has hold of it: the terminal's columns change under a drag, and
    /// putting the divider back at each would take it out of their hand.
    private func placeReadingDivider() {
        guard isReadingPane, !splitView.isDividerHeld else { return }
        let width = readingTerminalWidth
        if abs(terminalStack.frame.width - width) >= 0.5 {
            splitView.setPosition(width, ofDividerAt: 0)
        }
    }

    /// The user dragged the divider and let go. Beside the reading pane,
    /// that is where it stays, in this run and the next: on the nearest
    /// whole column. Beside any other pane it is that pane's for as long
    /// as it is open, and nothing is remembered.
    private func dividerMoved() {
        guard isReadingPane, let cells else { return }
        dividerMemory.columns = ReadingDivider.columns(
            draggedTo: terminalStack.frame.width, width: splitView.bounds.width,
            divider: splitView.dividerThickness, cells: cells)
        placeReadingDivider()
    }

    /// The user double-clicked the divider: it goes back to where the core
    /// says, the reader's place exactly, and nothing is remembered.
    private func dividerReset() {
        guard isReadingPane else { return }
        dividerMemory.columns = nil
        placeReadingDivider()
    }

    /// Puts a reading pane that has loaded its page where the reading pane
    /// on show is. If that pane had the keyboard, the terminal has it now:
    /// the pane the user clicked in is gone.
    private func swapInReading(_ incoming: WebPane) {
        guard incomingReading === incoming, let old = readingPane else { return }
        incomingReading = nil
        incoming.onSettled = nil
        let hadFocus = paneHasFocus
        old.tearDown()
        old.removeFromSuperview()
        place(incoming)
        if hadFocus {
            focusTerminal()
        }
    }

    private func dropIncomingReading() {
        incomingReading?.onSettled = nil
        incomingReading?.tearDown()
        incomingReading = nil
    }

    /// Takes the reading pane away: the core says web reading is off.
    private func closeReading() {
        dropIncomingReading()
        if readingSlot.close(over: occupant) == .close {
            removePane()
        }
    }

    // MARK: - Moving between the panes

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
            focusTerminal()
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

    // MARK: - The terminals' colours

    /// Gives every terminal the colours of the core's theme, as the colours
    /// of what no program has painted: the edge of the core's own terminal,
    /// and all of an editor's that sets no colours. A theme that leaves
    /// them to the terminal says none, and the terminals keep their own,
    /// which are light or dark as the Mac is. The terminals share one look,
    /// so an editor that is running changes with the rest.
    private func setTerminalColours(background: String?, foreground: String?) {
        var appearance = terminalAppearance
        appearance.background = background
        appearance.foreground = foreground
        guard appearance != terminalAppearance else { return }
        terminalAppearance = appearance
        surface.setAppearance(appearance)
    }

    // MARK: - Light and dark

    /// The app's own parts are made light or dark as the settings say, or
    /// left to the Mac: the title bar and the toolbar, the sheets, the
    /// settings window, and the reading pane with the pages Quill makes for
    /// it. The terminals' colours are the theme's, and come separately.
    private func setAppearance(_ setting: Appearance) {
        guard setting != look.setting else { return }
        look.setting = setting
        applyLook()
    }

    private func applyLook() {
        switch look.forced {
        case true?: NSApp.appearance = NSAppearance(named: .darkAqua)
        case false?: NSApp.appearance = NSAppearance(named: .aqua)
        case nil: NSApp.appearance = nil
        }
        reportSystemAppearance()
    }

    /// Whether the app looks dark at this moment.
    private var appIsDark: Bool {
        NSApp.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
    }

    /// Tells the core what the Mac looks like, when that is known and is
    /// not what it was last told. While the settings hold the app light or
    /// dark the Mac's own look is not asked after: the core has no use for
    /// it, and is told as soon as the settings go back to following it.
    private func reportSystemAppearance() {
        guard let dark = look.systemIsDark(appIsDark: appIsDark), dark != systemDarkTold else { return }
        systemDarkTold = dark
        bridge?.send(.appearanceChanged(dark: dark))
    }

    /// As if the Mac had gone dark, or light: for a scripted run, which
    /// must not change what the Mac really is.
    func pretendSystem(dark: Bool) {
        look.pretended = dark
        applyLook()
    }

    /// The look in a line, for the dump.
    var lookSummary: String {
        let mac = look.pretended.map { "pretended \($0 ? "dark" : "light")" } ?? "its own"
        let told = systemDarkTold.map { $0 ? "dark" : "light" } ?? "nothing"
        return "setting \(look.setting.rawValue), Mac \(mac), app \(appIsDark ? "dark" : "light"), "
            + "core told \(told)"
    }

    // MARK: - The editor

    /// The keys go to the terminal: to the editor laid over it, when the
    /// core has said the editor has them, and otherwise to the core's own.
    private func focusTerminal() {
        if let editor, editorPlace.keys, terminalStack.editorIsShown {
            editor.surface.focus()
        } else {
            surface.focus()
        }
    }

    private var editorHasFocus: Bool {
        guard let editor, let responder = window?.firstResponder as? NSView else { return false }
        return responder.isDescendant(of: editor.surface.view)
    }

    private var coreTerminalHasFocus: Bool {
        guard let responder = window?.firstResponder as? NSView else { return false }
        return responder.isDescendant(of: surface.view)
    }

    /// Has whichever terminal has the keys be the one the core said, when
    /// it is one of the two that has them. A pane the user is in keeps
    /// them, and so does a sheet.
    private func giveKeysAsTheCoreSaid() {
        guard editor != nil, window?.attachedSheet == nil, editorHasFocus || coreTerminalHasFocus else { return }
        focusTerminal()
    }

    /// Puts the editor's terminal over the cells the core said, or out of
    /// sight when it said none, and gives the keys to the terminal it said
    /// has them. The shell decides none of it: the place is the core's
    /// cells turned into points.
    private func placeEditor() {
        guard let editor else {
            terminalStack.hideEditor()
            return
        }
        let size = surface.grid.flatMap {
            CellSize(widthPixels: $0.cellWidthPixels, heightPixels: $0.cellHeightPixels, scale: $0.scale)
        }
        if let cells = editorPlace.cells, let size {
            let frames = EditorOverlay.frames(
                column: cells.column, row: cells.row, columns: cells.columns, rows: cells.rows,
                covered: cells.covered, cells: size)
            terminalStack.showEditor(shown: frames.shown, whole: frames.whole)
            // A terminal draws its cursor solid until it has once had the
            // keys and lost them. An editor that starts without them is
            // given them and has them taken back, so that the one solid
            // cursor in the window is where the typing goes.
            if !editor.hasBeenShown {
                editor.hasBeenShown = true
                if !editorPlace.keys, coreTerminalHasFocus {
                    editor.surface.focus()
                    surface.focus()
                }
            }
        } else {
            // Its keys go with it: nobody types into what they cannot see.
            let hadKeys = editorHasFocus
            terminalStack.hideEditor()
            if hadKeys {
                surface.focus()
            }
        }
        // A terminal that has the keys gives them to the other when the
        // core says so. The first time, there is nothing to take them from
        // but the core's own terminal, which has had them all along.
        if editorPlace.keys, terminalStack.editorIsShown, coreTerminalHasFocus {
            editor.surface.focus()
        } else if !editorPlace.keys, editorHasFocus {
            surface.focus()
        }
    }

    /// Runs an editor on the text of a message, in a terminal of its own
    /// laid over the core's: the Vim or nano that comes with macOS, or the
    /// user's own by their script when they have asked for that. The core
    /// is always answered: now, when nothing is started, and otherwise when
    /// the editor ends.
    private func openEditor(_ request: PaneEditor) {
        // The program is the core's to find. It is looked at once more
        // here, so that one that has gone since is an answer and not a
        // terminal with nothing in it.
        let available =
            editorSlot.isRunning || FileManager.default.isExecutableFile(atPath: request.program)
        if !available {
            NSLog("Quill: the editor is not there to run: %@", request.program)
        }
        switch editorSlot.request(title: request.title, available: available) {
        case let .refuse(answer):
            if editorSlot.isRunning {
                NSLog("Quill: an editor was asked for while one is running; the second was not started")
            }
            bridge?.send(.editorExited(answer))
        case .start:
            startEditor(request)
        }
    }

    private func startEditor(_ request: PaneEditor) {
        let launch = EditorLaunch(
            request: request, term: CoreLocator.term,
            base: CoreLocator.programEnvironment(extra: options.coreEnvironment))
        // As the core's terminal looks now: a surface made with another
        // look would give that look to both.
        let terminal = makeTerminalSurface(appearance: terminalAppearance)
        let running = RunningEditor(surface: terminal, kind: request.editor)
        editor = running
        terminal.onExit = { [weak self, weak running] exit in
            guard let self, let running else { return }
            editorDidExit(exit, of: running)
        }
        // The same door every other link goes through.
        terminal.onOpenLink = { [weak self] address in
            self?.openLink(address)
        }
        // Over the core's terminal, where the core has said, or out of
        // sight until it does. It starts once it has a size.
        terminalStack.put(editor: terminal.view)
        placeEditor()
        // On a terminal as any terminal gives it: the editor is the user's
        // own, and sets its terminal's modes for itself.
        terminal.start(
            TerminalCommand(
                executable: launch.executable,
                arguments: launch.arguments,
                environment: launch.environment,
                workingDirectory: launch.workingDirectory,
                rawTerminal: false
            ))
    }

    /// The editor has ended, or never started: its terminal goes, the
    /// keyboard is the core's terminal's again if the editor had it, and
    /// the core is told how it went.
    private func editorDidExit(_ exit: TerminalExit, of ended: RunningEditor) {
        guard let running = editor, running === ended else { return }
        let hadKeys = editorHasFocus
        editor = nil
        terminalStack.removeEditor()
        // Where the next one goes is for the core to say afresh.
        editorPlace = .hidden
        if hadKeys {
            surface.focus()
        }
        let ending: EditorEnding
        switch exit {
        case let .exited(code):
            ending = .exited(code: code)
        case let .signaled(signal):
            ending = .signaled(signal: signal)
        case let .failedToStart(reason):
            NSLog("Quill: Vim could not be started: %@", reason)
            ending = .failedToStart
        }
        if let answer = editorSlot.ended(ending) {
            bridge?.send(.editorExited(answer))
        }
        let next = afterEditor
        afterEditor = nil
        next?()
    }

    /// Asks Vim to save what has been written and leave, as someone at the
    /// keyboard would: `esc`, then `:wqa`. No verdict is written, so the
    /// core is left to ask what to do with the draft. A Vim that has not
    /// gone in two seconds is hung up on, and one that survives that is
    /// killed: the core is waiting to hear, and is told whatever results.
    private func stopEditor() {
        guard let running = editor, !running.isStopping else { return }
        running.isStopping = true
        running.surface.sendKeys(PoliteStop.leaveMode(for: running.kind))
        Task { [weak self, weak running] in
            try? await Task.sleep(for: .seconds(PoliteStop.pause))
            guard let self, let running, editor === running else { return }
            running.surface.sendKeys(PoliteStop.saveAndQuit(for: running.kind))
            try? await Task.sleep(for: .seconds(PoliteStop.patience))
            guard editor === running else { return }
            NSLog("Quill: the editor did not leave when asked, and was hung up on")
            running.surface.terminate()
            try? await Task.sleep(for: .seconds(PoliteStop.lastResort))
            guard editor === running else { return }
            running.surface.kill()
        }
    }

    // MARK: - The config

    /// Makes the config folder if it is not there yet, private to the user.
    private func makeConfigFolder() {
        try? FileManager.default.createDirectory(
            at: configFolder, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    }

    /// Opens the settings window. What is in it is the core's to say, so
    /// with no core to ask (it could not be started, or has no bridge to
    /// answer over) the settings file is opened, which is all there is.
    @objc func showSettings(_ sender: Any?) {
        guard coreIsRunning, bridge != nil else {
            editConfig(sender)
            return
        }
        settingsWindow.open()
    }

    /// Opens `config.toml` in the application the Mac edits text with. With
    /// no config yet there is nothing to open, so one is made: an empty
    /// config is a valid one, and says where to look for what goes in it.
    @objc func editConfig(_: Any?) {
        let file = configFolder.appendingPathComponent("config.toml")
        if !FileManager.default.fileExists(atPath: file.path) {
            makeConfigFolder()
            let start = [
                "# Quill's settings. The README says what can go here.",
                "# Quill reads this file when it starts.", "",
            ].joined(separator: "\n")
            FileManager.default.createFile(
                atPath: file.path, contents: Data(start.utf8), attributes: [.posixPermissions: 0o600])
        }
        fileOpener(file)
    }

    @objc func revealConfig(_: Any?) {
        makeConfigFolder()
        revealer(configFolder)
    }

    // MARK: - The user's editor script

    // MARK: - Themes

    /// Shows the chooser of themes to import, on the settings window when
    /// that is the one in front and otherwise on the main one.
    @objc func importTheme(_: Any?) {
        showThemeChooser()
    }

    private func showThemeChooser() {
        themeChooser.open(on: sheetWindow)
    }

    /// Copies a theme file into the themes folder, off the main thread,
    /// and tells the core its name: the core reads the folder again, and
    /// makes the theme the one in force if it is a theme it can use.
    func importTheme(from file: URL) {
        let importer = themeImport
        Task.detached(priority: .userInitiated) { [weak self] in
            let result = Result { try importer.importFile(file) }
            await MainActor.run { self?.themeImported(result, from: file) }
        }
    }

    private func themeImported(_ result: Result<String, any Error>, from file: URL) {
        switch result {
        case let .success(name):
            note("\(file.path) (imported as \(name.debugDescription))", in: &themeResults)
            if let imported = ThemesImported(name: name) {
                bridge?.send(.themesImported(imported))
            }
        case let .failure(error):
            note("\(file.path) (refused: \(error))", in: &themeResults)
            problemReporter(
                sheetWindow, "The theme was not imported",
                "Quill could not import \(file.lastPathComponent): \(error).")
        }
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
        // In a sheet the text being edited is a field of the sheet's, and
        // in the settings window one of that window's.
        let inSettings = settingsWindow.windowIfOpen?.isKeyWindow ?? false
        if !paneHasFocus, !inSettings, window?.attachedSheet == nil, commands.isEnabled(command) {
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

    /// What became of each theme import, in order.
    private(set) var themeResults: [String] = []

    /// The reading pane in a line, for the dump: `off`, or `on` and what
    /// it shows, or that another pane has its place for now.
    var readingSummary: String {
        guard let page = readingSlot.page else { return "off" }
        let file = URL(fileURLWithPath: page.path).pathComponents.suffix(3).joined(separator: "/")
        var words = ["on", page.title.debugDescription, file]
        if let shown = readingPane {
            // The pane on show may still be the page before, for a moment.
            let current = shown.document.path == page.path
            words.append(current ? shown.summary : "still showing \(shown.title.debugDescription)")
        } else {
            words.append("behind another pane")
        }
        if !page.header.isEmpty {
            words.append("header: " + page.header.joined(separator: " | "))
        }
        return words.joined(separator: ", ")
    }

    /// The colours the terminals were last given, for the dump.
    var terminalColours: String {
        "background \(terminalAppearance.background ?? "its own"), "
            + "foreground \(terminalAppearance.foreground ?? "its own")"
    }

    /// The reading pane's web view, for the picture in the dump.
    var readingPaneForPicture: WebPane? { readingPane }

    /// The terminal's grid in a line, for the dump, and whether the sum
    /// that says how many columns the window has room for agrees with it.
    var gridSummary: String {
        guard let grid = surface.grid else { return "not measured" }
        let width = terminalStack.frame.width
        return "\(grid.columns) columns, \(grid.rows) rows, cells \(grid.cellWidthPixels) px at \(grid.scale)x, "
            + "view \(width) pt of \(splitView.bounds.width)"
    }

    /// As if the user had dragged the divider until the terminal had this
    /// many columns, and let go. False when there is no pane or no measure.
    func dragDivider(toColumns columns: Int) -> Bool {
        guard pane != nil, let cells else { return false }
        splitView.drag(to: cells.width(forColumns: columns))
        return true
    }

    /// As if the user had double-clicked the divider.
    func doubleClickDivider() {
        dividerReset()
    }

    /// Where the divider is in the window, and where the mouse would have
    /// to take it for the terminal to have this many columns: for a
    /// scripted run that drags it with mouse events of its own.
    func dividerDrag(toColumns columns: Int) -> (from: NSPoint, to: NSPoint)? {
        guard pane != nil, let cells else { return nil }
        let terminal = terminalStack.frame
        let from = splitView.convert(
            NSPoint(x: terminal.maxX + splitView.dividerThickness / 2, y: splitView.bounds.midY), to: nil)
        return (from, NSPoint(x: from.x + cells.width(forColumns: columns) - terminal.width, y: from.y))
    }

    /// Where the first cell of `text` is in the window, by the terminal's
    /// screen as it is now: for a scripted run to click there. `nth` picks
    /// a later place the text is at, counted from 1. Nil when the text is
    /// not on the screen that many times.
    func terminalPoint(ofText text: String, nth: Int = 1) -> NSPoint? {
        guard let grid = surface.grid, grid.rows > 0, let screen = surface.visibleText(), nth > 0, !text.isEmpty
        else { return nil }
        var seen = 0
        for (row, line) in screen.components(separatedBy: "\n").enumerated() {
            var from = line.startIndex
            while let found = line.range(of: text, range: from..<line.endIndex) {
                seen += 1
                if seen == nth {
                    // A wide character takes two cells.
                    let column = line[..<found.lowerBound].unicodeScalars.reduce(0) { cells, scalar in
                        let wide = scalar.properties.isEmojiPresentation || (0x2E80...0xFFDC).contains(scalar.value)
                        return cells + (wide ? 2 : 1)
                    }
                    let view = surface.view
                    let cellWidth = CGFloat(grid.cellWidthPixels) / grid.scale
                    let cellHeight = view.bounds.height / CGFloat(grid.rows)
                    let down = (CGFloat(row) + 0.5) * cellHeight
                    let point = NSPoint(
                        x: (CGFloat(column) + 0.5) * cellWidth,
                        y: view.isFlipped ? down : view.bounds.height - down)
                    return view.convert(point, to: nil)
                }
                from = found.upperBound
            }
        }
        return nil
    }

    /// The editor's screen as text, while one is running.
    var editorText: String? { editor?.surface.visibleText() }

    /// The middle of the editor where it is seen, in the window: for a
    /// scripted run to click there. Nil when no editor is on show.
    var editorPoint: NSPoint? {
        terminalStack.editorFrame.map {
            terminalStack.convert(NSPoint(x: $0.midX, y: $0.midY), to: nil)
        }
    }

    /// Where the editor is, for a scripted run's dump: what the core said,
    /// where that put it, how many cells it came to, and who has the keys.
    var editorPlaceSummary: String {
        guard let editor else { return "no editor" }
        let said =
            editorPlace.cells.map {
                "column \($0.column) row \($0.row), \($0.columns) by \($0.rows), \($0.covered) covered"
            } ?? "nowhere"
        let seen =
            terminalStack.editorFrame.map {
                "at \($0.minX),\($0.minY) \($0.width) by \($0.height) pt"
            } ?? "not shown"
        let grid = editor.surface.grid.map { "\($0.columns) columns, \($0.rows) rows" } ?? "not measured"
        let keys =
            editorHasFocus ? "the editor" : coreTerminalHasFocus ? "the terminal" : "neither terminal"
        return "\(said); \(seen); \(grid); keys said for \(editorPlace.keys ? "the editor" : "the terminal"), "
            + "with \(keys)"
    }

    /// Types into the editor pane, as keys. False when no Vim is running.
    func typeIntoEditor(_ text: String) -> Bool {
        guard let editor else { return false }
        editor.surface.sendKeys(text)
        return true
    }

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
        case #selector(focusNextPane(_:)), #selector(focusPreviousPane(_:)):
            pane != nil
        case #selector(closePane(_:)):
            // The reading pane is switched off, not closed.
            pane != nil && !(pane is WebPane)
        case #selector(importTheme(_:)):
            // The core is the one that makes the theme the one in force.
            coreIsRunning && bridge != nil && sheetWindow?.attachedSheet == nil
        default:
            true
        }
    }
}

/// The split between the terminal and the second pane, which says when
/// the user has moved its divider themselves, or double-clicked it.
///
/// Its subviews fill it, so a click that reaches the split view itself is
/// one on the divider. AppKit follows a drag of the divider from inside
/// `mouseDown`, and returns when the button is let go.
@MainActor
final class DividerSplitView: NSSplitView {
    /// The divider was dragged somewhere else and let go.
    var onDividerMoved: (() -> Void)?
    var onDividerDoubleClicked: (() -> Void)?

    private var widthWhenPressed: CGFloat?

    /// The user has hold of the divider: from the press to the letting go.
    var isDividerHeld: Bool { widthWhenPressed != nil }

    /// The divider taken hold of, moved to `position` and let go, as a
    /// drag does it, for a scripted run.
    func drag(to position: CGFloat) {
        widthWhenPressed = arrangedSubviews.first?.frame.width
        setPosition(position, ofDividerAt: 0)
        layoutSubtreeIfNeeded()
        letGo()
    }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 {
            onDividerDoubleClicked?()
            return
        }
        widthWhenPressed = arrangedSubviews.first?.frame.width
        super.mouseDown(with: event)
        // AppKit has followed the drag to its end by now. Were it ever to
        // return with the button still down, the letting go is `mouseUp`.
        if NSEvent.pressedMouseButtons & 1 == 0 {
            letGo()
        }
    }

    override func mouseUp(with event: NSEvent) {
        super.mouseUp(with: event)
        letGo()
    }

    private func letGo() {
        guard let before = widthWhenPressed else { return }
        widthWhenPressed = nil
        if let now = arrangedSubviews.first?.frame.width, abs(now - before) >= 0.5 {
            onDividerMoved?()
        }
    }
}

/// The editor that has the text of a message: its terminal.
@MainActor
private final class RunningEditor {
    let surface: any TerminalSurface
    /// Which editor it is: each is asked to leave in its own keys.
    let kind: EditorKind
    /// The editor has been asked to save and leave.
    var isStopping = false
    /// It has been put on show at least once.
    var hasBeenShown = false

    init(surface: any TerminalSurface, kind: EditorKind) {
        self.surface = surface
        self.kind = kind
    }
}

/// A view whose y grows downwards, as a terminal's rows do.
private class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

/// The core's terminal, and over it the terminal of an editor that has the
/// text of the message being written.
///
/// The editor's terminal is a view of its own, in front of the core's, in
/// an opening that shows as much of it as the core has not drawn over.
/// Where the opening is and how much it shows are for whoever owns this to
/// say: nothing here knows what a cell is.
@MainActor
private final class TerminalStack: FlippedView {
    private let opening = FlippedView()
    private var editor: NSView?

    init(terminal: NSView) {
        super.init(frame: .zero)
        terminal.frame = bounds
        terminal.autoresizingMask = [.width, .height]
        addSubview(terminal)
        opening.isHidden = true
        opening.wantsLayer = true
        opening.layer?.masksToBounds = true
        addSubview(opening)
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) is not used")
    }

    /// Takes an editor's terminal, out of sight until it is given a place.
    func put(editor view: NSView) {
        removeEditor()
        editor = view
        opening.addSubview(view)
    }

    func removeEditor() {
        editor?.removeFromSuperview()
        editor = nil
        opening.isHidden = true
    }

    /// Shows the editor: `shown` is the part of it to be seen, in this
    /// view's points from its top left, and `whole` the editor's own frame
    /// inside that part.
    func showEditor(shown: CGRect, whole: CGRect) {
        guard let editor else { return }
        opening.frame = shown
        editor.frame = whole
        opening.isHidden = shown.height <= 0 || shown.width <= 0
    }

    func hideEditor() {
        opening.isHidden = true
    }

    var editorIsShown: Bool { editor != nil && !opening.isHidden }

    /// Whether the editor is what is seen at `point`, in this view's own
    /// points.
    func editorIsShown(at point: NSPoint) -> Bool {
        editorIsShown && opening.frame.contains(point)
    }

    /// Where the editor is seen, in this view's points from its top left.
    var editorFrame: CGRect? { editorIsShown ? opening.frame : nil }
}
