import AppKit
import Bridge
import Notifications

/// Drives the running app from the command line, for checks that need the
/// real window, menus and terminal but no person:
///
///     Quill --script "wait:2;type:jj;menu:message.toggle_star;press:cmd-j" \
///           --dump-screen /path/to/out.txt
///
/// Steps, separated by `;`:
///
/// - `wait:SECONDS`
/// - `type:TEXT`      paste text into the terminal
/// - `press:KEY`      a real key event through the app, in Quill's notation
/// - `menu:COMMAND`   choose the menu item of a core command, by its name
/// - `resize:WxH`     resize the window, in points
/// - `toolbar:COMMAND` click the toolbar button of a core command
/// - `item:TITLE`     choose one of the shell's own menu items, by its title
/// - `choose:PATH,PATH` what the open panel answers when the core next asks
///   for files; no panel is ever shown to a script
/// - `drop:PATH,PATH`  hand files over as if dropped on the window
/// - `saveto:PATH`    where the save panel answers when the core next asks
///   to save a file; without it the answer is that the user cancelled. No
///   panel is ever shown to a script, so inside the sandbox the place must
///   be in the app's container
/// - `openurl:URL`    hand an address over as the Mac does when a `mailto:`
///   link is clicked in another application
/// - `notifyclick:ID` click the notification of this id, as the Mac reports
///   it
/// - `notifyaction:NAME,ID` click the button of this name on the notification
///   of this id
/// - `importvimrc:PATH` import this file as the user's vimrc, as if it had
///   been chosen in the panel; inside the sandbox it must be in the app's
///   container. Neither the alert nor the panel is ever shown to a script:
///   the menu item itself is answered as if the user had cancelled
/// - `typeeditor:TEXT` type text into the editor pane, as keys and not as a
///   paste, while Vim is running there
///
/// The copy of the vimrc is kept in the scratch config folder when the run
/// has one (`--core-env QUILL_CONFIG_DIR=...`), in its `vim` folder, and
/// only otherwise in the user's own.
///
/// The editor pane is the real one when the build has a Vim in it, and it
/// reaches nothing outside the window. In a build without one, the dump
/// says the editor is not available. One case does reach outside: a config
/// that says `vim = "script"` has the user's own `editor-command.sh` run,
/// outside the sandbox, as it would be for them. A scripted run only does
/// that when the config it is pointed at asks for it.
///
/// No notification is ever shown to a script, and the user is never asked
/// whether Quill may show them: what the core asked for is written to the
/// dump instead.
///
/// Spelling is checked by the Mac's own checker in a scripted run as in any
/// other: it shows nothing and changes nothing. The dump says how many of
/// the core's questions were answered.
///
/// `press:` reaches whatever has the keyboard, a pane as well as the
/// terminal: `press:right` steps a Quick Look pane, `press:space` plays or
/// pauses a player, `press:esc` closes any pane but the editor's, where every
/// key is Vim's.
///
/// With `--dump-screen`, the terminal's text and the shell's state are
/// written to the file afterwards and the app quits.
@MainActor
enum DebugScript {
    static func runIfAsked(_ options: LaunchOptions, on controller: MainWindowController) {
        guard options.script != nil || options.dumpScreenTo != nil else { return }
        let steps = (options.script ?? "wait:2").split(separator: ";").map(String.init)
        // A check must not open the user's browser. What would have been
        // opened is written to the dump instead.
        controller.linkOpener = { url in opened.append(url.absoluteString) }
        // Nor change what is on the user's clipboard.
        controller.clipboardWriter = { text in copied.append(text) }
        // Nor wait on a panel nobody is there to answer. The files named by
        // the last `choose:` step are what the user is taken to have chosen.
        controller.filePicker = { _, done in
            panelsAsked += 1
            done(chosen)
        }
        // Nor start another program on a file. What would have been opened
        // is written to the dump.
        controller.fileOpener = { file in openedFiles.append(file.path) }
        // Nor bring the Finder forward.
        controller.revealer = { file in revealed.append(file.path) }
        // Nor wait on a save panel. The place named by the last `saveto:`
        // step is where the user is taken to have saved, once; with none,
        // they are taken to have cancelled.
        controller.savePanel = { _, name, done in
            savePanelsAsked.append(name)
            let destination = saveDestination
            saveDestination = nil
            done(destination)
        }
        // Nor show a notification, nor ask the user whether Quill may.
        controller.notifier = notifier
        // Nor explain what importing a vimrc means and wait for a file to
        // be chosen: the user is taken to have cancelled. `importvimrc:`
        // is how a script imports one.
        controller.vimrcPicker = { _, done in
            vimrcPanelsAsked += 1
            done(nil)
        }
        // Nor wait on an alert about something that could not be done. What
        // went wrong is in the dump, where the shell noted it.
        controller.problemReporter = { _, _, _ in }
        Task { @MainActor in
            for step in steps {
                await perform(step, on: controller)
            }
            guard let destination = options.dumpScreenTo else { return }
            // Let the last step's effects reach the screen.
            try? await Task.sleep(for: .milliseconds(400))
            dump(controller, to: destination)
            controller.stopCore()
            try? await Task.sleep(for: .milliseconds(1500))
            exit(0)
        }
    }

