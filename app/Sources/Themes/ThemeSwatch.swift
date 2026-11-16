import Foundation

/// The colours of a theme written for Ghostty, read to be shown: the patch
/// beside a theme's name in the chooser.
///
/// This is for the eye only. What a theme is, whether a file is one Quill
/// can use, and what each of its colours is used for are the core's to say;
/// a wrong patch here harms nothing. So only the few lines a patch needs are
/// read, in the same liberal way the core reads them: `key = value`, blank
/// lines and `#` comments skipped, a colour as `#rrggbb`, `rrggbb`, `#rgb`
/// or `rgb`, with or without quotes, and everything else ignored.
public struct ThemeSwatch: Equatable, Sendable {
    public struct Color: Equatable, Sendable {
        public let red: UInt8
        public let green: UInt8
        public let blue: UInt8

        public init(red: UInt8, green: UInt8, blue: UInt8) {
            self.red = red
            self.green = green
            self.blue = blue
        }

        /// Six hexadecimal digits or three, with or without `#`.
        public init?(_ text: String) {
            let digits = text.hasPrefix("#") ? text.dropFirst() : Substring(text)
            guard
                digits.utf8.allSatisfy({ byte in
                    (0x30...0x39).contains(byte) || (0x41...0x46).contains(byte) || (0x61...0x66).contains(byte)
                }), let value = UInt32(digits, radix: 16)
            else { return nil }
            switch digits.utf8.count {
            case 6:
                self.init(red: UInt8(value >> 16 & 0xff), green: UInt8(value >> 8 & 0xff), blue: UInt8(value & 0xff))
            case 3:
                // `#abc` is `#aabbcc`.
                self.init(
                    red: UInt8(value >> 8 & 0xf) * 17, green: UInt8(value >> 4 & 0xf) * 17,
                    blue: UInt8(value & 0xf) * 17)
            default:
                return nil
            }
        }

        /// How bright the eye takes it for, from 0 to 255.
        public var brightness: Double {
            0.299 * Double(red) + 0.587 * Double(green) + 0.114 * Double(blue)
        }
    }

    public let background: Color
    public let foreground: Color
    /// The colours a terminal program names, where the theme has them:
    /// red, green, yellow, blue, magenta and cyan, in that order.
    public let accents: [Color]

    /// A theme file larger than this is not one, nor one with more lines:
    /// the core's own limits.
    public static let largest = 64 * 1024
    public static let mostLines = 2000

    /// The places in the palette of the six colours shown.
    private static let shown = 1...6

    public init(background: Color, foreground: Color, accents: [Color] = []) {
        self.background = background
        self.foreground = foreground
        self.accents = accents
    }

    /// `nil` unless the text names a background and a foreground: without
    /// them it is no theme, here or to the core.
    public init?(text: String) {
        guard text.utf8.count <= Self.largest else { return nil }
        var background: Color?
        var foreground: Color?
        var palette: [Int: Color] = [:]
        var lines = 0
        for line in text.split(omittingEmptySubsequences: false, whereSeparator: \.isNewline) {
            lines += 1
            guard lines <= Self.mostLines else { return nil }
            let line = line.trimmingCharacters(in: .whitespaces)
            guard !line.isEmpty, !line.hasPrefix("#"), let equals = line.firstIndex(of: "=") else { continue }
            let key = line[..<equals].trimmingCharacters(in: .whitespaces)
            let value = Self.unquoted(String(line[line.index(after: equals)...]))
            switch key {
            case "background":
                background = Color(value) ?? background
            case "foreground":
                foreground = Color(value) ?? foreground
            case "palette":
                guard let split = value.firstIndex(of: "="),
                    let place = Int(value[..<split].trimmingCharacters(in: .whitespaces)),
                    Self.shown.contains(place),
                    let color = Color(Self.unquoted(String(value[value.index(after: split)...])))
                else { continue }
                palette[place] = color
            default:
                continue
            }
        }
        guard let background, let foreground else { return nil }
        self.init(
            background: background, foreground: foreground, accents: Self.shown.compactMap { palette[$0] })
    }

    /// Whether it is a dark theme, to the eye.
    public var isDark: Bool { background.brightness < 128 }

    private static func unquoted(_ value: String) -> String {
        let value = value.trimmingCharacters(in: .whitespaces)
        guard value.count >= 2, value.hasPrefix("\""), value.hasSuffix("\"") else { return value }
        return String(value.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
    }
}
