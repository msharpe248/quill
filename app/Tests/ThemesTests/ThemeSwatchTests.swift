import Foundation
import Testing

@testable import Themes

struct ThemeSwatchTests {
    private func color(_ red: UInt8, _ green: UInt8, _ blue: UInt8) -> ThemeSwatch.Color {
        ThemeSwatch.Color(red: red, green: green, blue: blue)
    }

    @Test func aColourIsSixDigitsOrThreeWithOrWithoutTheHash() {
        #expect(ThemeSwatch.Color("#282a36") == color(0x28, 0x2a, 0x36))
        #expect(ThemeSwatch.Color("282A36") == color(0x28, 0x2a, 0x36))
        #expect(ThemeSwatch.Color("#fa0") == color(0xff, 0xaa, 0x00))
        #expect(ThemeSwatch.Color("FA0") == color(0xff, 0xaa, 0x00))
        for not in [
            "", "#", "#12345", "#1234567", "12", "#ggg", "red", "#12 345", "##123456", "+12345", "-12345",
            "0x1234", "\u{661}\u{662}\u{663}",
        ] {
            #expect(ThemeSwatch.Color(not) == nil, "\(not)")
        }
    }

    @Test func aThemesFewColoursAreReadForItsPatch() throws {
        let swatch = try #require(ThemeSwatch(text: Written.night))
        #expect(swatch.background == color(0x1e, 0x20, 0x30))
        #expect(swatch.foreground == color(0xd0, 0xd4, 0xea))
        // Red, green, yellow, blue, magenta, cyan: not black, not white.
        #expect(
            swatch.accents == [
                color(0xf0, 0x60, 0x6a), color(0x6f, 0xd0, 0x8c), color(0xe8, 0xc4, 0x6a), color(0x7a, 0xa0, 0xf0),
                color(0xc5, 0x8a, 0xf0), color(0x6f, 0xd0, 0xe0),
            ])
        #expect(swatch.isDark)

        let paper = try #require(ThemeSwatch(text: Written.paper))
        #expect(!paper.isDark)
        // Only the colours the theme has.
        #expect(paper.accents == [color(0x2a, 0x5d, 0xb0)])
    }

    @Test func oddSpacingCommentsQuotesAndOtherKeysAreReadThrough() throws {
        let swatch = try #require(
            ThemeSwatch(
                text: """
                    # A theme of my own\r
                    \r
                    \tbackground=#102030   \r
                    foreground   =   "d0d0d0"
                    palette=1=#f00
                    palette = 4 = "#0000ff"
                    palette = 200=#123456
                    palette = 9=#123456
                    font-family = Menlo
                    keybind = ctrl+a=reload_config
                    a line with no equals sign
                    = #ffffff
                    # background = #ffffff
                    background = papayawhip
                    """))
        // A colour by name is not read, and what was said before stands.
        #expect(swatch.background == color(0x10, 0x20, 0x30))
        #expect(swatch.foreground == color(0xd0, 0xd0, 0xd0))
        #expect(swatch.accents == [color(0xff, 0x00, 0x00), color(0x00, 0x00, 0xff)])
    }

    @Test func whatIsNotAThemeHasNoPatch() {
        for text in [
            "", "\n\n# nothing here\n", "background = #000000\n", "foreground = #ffffff\n",
            "palette = 0=#000000\npalette = 1=#ff0000\n", "Remember the milk.\n",
            // One of Quill's own, in TOML.
            "name = \"mine\"\n[colors]\nbackground = \"#1a1b26\"\ntext = \"#c0caf5\"\n",
            "<html><body>background = red</body></html>",
        ] {
            #expect(ThemeSwatch(text: text) == nil, "\(text)")
        }
        // Too large, however good the start of it is; and too many lines.
        let large = Written.night + String(repeating: "#\n", count: ThemeSwatch.largest / 2)
        #expect(ThemeSwatch(text: large) == nil)
        let long = Written.paper + String(repeating: "\n", count: ThemeSwatch.mostLines)
        #expect(long.utf8.count <= ThemeSwatch.largest)
        #expect(ThemeSwatch(text: long) == nil)
        // Within both, it is read.
        #expect(ThemeSwatch(text: Written.paper + String(repeating: "\n", count: 100)) != nil)
    }
}
