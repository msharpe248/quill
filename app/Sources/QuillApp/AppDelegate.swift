import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var mainWindow: MainWindowController?
    /// Addresses the Mac handed over before there was a window to take
    /// them: Quill was started by a click on a `mailto:` link.
    private var waitingURLs: [URL] = []

    func applicationDidFinishLaunching(_: Notification) {
        let options = LaunchOptions(arguments: CommandLine.arguments)
        let controller = MainWindowController(options: options)
        mainWindow = controller
        // The menu bar starts with the items every Mac app has, and is
        // rebuilt around the core's commands once it has announced them.
        installMenu()
        controller.onCommandsChanged = { [weak self] in self?.installMenu() }
        controller.showWindow(nil)
        NSApp.activate(ignoringOtherApps: true)
        controller.open(urls: waitingURLs)
        waitingURLs = []
    }

    /// The Mac asks Quill to open addresses: `mailto:` is the only scheme
    /// `Info.plist` says it takes. The window decides what is passed on.
    func application(_: NSApplication, open urls: [URL]) {
        if let mainWindow {
            mainWindow.open(urls: urls)
        } else {
            waitingURLs += urls
        }
    }

    private func installMenu() {
        guard let controller = mainWindow else { return }
        NSApp.mainMenu = MainMenu.build(commands: controller.commands, panes: controller)
    }

    func applicationShouldTerminate(_: NSApplication) -> NSApplication.TerminateReply {
        // Give the core the chance to flush and exit cleanly first.
        mainWindow?.mustWaitForCoreBeforeTerminating() == true ? .terminateLater : .terminateNow
    }

    /// Closing the window does not quit Quill: it goes on fetching mail
    /// and announcing it, as a mail program does, until it is quit.
    func applicationShouldTerminateAfterLastWindowClosed(_: NSApplication) -> Bool {
        false
    }

    /// A click on the Dock's icon, or Quill opened again while it is
    /// running: the main window comes back if it was closed.
    func applicationShouldHandleReopen(_: NSApplication, hasVisibleWindows _: Bool) -> Bool {
        reopen()
        return true
    }

    /// Brings the main window back if it is not on show. Also what a
    /// scripted run does in place of a click on the Dock.
    func reopen() {
        guard let mainWindow, !mainWindow.mainWindowIsShown else { return }
        mainWindow.showMainWindow(nil)
    }

    func applicationSupportsSecureRestorableState(_: NSApplication) -> Bool {
        true
    }
}
