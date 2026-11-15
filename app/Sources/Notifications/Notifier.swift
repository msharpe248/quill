import Bridge
import CryptoKit
import Foundation
import UserNotifications

/// Shows the notifications the core asks for. What they say is the core's
/// business: this knows an id, some words and the names of some buttons.
@MainActor
public protocol Notifier: AnyObject {
    /// Shows a notification, in place of any earlier one with the same id.
    func post(_ notification: NotifyPost)
    /// Takes down the notifications with these ids, shown or waiting. One
    /// that is not there is not an error.
    func clear(ids: [String])
}

/// Stands in for the Mac's notifications in a scripted run and in tests:
/// nothing is shown and nobody is asked for permission. What would have
/// been done is kept, in order.
@MainActor
public final class RecordingNotifier: Notifier {
    public enum Event: Equatable, Sendable {
        case posted(NotifyPost)
        case cleared([String])
    }

    public private(set) var events: [Event] = []

    public init() {}

    public func post(_ notification: NotifyPost) {
        events.append(.posted(notification))
    }

    public func clear(ids: [String]) {
        events.append(.cleared(ids))
    }

    /// One line for each thing asked, in the order asked:
    ///
    ///     notified: message:4127 | Ana Ruiz | Work | Q4 roadmap | actions: archive,read,reply
    ///     cleared: message:4127 message:4128
    ///
    /// A notification with no subtitle has nothing between its bars, and a
    /// line break in its words is written `\n`.
    public var lines: [String] {
        events.map { event in
            switch event {
            case let .posted(notification):
                let words = [notification.id, notification.title, notification.subtitle ?? "", notification.body]
                    .map(Self.onOneLine)
                let actions = notification.actions.map(\.name).joined(separator: ",")
                return "notified: \(words.joined(separator: " | ")) | actions: \(actions)"
            case let .cleared(ids):
                return "cleared: \(ids.joined(separator: " "))"
            }
        }
    }

    private static func onOneLine(_ text: String) -> String {
        text.replacingOccurrences(of: "\r", with: "\\r").replacingOccurrences(of: "\n", with: "\\n")
    }
}

/// The Mac shows buttons on a notification by the name of a category
/// registered beforehand, not button by button. So each run of buttons the
/// core asks for gets a category of its own.
public enum NotificationCategory {
    /// The name of the category for these buttons, in this order: the same
    /// for the same names and titles in every run of the app, so that a
    /// notification left from an earlier run finds its buttons again.
    /// `nil` for no buttons, which needs no category.
    public static func identifier(for actions: [NotifyAction]) -> String? {
        guard !actions.isEmpty else { return nil }
        // Each word goes in with its length, so that no two lists of
        // buttons spell the same thing.
        let spelled = actions.flatMap { [$0.name, $0.title, $0.foreground ? "front" : "back"] }
            .map { "\($0.utf8.count):\($0)" }
            .joined()
        let digest = SHA256.hash(data: Data(spelled.utf8))
        return "quill.actions." + digest.prefix(16).map { String(format: "%02x", $0) }.joined()
    }
}

/// What the user did with a notification, as the Mac reports it.
public enum NotificationResponse {
    /// What the Mac calls a click on the notification itself.
    public static let click = UNNotificationDefaultActionIdentifier
    /// What the Mac calls putting the notification away.
    public static let dismissal = UNNotificationDismissActionIdentifier

    /// What to tell the core: the notification was clicked, or one of its
    /// buttons was, by the name the core gave it. `nil` when there is
    /// nothing to tell: the notification was only put away, or what came
    /// back is nothing the core could have sent.
    public static func used(id: String, actionIdentifier: String) -> NotifyUsed? {
        switch actionIdentifier {
        case dismissal: nil
        case click: NotifyUsed(id: id)
        default: NotifyUsed(id: id, action: actionIdentifier)
        }
    }
}

/// How a notification arrives while the app is running.
public enum NotificationPresentation {
    /// While Quill is in front its own list shows the new mail, so there is
    /// no banner and no sound: the notification goes quietly to
    /// Notification Centre. Otherwise it is shown and heard.
    public static func options(whileActive active: Bool) -> UNNotificationPresentationOptions {
        active ? [.list] : [.banner, .list, .sound]
    }
}
