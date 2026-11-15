import Bridge
import Testing
import UserNotifications

@testable import Notifications

// Nothing here makes a `SystemNotifier`: the Mac's notification centre
// stops a process that is not an app, and a test must show nothing anyway.

private let archive = NotifyAction(name: "archive", title: "Archive")!
private let read = NotifyAction(name: "read", title: "Mark as Read")!
private let reply = NotifyAction(name: "reply", title: "Reply")!

@Suite struct CategoryTests {
    @Test func theSameButtonsHaveTheSameCategoryInEveryRun() {
        // Written out, not worked out: a name that changed from one build
        // to the next would leave earlier notifications without buttons.
        #expect(
            NotificationCategory.identifier(for: [archive, read, reply])
                == "quill.actions.f640f3e323ce491c33f940f8e6c01238")
        #expect(
            NotificationCategory.identifier(for: [archive, read, reply])
                == NotificationCategory.identifier(for: [archive, read, reply]))
    }

    @Test func noButtonsNeedNoCategory() {
        #expect(NotificationCategory.identifier(for: []) == nil)
    }

    @Test func differentButtonsHaveDifferentCategories() {
        let lists: [[NotifyAction]] = [
            [archive, read, reply],
            // Another order is another row of buttons.
            [reply, read, archive],
            [archive, read],
            [archive],
            // Another title is another button, whatever its name.
            [archive, NotifyAction(name: "read", title: "Mark as Unread")!, reply],
            [NotifyAction(name: "delete", title: "Archive")!, read, reply],
            // Words that run together the same way are still different.
            [NotifyAction(name: "a", title: "bc")!],
            [NotifyAction(name: "ab", title: "c")!],
            [NotifyAction(name: "a", title: "b")!, NotifyAction(name: "c", title: "d")!],
            [NotifyAction(name: "a", title: "b1:c1:d")!],
        ]
        let identifiers = lists.map { NotificationCategory.identifier(for: $0) }
        #expect(!identifiers.contains(nil))
        #expect(Set(identifiers).count == lists.count)
        for identifier in identifiers {
            #expect(identifier?.hasPrefix("quill.actions.") == true)
            #expect(identifier?.count == "quill.actions.".count + 32)
        }
    }
}

@Suite struct ResponseTests {
    @Test func aClickOnTheNotificationNamesNoAction() {
        #expect(
            NotificationResponse.used(id: "message:4127", actionIdentifier: UNNotificationDefaultActionIdentifier)
                == NotifyUsed(id: "message:4127"))
        #expect(NotificationResponse.click == UNNotificationDefaultActionIdentifier)
        #expect(
            ShellToCore.notifyUsed(
                NotificationResponse.used(id: "message:4127", actionIdentifier: NotificationResponse.click)!
            )
            .line == #"{"jsonrpc":"2.0","method":"notify.used","params":{"id":"message:4127"}}"#)
    }

    @Test func aButtonIsNamedAsTheCoreNamedIt() {
        #expect(
            NotificationResponse.used(id: "message:4127", actionIdentifier: "archive")
                == NotifyUsed(id: "message:4127", action: "archive"))
        #expect(
            ShellToCore.notifyUsed(NotificationResponse.used(id: "message:4127", actionIdentifier: "archive")!).line
                == #"{"jsonrpc":"2.0","method":"notify.used","params":{"action":"archive","id":"message:4127"}}"#)
    }

    @Test func puttingANotificationAwayTellsTheCoreNothing() {
        #expect(
            NotificationResponse.used(id: "message:4127", actionIdentifier: UNNotificationDismissActionIdentifier)
                == nil)
        #expect(NotificationResponse.dismissal == UNNotificationDismissActionIdentifier)
    }

    @Test func whatTheCoreCouldNotHaveSentIsNotSentBack() {
        #expect(NotificationResponse.used(id: "", actionIdentifier: NotificationResponse.click) == nil)
        #expect(NotificationResponse.used(id: "", actionIdentifier: "archive") == nil)
        #expect(NotificationResponse.used(id: "message:4127", actionIdentifier: "") == nil)
        #expect(
            NotificationResponse.used(id: "message:4127", actionIdentifier: String(repeating: "a", count: 65)) == nil)
        #expect(
            NotificationResponse.used(id: String(repeating: "a", count: 129), actionIdentifier: "archive") == nil)
    }
}

@Suite struct PresentationTests {
    @Test func inFrontOfTheUserANotificationIsQuiet() {
        #expect(NotificationPresentation.options(whileActive: true) == [.list])
        #expect(NotificationPresentation.options(whileActive: false) == [.banner, .list, .sound])
        // The Dock badge is the unread count's, never a notification's.
        #expect(!NotificationPresentation.options(whileActive: false).contains(.badge))
    }
}

@MainActor
@Suite struct RecordingNotifierTests {
    @Test func itKeepsWhatWasAskedInOrder() {
        let notifier = RecordingNotifier()
        #expect(notifier.events.isEmpty)
        #expect(notifier.lines.isEmpty)

        let first = NotifyPost(
            id: "message:4127", title: "Ana Ruiz", subtitle: "Work", body: "Q4 roadmap",
            actions: [archive, read, reply])!
        let second = NotifyPost(id: "message:4128", title: "Kit Ng", body: "Lunch?\nThursday")!
        notifier.post(first)
        notifier.post(second)
        notifier.clear(ids: ["message:4127", "message:4128"])
        notifier.post(first)

        #expect(
            notifier.events == [
                .posted(first), .posted(second), .cleared(["message:4127", "message:4128"]), .posted(first),
            ])
        #expect(
            notifier.lines == [
                "notified: message:4127 | Ana Ruiz | Work | Q4 roadmap | actions: archive,read,reply",
                #"notified: message:4128 | Kit Ng |  | Lunch?\nThursday | actions: "#,
                "cleared: message:4127 message:4128",
                "notified: message:4127 | Ana Ruiz | Work | Q4 roadmap | actions: archive,read,reply",
            ])
    }

    @Test func itIsANotifier() {
        let recorder = RecordingNotifier()
        let notifier: any Notifier = recorder
        notifier.post(NotifyPost(id: "m", title: "Ana", body: "")!)
        notifier.clear(ids: ["m"])
        #expect(recorder.lines == ["notified: m | Ana |  |  | actions: ", "cleared: m"])
    }
}
