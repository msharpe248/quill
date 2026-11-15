import Bridge
import Foundation
import Testing

@testable import Spelling

/// The bytes of the first `word` in `text`.
private func place(of word: String, in text: String) -> Range<Int> {
    let found = text.range(of: word)!
    return text.utf8.distance(
        from: text.startIndex, to: found.lowerBound)..<text.utf8.distance(
            from: text.startIndex, to: found.upperBound)
}

/// Which of `words`, each looked for in `text`, are the user's own prose.
private func prose(_ words: [String], in text: String) -> [String] {
    let kept = Prose.words(words.map { place(of: $0, in: text) }, in: text)
    let bytes = Array(text.utf8)
    return kept.map { String(decoding: bytes[$0], as: UTF8.self) }
}

/// The parts of `text` that are skipped, as text.
private func skipped(in text: String) -> [String] {
    let bytes = Array(text.utf8)
    return Prose.skipped(in: text).map { String(decoding: bytes[$0], as: UTF8.self) }
}

@Suite struct ProseTests {
    @Test func quotedLinesAreSomebodyElses() {
        let text = """
            Thanks, I agre.
            > Can you send teh agenda?
            >and the feedbak
            >> an older wrold
             > indented, so mine: indentd
            It is atached.
            >
            > last lien
            """
        #expect(
            prose(["agre", "teh", "feedbak", "wrold", "indentd", "atached", "lien"], in: text)
                == ["agre", "indentd", "atached"])
        #expect(
            skipped(in: text) == [
                "> Can you send teh agenda?", ">and the feedbak", ">> an older wrold", ">", "> last lien",
            ])
        // A `>` anywhere but the start of a line quotes nothing.
        #expect(prose(["teh", "wrold"], in: "if a > b teh wrold\na -> b") == ["teh", "wrold"])
        #expect(skipped(in: "if a > b teh wrold\na -> b").isEmpty)
    }

    @Test func aLineEndsHoweverItIsEnded() {
        #expect(prose(["teh", "wrold"], in: "> teh\r\nwrold") == ["wrold"])
        #expect(prose(["teh", "wrold"], in: "wrold\r\n> teh\r\n") == ["wrold"])
        #expect(prose(["teh", "wrold"], in: "> teh\rwrold") == ["wrold"])
        #expect(prose(["teh", "wrold"], in: "> teh\u{2028}wrold") == ["wrold"])
        #expect(prose(["teh", "wrold"], in: "> teh\u{2029}wrold") == ["wrold"])
        #expect(prose(["teh", "wrold"], in: "> teh\u{85}wrold") == ["wrold"])
        // A tab or a wide space does not end a line.
        #expect(prose(["teh", "wrold"], in: "> teh\twrold\u{A0}x\u{3000}y").isEmpty)
        // The carriage return of a line is not part of what is skipped.
        #expect(Prose.skipped(in: "> teh\r\nwrold") == [0..<5])
        #expect(Prose.skipped(in: "a\n> teh") == [2..<7])
        #expect(Prose.skipped(in: ">") == [0..<1])
        #expect(Prose.skipped(in: "\n>\n\n>") == [1..<2, 4..<5])
    }

    @Test func webAddressesAreNotProse() {
        let text =
            "See https://exampel.com/atached?feedbak=wrold#lien now, or (http://exampl.org/teh). "
            + "Also www.exampul.com/agre and <WWW.Exampil.com>, then ftp://filez.example/pathh. Realy."
        #expect(
            prose(
                [
                    "exampel", "atached", "feedbak", "wrold", "lien", "exampl", "teh", "exampul", "agre", "Exampil",
                    "filez", "pathh", "Realy",
                ], in: text) == ["Realy"])
        #expect(
            skipped(in: text) == [
                "https://exampel.com/atached?feedbak=wrold#lien", "(http://exampl.org/teh).", "www.exampul.com/agre",
                "<WWW.Exampil.com>,", "ftp://filez.example/pathh.",
            ])
        // The words around an address are still prose.
        #expect(prose(["Sea", "nowe"], in: "Sea https://example.com nowe") == ["Sea", "nowe"])
        // Any white space ends an address, not only a space.
        #expect(prose(["teh"], in: "https://example.com\nteh") == ["teh"])
        #expect(prose(["teh"], in: "https://example.com\tteh") == ["teh"])
        #expect(prose(["teh"], in: "https://example.com\u{A0}teh") == ["teh"])
        #expect(prose(["teh"], in: "https://example.com\u{3000}teh") == ["teh"])
        // With no white space, what follows is part of it.
        #expect(prose(["teh"], in: "https://example.com,teh").isEmpty)
    }

    @Test func mailAddressesAreNotProse() {
        let text =
            "Write to robinn@exampel.com or <ana.ruizz+listt@mail.exampul.org>, "
            + "mailto:kitt@exampil.com?subject=Helo, j\u{FC}rgenn@ex\u{E4}mpel.de. Thanx."
        #expect(
            prose(
                [
                    "robinn", "exampel", "ruizz", "listt", "exampul", "kitt", "exampil", "Helo", "rgenn", "mpel",
                    "Thanx",
                ],
                in: text) == ["Thanx"])
        #expect(
            skipped(in: text) == [
                "robinn@exampel.com", "<ana.ruizz+listt@mail.exampul.org>,", "mailto:kitt@exampil.com?subject=Helo,",
                "j\u{FC}rgenn@ex\u{E4}mpel.de.",
            ])
        #expect(skipped(in: "(MAILTO:?subject=Helo)") == ["(MAILTO:?subject=Helo)"])
    }

    @Test func whatOnlyLooksLikeAnAddressIsProse() {
        for text in [
            "meet @ noonn", "meet me @noonn", "noonn@ the cafe", "noonn@", "@noonn", "noonn @", "noonn@.", "50%@noonn",
            "and/or noonn", "noonn:/ slash", "see: noonn", "wwww.noonn", "www noonn", "wwwnoonn.",
            "mailto noonn", "the-www.noonn", "http noonn", "a.noonn", "noonn...", "\"noonn\"", "(noonn)", "noonn/",
            "x://\u{A0}noonn", "a@\u{A0}noonn", "@ noonn www",
        ] {
            #expect(prose(["noonn"], in: text) == ["noonn"], "\(text)")
        }
        // And what does not look like one much, but is.
        for text in [
            "x://noonn", "a@noonn", "noonn@b", "(www.noonn", "\"mailto:noonn\"", "wWw.noonn", "://noonn", "noonn://",
        ] {
            #expect(prose(["noonn"], in: text).isEmpty, "\(text)")
        }
    }

    @Test func theOffsetsAreBytes() {
        // Letters of two, three and four bytes before, and inside, what is
        // skipped and what is kept.
        let text =
            "Gr\u{FC}\u{DF}e \u{1F44D}\u{1F3FD} teh https://ex\u{E4}mpel.de/\u{65E5}\u{672C} wrold\n> \u{E9}t\u{E9} agre"
        #expect(prose(["teh", "ex\u{E4}mpel", "wrold", "agre"], in: text) == ["teh", "wrold"])
        #expect(Prose.skipped(in: text) == [21..<47, 54..<66])
        #expect(skipped(in: text) == ["https://ex\u{E4}mpel.de/\u{65E5}\u{672C}", "> \u{E9}t\u{E9} agre"])
    }

    @Test func aCandidateThatTouchesWhatIsSkippedIsDropped() {
        let text = "ab https://example.com cd\n> ef\ngh"
        let address = place(of: "https://example.com", in: text)
        let quoted = place(of: "> ef", in: text)
        #expect(address == 3..<22)
        #expect(quoted == 26..<30)
        let candidates = [
            0..<2,  // ab
            0..<3,  // up to the address, not into it
            0..<4,  // one byte into it
            2..<3,  // the space before it
            3..<22,  // all of it
            10..<12,  // inside it
            21..<23,  // out of its end
            22..<23,  // the space after it
            23..<25,  // cd
            23..<27,  // into the quoted line
            25..<26,  // the line break before it
            26..<27,  // its `>`
            29..<31,  // out of its end
            30..<31,  // the line break after it
            31..<33,  // gh
            0..<33,  // everything
        ]
        #expect(
            Prose.words(candidates, in: text) == [0..<2, 0..<3, 2..<3, 22..<23, 23..<25, 25..<26, 30..<31, 31..<33])
        // The order given is the order kept.
        #expect(Prose.words([31..<33, 10..<12, 0..<2, 23..<25], in: text) == [31..<33, 0..<2, 23..<25])
        #expect(Prose.words([], in: text).isEmpty)
        #expect(Prose.words([0..<3], in: "teh") == [0..<3])
    }

    @Test func nothingIsSkippedInPlainProse() {
        #expect(Prose.skipped(in: "").isEmpty)
        #expect(Prose.skipped(in: "\n\n").isEmpty)
        #expect(Prose.skipped(in: "The agenda is atached. Thanks for the feedbak.").isEmpty)
        #expect(Prose.skipped(in: "\u{65E5}\u{672C}\u{8A9E}\u{3000}cafe\u{301} \u{1F44D}\u{1F3FD}").isEmpty)
    }
}

