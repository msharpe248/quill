import AppKit
import Bridge

/// The shell's whole knowledge of what the app can do: the commands the core
/// announced, and which of them apply right now.
///
/// It turns them into menu items and toolbar buttons and sends a
/// `command.run` when one is used. It knows names, titles and keys. It does
/// not know what any command does.
@MainActor
final class CommandCenter: NSObject {
    private(set) var commands: [CommandInfo] = []
    private var enabled: Set<String> = []
    private var menuItems: [String: NSMenuItem] = [:]
    private var toolbarItems: [String: NSToolbarItem] = [:]

    /// Where `command.run` goes.
    var send: ((ShellToCore) -> Void)?

    func setCommands(_ commands: [CommandInfo]) {
        self.commands = commands
        menuItems.removeAll()
        toolbarItems.removeAll()
    }

    func setEnabled(_ names: [String]) {
        enabled = Set(names)
        for (name, item) in menuItems {
            item.isEnabled = enabled.contains(name)
        }
        for (name, item) in toolbarItems {
            item.isEnabled = enabled.contains(name)
        }
    }

    /// The commands that carry a symbol, in the order the core listed them.
    var toolbarCommands: [CommandInfo] {
        commands.filter { $0.symbol != nil }
    }

    /// Menu items for the commands the core placed in `menu`.
    func makeMenuItems(for menu: Bridge.Menu) -> [NSMenuItem] {
        commands.filter { $0.menu == menu }.map { command in
            let item = NSMenuItem(title: Self.menuTitle(command.title), action: #selector(run(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = command.name
            item.isEnabled = enabled.contains(command.name)
            if let symbol = command.symbol {
                item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
            }
            if let key = command.keys.first.flatMap(KeyNotation.init) {
                if let shortcut = key.menuShortcut {
                    // A cmd chord: the menu owns it and the terminal never sees it.
                    item.keyEquivalent = shortcut.key
                    item.keyEquivalentModifierMask = shortcut.modifiers.eventFlags
                } else if #available(macOS 14.0, *) {
                    // Any other key belongs to the terminal. Show it without
                    // claiming it, so pressing it still reaches the core.
                    item.badge = NSMenuItemBadge(string: key.display)
                } else {
                    item.title += "  (\(key.display))"
                }
            }
            menuItems[command.name] = item
            return item
        }
    }

    func makeToolbarItem(for name: String) -> NSToolbarItem? {
        guard let command = commands.first(where: { $0.name == name }), let symbol = command.symbol else {
            return nil
        }
        let item = NSToolbarItem(itemIdentifier: NSToolbarItem.Identifier(name))
        item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: command.title)
        item.label = Self.menuTitle(command.title)
        item.paletteLabel = item.label
        let key = command.keys.first.flatMap(KeyNotation.init)?.display
        item.toolTip = key.map { "\(command.title) (\($0))" } ?? command.title
        item.isBordered = true
        item.target = self
        item.action = #selector(run(_:))
        // Enabled from the core's state, not by asking the responder chain.
        item.autovalidates = false
        item.isEnabled = enabled.contains(name)
        toolbarItems[name] = item
        return item
    }

    @objc private func run(_ sender: Any?) {
        let name: String? =
            switch sender {
            case let item as NSMenuItem: item.representedObject as? String
            case let item as NSToolbarItem: item.itemIdentifier.rawValue
            default: nil
            }
        if let name {
            send?(.commandRun(name: name))
        }
    }

    /// "Zoom the focused pane" reads as "Zoom the Focused Pane" in a menu.
    static func menuTitle(_ title: String) -> String {
        let small: Set<String> = ["a", "an", "and", "for", "of", "or", "the", "this", "to"]
        return title.split(separator: " ").enumerated().map { index, word in
            let lower = word.lowercased()
            if index > 0, small.contains(lower) { return lower }
            return word.prefix(1).uppercased() + word.dropFirst()
        }
        .joined(separator: " ")
    }
}

extension KeyNotation.Modifiers {
    var eventFlags: NSEvent.ModifierFlags {
        var flags: NSEvent.ModifierFlags = []
        if contains(.control) { flags.insert(.control) }
        if contains(.option) { flags.insert(.option) }
        if contains(.shift) { flags.insert(.shift) }
        if contains(.command) { flags.insert(.command) }
        return flags
    }
}
