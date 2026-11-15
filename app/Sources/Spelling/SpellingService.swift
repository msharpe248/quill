import Bridge
import Foundation

/// Answers the core's questions about spelling: one at a time, in the order
/// they were asked, and every one of them. The core waits for each answer by
/// its id, so a checker that fails or never comes back is answered for: no
/// misspelt words, no suggestions.
@MainActor
public final class SpellingService {
    /// Who is asked. A scripted run or a test may put another here; a
    /// question already being answered stays with the one it was put to.
    public var checker: any SpellChecker

    /// How many `spell.check` requests have been answered.
    public private(set) var checksAnswered = 0
    /// How many `spell.suggest` requests have been answered.
    public private(set) var suggestionsAnswered = 0

    private enum Request {
        case check(SpellCheck)
        case suggest(SpellSuggest)
    }

    private let patience: Duration
    private let send: (ShellToCore) -> Void
    private var waiting: [Request] = []
    private var working = false

    /// `send` is handed each `spellChecked` and `spellSuggested`, on the
    /// main actor. `patience` is how long the checker is given for one
    /// question before it is answered for.
    public init(
        checker: any SpellChecker, patience: Duration = .seconds(10), send: @escaping (ShellToCore) -> Void
    ) {
        self.checker = checker
        self.patience = patience
        self.send = send
    }

    public func check(_ request: SpellCheck) {
        take(.check(request))
    }

    public func suggest(_ request: SpellSuggest) {
        take(.suggest(request))
    }

    private func take(_ request: Request) {
        waiting.append(request)
        guard !working else { return }
        working = true
        Task { @MainActor [self] in
            while !waiting.isEmpty {
                await answer(waiting.removeFirst())
            }
            working = false
        }
    }

    private func answer(_ request: Request) async {
        let checker = checker
        switch request {
        case let .check(check):
            let text = check.text
            let found = await within(patience) { await checker.misspelt(in: text) }
            if found == nil {
                NSLog("Quill: the spelling checker did not answer check %ld", check.id)
            }
            let words = await Self.sift(found ?? [], in: text)
            // The second cannot fail: the id is one the core sent.
            if let checked = SpellChecked(id: check.id, words: words) ?? SpellChecked(id: check.id, words: []) {
                checksAnswered += 1
                send(.spellChecked(checked))
            }
        case let .suggest(suggest):
            let word = suggest.word
            let guesses = await within(patience) { await checker.guesses(for: word) }
            if guesses == nil {
                NSLog("Quill: the spelling checker did not answer suggestion %ld", suggest.id)
            }
            let words = Suggestions.offered(from: guesses ?? [])
            if let suggested = SpellSuggested(id: suggest.id, words: words)
                ?? SpellSuggested(id: suggest.id, words: [])
            {
                suggestionsAnswered += 1
                send(.spellSuggested(suggested))
            }
        }
    }

    /// Off the main thread: the longest text a check takes has a hundred
    /// thousand characters to count.
    @concurrent
    private nonisolated static func sift(_ ranges: [NSRange], in text: String) async -> [Misspelt] {
        Misspellings.words(in: text, at: ranges)
    }
}

/// What `work` returns, or `nil` if it has not returned within `limit`. Work
/// that overruns is left to finish by itself; nobody hears what it says.
@MainActor
private func within<Value: Sendable>(
    _ limit: Duration, _ work: @escaping @MainActor () async -> Value
) async -> Value? {
    await withCheckedContinuation { continuation in
        let once = Once(continuation)
        let timer = Task { @MainActor in
            guard (try? await Task.sleep(for: limit)) != nil else { return }
            once.resume(nil)
        }
        Task { @MainActor in
            let value = await work()
            timer.cancel()
            once.resume(value)
        }
    }
}

/// A continuation that is resumed by whoever gets to it first, and by
/// nobody after that.
@MainActor
private final class Once<Value: Sendable> {
    private var continuation: CheckedContinuation<Value?, Never>?

    init(_ continuation: CheckedContinuation<Value?, Never>) {
        self.continuation = continuation
    }

    func resume(_ value: Value?) {
        continuation?.resume(returning: value)
        continuation = nil
    }
}