@Suite struct MisspellingsTests {
    private func range(of word: String, in text: String) -> NSRange {
        (text as NSString).range(of: word)
    }

    private func cut(_ text: String, _ words: [Misspelt]) -> [String] {
        let bytes = Array(text.utf8)
        return words.map { String(decoding: bytes[$0.start..<$0.end], as: UTF8.self) }
    }

    @Test func theCheckersRangesBecomeTheCoresOffsets() {
        let text = "The agenda is atached. Thanks for the feedbak."
        let words = Misspellings.words(
            in: text, at: [NSRange(location: 14, length: 7), NSRange(location: 38, length: 7)])
        #expect(words == [Misspelt(start: 14, end: 21)!, Misspelt(start: 38, end: 45)!])
        // Exactly the example in `schema/examples/shell-to-core/spell.checked.json`.
        #expect(
            ShellToCore.spellChecked(SpellChecked(id: 7, words: words)!).line
                == #"{"jsonrpc":"2.0","method":"spell.checked","params":{"id":7,"words":[{"end":21,"start":14},{"end":45,"start":38}]}}"#
        )
        #expect(Misspellings.words(in: text, at: []).isEmpty)
        #expect(Misspellings.words(in: "", at: []).isEmpty)
    }

    @Test func offsetsAreBytesWhateverTheLetters() {
        let text =
            "Gr\u{FC}\u{DF}e \u{1F44D}\u{1F3FD}, the na\u{EF}vley atached \u{65E5}\u{672C}\u{8A9E} cafe\u{301}s wrold"
        let words = Misspellings.words(
            in: text,
            at: ["na\u{EF}vley", "atached", "cafe\u{301}s", "wrold"].map { range(of: $0, in: text) })
        #expect(cut(text, words) == ["na\u{EF}vley", "atached", "cafe\u{301}s", "wrold"])
        #expect(words.first == Misspelt(start: 22, end: 30))
        #expect(words.last?.end == text.utf8.count)
        #expect(SpellChecked(id: 1, words: words) != nil)
    }

    @Test func whatIsNotProseIsLeftOut() {
        let text = "Teh agenda is at https://exampel.com/atached, ask robinn@exampel.com.\n> teh old feedbak\nThanx"
        let words = Misspellings.words(
            in: text,
            at: ["Teh", "exampel", "atached", "robinn", "teh", "feedbak", "Thanx"].map { range(of: $0, in: text) })
        #expect(cut(text, words) == ["Teh", "Thanx"])
    }

    @Test func badRangesAreLeftOutNotMended() {
        let text = "\u{1F600}teh wrold cafe\u{301} agre"
        let teh = range(of: "teh", in: text)
        let wrold = range(of: "wrold", in: text)
        let agre = range(of: "agre", in: text)
        let words = Misspellings.words(
            in: text,
            at: [
                NSRange(location: NSNotFound, length: 0),
                // Half the emoji and the word after it.
                NSRange(location: 1, length: 4),
                teh,
                NSRange(location: wrold.location, length: 0),
                NSRange(location: wrold.location + 5, length: -5),
                wrold,
                // A letter without its accent.
                NSRange(location: 12, length: 4),
                NSRange(location: agre.location, length: 40),
                agre,
                NSRange(location: 400, length: 4),
            ])
        #expect(cut(text, words) == ["teh", "wrold", "agre"])
    }

    @Test func wordsComeInOrderAndNeverOverlap() {
        let text = "one two three four five"
        let ranges = ["four", "two", "five", "one", "three"].map { range(of: $0, in: text) }
        #expect(cut(text, Misspellings.words(in: text, at: ranges)) == ["one", "two", "three", "four", "five"])
        // The same word twice, a word inside another, a word across two.
        let muddled = [
            range(of: "two", in: text), range(of: "two", in: text), range(of: "one two", in: text),
            range(of: "wo thr", in: text), range(of: "hre", in: text), range(of: "four", in: text),
            range(of: "ou", in: text), range(of: "r f", in: text),
        ]
        let words = Misspellings.words(in: text, at: muddled)
        // The earliest start wins, and then the shorter of two that share it.
        #expect(cut(text, words) == ["one two", "hre", "four"])
        #expect(SpellChecked(id: 1, words: words) != nil)
        // Words that touch do not overlap.
        let touching = Misspellings.words(
            in: "tehwrold", at: [NSRange(location: 3, length: 5), NSRange(location: 0, length: 3)])
        #expect(touching == [Misspelt(start: 0, end: 3)!, Misspelt(start: 3, end: 8)!])
        #expect(SpellChecked(id: 1, words: touching) != nil)
    }

    @Test func noMoreWordsThanOneAnswerNames() {
        // 2500 words of three letters with a letter of two bytes in each.
        let text = String(repeating: "t\u{E9}h ", count: 2500)
        let ranges = (0..<2500).map { NSRange(location: $0 * 4, length: 3) }
        let words = Misspellings.words(in: text, at: ranges)
        #expect(words.count == 2000)
        #expect(words.first == Misspelt(start: 0, end: 4))
        // The first 2000 in the text, whatever order they came in.
        #expect(words.last == Misspelt(start: 1999 * 5, end: 1999 * 5 + 4))
        #expect(Misspellings.words(in: text, at: ranges.reversed()) == words)
        #expect(SpellChecked(id: 1, words: words) != nil)
        // What is left out for other reasons does not count towards them.
        let quoted = "> " + text + "\n" + text
        let later = (0..<2500).map { NSRange(location: 2 + 10_000 + 1 + $0 * 4, length: 3) }
        let kept = Misspellings.words(
            in: quoted, at: ranges.map { NSRange(location: $0.location + 2, length: 3) } + later)
        #expect(kept.count == 2000)
        #expect(kept.first?.start == 2 + 12_500 + 1)
    }
}

