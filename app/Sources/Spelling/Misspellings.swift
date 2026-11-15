import Bridge
import Foundation

// What is done with the checker's answers before the core sees them. All of
// it is plain arithmetic on the text: nothing here asks the checker anything.

/// The Mac's checker counts a text in UTF-16 units. The core counts the same
/// text in bytes of UTF-8.
public enum TextOffsets {
    /// For each range of UTF-16 units of `text`, the bytes of the same
    /// characters in the text as UTF-8.
    ///
    /// `nil` for a range that is empty, goes backwards, reaches outside the
    /// text, or does not start and end between characters: one that splits
    /// an emoji in two, or leaves an accent behind without its letter. The
    /// core cuts the text at these offsets, so a wrong one is worse than
    /// none.
    public static func utf8Ranges(of ranges: [NSRange], in text: String) -> [Range<Int>?] {
        guard !ranges.isEmpty else { return [] }
        let map = Map(text)
        return ranges.map(map.utf8Range)
    }

    /// Where every character of a text starts, counted both ways.
    private struct Map {
        /// At each UTF-16 offset where a character starts, the byte offset
        /// where it starts in UTF-8; -1 everywhere else. One longer than
        /// the text, for where it ends.
        private let bytes: [Int]

        init(_ text: String) {
            var bytes = [Int](repeating: -1, count: text.utf16.count + 1)
            var units = 0
            var count = 0
            for character in text {
                bytes[units] = count
                units += character.utf16.count
                count += character.utf8.count
            }
            bytes[units] = count
            self.bytes = bytes
        }

        func utf8Range(of range: NSRange) -> Range<Int>? {
            let units = bytes.count - 1
            // `NSNotFound` as a location, which is how the checker says
            // "nothing", is far past the end of any text.
            guard range.length > 0, range.location >= 0, range.location <= units,
                range.length <= units - range.location
            else { return nil }
            let start = bytes[range.location]
            let end = bytes[range.location + range.length]
            guard start >= 0, end > start else { return nil }
            return start..<end
        }
    }
}

/// Which parts of a text are the user's own prose. A checker has no
/// business with the rest: an address is not a misspelling, and quoted text
/// is somebody else's.
public enum Prose {
    /// The candidates, each a range of bytes of `text` as UTF-8, that touch
    /// none of the text's `skipped` parts. Their order is kept.
    public static func words(_ candidates: [Range<Int>], in text: String) -> [Range<Int>] {
        guard !candidates.isEmpty else { return [] }
        let skipped = skipped(in: text)
        guard !skipped.isEmpty else { return candidates }
        return candidates.filter { candidate in
            // The first skipped part that ends after the candidate starts
            // is the only one that could reach it.
            var low = 0
            var high = skipped.count
            while low < high {
                let middle = (low + high) / 2
                if skipped[middle].upperBound <= candidate.lowerBound {
                    low = middle + 1
                } else {
                    high = middle
                }
            }
            return low == skipped.count || skipped[low].lowerBound >= candidate.upperBound
        }
    }

    /// The parts of a text that are not prose, as ranges of its bytes as
    /// UTF-8, in order and not overlapping:
    ///
    /// - every line whose first character is `>`: text quoted from a
    ///   message the user is answering;
    /// - every run of characters between white space that is an address:
    ///   it has `://` in it, or it starts `www.` or `mailto:` (in either
    ///   case, after any brackets, quotes or other ASCII punctuation it
    ///   opens with), or it has an `@` with a letter or digit on both sides.
    ///
    /// The whole run is skipped, with whatever punctuation is attached to
    /// it. A line ends at a line feed, a carriage return, or one of
    /// Unicode's other line and paragraph separators.
    public static func skipped(in text: String) -> [Range<Int>] {
        let bytes = Array(text.utf8)
        var skipped: [Range<Int>] = []
        var offset = 0
        var lineStart = 0
        var quoted = bytes.first == UInt8(ascii: ">")
        // Where the run of characters being read started, while in one.
        var run: Int?

        func endRun(at end: Int) {
            if let start = run, !quoted, isAddress(bytes[start..<end]) {
                skipped.append(start..<end)
            }
            run = nil
        }
        func endLine(at end: Int) {
            if quoted, lineStart < end {
                skipped.append(lineStart..<end)
            }
        }

        for scalar in text.unicodeScalars {
            let next = offset + UTF8.width(scalar)
            if scalar.properties.isWhitespace {
                endRun(at: offset)
                if isLineEnd(scalar) {
                    endLine(at: offset)
                    lineStart = next
                    quoted = next < bytes.count && bytes[next] == UInt8(ascii: ">")
                }
            } else if run == nil {
                run = offset
            }
            offset = next
        }
        endRun(at: offset)
        endLine(at: offset)
        return skipped
    }

