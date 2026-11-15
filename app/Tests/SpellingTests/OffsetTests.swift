import Foundation
import Testing

@testable import Spelling

/// The Mac's checker says where a word is in UTF-16 units. The core wants
/// the same word in bytes of UTF-8. Nearly every text that is not plain
/// English counts differently the two ways.
@Suite struct OffsetTests {
    private func bytes(_ location: Int, _ length: Int, in text: String) -> Range<Int>? {
        TextOffsets.utf8Ranges(of: [NSRange(location: location, length: length)], in: text)[0]
    }

    /// The bytes of `text` in `range`, as text again.
    private func cut(_ text: String, _ range: Range<Int>?) -> String? {
        range.map { String(decoding: Array(text.utf8)[$0], as: UTF8.self) }
    }

    @Test func plainLettersCountTheSameBothWays() {
        let text = "The agenda is atached. Thanks for the feedbak."
        #expect(bytes(14, 7, in: text) == 14..<21)
        #expect(bytes(38, 7, in: text) == 38..<45)
        #expect(bytes(0, 3, in: text) == 0..<3)
        // The whole of it, to the very end.
        #expect(bytes(0, 46, in: text) == 0..<46)
        #expect(bytes(45, 1, in: text) == 45..<46)
    }

    @Test func lettersBeforeTheWordMoveItByTheirBytes() {
        // An accented letter: one unit, two bytes.
        #expect(bytes(2, 7, in: "\u{E9} atached") == 3..<10)
        #expect(bytes(7, 7, in: "Gr\u{FC}\u{DF}e, atached") == 9..<16)
        // Chinese, Japanese and Korean: one unit, three bytes.
        #expect(bytes(4, 7, in: "\u{65E5}\u{672C}\u{8A9E} atached") == 10..<17)
        #expect(bytes(3, 7, in: "\u{D55C}\u{AE00} atached") == 7..<14)
        // A euro sign: one unit, three bytes.
        #expect(bytes(4, 7, in: "5 \u{20AC} atached") == 6..<13)
        // An emoji outside the first plane: two units, four bytes.
        #expect(bytes(3, 7, in: "\u{1F600} atached") == 5..<12)
        // A thumb with a skin tone: four units, eight bytes.
        #expect(bytes(5, 7, in: "\u{1F44D}\u{1F3FD} atached") == 9..<16)
        // A family joined with zero-width joiners: eight units, eighteen bytes.
        #expect(bytes(9, 7, in: "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} atached") == 19..<26)
        // A flag: four units, eight bytes.
        #expect(bytes(5, 7, in: "\u{1F1EB}\u{1F1F7} atached") == 9..<16)
        // A letter with its accent written after it: two units, three bytes.
        #expect(bytes(6, 7, in: "cafe\u{301} atached") == 7..<14)
        // Several lines of it.
        let text = "Gr\u{FC}\u{DF}e \u{1F44D}\u{1F3FD}\n\u{65E5}\u{672C}\u{8A9E}\ncafe\u{301} atached"
        #expect(cut(text, bytes(21, 7, in: text)) == "atached")
        #expect(bytes(21, 7, in: text) == 34..<41)
    }

