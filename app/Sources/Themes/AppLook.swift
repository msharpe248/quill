import Bridge

/// Whether the app's own parts are light or dark: its windows, sheets and
/// the reading pane.
///
/// The settings say light, dark, or as the Mac is, and the core passes that
/// on (`appearance` in `state.update`). Light and dark are put on the app
/// whatever the Mac is. As the Mac is means the app is left alone, and
/// follows the Mac by itself; the core is then told what the Mac is, and
/// told again when it changes, so that it can draw with the theme for it.
///
/// A scripted run never asks what the Mac is. It is told, by its script,
/// and the app is put in that appearance outright.
public struct AppLook: Equatable, Sendable {
    /// What the settings say.
    public var setting: Appearance
    /// What a script says the Mac is, in a scripted run.
    public var pretended: Bool?

    public init(setting: Appearance = .system, pretended: Bool? = nil) {
        self.setting = setting
        self.pretended = pretended
    }

    /// What is put on the app: dark, light, or nothing, which leaves it to
    /// the Mac.
    public var forced: Bool? {
        switch setting {
        case .dark: true
        case .light: false
        case .system: pretended
        }
    }

    /// Whether the app's appearance is the Mac's own at this moment, so
    /// that asking the app what it looks like says what the Mac is.
    public var showsTheMac: Bool { setting == .system && pretended == nil }

    /// What to tell the core the Mac is, given what the app looks like now
    /// (`appIsDark`), or `nil` when that cannot be known: while the app is
    /// held light or dark, its look says nothing of the Mac's. The core
    /// has no use for it then either, and is told when the setting goes
    /// back to the Mac's.
    public func systemIsDark(appIsDark: Bool) -> Bool? {
        if let pretended { return pretended }
        return setting == .system ? appIsDark : nil
    }
}
