import AppKit
import Bridge
import Notifications
import Panes
import WebKit

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
/// - `clicktext:TEXT` click the terminal where this text first is on its
///   screen, with mouse events sent to the window; `clicktext:2,TEXT` clicks
///   the second place it is at. The pointer is not moved
/// - `clickeditor:`   click the middle of the editor that has the text of
///   the message being written, where it lies over the terminal, with mouse
///   events sent to the window. The pointer is not moved
/// - `notice:COMMAND` click the button of a core command in the notice above
///   the page in the reading pane
/// - `item:TITLE`     choose one of the shell's own menu items, by its title
/// - `choose:PATH,PATH` what the open panel answers when the core next asks
///   for files; no panel is ever shown to a script
/// - `drop:PATH,PATH`  hand files over as if dropped on the window
/// - `saveto:PATH`    where the save panel answers when the core next asks
///   to save a file; without it the answer is that the user cancelled. No
///   panel is ever shown to a script
/// - `openurl:URL`    hand an address over as the Mac does when a `mailto:`
///   link is clicked in another application
/// - `notifyclick:ID` click the notification of this id, as the Mac reports
///   it
/// - `notifyaction:NAME,ID` click the button of this name on the notification
///   of this id
/// - `keys:close`     close the window of keys, which `menu:help.keys`
///   brings up; `keysearch:TEXT` types into its search field. Its picture
///   is written with `.keys.png` after the dump's name
/// - `typeeditor:TEXT` type text into the editor that has the text of the
///   message being written, as keys and not as a paste, wherever the
///   keyboard is
/// - `sheetset:FIELD=VALUE` put a value in a field of the form in the sheet,
///   by the field's name, as typing it does; for a pop-up the value is the
///   option's name, and choosing it tells the core, as choosing it by hand
///   does
/// - `sheetpress:BUTTON` press a button of the form in the sheet, by its name
/// - `divider:COLUMNS` put the divider between the terminal and the pane
///   where the terminal has this many columns, as a drag that ended there
///   does; `divider:reset` puts it back, as a double-click does
/// - `dragdivider:COLUMNS` the same drag, done with mouse events sent to the
///   window: press on the divider, move, let go. `clickdivider:2`
///   double-clicks the divider the same way. Neither moves the pointer
/// - `snap:NAME`      write the dump as it stands now, to the `--dump-screen`
///   file with `.NAME` added to its name (and its picture beside it), and go
///   on: several states of one run can then be checked from one launch
/// - `settings:open`  open the settings window, as Settings in the Quill menu
///   does; `settings:close` closes it, as its close button does, and
///   `settings:file` presses its button that opens the settings file
/// - `settingspane:PANE` choose a pane of the settings window by its name, as
///   a click on its toolbar button does
/// - `settingsset:PANE.FIELD=VALUE` give a control of the settings window a
///   value, as the user does: the name of an option for a pop-up, or what the
///   option is called (a theme the user named has a made-up name), `on` or
///   `off` for a checkbox, what is typed for a field
/// - `settingsstep:PANE.FIELD=up` click the stepper of a number field once,
///   up or `down`
/// - `settingspick:PANE.FIELD=TITLE` pick the row of a list that has this
///   title, as a click on it does; with no title, none
/// - `settingspress:PANE.FIELD.ACTION` press a button of a list
/// - `settingskey:KEY` a real key event to the settings window, in Quill's
///   notation: `settingskey:esc`, `settingskey:cmd-w`
/// - `settingspress:PANE.FIELD` also presses a button that is a field of its
///   own, as Import Theme is
/// - `themes:open`    open the chooser of themes to import, as Import Theme in
///   the Quill menu does; `themes:import`, `themes:other` and `themes:cancel`
///   press its buttons
/// - `themesearch:TEXT` type into the chooser's search field, in place of what
///   is there
/// - `themescope:dark` list only the dark themes in the chooser, or `light`,
///   or `all`
/// - `themepick:NAME` pick the row of the theme with this name, as a click on
///   it does
/// - `themefile:PATH` what the open panel answers when Other is next pressed
///   in the chooser; without it the answer is that the user cancelled. No
///   panel is ever shown to a script
/// - `importtheme:PATH` import this theme file, as if it had been chosen
/// - `probefolder:PATH` note whether the app can list the folder at this path,
///   and how many things are in it: for finding out what the App Sandbox lets
///   the app see. Nothing in the folder is opened, and no name is written
/// - `system:dark`    as if the Mac had gone dark; `system:light`, light. The
///   Mac itself is not touched, and a scripted run never asks what it is: it
///   is light until a step says otherwise
///
/// The widths are in the dump, so that the place of the reading pane can be
/// checked without a person: the terminal's columns and the width of its
/// cells, how many columns the window has room for and what the core was
/// told, where the core says the reading pane goes and where the user left
/// the divider, and the width in columns of each box on the terminal's
/// first row (the panes the core has drawn). A scripted run remembers a
/// dragged divider for the run only: it starts with none, at the same
/// window size every time, and neither reads nor writes the user's own
/// defaults.
///
/// So are the colours the terminals were last given, the core's and the
/// editor's alike: the background and the text of the core's theme, or
/// `its own` for each one the theme leaves to the terminal.
///
/// The reading pane is in the dump: whether web reading is on, the page the
/// core last asked it to show (its title, the last of its path, whether it
/// may load remote content, how far it has loaded, how often it was
/// scrolled) and the lines above it. A web view's page is drawn into the
/// picture from the web view's own snapshot of itself.
///
/// The sheet is the real one, on the window, whichever of the core's
/// questions it is showing (the form that adds an account, a sign-in, a
/// certificate, a time, unsubscribing); `press:` reaches it while it is up,
/// so `press:enter` is its default button and `press:esc` its cancel. The
/// dump says whether it is up, its step, what it says, and each field with
/// what it holds. What a secret field holds is never written: the dump has a
/// fixed marker in its place, and the picture shows bullets. Nor is a code
/// to sign in with, or the page to type it at: the dump has the same marker
/// for each, and only the picture shows them.
///
/// The settings window is the real one. The dump says whether it is open,
/// what it is called, how large it is and whether it can be resized, its
/// toolbar, what each control shows, what the core was told, and every pane
/// the core described with each field and what it holds; its picture is
/// written beside the window's, with `.settings.png` after the dump's name.
/// Which pane was last on show is kept for the run only: a scripted run
/// neither reads nor writes the user's own.
///
/// The chooser of themes is the real one, a sheet on the window it was asked
/// for from. The dump says whether it is up, what is typed in its search
/// field, how many themes it lists and the first of them, which is picked,
/// and where themes were looked for with what was found in each place; its
/// picture is written with `.themes.png` after the dump's name. The dump also
/// says whether the app is light or dark and what the core was told the Mac
/// is, and what became of each theme imported. A theme is copied into the
/// scratch config folder's `themes` folder when the run has one, and only
/// otherwise into the user's own.
///
/// The editor is the real one: the user's own, as their settings name it
/// or as it is found on the `PATH`, in a terminal laid over the core's, with
/// the user's own settings for it. A check that must not read or write
/// those gives the run a home folder of its own, which the editor is given
/// too: `--core-env HOME=...`.
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
/// pauses a player, `press:esc` closes a viewer's pane. While the editor has
/// the keyboard every key is its own, `esc` too; a `cmd` key that a menu has
/// is still the menu's.
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
        // Nor wait on a panel for a theme file: the file named by the last
        // `themefile:` step is what the user is taken to have chosen, once.
        controller.themeFilePicker = { _, done in
            themePanelsAsked += 1
            let file = themeFile
            themeFile = nil
            done(file)
        }
        // Nor wait on an alert about something that could not be done. What
        // went wrong is in the dump, where the shell noted it.
        controller.problemReporter = { _, _, _ in }
        Task { @MainActor in
            for step in steps {
                await perform(step, on: controller, dumpingTo: options.dumpScreenTo)
            }
            guard let destination = options.dumpScreenTo else { return }
            // Let the last step's effects reach the screen.
            try? await Task.sleep(for: .milliseconds(400))
            await dumpWithPages(controller, to: destination)
            controller.stopCore()
            try? await Task.sleep(for: .milliseconds(1500))
            exit(0)
        }
    }

    private static func perform(
        _ step: String, on controller: MainWindowController, dumpingTo destination: URL?
    ) async {
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
        case "clicktext":
            var (text, nth) = (argument, 1)
            if let comma = argument.firstIndex(of: ","), let count = Int(argument[..<comma]) {
                (text, nth) = (String(argument[argument.index(after: comma)...]), count)
            }
            if let window = controller.window, let at = controller.terminalPoint(ofText: text, nth: nth) {
                postMouse(.leftMouseDown, at: at, clicks: 1, in: window)
                postMouse(.leftMouseUp, at: at, clicks: 1, in: window)
            } else {
                note("\(text.debugDescription) is not on the terminal's screen to click")
            }
            try? await Task.sleep(for: .milliseconds(400))
        case "clickeditor":
            if let window = controller.window, let at = controller.editorPoint {
                postMouse(.leftMouseDown, at: at, clicks: 1, in: window)
                postMouse(.leftMouseUp, at: at, clicks: 1, in: window)
            } else {
                note("no editor on show to click")
            }
            try? await Task.sleep(for: .milliseconds(400))
        case "notice":
            if !controller.pressReadingNotice(argument) {
                note("no notice button for \(argument)")
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
        case "keys":
            // `keys:close` closes the window of keys, as its close button
            // does. It is brought up from the Help menu, with `menu:`.
            if argument == "close" {
                controller.keysWindow.close()
            } else {
                note("keys:\(argument) is not a step")
            }
            try? await Task.sleep(for: .milliseconds(200))
        case "keysearch":
            if !controller.keysWindow.search(for: argument) {
                note("no window of keys to search for \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(200))
        case "typeeditor":
            if !controller.typeIntoEditor(argument) {
                note("no editor to type \(argument) into")
            }
            try? await Task.sleep(for: .milliseconds(150))
        case "sheetset":
            // The name first: a value may have an equals sign in it. The
            // value is not noted when it cannot be set: it may be a secret.
            let (field, value) = split(argument, at: "=")
            if !controller.formSheet.set(field, to: value) {
                note("no field \(field) in the sheet to set")
            }
            try? await Task.sleep(for: .milliseconds(300))
        case "sheetpress":
            if !controller.formSheet.press(argument) {
                note("no button \(argument) in the sheet to press")
            }
            try? await Task.sleep(for: .milliseconds(300))
        case "divider":
            if argument == "reset" {
                controller.doubleClickDivider()
            } else if let columns = Int(argument), controller.dragDivider(toColumns: columns) {
                // Dragged, as far as the window is concerned.
            } else {
                note("cannot drag the divider to \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(300))
        case "dragdivider":
            if let columns = Int(argument), let window = controller.window,
                let (from, to) = controller.dividerDrag(toColumns: columns)
            {
                let halfway = NSPoint(x: (from.x + to.x) / 2, y: from.y)
                postMouse(.leftMouseDown, at: from, clicks: 1, in: window)
                postMouse(.leftMouseDragged, at: halfway, clicks: 1, in: window)
                postMouse(.leftMouseDragged, at: to, clicks: 1, in: window)
                postMouse(.leftMouseUp, at: to, clicks: 1, in: window)
            } else {
                note("cannot drag the divider to \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(600))
        case "clickdivider":
            if let clicks = Int(argument), clicks > 0, let window = controller.window,
                let (at, _) = controller.dividerDrag(toColumns: 1)
            {
                for click in 1...clicks {
                    postMouse(.leftMouseDown, at: at, clicks: click, in: window)
                    postMouse(.leftMouseUp, at: at, clicks: click, in: window)
                }
            } else {
                note("cannot click the divider \(argument) times")
            }
            try? await Task.sleep(for: .milliseconds(600))
        case "settings":
            switch argument {
            case "open":
                controller.showSettings(nil)
            case "close":
                controller.settingsWindow.close()
            case "file":
                if !controller.settingsWindow.pressFileButton() {
                    note("no settings window to open the file from")
                }
            default:
                note("unknown step \(step)")
            }
            try? await Task.sleep(for: .milliseconds(500))
        case "settingspane":
            if !controller.settingsWindow.choose(pane: argument) {
                note("no settings pane \(argument)")
            }
            try? await Task.sleep(for: .milliseconds(400))
        case "settingsset":
            let (name, value) = split(argument, at: "=")
            if !controller.settingsWindow.set(name, to: value) {
                note("cannot set \(name) to \(value) in the settings")
            }
            try? await Task.sleep(for: .milliseconds(400))
        case "settingsstep":
            let (name, way) = split(argument, at: "=")
            if !controller.settingsWindow.step(name, up: way != "down") {
                note("no stepper for \(name) in the settings")
            }
            try? await Task.sleep(for: .milliseconds(400))
        case "settingspick":
            let (name, title) = split(argument, at: "=")
            if !controller.settingsWindow.pick(name, rowTitled: title) {
                note("no row \(title) in \(name) in the settings")
            }
            try? await Task.sleep(for: .milliseconds(200))
        case "settingspress":
            if !controller.settingsWindow.press(argument) {
                note("no button \(argument) to press in the settings")
            }
            try? await Task.sleep(for: .milliseconds(400))
        case "settingskey":
            if let window = controller.settingsWindow.windowIfOpen {
                press(argument, in: window)
            } else {
                note("no settings window to press \(argument) in")
            }
            try? await Task.sleep(for: .milliseconds(400))
        case "themes":
            switch argument {
            case "open":
                controller.importTheme(nil)
            case "import", "other", "cancel":
                if !controller.themeChooser.press(argument) {
                    note("cannot press \(argument) in the theme chooser")
                }
            default:
                note("unknown step \(step)")
            }
            // The themes are looked for off the main thread, and a theme
            // chosen is copied in there too.
            try? await Task.sleep(for: .milliseconds(900))
        case "themesearch":
            if !controller.themeChooser.search(for: argument) {
                note("no theme chooser to search in")
            }
            try? await Task.sleep(for: .milliseconds(200))
        case "themescope":
            if !controller.themeChooser.show(scope: argument) {
                note("cannot show \(argument) themes in the chooser")
            }
            try? await Task.sleep(for: .milliseconds(200))
        case "themepick":
            if !controller.themeChooser.pick(named: argument) {
                note("no theme \(argument) in the chooser to pick")
            }
            try? await Task.sleep(for: .milliseconds(200))
        case "themefile":
            // What the open panel will answer when it is next asked for.
            themeFile = argument.isEmpty ? nil : URL(fileURLWithPath: argument)
        case "importtheme":
            controller.importTheme(from: URL(fileURLWithPath: argument))
            try? await Task.sleep(for: .milliseconds(900))
        case "probefolder":
            do {
                let count = try FileManager.default.contentsOfDirectory(atPath: argument).count
                note("\(argument) can be listed (\(count) things)")
            } catch {
                note("\(argument) cannot be listed (\((error as NSError).localizedFailureReason ?? "no reason given"))")
            }
        case "system":
            switch argument {
            case "dark": controller.pretendSystem(dark: true)
            case "light": controller.pretendSystem(dark: false)
            default: note("unknown step \(step)")
            }
            try? await Task.sleep(for: .milliseconds(500))
        case "snap":
            let plain = !argument.isEmpty && argument.allSatisfy { $0.isLetter || $0.isNumber || $0 == "-" }
            if let destination, plain {
                await dumpWithPages(controller, to: destination.appendingPathExtension(argument))
            } else {
                note("cannot snap \(argument)")
            }
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
    private static var themeFile: URL?
    private static var themePanelsAsked = 0
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
        // A key goes to the sheet while one is up, as it does for a person.
        guard let window = window?.attachedSheet ?? window, let chord = KeyNotation(notation)?.chords.first,
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

    private static var mouseEvents = 0

    /// Queues a mouse event for the window, at a place in it. The pointer
    /// itself is not moved, and no other application sees it.
    private static func postMouse(_ type: NSEvent.EventType, at point: NSPoint, clicks: Int, in window: NSWindow) {
        mouseEvents += 1
        guard
            let event = NSEvent.mouseEvent(
                with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                windowNumber: window.windowNumber, context: nil, eventNumber: mouseEvents, clickCount: clicks,
                pressure: type == .leftMouseUp ? 0 : 1)
        else {
            note("no mouse event could be made")
            return
        }
        window.postEvent(event, atStart: false)
    }

    // MARK: - The dump

    /// The width, in characters, of each box drawn on the terminal's first
    /// row: from a top-left corner to the next top-right one. Nothing in
    /// the titles the core draws there is wider than a cell.
    static func boxWidths(in text: String) -> [Int] {
        guard let row = text.split(separator: "\n", omittingEmptySubsequences: false).first else { return [] }
        var widths: [Int] = []
        var start: Int?
        for (index, character) in row.enumerated() {
            if "\u{250C}\u{250F}\u{256D}\u{2554}".contains(character) {
                start = index
            } else if "\u{2510}\u{2513}\u{256E}\u{2557}".contains(character), let from = start {
                widths.append(index - from + 1)
                start = nil
            }
        }
        return widths
    }

    /// The dump, with the page of a web view on show drawn into the
    /// picture: a web view draws in a process of its own, and is asked for
    /// a picture of itself.
    static func dumpWithPages(_ controller: MainWindowController, to destination: URL?) async {
        var page: (NSImage, NSRect)?
        if let pane = controller.pane as? WebPane, let picture = await pane.picture() {
            page = (picture, pane.pageFrame)
        }
        dump(controller, to: destination, page: page)
    }

    static func dump(_ controller: MainWindowController, to destination: URL?, page: (NSImage, NSRect)? = nil) {
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
        // terminal: 147 columns, 45 rows, cells 16 px at 2.0x, view 1180.0 pt of 1180.0
        lines.append("terminal: \(controller.gridSummary)")
        // terminal colours: background #1a1b26, foreground #c0caf5
        lines.append("terminal colours: \(controller.terminalColours)")
        // window columns: 147 (core told 147)
        let room = controller.windowColumns.map(String.init) ?? "unknown"
        let told = controller.windowColumnsSent.map(String.init) ?? "nothing"
        lines.append("window columns: \(room) (core told \(told))")
        // reading place: core says 85 columns, divider left at none
        let asked = controller.readingColumnsAsked.map { "\($0) columns" } ?? "nothing"
        let left = controller.dividerMemory.columns.map { "\($0) columns" } ?? "none"
        lines.append("reading place: core says \(asked), divider left at \(left)")
        // boxes: 24 61 62
        lines.append("boxes: \(boxWidths(in: controller.terminalText).map(String.init).joined(separator: " "))")
        // reading: off
        // reading: on, "Subject", m12/page/message.html, remote content blocked, loaded, header: From: ... | To: ...
        lines.append("reading: \(controller.readingSummary)")
        lines.append("reading requests: \(controller.readingRequests)")
        let responder = controller.window?.firstResponder.map { String(describing: type(of: $0)) }
        lines.append("keys go to: \(responder ?? "nothing")")
        lines.append("opened: \(opened.joined(separator: " "))")
        lines.append("enabled: \(controller.commands.enabledNames.joined(separator: " "))")
        // The commands that are switched on: their menu items are checked.
        lines.append("checked: \(controller.commands.checkedNames.joined(separator: " "))")
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
        // editor place: column 70 row 7, 76 by 30, 0 covered; at 595.0,122.5 646.0 by 525.0 pt; 76 columns,
        // 30 rows; keys said for the editor, with the editor
        lines.append("editor place: \(controller.editorPlaceSummary)")
        // look: setting system, Mac pretended light, app light, core told light
        lines.append("look: \(controller.lookSummary)")
        // theme chooser: closed | up, with what is typed, how many it lists,
        // the first of them and which is picked; then where themes were
        // looked for, and what was found in each place.
        lines.append(contentsOf: controller.themeChooser.summary)
        // themes imported: /path/Dracula (imported as "Dracula"); /path/notes (refused: ...)
        lines.append("themes imported: \(controller.themeResults.joined(separator: "; "))")
        lines.append("theme panels asked for: \(themePanelsAsked)")
        // sheet: none
        // sheet: up, then its step, title, note, error and busy, a line for
        // each field (`sheet field name [text] = "Work" (focus)`), and its
        // buttons, which field of it has the keyboard, and how many times a
        // sheet has been opened in this run. A secret field is
        // `[secret] = <secret>` whatever it holds.
        lines.append(contentsOf: controller.formSheet.summary)
        // settings window: closed | open, titled "General", with its size,
        // its toolbar, what each control shows and what the core was told;
        // then what the core described: a line for each pane and for each
        // field (`settings field general.theme [choice] = dark (dark|light)`).
        lines.append(contentsOf: controller.settingsWindow.summary)
        // keys window: closed | open, with its title and size, whether it
        // floats and has the keyboard, what is typed in its search field,
        // and a line for every heading and every command it shows.
        lines.append(contentsOf: controller.keysWindow.summary)
        let toolbar = (controller.window?.toolbar?.items ?? [])
            .filter { $0.itemIdentifier != .flexibleSpace }
            .map { "\($0.isEnabled ? "+" : "-")\($0.itemIdentifier.rawValue)" }
        lines.append("toolbar: \(toolbar.joined(separator: " "))")
        for top in NSApp.mainMenu?.items ?? [] {
            guard let menu = top.submenu else { continue }
            let items = menu.items.filter { !$0.isSeparatorItem }.map { item -> String in
                let isCommand = item.representedObject is String
                var state = isCommand ? (item.isEnabled ? "+" : "-") : ""
                if isCommand, item.state == .on {
                    state += "\u{2713}"
                }
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
            // A web view's page, where the web view is in the window.
            if let (picture, place) = page, let context = NSGraphicsContext(bitmapImageRep: bitmap) {
                NSGraphicsContext.saveGraphicsState()
                NSGraphicsContext.current = context
                picture.draw(in: place, from: .zero, operation: .copy, fraction: 1, respectFlipped: true, hints: nil)
                NSGraphicsContext.restoreGraphicsState()
            }
            // A sheet is a window of its own: it is drawn into the picture
            // where it lies over the window, and saved by itself as well,
            // to be looked at closely. The picture is what each window
            // drew, put together here: the shadow, the rounded corners and
            // the dimming the Mac gives a sheet are not in it.
            if let window = controller.window, let sheet = controller.formSheet.window,
                let sheetFrame = sheet.contentView?.superview,
                let sheetBitmap = sheetFrame.bitmapImageRepForCachingDisplay(in: sheetFrame.bounds)
            {
                sheetFrame.cacheDisplay(in: sheetFrame.bounds, to: sheetBitmap)
                try? sheetBitmap.representation(using: .png, properties: [:])?
                    .write(to: destination.appendingPathExtension("sheet.png"))
                let origin = NSPoint(
                    x: sheet.frame.minX - window.frame.minX, y: sheet.frame.minY - window.frame.minY)
                // Into this picture only when it is this window's sheet: one
                // on the settings window is in that window's picture.
                if window.attachedSheet === sheet, let context = NSGraphicsContext(bitmapImageRep: bitmap) {
                    NSGraphicsContext.saveGraphicsState()
                    NSGraphicsContext.current = context
                    sheetBitmap.draw(
                        in: NSRect(origin: origin, size: sheetFrame.bounds.size), from: .zero,
                        operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
                    NSGraphicsContext.restoreGraphicsState()
                }
            }
            // The chooser of themes is a sheet too, and is drawn where it
            // lies when it is this window's.
            if let window = controller.window, let chooser = controller.themeChooser.window,
                window.attachedSheet === chooser, let chooserFrame = chooser.contentView?.superview,
                let chooserBitmap = chooserFrame.bitmapImageRepForCachingDisplay(in: chooserFrame.bounds),
                let context = NSGraphicsContext(bitmapImageRep: bitmap)
            {
                chooserFrame.cacheDisplay(in: chooserFrame.bounds, to: chooserBitmap)
                let origin = NSPoint(
                    x: chooser.frame.minX - window.frame.minX, y: chooser.frame.minY - window.frame.minY)
                NSGraphicsContext.saveGraphicsState()
                NSGraphicsContext.current = context
                chooserBitmap.draw(
                    in: NSRect(origin: origin, size: chooserFrame.bounds.size), from: .zero,
                    operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
                NSGraphicsContext.restoreGraphicsState()
            }
            try? bitmap.representation(using: .png, properties: [:])?
                .write(to: destination.appendingPathExtension("png"))
        }
        // The chooser by itself as well, to be looked at closely.
        if let chooser = controller.themeChooser.window, let frame = chooser.contentView?.superview,
            let bitmap = frame.bitmapImageRepForCachingDisplay(in: frame.bounds)
        {
            frame.cacheDisplay(in: frame.bounds, to: bitmap)
            try? bitmap.representation(using: .png, properties: [:])?
                .write(to: destination.appendingPathExtension("themes.png"))
        }
        // The settings window is a window of its own, and has a picture of
        // its own: what it drew, title bar, toolbar and all, with a sheet
        // on it drawn where it lies. As with the main window's, the Mac's
        // shadow and corners are not in it.
        if let settings = controller.settingsWindow.windowIfOpen, let frame = settings.contentView?.superview,
            let bitmap = frame.bitmapImageRepForCachingDisplay(in: frame.bounds)
        {
            frame.cacheDisplay(in: frame.bounds, to: bitmap)
            if let sheet = settings.attachedSheet, let sheetFrame = sheet.contentView?.superview,
                let sheetBitmap = sheetFrame.bitmapImageRepForCachingDisplay(in: sheetFrame.bounds),
                let context = NSGraphicsContext(bitmapImageRep: bitmap)
            {
                sheetFrame.cacheDisplay(in: sheetFrame.bounds, to: sheetBitmap)
                let origin = NSPoint(
                    x: sheet.frame.minX - settings.frame.minX, y: sheet.frame.minY - settings.frame.minY)
                NSGraphicsContext.saveGraphicsState()
                NSGraphicsContext.current = context
                sheetBitmap.draw(
                    in: NSRect(origin: origin, size: sheetFrame.bounds.size), from: .zero,
                    operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
                NSGraphicsContext.restoreGraphicsState()
            }
            try? bitmap.representation(using: .png, properties: [:])?
                .write(to: destination.appendingPathExtension("settings.png"))
        }
        // The window of keys, likewise a window of its own.
        if let keys = controller.keysWindow.windowIfOpen, let frame = keys.contentView?.superview,
            let bitmap = frame.bitmapImageRepForCachingDisplay(in: frame.bounds)
        {
            frame.cacheDisplay(in: frame.bounds, to: bitmap)
            try? bitmap.representation(using: .png, properties: [:])?
                .write(to: destination.appendingPathExtension("keys.png"))
        }
        do {
            try Data(lines.joined(separator: "\n").utf8).write(to: destination)
        } catch {
            FileHandle.standardError.write(Data("quill: cannot write \(destination.path): \(error)\n".utf8))
        }
    }
}