    private static func isLineEnd(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x0A...0x0D, 0x85, 0x2028, 0x2029: true
        default: false
        }
    }

    private static let schemeMark = Array("://".utf8)
    private static let openings = [Array("www.".utf8), Array("mailto:".utf8)]

    /// Whether a run of characters with no white space in it is a web or
    /// mail address. See `skipped`.
    static func isAddress(_ run: ArraySlice<UInt8>) -> Bool {
        if contains(run, schemeMark) {
            return true
        }
        let opened = run.drop { $0 < 0x80 && !isWordByte($0) }
        if openings.contains(where: { opening in opened.prefix(opening.count).map(lowered).elementsEqual(opening) }) {
            return true
        }
        // name@host, however short. A lone `@`, or one beside punctuation,
        // is somebody writing "at".
        for index in run.indices.dropFirst().dropLast() where run[index] == UInt8(ascii: "@") {
            if isWordByte(run[index - 1]), isWordByte(run[index + 1]) {
                return true
            }
        }
        return false
    }

    /// An ASCII letter or digit, or a byte of any character outside ASCII:
    /// names and hosts are written in every script.
    private static func isWordByte(_ byte: UInt8) -> Bool {
        switch byte {
        case 0x30...0x39, 0x41...0x5A, 0x61...0x7A, 0x80...: true
        default: false
        }
    }

    private static func lowered(_ byte: UInt8) -> UInt8 {
        (0x41...0x5A).contains(byte) ? byte + 0x20 : byte
    }

    private static func contains(_ run: ArraySlice<UInt8>, _ mark: [UInt8]) -> Bool {
        guard run.count >= mark.count else { return false }
        return (run.startIndex...(run.endIndex - mark.count)).contains {
            run[$0..<($0 + mark.count)].elementsEqual(mark)
        }
    }
}

/// The checker's answer to a check, made fit to send.
public enum Misspellings {
    /// The misspelt words of `text` for the core, given where the checker
    /// found them in UTF-16 units: as bytes of UTF-8, only whole characters,
    /// only in the user's own prose, in the order they come in the text,
    /// none overlapping another, and at most as many as one answer names.
    /// What the checker got wrong is left out, never mended.
    public static func words(in text: String, at ranges: [NSRange]) -> [Misspelt] {
        let whole = TextOffsets.utf8Ranges(of: ranges, in: text).compactMap { $0 }
        let prose = Prose.words(whole, in: text)
            .sorted { ($0.lowerBound, $0.upperBound) < ($1.lowerBound, $1.upperBound) }
        var words: [Misspelt] = []
        var last = 0
        for range in prose {
            guard words.count < SpellChecked.mostWords else { break }
            guard range.lowerBound >= last, let word = Misspelt(start: range.lowerBound, end: range.upperBound) else {
                continue
            }
            words.append(word)
            last = word.end
        }
        return words
    }
}

/// The checker's guesses at a word, made fit to send.
public enum Suggestions {
    /// The guesses the core can take, in the order given: none empty, none
    /// longer than a word may be, and at most as many as one answer makes.
    public static func offered(from guesses: [String]) -> [String] {
        Array(
            guesses.lazy
                .filter { (1...SpellSuggest.longestWord).contains($0.utf8.count) }
                .prefix(SpellSuggested.mostWords))
    }
}
