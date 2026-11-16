import Bridge
import Testing

@testable import Themes

struct AppLookTests {
    @Test func lightAndDarkArePutOnTheAppAndTheMacsIsLeftToTheMac() {
        #expect(AppLook().setting == .system)
        #expect(AppLook(setting: .system).forced == nil)
        #expect(AppLook(setting: .light).forced == false)
        #expect(AppLook(setting: .dark).forced == true)
        // Only while it is left to the Mac does the app show what the Mac
        // is.
        #expect(AppLook(setting: .system).showsTheMac)
        #expect(!AppLook(setting: .light).showsTheMac)
        #expect(!AppLook(setting: .dark).showsTheMac)
    }

    @Test func theCoreIsToldWhatTheMacIsOnlyWhileTheAppFollowsIt() {
        let following = AppLook(setting: .system)
        #expect(following.systemIsDark(appIsDark: true) == true)
        #expect(following.systemIsDark(appIsDark: false) == false)
        // Held light or dark, the app's look says nothing of the Mac's.
        for setting in [Appearance.light, .dark] {
            for appIsDark in [true, false] {
                #expect(AppLook(setting: setting).systemIsDark(appIsDark: appIsDark) == nil)
            }
        }
    }

    @Test func aScriptedRunIsToldWhatTheMacIsAndNeverAsks() {
        // The app is put in that appearance outright, and the core told
        // it, whatever the app happens to look like.
        var look = AppLook(setting: .system, pretended: false)
        #expect(look.forced == false && !look.showsTheMac)
        #expect(look.systemIsDark(appIsDark: true) == false)
        look.pretended = true
        #expect(look.forced == true)
        #expect(look.systemIsDark(appIsDark: false) == true)
        // The settings still win for the app itself, and the core still
        // hears what the Mac is said to be, for when they stop winning.
        look.setting = .light
        #expect(look.forced == false)
        #expect(look.systemIsDark(appIsDark: false) == true)
        look.setting = .dark
        look.pretended = false
        #expect(look.forced == true)
        #expect(look.systemIsDark(appIsDark: true) == false)
    }
}