    @Test func lettersInsideTheWordMakeItLonger() {
        #expect(bytes(0, 7, in: "na\u{EF}vley") == 0..<8)
        #expect(bytes(4, 7, in: "the na\u{EF}vley put") == 4..<12)
        // The accent written after its letter is part of the word.
        #expect(bytes(0, 6, in: "cafe\u{301}s") == 0..<7)
        #expect(bytes(0, 5, in: "cafe\u{301}") == 0..<6)
        #expect(bytes(0, 6, in: "\u{43F}\u{440}\u{438}\u{432}\u{435}\u{442}") == 0..<12)
        #expect(bytes(1, 2, in: "\u{65E5}\u{672C}\u{8A9E}") == 3..<9)
        // An emoji in the middle of what the checker calls a word.
        #expect(bytes(0, 6, in: "ab\u{1F600}cd") == 0..<8)
        #expect(
            cut("x na\u{EF}ve\u{1F600}t\u{E9} y", bytes(2, 9, in: "x na\u{EF}ve\u{1F600}t\u{E9} y"))
                == "na\u{EF}ve\u{1F600}t\u{E9}")
    }

    @Test func aRangeThatSplitsACharacterIsDropped() {
        // Half an emoji is not text at all.
        let emoji = "\u{1F600} ok"
        #expect(bytes(0, 1, in: emoji) == nil)
        #expect(bytes(1, 1, in: emoji) == nil)
        #expect(bytes(1, 4, in: emoji) == nil)
        #expect(bytes(0, 2, in: emoji) == 0..<4)
        #expect(bytes(3, 2, in: emoji) == 5..<7)
        // A thumb without its skin tone, a tone without its thumb.
        let thumb = "\u{1F44D}\u{1F3FD}"
        #expect(bytes(0, 2, in: thumb) == nil)
        #expect(bytes(2, 2, in: thumb) == nil)
        #expect(bytes(0, 4, in: thumb) == 0..<8)
        // A letter without its accent, an accent without its letter.
        let accent = "cafe\u{301} au lait"
        #expect(bytes(0, 4, in: accent) == nil)
        #expect(bytes(4, 1, in: accent) == nil)
        #expect(bytes(4, 4, in: accent) == nil)
        #expect(bytes(0, 5, in: accent) == 0..<6)
        // One of a family, half a flag.
        #expect(bytes(0, 2, in: "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}") == nil)
        #expect(bytes(0, 2, in: "\u{1F1EB}\u{1F1F7}") == nil)
        // A carriage return and the line feed after it are one line break.
        let lines = "teh\r\nend"
        #expect(bytes(0, 3, in: lines) == 0..<3)
        #expect(bytes(0, 4, in: lines) == nil)
        #expect(bytes(4, 4, in: lines) == nil)
        #expect(bytes(5, 3, in: lines) == 5..<8)
    }

    @Test func aRangeThatIsEmptyBackwardsOrOutsideTheTextIsDropped() {
        let text = "na\u{EF}ve teh"
        #expect(bytes(6, 3, in: text) == 7..<10)
        // Empty, at the start, in the middle and at the end.
        #expect(bytes(0, 0, in: text) == nil)
        #expect(bytes(6, 0, in: text) == nil)
        #expect(bytes(9, 0, in: text) == nil)
        // Backwards.
        #expect(bytes(6, -3, in: text) == nil)
        #expect(bytes(9, -9, in: text) == nil)
        #expect(bytes(-3, 6, in: text) == nil)
        #expect(bytes(-1, 1, in: text) == nil)
        // Past the end, by a little and by as much as there is.
        #expect(bytes(6, 4, in: text) == nil)
        #expect(bytes(9, 1, in: text) == nil)
        #expect(bytes(10, 1, in: text) == nil)
        #expect(bytes(0, 10, in: text) == nil)
        #expect(bytes(1, Int.max, in: text) == nil)
        #expect(bytes(Int.max, 1, in: text) == nil)
        #expect(bytes(Int.max, Int.max, in: text) == nil)
        #expect(bytes(Int.min, 3, in: text) == nil)
        #expect(bytes(3, Int.min, in: text) == nil)
        // How the checker says it found nothing.
        #expect(bytes(NSNotFound, 0, in: text) == nil)
        #expect(bytes(NSNotFound, 3, in: text) == nil)
        // Nothing is anywhere in no text.
        #expect(bytes(0, 0, in: "") == nil)
        #expect(bytes(0, 1, in: "") == nil)
    }

    @Test func everyRangeIsAnsweredInItsPlace() {
        let text = "\u{1F600} teh wrold"
        let ranges = [
            NSRange(location: 7, length: 5), NSRange(location: 1, length: 2), NSRange(location: 3, length: 3),
            NSRange(location: NSNotFound, length: 0), NSRange(location: 3, length: 3),
        ]
        #expect(TextOffsets.utf8Ranges(of: ranges, in: text) == [9..<14, nil, 5..<8, nil, 5..<8])
        #expect(TextOffsets.utf8Ranges(of: [], in: text).isEmpty)
    }

    /// Every range there could be in a text of every kind of character,
    /// against the answer worked out the slow way, index by index.
    @Test func everyRangeOfAMixedTextAgreesWithTheStringItself() {
        let text =
            "A\u{E9}e\u{301} \u{20AC}\u{65E5}\u{1F600}\u{1F44D}\u{1F3FD}\r\n\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"
            + "\u{1F1EB}\u{1F1F7}z\u{D55C}\u{5E9}\u{5DC}\u{5D5}\u{5DD}\u{E01}\u{E33} o\u{308}\u{304}k"
        let units = text.utf16.count
        // Where each character starts, in units and in bytes.
        var starts: [Int: Int] = [:]
        for index in text.indices {
            starts[text.utf16.distance(from: text.startIndex, to: index)] =
                text.utf8.distance(from: text.startIndex, to: index)
        }
        starts[units] = text.utf8.count
        #expect(starts.count == text.count + 1)
        #expect(starts.count < units)

        var ranges: [NSRange] = []
        for location in 0...units {
            for length in 0...(units - location + 1) {
                ranges.append(NSRange(location: location, length: length))
            }
        }
        let answers = TextOffsets.utf8Ranges(of: ranges, in: text)
        #expect(answers.count == ranges.count)
        var kept = 0
        for (range, answer) in zip(ranges, answers) {
            let expected: Range<Int>? =
                if range.length > 0, let start = starts[range.location], let end = starts[range.location + range.length]
                {
                    start..<end
                } else {
                    nil
                }
            #expect(answer == expected, "\(range)")
            if let answer {
                kept += 1
                // The same letters, whichever way they are counted.
                #expect(cut(text, answer) == (text as NSString).substring(with: range), "\(range)")
            }
        }
        // Every pair of places a character starts, and nothing else.
        #expect(kept == text.count * (text.count + 1) / 2)
    }

    @Test func aLongTextIsCountedOnce() {
        // As much as one check takes, in letters of every length.
        let line = "Gr\u{FC}\u{DF}e \u{1F44D}\u{1F3FD} \u{65E5}\u{672C}\u{8A9E} cafe\u{301} atached\n"
        let text = String(repeating: line, count: 100_000 / line.utf8.count)
        let word = (line as NSString).range(of: "atached")
        let lineUnits = line.utf16.count
        let ranges = (0..<(text.utf16.count / lineUnits)).map {
            NSRange(location: $0 * lineUnits + word.location, length: word.length)
        }
        let answers = TextOffsets.utf8Ranges(of: ranges, in: text)
        #expect(answers.count == ranges.count)
        let all = Array(text.utf8)
        let atached = Array("atached".utf8)
        #expect(answers.allSatisfy { $0.map { all[$0].elementsEqual(atached) } == true })
    }
}
