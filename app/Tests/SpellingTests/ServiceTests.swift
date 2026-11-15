import Bridge
import Foundation
import Testing

@testable import Spelling

/// A checker that says what it is told to, and remembers what it was asked.
/// It can be made to hold every answer back until it is let go.
@MainActor
private final class FakeChecker: SpellChecker {
    var finds: (String) -> [NSRange] = { _ in [] }
    var offers: (String) -> [String] = { _ in [] }
    /// What was asked, in order: `check TEXT` or `suggest WORD`.
    private(set) var asked: [String] = []
    var holding = false
    private var held: [CheckedContinuation<Void, Never>] = []

    func misspelt(in text: String) async -> [NSRange] {
        asked.append("check \(text)")
        await hold()
        return finds(text)
    }

    func guesses(for word: String) async -> [String] {
        asked.append("suggest \(word)")
        await hold()
        return offers(word)
    }

    private func hold() async {
        guard holding else { return }
        await withCheckedContinuation { held.append($0) }
    }

    /// Lets every answer held so far go, and holds no more.
    func letGo() {
        holding = false
        held.forEach { $0.resume() }
        held = []
    }
}

/// Where the answers go.
@MainActor
private final class Outbox {
    private(set) var sent: [ShellToCore] = []

    func take(_ message: ShellToCore) {
        sent.append(message)
    }

    /// What has been sent once there are `count` of them, or what there is
    /// after a few seconds of waiting.
    func wait(for count: Int) async -> [ShellToCore] {
        await eventually { self.sent.count >= count }
        return sent
    }
}

@MainActor
private func eventually(_ condition: () -> Bool) async {
    for _ in 0..<2500 where !condition() {
        try? await Task.sleep(for: .milliseconds(2))
    }
}

private func check(_ id: Int, _ text: String) -> SpellCheck {
    SpellCheck(id: id, text: text)!
}

private func suggest(_ id: Int, _ word: String) -> SpellSuggest {
    SpellSuggest(id: id, word: word)!
}

private func checked(_ id: Int, _ words: [(Int, Int)]) -> ShellToCore {
    .spellChecked(SpellChecked(id: id, words: words.map { Misspelt(start: $0.0, end: $0.1)! })!)
}

private func suggested(_ id: Int, _ words: [String]) -> ShellToCore {
    .spellSuggested(SpellSuggested(id: id, words: words)!)
}