    private static func perform(_ step: String, on controller: MainWindowController) async {
        let (verb, argument) = split(step)
        switch verb {
        case "wait":
            try? await Task.sleep(for: .seconds(Double(argument) ?? 1))
        case "type":
            controller.typeIntoTerminal(argument)
            try? await Task.sleep(for: .milliseconds(150))
        case "press":
            press(argument, in: controller.window)
            try? await Task.sleep(for: .milliseconds(150))
        case "resize":
            let parts = argument.split(separator: "x").compactMap { Double($0) }
            if parts.count == 2, let window = controller.window {
                var frame = window.frame
                frame.size = NSSize(width: parts[0], height: parts[1])
                window.setFrame(frame, display: true)
            } else {
                note("cannot resize to \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(300))
        case "menu":
            if let (menu, index) = menuItem(forCommand: argument) {
                menu.performActionForItem(at: index)
            } else {
                note("no menu item for \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(150))
        case "toolbar":
            let items = controller.window?.toolbar?.items ?? []
            if let item = items.first(where: { $0.itemIdentifier.rawValue == argument }), item.isEnabled {
                _ = NSApp.sendAction(item.action!, to: item.target, from: item)
            } else {
                note("no enabled toolbar item for \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(150))
        case "item":
            // A menu item of the shell's own, by its title.
            var found = false
            for top in NSApp.mainMenu?.items ?? [] {
                guard let menu = top.submenu,
                    let index = menu.items.firstIndex(where: { $0.title == argument })
                else { continue }
                menu.update()
                if menu.items[index].isEnabled {
                    menu.performActionForItem(at: index)
                } else {
                    note("\(argument) is disabled")
                }
                found = true
                break
            }
            if !found {
                note("no menu item titled \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(150))
        case "choose":
            // What the open panel will answer when it is next asked for.
            chosen = paths(argument)
        case "drop":
            controller.deliver(files: paths(argument))
            try? await Task.sleep(for: .milliseconds(300))
        case "saveto":
            // Where the save panel will answer when it is next asked for.
            saveDestination = argument.isEmpty ? nil : URL(fileURLWithPath: argument)
        case "openurl":
            // By the door the Mac itself uses.
            if let url = URL(string: argument), let open = NSApp.delegate?.application(_:open:) {
                open(NSApp, [url])
            } else {
                note("cannot open \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(300))
        case "notifyclick":
            controller.notificationUsed(id: argument, actionIdentifier: NotificationResponse.click)
            try? await Task.sleep(for: .milliseconds(300))
        case "notifyaction":
            // The name first: an id may have a comma in it.
            let (name, id) = split(argument, at: ",")
            controller.notificationUsed(id: id, actionIdentifier: name)
            try? await Task.sleep(for: .milliseconds(300))
        case "importvimrc":
            controller.importVimrc(from: URL(fileURLWithPath: argument))
        case "typeeditor":
            if !controller.typeIntoEditor(argument) {
                note("no editor to type \(argument) into")
            }
            try? await Task.sleep(for: .milliseconds(150))
        default:
            note("unknown step \(step)")
        }
    }

    private static func split(_ step: String, at separator: Character = ":") -> (String, String) {
        guard let colon = step.firstIndex(of: separator) else { return (step, "") }
        return (String(step[..<colon]), String(step[step.index(after: colon)...]))
    }

    private static var notes: [String] = []
    private static var opened: [String] = []
    private static var copied: [String] = []
    private static var chosen: [URL] = []
    private static var panelsAsked = 0
    private static var openedFiles: [String] = []
    private static var revealed: [String] = []
    private static var saveDestination: URL?
    private static var savePanelsAsked: [String] = []
    private static var vimrcPanelsAsked = 0
    private static let notifier = RecordingNotifier()

    /// Paths separated by commas.
    private static func paths(_ argument: String) -> [URL] {
        argument.split(separator: ",").map { URL(fileURLWithPath: String($0)) }
    }

    private static func note(_ text: String) {
        notes.append(text)
    }

    // MARK: - Finding and pressing things

    private static func menuItem(forCommand name: String) -> (NSMenu, Int)? {
        for top in NSApp.mainMenu?.items ?? [] {
            guard let menu = top.submenu else { continue }
            if let index = menu.items.firstIndex(where: { $0.representedObject as? String == name }) {
                return (menu, index)
            }
        }
        return nil
    }

    /// Key codes of a US keyboard, for the keys the checks press.
    private static let keyCodes: [String: UInt16] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
        "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "o": 31, "u": 32,
        "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
        "]": 30, "[": 33, "enter": 36, "tab": 48, "space": 49, "esc": 53,
        "up": 126, "down": 125, "left": 123, "right": 124,
        "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
    ]

    private static let characters: [String: String] = [
        "enter": "\r", "tab": "\t", "space": " ", "esc": "\u{1B}",
        "up": "\u{F700}", "down": "\u{F701}", "left": "\u{F702}", "right": "\u{F703}",
        "home": "\u{F729}", "end": "\u{F72B}", "pageup": "\u{F72C}", "pagedown": "\u{F72D}",
    ]

    /// What a real keyboard sets on these keys besides the modifiers held.
    private static let extraFlags: [String: NSEvent.ModifierFlags] = [
        "up": [.numericPad, .function], "down": [.numericPad, .function],
        "left": [.numericPad, .function], "right": [.numericPad, .function],
        "home": .function, "end": .function, "pageup": .function, "pagedown": .function,
    ]

    private static func press(_ notation: String, in window: NSWindow?) {
        guard let window, let chord = KeyNotation(notation)?.chords.first,
            let keyCode = keyCodes[chord.key.lowercased()]
        else {
            note("cannot press \(notation)")
            return
        }
        var flags = chord.modifiers.eventFlags
        flags.formUnion(extraFlags[chord.key.lowercased()] ?? [])
        if chord.key.count == 1, chord.key != chord.key.lowercased() {
            flags.insert(.shift)
        }
        let plain = characters[chord.key] ?? chord.key.lowercased()
        let typed = flags.contains(.shift) ? plain.uppercased() : plain
        for type in [NSEvent.EventType.keyDown, .keyUp] {
            guard
                let event = NSEvent.keyEvent(
                    with: type,
                    location: .zero,
                    modifierFlags: flags,
                    timestamp: ProcessInfo.processInfo.systemUptime,
                    windowNumber: window.windowNumber,
                    context: nil,
                    characters: typed,
                    charactersIgnoringModifiers: plain,
                    isARepeat: false,
                    keyCode: keyCode
                )
            else { continue }
            NSApp.sendEvent(event)
        }
    }

    // MARK: - The dump

    static func dump(_ controller: MainWindowController, to destination: URL?) {
        guard let destination else { return }
        var lines = [controller.terminalText, "", "--- shell ---"]
        lines.append("title: \(controller.window?.title ?? "")")
        lines.append("subtitle: \(controller.window?.subtitle ?? "")")
        lines.append("dock badge: \(NSApp.dockTile.badgeLabel ?? "")")
        if let pane = controller.pane {
            // preview "Report" item 2 of 5, report.pdf
            // player "Clip" paused at 0:05 of 1:15
            // web "Subject" remote content blocked, loaded
            // editor "Re: Q4 roadmap" running
            lines.append("pane: \(pane.kind.rawValue) \(pane.title.debugDescription) \(pane.summary)")
        } else {
            lines.append("pane: none")
        }
        let responder = controller.window?.firstResponder.map { String(describing: type(of: $0)) }
        lines.append("keys go to: \(responder ?? "nothing")")
        lines.append("opened: \(opened.joined(separator: " "))")
        lines.append("enabled: \(controller.commands.enabledNames.joined(separator: " "))")
        lines.append("file panels asked for: \(panelsAsked)")
        lines.append("copied: \(copied.map { $0.debugDescription }.joined(separator: " "))")
        lines.append("opened files: \(openedFiles.joined(separator: " "))")
        lines.append("revealed: \(revealed.joined(separator: " "))")
        lines.append("save panels asked for: \(savePanelsAsked.map { $0.debugDescription }.joined(separator: " "))")
        lines.append("saved: \(controller.saveResults.joined(separator: "; "))")
        lines.append("thumbnails: \(controller.thumbnailResults.joined(separator: ", "))")
        // notified: message:4127 | Ana Ruiz | Work | Q4 roadmap | actions: archive,read,reply
        // cleared: message:4127 message:4128
        lines.append(contentsOf: notifier.lines)
        lines.append("notifications used: \(controller.notificationsUsed.joined(separator: "; "))")
        lines.append("urls passed on: \(controller.urlsPassedOn.joined(separator: " "))")
        // How many of the core's questions about spelling have been
        // answered. The answers themselves are the core's to show.
        lines.append("spell checks: \(controller.spelling.checksAnswered)")
        lines.append("spell suggestions: \(controller.spelling.suggestionsAnswered)")
        // editor: none | running "Re: Q4 roadmap" | exited 0 | not available
        //         | failed to start
        // A Vim ended by a signal is "exited" with the signal below zero.
        lines.append("editor: \(controller.editorSlot.summary)")
        lines.append("editor requests: \(controller.editorSlot.requests)")
        // vimrc: none | imported, 1234 bytes
        // vimrc imports: /path (imported, 1234 bytes); /path (refused: the file is larger than 1 MB); removed
        lines.append("vimrc: \(controller.vimrc.size.map { "imported, \($0) bytes" } ?? "none")")
        lines.append("vimrc imports: \(controller.vimrcResults.joined(separator: "; "))")
        lines.append("vimrc panels asked for: \(vimrcPanelsAsked)")
        let toolbar = (controller.window?.toolbar?.items ?? [])
            .filter { $0.itemIdentifier != .flexibleSpace }
            .map { "\($0.isEnabled ? "+" : "-")\($0.itemIdentifier.rawValue)" }
        lines.append("toolbar: \(toolbar.joined(separator: " "))")
        for top in NSApp.mainMenu?.items ?? [] {
            guard let menu = top.submenu else { continue }
            let items = menu.items.filter { !$0.isSeparatorItem }.map { item -> String in
                let isCommand = item.representedObject is String
                let state = isCommand ? (item.isEnabled ? "+" : "-") : ""
                var key = ""
                if !item.keyEquivalent.isEmpty {
                    key = " <\(item.keyEquivalentModifierMask.contains(.command) ? "cmd-" : "")\(item.keyEquivalent)>"
                } else if #available(macOS 14.0, *), let badge = item.badge?.stringValue {
                    key = " [\(badge)]"
                }
                return "\(state)\(item.title)\(key)"
            }
            lines.append("menu \(menu.title): \(items.joined(separator: " | "))")
        }
        if !notes.isEmpty {
            lines.append("notes: \(notes.joined(separator: "; "))")
        }
        // What Vim is showing, while it runs.
        if let editorText = controller.editorText {
            lines.append(contentsOf: ["", "--- editor ---", editorText])
        }
        // A picture of the window beside the text, for a person to look at.
        if let frame = controller.window?.contentView?.superview,
            let bitmap = frame.bitmapImageRepForCachingDisplay(in: frame.bounds)
        {
            frame.cacheDisplay(in: frame.bounds, to: bitmap)
            try? bitmap.representation(using: .png, properties: [:])?
                .write(to: destination.appendingPathExtension("png"))
        }
        do {
            try Data(lines.joined(separator: "\n").utf8).write(to: destination)
        } catch {
            FileHandle.standardError.write(Data("quill: cannot write \(destination.path): \(error)\n".utf8))
        }
    }
}
