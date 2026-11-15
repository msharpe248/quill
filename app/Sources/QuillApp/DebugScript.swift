import AppKit
import Bridge

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
///
/// With `--dump-screen`, the terminal's text and the shell's state are
/// written to the file afterwards and the app quits.
@MainActor
enum DebugScript {
    static func runIfAsked(_ options: LaunchOptions, on controller: MainWindowController) {
        guard options.script != nil || options.dumpScreenTo != nil else { return }
        let steps = (options.script ?? "wait:2").split(separator: ";").map(String.init)
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
        default:
            note("unknown step \(step)")
        }
    }

    private static func split(_ step: String) -> (String, String) {
        guard let colon = step.firstIndex(of: ":") else { return (step, "") }
        return (String(step[..<colon]), String(step[step.index(after: colon)...]))
    }

    private static var notes: [String] = []

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
    ]

    private static let characters: [String: String] = [
        "enter": "\r", "tab": "\t", "space": " ", "esc": "\u{1B}",
    ]

    private static func press(_ notation: String, in window: NSWindow?) {
        guard let window, let chord = KeyNotation(notation)?.chords.first,
            let keyCode = keyCodes[chord.key.lowercased()]
        else {
            note("cannot press \(notation)")
            return
        }
        var flags = chord.modifiers.eventFlags
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
        lines.append("preview: \(controller.previewPane?.fileURL?.lastPathComponent ?? "")")
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