@Suite struct SuggestionsTests {
    @Test func guessesAreOfferedInTheOrderGiven() {
        #expect(
            Suggestions.offered(from: ["attached", "attaches", "attacked"]) == ["attached", "attaches", "attacked"])
        #expect(Suggestions.offered(from: []).isEmpty)
        #expect(
            Suggestions.offered(from: ["at ached", "caf\u{E9}", "\u{65E5}\u{672C}"]) == [
                "at ached", "caf\u{E9}", "\u{65E5}\u{672C}",
            ])
    }

    @Test func emptyAndOverlongGuessesAreDropped() {
        let longest = String(repeating: "a", count: 200)
        // 101 letters of two bytes each are 202 bytes.
        let wide = String(repeating: "\u{FC}", count: 101)
        #expect(
            Suggestions.offered(from: ["", "attached", longest + "a", longest, wide, String(wide.dropLast()), ""])
                == ["attached", longest, String(wide.dropLast())])
        #expect(Suggestions.offered(from: ["", ""]).isEmpty)
    }

    @Test func noMoreGuessesThanOneAnswerMakes() {
        let guesses = (1...30).map { "guess\($0)" }
        #expect(Suggestions.offered(from: guesses) == Array(guesses.prefix(20)))
        // The ones dropped do not count towards the twenty.
        let holed = guesses.flatMap { ["", $0, String(repeating: "x", count: 201)] }
        #expect(Suggestions.offered(from: holed) == Array(guesses.prefix(20)))
        #expect(SpellSuggested(id: 1, words: Suggestions.offered(from: holed)) != nil)
    }
}
