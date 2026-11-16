import Foundation
import Testing

@testable import Themes

struct GhosttyThemesTests {
    @Test func themesAreLookedForInThreePlacesInOrder() {
        let folders = GhosttyThemes.folders(home: URL(fileURLWithPath: "/Users/robin", isDirectory: true))
        #expect(
            folders.map(\.path) == [
                "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
                "/Users/robin/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
                "/Users/robin/.config/ghostty/themes",
            ])
        // With no home to look in, the one place every user shares.
        #expect(GhosttyThemes.folders(home: nil).count == 1)
        #expect(GhosttyThemes.realHome() != nil)
    }

    private func rig() throws -> (Scratch, [URL]) {
        let scratch = try Scratch()
        try scratch.write(Written.night, to: "ghostty/Night Owl+")
        try scratch.write(Written.paper, to: "ghostty/Paper")
        try scratch.write(Written.night, to: "ghostty/aardvark")
        // Not themes: notes, a hidden file, a folder, a file too large to
        // be one, and one of Quill's own in TOML.
        try scratch.write("Remember the milk.\n", to: "ghostty/notes.txt")
        try scratch.write(Written.night, to: "ghostty/.hidden")
        try scratch.write(Written.night, to: "ghostty/A Folder/inside")
        try scratch.write(
            Written.night + String(repeating: "#\n", count: 40_000), to: "ghostty/Huge")
        try scratch.write("name = \"mine\"\n[colors]\ntext = \"#112233\"\n", to: "ghostty/mine.toml")
        try scratch.write(Written.night, to: "user/Mine")
        // The same name again: the first folder's is the one.
        try scratch.write(Written.night, to: "user/Paper")
        return (scratch, [scratch.file("ghostty"), scratch.file("user"), scratch.file("not there")])
    }

    @Test func theThemesThereAreToImportAreTheFilesThatAreThemes() throws {
        let (scratch, folders) = try rig()
        let (themes, report) = GhosttyThemes.themes(in: folders)
        // By name, whatever the case.
        #expect(themes.map(\.name) == ["aardvark", "Mine", "Night Owl+", "Paper"])
        #expect(themes.map(\.swatch.isDark) == [true, true, true, false])
        #expect(themes[3].file.path == scratch.file("ghostty/Paper").path)
        #expect(themes[1].file.path == scratch.file("user/Mine").path)
        #expect(themes.map(\.id) == themes.map(\.name))
        #expect(
            report.map(\.outcome) == [.found(3), .found(1), .notThere])
        #expect(report[0].summary == "\(scratch.file("ghostty").path): 3 themes")
        #expect(report[2].summary == "\(scratch.file("not there").path): not there")
        // A file where a folder was looked for is not one.
        let (none, said) = GhosttyThemes.themes(in: [scratch.file("ghostty/Paper")])
        #expect(none.isEmpty && said.map(\.outcome) == [.notThere])
        #expect(GhosttyThemes.themes(in: []).themes.isEmpty)
    }

    @Test func aFolderThatMayNotBeReadSaysSo() throws {
        let scratch = try Scratch()
        try scratch.write(Written.night, to: "locked/Night")
        try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: scratch.file("locked").path)
        defer {
            try? FileManager.default.setAttributes(
                [.posixPermissions: 0o700], ofItemAtPath: scratch.file("locked").path)
        }
        let (themes, report) = GhosttyThemes.themes(in: [scratch.file("locked")])
        #expect(themes.isEmpty)
        #expect(report.map(\.outcome) == [.unreadable])
        #expect(report[0].summary.hasSuffix("cannot be read"))
    }

    private func named(_ names: [String], dark: Set<String> = []) -> [ThemeSource] {
        let night = ThemeSwatch(text: Written.night)!
        let paper = ThemeSwatch(text: Written.paper)!
        return names.map { name in
            ThemeSource(
                name: name, file: URL(fileURLWithPath: "/themes/\(name)"),
                swatch: dark.contains(name) ? night : paper)
        }
    }

    @Test func theSearchFindsEveryWordTypedWhateverTheCase() {
        let themes = named([
            "Catppuccin Latte", "Catppuccin Mocha", "Dracula", "Dracula+", "GitHub Dark", "GitHub Light Default",
            "Monokai Pro Light", "Solarized Dark Higher Contrast", "Rosé Pine", "iTerm2 Solarized Light",
        ])
        let find = { (query: String) in GhosttyThemes.filter(themes, by: query).map(\.name) }
        // Nothing typed: all of them, in the order given.
        #expect(find("") == themes.map(\.name))
        #expect(find("   ") == themes.map(\.name))
        #expect(find("drac") == ["Dracula", "Dracula+"])
        #expect(find("DRACULA+") == ["Dracula+"])
        // Anywhere in the name; those that start with it come first.
        #expect(find("light") == ["GitHub Light Default", "Monokai Pro Light", "iTerm2 Solarized Light"])
        #expect(find("sol") == ["Solarized Dark Higher Contrast", "iTerm2 Solarized Light"])
        #expect(find("pro") == ["Monokai Pro Light"])
        #expect(find("c").first == "Catppuccin Latte")
        #expect(find("i").first == "iTerm2 Solarized Light")
        #expect(find("git") == ["GitHub Dark", "GitHub Light Default"])
        // Every word, in any order.
        #expect(find("light github") == ["GitHub Light Default"])
        #expect(find("solarized  light") == ["iTerm2 Solarized Light"])
        #expect(find("mocha latte").isEmpty)
        // Without its accent, as it is typed.
        #expect(find("rose") == ["Rosé Pine"])
        #expect(find("nothing of the kind").isEmpty)
    }

    @Test func theScopeListsTheDarkOnesOrTheLightOnes() {
        let themes = named(["Dawn", "Dusk", "Midnight", "Noon"], dark: ["Dusk", "Midnight"])
        #expect(GhosttyThemes.filter(themes, by: "", scope: .all).map(\.name) == ["Dawn", "Dusk", "Midnight", "Noon"])
        #expect(GhosttyThemes.filter(themes, by: "", scope: .dark).map(\.name) == ["Dusk", "Midnight"])
        #expect(GhosttyThemes.filter(themes, by: "", scope: .light).map(\.name) == ["Dawn", "Noon"])
        // With the search, both hold.
        #expect(GhosttyThemes.filter(themes, by: "d", scope: .dark).map(\.name) == ["Dusk", "Midnight"])
        #expect(GhosttyThemes.filter(themes, by: "d", scope: .light).map(\.name) == ["Dawn"])
        #expect(ThemeScope.allCases.map(\.rawValue) == ["all", "dark", "light"])
    }

    /// Ghostty's own themes, where Ghostty is installed: read where they
    /// are, and nothing written anywhere. Skipped on a Mac without it.
    @Test func theThemesGhosttyShipsAreAllFound() throws {
        let installed = GhosttyThemes.folders(home: nil)
        let names = (try? FileManager.default.contentsOfDirectory(atPath: installed[0].path)) ?? []
        guard !names.isEmpty else { return }
        let (themes, report) = GhosttyThemes.themes(in: installed)
        #expect(themes.count == names.filter { !$0.hasPrefix(".") }.count)
        #expect(report.map(\.outcome) == [.found(themes.count)])
        // In the order the chooser lists them, and each with its colours.
        #expect(themes.map { $0.name.lowercased() } == themes.map { $0.name.lowercased() }.sorted())
        #expect(
            GhosttyThemes.filter(themes, by: "", scope: .dark).count
                + GhosttyThemes.filter(themes, by: "", scope: .light).count == themes.count)
    }
}
