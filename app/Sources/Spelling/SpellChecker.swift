import AppKit

/// Something that knows how words are spelt. What the words are for is the
/// core's business: this is handed a text, or a word, and nothing else.
@MainActor
public protocol SpellChecker: AnyObject {
    /// Where the misspelt words of `text` are, each as a range of the
    /// text's UTF-16 units, which is how the Mac counts. None when there
    /// are none, or when the text could not be checked.
    func misspelt(in text: String) async -> [NSRange]
    /// What a misspelt word might have been meant to be, the likeliest
    /// first. None is an answer.
    func guesses(for word: String) async -> [String]
}

/// The Mac's own checker, with the user's languages and the words they have
/// taught it.
///
/// It is only ever asked. Nothing here teaches it a word, has it ignore
/// one, changes its language or shows its panel: what the user's dictionary
/// holds afterwards is what it held before.
@MainActor
public final class SystemSpellChecker: SpellChecker {
    public init() {}

    public func misspelt(in text: String) async -> [NSRange] {
        guard !text.isEmpty else { return [] }
        let checker = NSSpellChecker.shared
        // A document of its own, so that nothing ignored anywhere else
        // applies here, and closed again so that nothing is kept of it.
        let tag = NSSpellChecker.uniqueSpellDocumentTag()
        defer { checker.closeSpellDocument(withTag: tag) }
        var types: NSTextCheckingResult.CheckingType = .spelling
        if checker.automaticallyIdentifiesLanguages {
            // The user has the Mac work out which of their languages each
            // stretch of text is in, which it does only when asked to.
            types.insert(.orthography)
        }
        // The Mac checks on a thread of its own and answers there. The
        // closure is `@Sendable`, which keeps it off the main actor, and
        // takes nothing out of the answer but the ranges.
        return await withCheckedContinuation { continuation in
            checker.requestChecking(
                of: text, range: NSRange(location: 0, length: text.utf16.count), types: types.rawValue,
                options: nil, inSpellDocumentWithTag: tag
            ) { @Sendable _, results, _, _ in
                continuation.resume(returning: results.filter { $0.resultType == .spelling }.map(\.range))
            }
        }
    }

    public func guesses(for word: String) async -> [String] {
        guard !word.isEmpty else { return [] }
        let checker = NSSpellChecker.shared
        let tag = NSSpellChecker.uniqueSpellDocumentTag()
        defer { checker.closeSpellDocument(withTag: tag) }
        // One word, which takes the checker a few thousandths of a second.
        // No language is named: a word alone does not say which it is in,
        // so the checker uses the one it would use for the user anyway.
        return checker.guesses(
            forWordRange: NSRange(location: 0, length: word.utf16.count), in: word, language: nil,
            inSpellDocumentWithTag: tag) ?? []
    }
}
