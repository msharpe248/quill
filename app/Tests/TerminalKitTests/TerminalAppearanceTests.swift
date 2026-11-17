import Testing

@testable import TerminalKit

@Suite struct TerminalAppearanceTests {
    @Test func withNoColoursAskedForTheTerminalKeepsItsOwn() {
        let appearance = TerminalAppearance()
        #expect(appearance.background == nil && appearance.foreground == nil)
        #expect(appearance.palette == .followsMac)
        #expect(appearance.checkedBackground == nil && appearance.checkedForeground == nil)
        // Text of another colour on the terminal's own background is still
        // on the Mac's light or dark.
        #expect(TerminalAppearance(foreground: "#c0caf5").palette == .followsMac)
    }

    @Test func theOtherColoursAreTheOnesThatCanBeReadOnTheBackground() {
        // Quill's dark and light themes.
        #expect(TerminalAppearance(background: "#1a1b26", foreground: "#c0caf5").palette == .dark)
        #expect(TerminalAppearance(background: "#fafafa", foreground: "#383a42").palette == .light)
        #expect(TerminalAppearance(background: "#000000").palette == .dark)
        #expect(TerminalAppearance(background: "#ffffff").palette == .light)
        // A strong blue is dark to the eye, a strong yellow light.
        #expect(TerminalAppearance(background: "#0000ff").palette == .dark)
        #expect(TerminalAppearance(background: "#ffff00").palette == .light)
        #expect(TerminalAppearance(background: "#7f7f7f").palette == .dark)
        #expect(TerminalAppearance(background: "#818181").palette == .light)
    }

    @Test func onlyAColourIsGivenToTheTerminal() {
        let good = TerminalAppearance(background: "#1a1b26", foreground: "#C0CAF5")
        #expect(good.checkedBackground == "#1a1b26")
        #expect(good.checkedForeground == "#C0CAF5")
        let channels = TerminalAppearance.channels(of: "#1a1b26")
        #expect(channels?.red == 0x1a && channels?.green == 0x1b && channels?.blue == 0x26)
        #expect(TerminalAppearance.channels(of: nil) == nil)
        // Anything else is not passed on, least of all a second line of
        // the terminal's configuration.
        for bad in [
            "", "blue", "#fff", "1a1b26", "#1a1b2", "#1a1b26ff", "#1a1b2g", "#1a1b2\n", "#1a\nb26",
            "#+a1b26", "#-1a1b2", "# 1a1b2", "#1a1b26\nkeybind = all", "#\u{ff11}a1b26", "#\u{0661}a1b26",
        ] {
            let appearance = TerminalAppearance(background: bad, foreground: bad)
            #expect(appearance.checkedBackground == nil, "\(bad.debugDescription)")
            #expect(appearance.checkedForeground == nil, "\(bad.debugDescription)")
            #expect(appearance.palette == .followsMac, "\(bad.debugDescription)")
        }
    }

    @Test func onlyAFontsNameAndASizeATerminalCanHaveAreGivenToTheTerminal() {
        // With none named the terminal keeps its own font.
        #expect(TerminalAppearance().checkedFontFamily == nil)
        #expect(TerminalAppearance().checkedFontSize == 13)
        let named = TerminalAppearance(fontFamily: "JetBrains Mono", fontSize: 14.5)
        #expect(named.checkedFontFamily == "JetBrains Mono")
        #expect(named.checkedFontSize == 14.5)
        // The name goes into the terminal's configuration as a line of it:
        // nothing that could be another line is one.
        for bad in ["", "Menlo\nkeybind = all", "Menlo\r", "Men\u{7f}lo", String(repeating: "m", count: 129)] {
            #expect(TerminalAppearance(fontFamily: bad).checkedFontFamily == nil, "\(bad.debugDescription)")
        }
        // A size no terminal is read at is brought to one that is.
        #expect(TerminalAppearance(fontSize: 2).checkedFontSize == 6)
        #expect(TerminalAppearance(fontSize: 500).checkedFontSize == 72)
        #expect(TerminalAppearance(fontSize: .nan).checkedFontSize == 13)
        #expect(TerminalAppearance(fontSize: .infinity).checkedFontSize == 13)
    }
}