@MainActor
@Suite(.timeLimit(.minutes(1))) struct ServiceTests {
    private let checker = FakeChecker()
    private let outbox = Outbox()

    private func service(patience: Duration = .seconds(30)) -> SpellingService {
        SpellingService(checker: checker, patience: patience) { [outbox] in outbox.take($0) }
    }

    @Test func aCheckIsAnsweredWithTheCoresOffsets() async {
        let text = "Gr\u{FC}\u{DF}e! The agenda is atached \u{1F44D}\u{1F3FD} feedbak"
        checker.finds = { text in
            ["atached", "feedbak"].map { (text as NSString).range(of: $0) }
        }
        let service = service()
        service.check(check(7, text))
        // "Gr", two letters of two bytes each and "e! The agenda is " come
        // to 23 bytes; the thumb and its skin tone to eight.
        #expect(await outbox.wait(for: 1) == [checked(7, [(23, 30), (40, 47)])])
        #expect(checker.asked == ["check \(text)"])
        #expect(service.checksAnswered == 1)
        #expect(service.suggestionsAnswered == 0)
    }

    @Test func aSuggestionIsAnsweredWithTheGuesses() async {
        checker.offers = { $0 == "atached" ? ["attached", "attaches", "attacked"] : [] }
        let service = service()
        service.suggest(suggest(8, "atached"))
        service.suggest(suggest(9, "xqzvv"))
        #expect(
            await outbox.wait(for: 2) == [suggested(8, ["attached", "attaches", "attacked"]), suggested(9, [])])
        #expect(checker.asked == ["suggest atached", "suggest xqzvv"])
        #expect(service.checksAnswered == 0)
        #expect(service.suggestionsAnswered == 2)
    }

    @Test func questionsAreAnsweredOneAtATimeInTheOrderAsked() async {
        checker.finds = { text in text.hasPrefix("teh") ? [NSRange(location: 0, length: 3)] : [] }
        checker.offers = { [$0 + "!"] }
        checker.holding = true
        let service = service()
        service.check(check(1, "teh first"))
        service.suggest(suggest(2, "second"))
        service.check(check(3, "third"))
        service.check(check(1, "teh fourth, with an id used before"))
        service.suggest(suggest(5, "fifth"))

        // The first is with the checker. Nothing else has been put to it,
        // and nothing has been answered.
        await eventually { !checker.asked.isEmpty }
        try? await Task.sleep(for: .milliseconds(50))
        #expect(checker.asked == ["check teh first"])
        #expect(outbox.sent.isEmpty)
        #expect(service.checksAnswered == 0)

        checker.letGo()
        #expect(
            await outbox.wait(for: 5) == [
                checked(1, [(0, 3)]), suggested(2, ["second!"]), checked(3, []), checked(1, [(0, 3)]),
                suggested(5, ["fifth!"]),
            ])
        #expect(
            checker.asked == [
                "check teh first", "suggest second", "check third", "check teh fourth, with an id used before",
                "suggest fifth",
            ])
        #expect(service.checksAnswered == 3)
        #expect(service.suggestionsAnswered == 2)

        // And again once it has gone quiet.
        service.suggest(suggest(6, "sixth"))
        #expect(await outbox.wait(for: 6).last == suggested(6, ["sixth!"]))
        #expect(service.suggestionsAnswered == 3)
    }

    @Test func whatTheCheckerGetsWrongIsLeftOut() async {
        let text = "\u{1F600}teh wrold cafe\u{301} agre\n> teh quoted\nsee https://exampel.com/teh"
        checker.finds = { text in
            let string = text as NSString
            return [
                string.range(of: "agre"),
                NSRange(location: NSNotFound, length: 0),
                // Half the emoji.
                NSRange(location: 1, length: 4),
                string.range(of: "wrold"),
                string.range(of: "rol"),
                NSRange(location: 6, length: -2),
                NSRange(location: 6, length: 0),
                // A letter without its accent.
                NSRange(location: 12, length: 4),
                string.range(of: "teh"),
                string.range(of: "teh quoted"),
                string.range(of: "exampel"),
                NSRange(location: 0, length: 4000),
                NSRange(location: string.length, length: 1),
            ]
        }
        let service = service()
        service.check(check(4, text))
        // Out of order, they are put in order; the rest is dropped.
        #expect(await outbox.wait(for: 1) == [checked(4, [(4, 7), (8, 13), (21, 25)])])
        #expect(service.checksAnswered == 1)
    }

    @Test func noAnswerIsLongerThanTheCoreTakes() async {
        let text = String(repeating: "t\u{E9}h ", count: 3000)
        checker.finds = { _ in (0..<3000).map { NSRange(location: $0 * 4, length: 3) } }
        checker.offers = { _ in
            (1...40).flatMap { ["", "guess\($0)", String(repeating: "\u{FC}", count: 101)] }
        }
        let service = service()
        service.check(check(1, text))
        service.suggest(suggest(2, "teh"))
        let sent = await outbox.wait(for: 2)
        #expect(
            sent == [checked(1, (0..<2000).map { ($0 * 5, $0 * 5 + 4) }), suggested(2, (1...20).map { "guess\($0)" })])
        // Each is a line the core takes.
        for message in sent {
            #expect(throws: Never.self) { try ShellToCore(line: message.line) }
        }
    }

    @Test func aCheckerThatNeverAnswersIsAnsweredFor() async {
        checker.finds = { _ in [NSRange(location: 0, length: 3)] }
        checker.offers = { _ in ["the"] }
        checker.holding = true
        let service = service(patience: .milliseconds(60))
        service.check(check(1, "teh"))
        service.suggest(suggest(2, "teh"))
        service.check(check(3, "teh"))
        // Nothing misspelt, nothing to suggest: the core is not kept waiting.
        #expect(await outbox.wait(for: 3) == [checked(1, []), suggested(2, []), checked(3, [])])
        #expect(checker.asked == ["check teh", "suggest teh", "check teh"])
        #expect(service.checksAnswered == 2)
        #expect(service.suggestionsAnswered == 1)

        // What it says when it does come back is too late to send.
        checker.letGo()
        try? await Task.sleep(for: .milliseconds(100))
        #expect(outbox.sent.count == 3)

        // The next question is asked and answered as ever.
        service.check(check(4, "teh"))
        #expect(await outbox.wait(for: 4).last == checked(4, [(0, 3)]))
        #expect(service.checksAnswered == 3)
    }

    @Test func nothingToCheckIsStillAnswered() async {
        let service = service()
        service.check(check(0, ""))
        service.check(check(1, "\n\n"))
        #expect(await outbox.wait(for: 2) == [checked(0, []), checked(1, [])])
        #expect(service.checksAnswered == 2)
    }

    @Test func anotherCheckerCanBePutInItsPlace() async {
        let other = FakeChecker()
        other.finds = { _ in [NSRange(location: 4, length: 3)] }
        other.offers = { _ in ["other"] }
        checker.offers = { _ in ["first"] }
        let service = service()
        service.suggest(suggest(1, "teh"))
        #expect(await outbox.wait(for: 1) == [suggested(1, ["first"])])
        service.checker = other
        service.suggest(suggest(2, "teh"))
        service.check(check(3, "the teh"))
        #expect(await outbox.wait(for: 3) == [suggested(1, ["first"]), suggested(2, ["other"]), checked(3, [(4, 7)])])
        #expect(checker.asked == ["suggest teh"])
        #expect(other.asked == ["suggest teh", "check the teh"])
    }
}

