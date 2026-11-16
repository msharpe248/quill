import Bridge
import Darwin
import Foundation

/// A theme written for Ghostty that is there to import: what it is called,
/// where its file is, and its colours for the chooser.
public struct ThemeSource: Equatable, Sendable, Identifiable {
    public let name: String
    public let file: URL
    public let swatch: ThemeSwatch

    public var id: String { name }

    public init(name: String, file: URL, swatch: ThemeSwatch) {
        self.name = name
        self.file = file
        self.swatch = swatch
    }
}

/// Which themes the chooser lists.
public enum ThemeScope: String, Equatable, Sendable, CaseIterable {
    case all, dark, light
}

/// What became of looking in one folder for themes.
public struct ThemeFolderReport: Equatable, Sendable {
    public enum Outcome: Equatable, Sendable {
        /// This many themes were found in it.
        case found(Int)
        /// There is no such folder.
        case notThere
        /// It is there, and may not be read: inside the App Sandbox, a
        /// folder of the user's own that they have not chosen.
        case unreadable
    }

    public let folder: URL
    public let outcome: Outcome

    public var summary: String {
        switch outcome {
        case let .found(count): "\(folder.path): \(count) themes"
        case .notThere: "\(folder.path): not there"
        case .unreadable: "\(folder.path): cannot be read"
        }
    }
}

/// The themes of a Ghostty installed on this Mac.
///
/// They are found by looking in the places Ghostty keeps them, and each
/// file is read only for its colours. Nothing here knows what a theme is
/// used for.
public enum GhosttyThemes {
    /// Where Ghostty keeps the themes it comes with, inside its application.
    static let insideGhostty = "Ghostty.app/Contents/Resources/ghostty/themes"

    /// The most files of one folder that are looked at.
    public static let mostFiles = 5000

    /// Where themes are looked for, in order: the Ghostty in
    /// `/Applications`, one in the user's own `Applications`, and the
    /// themes the user keeps for Ghostty themselves. `home` is the user's
    /// real home folder, which inside the App Sandbox is not the app's.
    public static func folders(home: URL?) -> [URL] {
        var folders = [URL(fileURLWithPath: "/Applications/\(insideGhostty)", isDirectory: true)]
        if let home {
            folders.append(home.appendingPathComponent("Applications/\(insideGhostty)", isDirectory: true))
            folders.append(home.appendingPathComponent(".config/ghostty/themes", isDirectory: true))
        }
        return folders
    }

    /// The user's real home folder, as the system has it: inside the App
    /// Sandbox `NSHomeDirectory()` is the app's container.
    public static func realHome() -> URL? {
        guard let entry = getpwuid(getuid()), let home = entry.pointee.pw_dir else { return nil }
        return URL(fileURLWithPath: String(cString: home), isDirectory: true)
    }

    /// The themes in `folders`, by name whatever the case, and what became
    /// of looking in each folder. A name found in an earlier folder is not
    /// taken from a later one. A file is a theme when its name is one a
    /// theme can have and it names a background and a foreground.
    public static func themes(in folders: [URL]) -> (themes: [ThemeSource], report: [ThemeFolderReport]) {
        var found: [ThemeSource] = []
        var seen: Set<String> = []
        var report: [ThemeFolderReport] = []
        for folder in folders {
            // Asked of the system outright, so that a folder that is not
            // there is told from one the App Sandbox will not show: the
            // first is no such file, the second is not permitted.
            var status = stat()
            guard stat(folder.path, &status) == 0 else {
                let outcome: ThemeFolderReport.Outcome = errno == ENOENT || errno == ENOTDIR ? .notThere : .unreadable
                report.append(ThemeFolderReport(folder: folder, outcome: outcome))
                continue
            }
            guard status.st_mode & S_IFMT == S_IFDIR else {
                report.append(ThemeFolderReport(folder: folder, outcome: .notThere))
                continue
            }
            guard let names = try? FileManager.default.contentsOfDirectory(atPath: folder.path) else {
                report.append(ThemeFolderReport(folder: folder, outcome: .unreadable))
                continue
            }
            var count = 0
            for name in names.prefix(mostFiles) where ThemeName.isValid(name) && !seen.contains(name) {
                let file = folder.appendingPathComponent(name)
                guard let text = try? ThemeFile.read(file), let swatch = ThemeSwatch(text: text) else { continue }
                found.append(ThemeSource(name: name, file: file, swatch: swatch))
                seen.insert(name)
                count += 1
            }
            report.append(ThemeFolderReport(folder: folder, outcome: .found(count)))
        }
        found.sort { one, other in
            let (a, b) = (one.name.lowercased(), other.name.lowercased())
            return a == b ? one.name < other.name : a < b
        }
        return (found, report)
    }

    /// The themes of `themes` that `scope` has and `query` finds: every
    /// word typed is somewhere in the name, whatever the case and the
    /// accents. Those whose name starts with what was typed come first;
    /// otherwise the order is kept.
    public static func filter(_ themes: [ThemeSource], by query: String, scope: ThemeScope = .all) -> [ThemeSource] {
        let scoped = themes.filter { theme in
            switch scope {
            case .all: true
            case .dark: theme.swatch.isDark
            case .light: !theme.swatch.isDark
            }
        }
        let typed = fold(query).trimmingCharacters(in: .whitespaces)
        let words = typed.split(separator: " ").map(String.init)
        guard !words.isEmpty else { return scoped }
        var starting: [ThemeSource] = []
        var holding: [ThemeSource] = []
        for theme in scoped {
            let name = fold(theme.name)
            guard words.allSatisfy(name.contains) else { continue }
            if name.hasPrefix(typed) {
                starting.append(theme)
            } else {
                holding.append(theme)
            }
        }
        return starting + holding
    }

    /// Text for comparing: no case, no accents.
    private static func fold(_ text: String) -> String {
        text.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
    }
}
