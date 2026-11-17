import AppKit

/// The fonts on this Mac that a terminal can be drawn in: those whose
/// letters are all one width, by their families' names. The settings window
/// offers them; which one is in use is the settings file's to say, and the
/// core's to pass on.
@MainActor
enum FixedPitchFonts {
    /// Looked up once: the list is long, and the fonts on a Mac do not
    /// change while a settings window is open often enough to ask again.
    static let families: [String] = {
        let manager = NSFontManager.shared
        let names = manager.availableFontNames(with: .fixedPitchFontMask) ?? []
        var found = Set<String>()
        for name in names {
            // A font the system keeps for itself has a name that starts
            // with a dot, and is not one to choose.
            guard let family = NSFont(name: name, size: 12)?.familyName, !family.hasPrefix(".") else { continue }
            found.insert(family)
        }
        return found.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }()
}
