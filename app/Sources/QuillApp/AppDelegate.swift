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

    func applicationShouldTerminateAfterLastWindowClosed(_: NSApplication) -> Bool {
        true
    }

    func applicationSupportsSecureRestorableState(_: NSApplication) -> Bool {
        true
    }
}
