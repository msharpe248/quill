import Foundation

/// Quill's way of writing keys, as the core sends them in `commands.list`:
/// `j`, `G`, `enter`, `ctrl-k`, `shift-tab`, `cmd-enter`, and `g g` for two
/// keys in a row. The shell only ever displays these or hands a `cmd` chord
/// to a menu; it never interprets what a key does.
public struct KeyNotation: Equatable, Sendable {
    public struct Modifiers: OptionSet, Sendable, Hashable {
        public let rawValue: Int
        public init(rawValue: Int) { self.rawValue = rawValue }

        public static let control = Modifiers(rawValue: 1 << 0)
        public static let option = Modifiers(rawValue: 1 << 1)
        public static let shift = Modifiers(rawValue: 1 << 2)
        public static let command = Modifiers(rawValue: 1 << 3)
    }

    public struct Chord: Equatable, Sendable {
        public var modifiers: Modifiers
        /// A single character, or a name such as `enter` or `pageup`.
        public var key: String
    }

    public var chords: [Chord]

    /// Parses a key sequence. Returns `nil` for text that is not one.
    public init?(_ text: String) {
        let parts = text.split(separator: " ", omittingEmptySubsequences: true)
        guard !parts.isEmpty else { return nil }
        var chords: [Chord] = []
        for part in parts {
            guard let chord = Self.chord(String(part)) else { return nil }
            chords.append(chord)
        }
        self.chords = chords
    }

    private static func chord(_ text: String) -> Chord? {
        // The last piece is the key, which may itself be "-".
        let prefix: Substring
        let key: String
        if text == "-" {
            (prefix, key) = ("", "-")
        } else if text.hasSuffix("--") {
            (prefix, key) = (text.dropLast(2), "-")
        } else if let dash = text.lastIndex(of: "-") {
            (prefix, key) = (text[..<dash], String(text[text.index(after: dash)...]))
        } else {
            (prefix, key) = ("", text)
        }
        guard !key.isEmpty else { return nil }
        var modifiers: Modifiers = []
        for name in prefix.split(separator: "-") {
            switch name.lowercased() {
            case "ctrl", "control": modifiers.insert(.control)
            case "alt", "opt", "option": modifiers.insert(.option)
            case "shift": modifiers.insert(.shift)
            case "cmd", "super": modifiers.insert(.command)
            default: return nil
            }
        }
        return Chord(modifiers: modifiers, key: key.count == 1 ? key : key.lowercased())
    }

    /// A key that was pressed, as Quill writes it: `j`, `G`, `ctrl-k`,
    /// `shift-tab`, `cmd-enter`. `characters` is what the key types with
    /// nothing held but shift, which is how AppKit reports a key apart from
    /// its modifiers. `nil` for what is no key of Quill's: a modifier by
    /// itself, or a key it has no name for.
    ///
    /// Shift is part of a letter or a sign (`G`, `?`) and is written only
    /// with a key that has a name, as the core reads it.
    public static func written(characters: String, modifiers: Modifiers) -> String? {
        let scalars = Array(characters.unicodeScalars)
        guard scalars.count == 1, let scalar = scalars.first else { return nil }
        var held = modifiers
        let key: String
        if let name = Self.pressedNames[scalar.value] {
            key = name
        } else if (0xF704...0xF71B).contains(scalar.value) {
            key = "f\(scalar.value - 0xF703)"
        } else if scalar.value > 0x20, scalar.value != 0x7F, !(0xF700...0xF8FF).contains(scalar.value) {
            key = String(Character(scalar))
            held.remove(.shift)
        } else {
            return nil
        }
        var text = ""
        if held.contains(.control) { text += "ctrl-" }
        if held.contains(.option) { text += "alt-" }
        if held.contains(.shift) { text += "shift-" }
        if held.contains(.command) { text += "cmd-" }
        return text + key
    }

    /// The keys with names, by what AppKit says each types.
    private static let pressedNames: [UInt32: String] = [
        0x0D: "enter", 0x03: "enter", 0x09: "tab", 0x19: "tab", 0x1B: "esc", 0x20: "space",
        0x7F: "backspace", 0x08: "backspace", 0xF728: "delete",
        0xF700: "up", 0xF701: "down", 0xF702: "left", 0xF703: "right",
        0xF729: "home", 0xF72B: "end", 0xF72C: "pageup", 0xF72D: "pagedown",
    ]

    /// The key as a menu shortcut, when a menu can own it: a single chord
    /// that includes `cmd`. Every other key belongs to the terminal, and a
    /// menu that claimed it would steal it from the core.
    public var menuShortcut: (key: String, modifiers: Modifiers)? {
        guard chords.count == 1, let chord = chords.first, chord.modifiers.contains(.command) else {
            return nil
        }
        if chord.key.count == 1 {
            // AppKit reads an upper-case key equivalent as shift held down.
            let isUpper = chord.key != chord.key.lowercased()
            let modifiers = isUpper ? chord.modifiers.union(.shift) : chord.modifiers
            return (chord.key.lowercased(), modifiers)
        }
        guard let character = Self.menuCharacters[chord.key] else { return nil }
        return (character, chord.modifiers)
    }

    private static let menuCharacters: [String: String] = [
        "enter": "\r", "return": "\r", "tab": "\t", "esc": "\u{1B}", "escape": "\u{1B}",
        "space": " ", "backspace": "\u{08}", "delete": "\u{7F}", "del": "\u{7F}",
        "up": "\u{F700}", "down": "\u{F701}", "left": "\u{F702}", "right": "\u{F703}",
        "home": "\u{F729}", "end": "\u{F72B}", "pageup": "\u{F72C}", "pagedown": "\u{F72D}",
    ]

    /// The key as people read it on a Mac: `⌃K`, `⇧⇥`, `g g`, `↩`.
    public var display: String {
        chords.map { chord in
            var text = ""
            if chord.modifiers.contains(.control) { text += "⌃" }
            if chord.modifiers.contains(.option) { text += "⌥" }
            if chord.modifiers.contains(.shift) { text += "⇧" }
            if chord.modifiers.contains(.command) { text += "⌘" }
            let key = Self.displayNames[chord.key] ?? chord.key
            // With a modifier a letter reads as its key cap. Alone, case is
            // the whole difference between `g` and `G`.
            return text + (chord.modifiers.isEmpty || key.count > 1 ? key : key.uppercased())
        }
        .joined(separator: " ")
    }

    private static let displayNames: [String: String] = [
        "enter": "↩", "return": "↩", "tab": "⇥", "esc": "esc", "escape": "esc",
        "space": "space", "backspace": "⌫", "delete": "⌦", "del": "⌦",
        "up": "↑", "down": "↓", "left": "←", "right": "→",
        "home": "↖", "end": "↘", "pageup": "⇞", "pagedown": "⇟",
    ]
}
