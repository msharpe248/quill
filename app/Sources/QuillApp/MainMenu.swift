import AppKit
import Bridge

/// Builds the menu bar: the items every Mac app has, which the shell owns,
/// around the items for the commands the core announced.
@MainActor
enum MainMenu {
    static func build(commands: CommandCenter, panes: AnyObject?) -> NSMenu {
        let bar = NSMenu()

        // Quill
        let app = submenu(of: bar, title: "Quill")
        app.addItem(
            withTitle: "About Quill", action: #selector(NSApplication.orderFrontStandardAboutPanel(_:)),
            keyEquivalent: "")
        app.addItem(.separator())
        append(commands.makeMenuItems(for: .app), to: app, thenSeparator: true)
        // The config is a file in the app's container, which is not a
        // place anyone finds by looking. These two get to it.
        let editConfig = app.addItem(
            withTitle: "Edit Config", action: #selector(MainWindowController.editConfig(_:)), keyEquivalent: ",")
        editConfig.target = panes
        let revealConfig = app.addItem(
            withTitle: "Reveal Config in Finder", action: #selector(MainWindowController.revealConfig(_:)),
            keyEquivalent: "")
        revealConfig.target = panes
        // The vimrc for writing mail in Vim is copied in by the shell: the
        // core cannot be shown a file outside the sandbox, and has no need
        // of this one.
        // Where the user's own script for their own Vim goes: a folder the
        // app can read and only the user can write to.
        let scriptFolder = app.addItem(
            withTitle: "Show Editor Script Folder",
            action: #selector(MainWindowController.showEditorScriptFolder(_:)), keyEquivalent: "")
        scriptFolder.target = panes
        let importVimrc = app.addItem(
            withTitle: "Import vimrc\u{2026}", action: #selector(MainWindowController.importVimrc(_:)),
            keyEquivalent: "")
        importVimrc.target = panes
        let removeVimrc = app.addItem(
            withTitle: "Remove Imported vimrc", action: #selector(MainWindowController.removeImportedVimrc(_:)),
            keyEquivalent: "")
        removeVimrc.target = panes
        app.addItem(.separator())
        app.addItem(withTitle: "Hide Quill", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        let hideOthers = app.addItem(
            withTitle: "Hide Others", action: #selector(NSApplication.hideOtherApplications(_:)), keyEquivalent: "h")
        hideOthers.keyEquivalentModifierMask = [.command, .option]
        app.addItem(
            withTitle: "Show All", action: #selector(NSApplication.unhideAllApplications(_:)), keyEquivalent: "")
        app.addItem(.separator())
        app.addItem(withTitle: "Quit Quill", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")

        // File, if the core has anything for it.
        commandMenu(.file, title: "File", in: bar, commands: commands)

        // Edit
        let edit = submenu(of: bar, title: "Edit")
        append(commands.makeMenuItems(for: .edit), to: edit, thenSeparator: true)
        // Cut, Copy and Select All are the core's while it is editing text
        // and the focused view's otherwise; the window decides which. They
        // are sent to it by name: an item with nobody to answer for it is
        // disabled, and a disabled item still takes its key.
        let cut = edit.addItem(
            withTitle: "Cut", action: #selector(MainWindowController.editCut(_:)), keyEquivalent: "x")
        cut.target = panes
        let copy = edit.addItem(
            withTitle: "Copy", action: #selector(MainWindowController.editCopy(_:)), keyEquivalent: "c")
        copy.target = panes
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        let selectAll = edit.addItem(
            withTitle: "Select All", action: #selector(MainWindowController.editSelectAll(_:)), keyEquivalent: "a")
        selectAll.target = panes

        // View
        let view = submenu(of: bar, title: "View")
        append(commands.makeMenuItems(for: .view), to: view, thenSeparator: true)
        // Moving between panes is the shell's business, so these are cmd
        // shortcuts and can never collide with a key the terminal uses.
        let next = view.addItem(
            withTitle: "Focus Next Pane", action: #selector(MainWindowController.focusNextPane(_:)), keyEquivalent: "]")
        next.target = panes
        let previous = view.addItem(
            withTitle: "Focus Previous Pane", action: #selector(MainWindowController.focusPreviousPane(_:)),
            keyEquivalent: "[")
        previous.target = panes
        let close = view.addItem(
            withTitle: "Close Pane", action: #selector(MainWindowController.closePane(_:)),
            keyEquivalent: "w")
        close.keyEquivalentModifierMask = [.command, .shift]
        close.target = panes
        view.addItem(.separator())
        view.addItem(withTitle: "Show Toolbar", action: #selector(NSWindow.toggleToolbarShown(_:)), keyEquivalent: "")
        let fullScreen = view.addItem(
            withTitle: "Enter Full Screen", action: #selector(NSWindow.toggleFullScreen(_:)), keyEquivalent: "f")
        fullScreen.keyEquivalentModifierMask = [.command, .control]

        commandMenu(.mailbox, title: "Mailbox", in: bar, commands: commands)
        commandMenu(.message, title: "Message", in: bar, commands: commands)

        // Window
        let window = submenu(of: bar, title: "Window")
        append(commands.makeMenuItems(for: .window), to: window, thenSeparator: true)
        window.addItem(withTitle: "Minimize", action: #selector(NSWindow.performMiniaturize(_:)), keyEquivalent: "m")
        window.addItem(withTitle: "Zoom", action: #selector(NSWindow.performZoom(_:)), keyEquivalent: "")
        NSApp.windowsMenu = window

        // Help
        let help = submenu(of: bar, title: "Help")
        append(commands.makeMenuItems(for: .help), to: help, thenSeparator: false)
        NSApp.helpMenu = help

        return bar
    }

    private static func submenu(of bar: NSMenu, title: String) -> NSMenu {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let menu = NSMenu(title: title)
        item.submenu = menu
        bar.addItem(item)
        return menu
    }

    /// A menu made only of the core's commands. Left out while it is empty.
    private static func commandMenu(_ kind: Bridge.Menu, title: String, in bar: NSMenu, commands: CommandCenter) {
        let items = commands.makeMenuItems(for: kind)
        guard !items.isEmpty else { return }
        let menu = submenu(of: bar, title: title)
        // Enabled from the core's state, not by asking the responder chain.
        menu.autoenablesItems = false
        items.forEach(menu.addItem)
    }

    /// Adds command items to a menu the shell also has items of its own in.
    private static func append(_ items: [NSMenuItem], to menu: NSMenu, thenSeparator: Bool) {
        guard !items.isEmpty else { return }
        items.forEach(menu.addItem)
        if thenSeparator {
            menu.addItem(.separator())
        }
    }
}