/// The Mac's own checker, asked for real. Which words it knows depends on
/// the Mac and its user, so nothing here says what is or is not misspelt:
/// only that whatever it answers is something the core can take.
@MainActor
@Suite(.timeLimit(.minutes(1))) struct SystemSpellCheckerTests {
    @Test func whateverItFindsIsInsideTheText() async {
        let text =
            "Gr\u{FC}\u{DF}e \u{1F44D}\u{1F3FD}! The agenda is atached, na\u{EF}vley. \u{65E5}\u{672C}\u{8A9E} cafe\u{301}s wrold.\n"
            + "> teh quoted feedbak\nSee https://exampel.com/atached or robinn@exampel.com \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"
        let checker = SystemSpellChecker()
        let ranges = await checker.misspelt(in: text)
        let units = text.utf16.count
        for range in ranges {
            #expect(range.location >= 0 && range.length > 0 && range.location + range.length <= units, "\(range)")
        }
        // Whatever it found, what is sent is in order, inside the text and
        // on whole characters.
        let words = Misspellings.words(in: text, at: ranges)
        #expect(words.count <= ranges.count)
        #expect(SpellChecked(id: 1, words: words) != nil)
        let bytes = Array(text.utf8)
        for word in words {
            #expect(word.end <= bytes.count)
            #expect(String(bytes: bytes[word.start..<word.end], encoding: .utf8) != nil, "\(word)")
        }
        #expect(await checker.misspelt(in: "").isEmpty)
    }

    @Test func whateverItGuessesCanBeSent() async {
        let checker = SystemSpellChecker()
        for word in ["atached", "na\u{EF}vley", "xqzvv", "\u{1F44D}\u{1F3FD}", String(repeating: "a", count: 200)] {
            let offered = Suggestions.offered(from: await checker.guesses(for: word))
            #expect(SpellSuggested(id: 1, words: offered) != nil, "\(word)")
        }
        #expect(await checker.guesses(for: "").isEmpty)
    }

    @Test func theServiceAnswersWithIt() async {
        let outbox = Outbox()
        let service = SpellingService(checker: SystemSpellChecker()) { [outbox] in outbox.take($0) }
        service.check(check(1, "The agenda is atached."))
        service.suggest(suggest(2, "atached"))
        let sent = await outbox.wait(for: 2)
        #expect(sent.count == 2)
        guard sent.count == 2, case let .spellChecked(checked) = sent[0], case let .spellSuggested(suggested) = sent[1]
        else {
            Issue.record("not a spell.checked and then a spell.suggested")
            return
        }
        #expect(checked.id == 1)
        #expect(suggested.id == 2)
        #expect(service.checksAnswered == 1)
        #expect(service.suggestionsAnswered == 1)
    }
}
