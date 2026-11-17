import Bridge
import Foundation

/// The rows of the window of keys: the core's list, heading by heading, or
/// what is left of it when the user has typed something to find.
///
/// The list is the core's (`keys.list`): which commands there are, what
/// each is called, under which heading and in what order. Here it is only
/// set out in rows and narrowed. A key is shown as people read it on a Mac
/// and found as it is written either way, so `cmd-e` and `⌘E` both find
/// the same row.
public struct KeyRows: Equatable, Sendable {
    public enum Row: Equatable, Sendable {
        case heading(String)
        /// What a command does, and its keys as they are shown, the
        /// preferred one first. No keys for a command none is bound to.
        case key(title: String, keys: [String])
    }

    public let rows: [Row]
    /// How many commands are among the rows.
    public let count: Int

    /// The whole list with nothing typed. Otherwise the commands that have
    /// every word typed, in what they are called or in one of their keys,
    /// and every command under a heading that has them all. A heading with
    /// nothing left under it is left out.
    public init(_ list: KeysList, matching query: String = "") {
        let words = query.split(whereSeparator: \.isWhitespace).map { Self.folded(String($0)) }
        var rows: [Row] = []
        var count = 0
        for section in list.sections {
            let whole = !words.isEmpty && Self.has(words, in: [Self.folded(section.title)])
            let lines = section.keys.filter { line in
                words.isEmpty || whole || Self.has(words, in: Self.haystack(line))
            }
            guard !lines.isEmpty else { continue }
            rows.append(.heading(section.title))
            for line in lines {
                rows.append(.key(title: line.title, keys: line.keys.map(Self.shown)))
            }
            count += lines.count
        }
        self.rows = rows
        self.count = count
    }

    /// A key as the window shows it: as a Mac writes it where it is one of
    /// Quill's, and as it came otherwise.
    public static func shown(_ key: String) -> String {
        KeyNotation(key)?.display ?? key
    }

    /// Everything a command can be found by.
    private static func haystack(_ line: KeysList.Line) -> [String] {
        [folded(line.title)] + line.keys.map(folded) + line.keys.map { folded(shown($0)) }
    }

    /// Whether every word is in one of the texts.
    private static func has(_ words: [String], in texts: [String]) -> Bool {
        words.allSatisfy { word in texts.contains { $0.contains(word) } }
    }

    /// Text as it is compared: without case or accents.
    private static func folded(_ text: String) -> String {
        text.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
    }
}
