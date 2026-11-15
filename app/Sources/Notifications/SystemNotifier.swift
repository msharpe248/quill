import AppKit
import Bridge
import UserNotifications

/// The Mac's own notifications.
///
/// Only the running app may make one: the notification centre belongs to an
/// app with a bundle identifier, and asking for it in any other process (a
/// test, for one) stops that process.
@MainActor
public final class SystemNotifier: NSObject, Notifier, UNUserNotificationCenterDelegate {
    /// Called when the user clicks a notification or one of its buttons,
    /// with the notification's id and the Mac's name for what was chosen.
    /// See `NotificationResponse`.
    public var onResponse: ((_ id: String, _ actionIdentifier: String) -> Void)?

    private let center: UNUserNotificationCenter
    /// Notifications waiting to hear whether the user allows them.
    private var waiting: [NotifyPost] = []
    private var asking = false
    /// The buttons of every category registered in this run, by its name.
    private var categories: [String: [NotifyAction]] = [:]

    /// Starts listening for what the user does with notifications, those
    /// left from an earlier run included. Asks the user nothing: that waits
    /// for the first notification. `nil` in a process that is not an app.
    public static func forThisApp() -> SystemNotifier? {
        Bundle.main.bundleIdentifier == nil ? nil : SystemNotifier()
    }

    private override init() {
        center = .current()
        super.init()
        center.delegate = self
    }

    // The Mac answers each question below on a thread of its own choosing.
    // So every closure handed to it is `@Sendable`, which keeps it off the
    // main actor, and comes back to the main thread by itself.

    public func post(_ notification: NotifyPost) {
        waiting.removeAll { $0.id == notification.id }
        waiting.append(notification)
        guard !asking else { return }
        asking = true
        // What the user has said, which they may have changed in Settings
        // since they were asked.
        center.getNotificationSettings { @Sendable [weak self] settings in
            let status = settings.authorizationStatus
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.learned(status) } }
        }
    }

    private func learned(_ status: UNAuthorizationStatus) {
        switch status {
        case .notDetermined:
            // Nobody has asked them yet. This is the only place Quill does,
            // and the Mac puts the question once. The badge is asked for
            // along with the rest because the Dock badge, which is set from
            // the unread count and never by a notification, is theirs to
            // turn off in the same place.
            center.requestAuthorization(options: [.alert, .sound, .badge]) { @Sendable [weak self] granted, error in
                let problem = error.map { String(describing: $0) }
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        if let problem {
                            NSLog("Quill: notifications could not be asked for: %@", problem)
                        }
                        self?.answered(allowed: granted)
                    }
                }
            }
        case .denied:
            answered(allowed: false)
        default:
            answered(allowed: true)
        }
    }

    public func clear(ids: [String]) {
        waiting.removeAll { ids.contains($0.id) }
        center.removePendingNotificationRequests(withIdentifiers: ids)
        center.removeDeliveredNotifications(withIdentifiers: ids)
    }

    private func answered(allowed: Bool) {
        asking = false
        let allowedNow = waiting
        waiting = []
        // A user who said no is not told again, and nothing is kept for later.
        guard allowed else { return }
        allowedNow.forEach(deliver)
    }

    private func deliver(_ notification: NotifyPost) {
        let content = UNMutableNotificationContent()
        // Plain properties: the words are shown as they are, never read as
        // a format or a key to look up.
        content.title = notification.title
        content.body = notification.body
        if let subtitle = notification.subtitle {
            content.subtitle = subtitle
            // Notification Centre gathers those that share a subtitle.
            content.threadIdentifier = subtitle
        }
        content.sound = .default
        if let category = category(for: notification.actions) {
            content.categoryIdentifier = category
        }
        // The core's id is the request's: a second request of one id takes
        // the place of the first, and `clear` finds it.
        let request = UNNotificationRequest(identifier: notification.id, content: content, trigger: nil)
        center.add(request) { @Sendable error in
            if let error {
                NSLog("Quill: a notification could not be shown: %@", String(describing: error))
            }
        }
    }

    /// The category for these buttons, registered if this run has not seen
    /// them before. Registering replaces the whole set, so every category
    /// seen goes in each time.
    private func category(for actions: [NotifyAction]) -> String? {
        guard let identifier = NotificationCategory.identifier(for: actions) else { return nil }
        if categories[identifier] == nil {
            categories[identifier] = actions
            let all = categories.map { identifier, actions in
                UNNotificationCategory(
                    identifier: identifier,
                    // A button does its work without bringing Quill to the
                    // front, unless the core says its work is done there.
                    actions: actions.map {
                        UNNotificationAction(
                            identifier: $0.name, title: $0.title, options: $0.foreground ? [.foreground] : [])
                    },
                    intentIdentifiers: [],
                    options: [])
            }
            center.setNotificationCategories(Set(all))
        }
        return identifier
    }

    // MARK: - UNUserNotificationCenterDelegate

    // The Mac does not say which thread it calls these on.

    public nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let id = response.notification.request.identifier
        let actionIdentifier = response.actionIdentifier
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.onResponse?(id, actionIdentifier) }
        }
        completionHandler()
    }

    /// Asked only while the app is running in front of the user.
    public nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        willPresent _: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler(NotificationPresentation.options(whileActive: Self.appIsActive()))
    }

    private nonisolated static func appIsActive() -> Bool {
        if Thread.isMainThread {
            return MainActor.assumeIsolated { NSApp.isActive }
        }
        return DispatchQueue.main.sync { MainActor.assumeIsolated { NSApp.isActive } }
    }
}
