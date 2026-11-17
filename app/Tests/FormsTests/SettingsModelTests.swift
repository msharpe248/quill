import Bridge
import Foundation
import Testing

@testable import Forms

/// The settings window, without a window: what is on show for what the core
/// sends, what a thing done to a control tells the core, and what is drawn
/// again when the core answers.
struct SettingsModelTests {
    private func general(
        theme: String = "dark", notifications: Bool = true, help: String? = nil, canImport: Bool = true
    ) -> SettingsPane {
        SettingsPane(
            id: "general", title: "General", symbol: "gearshape",
            fields: [
                FormField(
                    id: "theme", label: "Theme:",
                    kind: .choice(
                        value: theme,
                        options: [FormOption(id: "dark", title: "Dark"), FormOption(id: "light", title: "Light")]),
                    help: help),
                FormField(
                    id: "notifications", label: "New mail:",
                    kind: .toggle(title: "Show a notification", value: notifications)),
                FormField(id: "nickname", label: "Called:", kind: .text(value: "Robin")),
                FormField(id: "where", label: "Kept in:", kind: .fixed(value: "This Mac")),
                FormField(
                    id: "import_theme", label: "", kind: .button(title: "Import Theme\u{2026}", enabled: canImport)),
            ], more: "More is set in the settings file.")
    }

    private func composing(seconds: Double = 10) -> SettingsPane {
        SettingsPane(
            id: "composing", title: "Composing", symbol: "square.and.pencil",
            fields: [
                FormField(
                    id: "undo_send_seconds", label: "Undo send:",
                    kind: .number(FormNumber(value: seconds, min: 0, max: 60, step: 1, decimals: 0, unit: "seconds"))),
                FormField(
                    id: "cache_gb", label: "Keep up to:",
                    kind: .number(FormNumber(value: 5, min: 0, step: 1, decimals: 2, unit: "GB"))),
            ])
    }

    private func accounts(
        rows: [FormList.Row] = [
            FormList.Row(id: "work", title: "Work", detail: "Microsoft 365", actions: ["sign_in"]),
            FormList.Row(id: "home", title: "Home", detail: "IMAP"),
        ], canAdd: Bool = true
    ) -> SettingsPane {
        SettingsPane(
            id: "accounts", title: "Accounts", symbol: "at",
            fields: [
                FormField(
                    id: "accounts", label: "",
                    kind: .list(
                        FormList(
                            rows: rows,
                            actions: [
                                FormList.Action(id: "add", title: "Add an Account…", row: false, enabled: canAdd),
                                FormList.Action(id: "sign_in", title: "Sign In…", row: true),
                            ], empty: "No accounts yet.")))
            ])
    }

    private func settings(_ panes: [SettingsPane], problem: String? = nil) -> SettingsShow {
        SettingsShow(panes: panes, problem: problem)!
    }

    private func whole() -> SettingsShow {
        settings([general(), accounts(), composing()])
    }

    @Test func thePaneOnShowIsTheOneTheUserLastHadIfThereIsStillOne() {
        var model = SettingsModel()
        #expect(model.settings == nil && model.pane == nil && model.panes.isEmpty)
        #expect(model.summary == ["settings: nothing from the core"])

        // The first time: every pane is drawn, and the one remembered from
        // before is on show.
        var update = model.show(whole(), remembered: "composing")
        #expect(update.panesChanged)
        #expect(update.redraw == ["general", "accounts", "composing"])
        #expect(update.pane == "composing" && model.pane == "composing")
        // The same again: nothing to draw, and the pane stays, whatever
        // was remembered.
        update = model.show(whole(), remembered: "general")
        #expect(!update.panesChanged && update.redraw.isEmpty && update.pane == "composing")
        // The user chooses another; one there is not is not chosen.
        let chosen = model.select(pane: "accounts")
        #expect(chosen && model.pane == "accounts")
        let missing = model.select(pane: "privacy")
        #expect(!missing && model.pane == "accounts")
        // The pane on show is gone from what the core sends: the first.
        update = model.show(settings([general(), composing()]), remembered: "accounts")
        #expect(update.panesChanged && update.pane == "general")

        // Remembering a pane there is none of, or nothing: the first.
        for remembered in ["storage", "The General Pane", nil] as [String?] {
            var fresh = SettingsModel()
            let first = fresh.show(whole(), remembered: remembered)
            #expect(first.pane == "general")
        }
    }

    @Test func onlyWhatDiffersFromWhatIsOnShowIsDrawnAgain() {
        var model = SettingsModel()
        _ = model.show(whole(), remembered: nil)
        // One pane says something else: that one alone.
        var update = model.show(settings([general(theme: "light"), accounts(), composing()]), remembered: nil)
        #expect(!update.panesChanged && update.redraw == ["general"])
        // Something is wrong: it is said on every pane, and again when it
        // is no longer so.
        update = model.show(
            settings([general(theme: "light"), accounts(), composing()], problem: "That was not changed."),
            remembered: nil)
        #expect(update.redraw == ["general", "accounts", "composing"])
        #expect(model.problem == "That was not changed.")
        update = model.show(settings([general(theme: "light"), accounts(), composing()]), remembered: nil)
        #expect(update.redraw == ["general", "accounts", "composing"] && model.problem == nil)
        // Another title is another toolbar.
        let renamed = SettingsPane(id: "general", title: "Common", symbol: "gearshape", fields: [])
        update = model.show(settings([renamed, accounts(), composing()]), remembered: nil)
        #expect(update.panesChanged)
    }

    @Test func aChangeIsShownAtOnceAndToldToTheCoreAndItsAnswerMovesNothing() throws {
        var model = SettingsModel()
        _ = model.show(whole(), remembered: nil)

        // A choice, by the name of the option.
        let answer1 = model.set(.text("light"), in: "theme", of: "general")
        var used = try #require(answer1)
        #expect(used == SettingsUsed(pane: "general", field: "theme", did: .set(.text("light"))))
        #expect(model.settings?.field("theme", in: "general")?.value == "light")
        // The core made the change and says the same: nothing is drawn.
        var update = model.show(settings([general(theme: "light"), accounts(), composing()]), remembered: nil)
        #expect(update.redraw.isEmpty)

        // A toggle.
        let answer2 = model.set(.flag(false), in: "notifications", of: "general")
        used = try #require(answer2)
        #expect(used.did == .set(.flag(false)))
        #expect(
            model.settings?.field("notifications", in: "general")?.kind
                == .toggle(title: "Show a notification", value: false))
        // The core did not make the change, and says what is still so: the
        // pane is drawn again, with the control back where it was.
        update = model.show(
            settings([general(theme: "light"), accounts(), composing()], problem: "That was not changed."),
            remembered: nil)
        #expect(update.redraw.contains("general"))
        #expect(
            model.settings?.field("notifications", in: "general")?.kind
                == .toggle(title: "Show a notification", value: true))

        // The core made the change and says something more beside it: the
        // pane is drawn again for that.
        _ = model.show(whole(), remembered: nil)
        _ = model.set(.text("light"), in: "theme", of: "general")
        update = model.show(
            settings([
                general(theme: "light", help: "Takes effect when Quill is next opened."), accounts(), composing(),
            ]),
            remembered: nil)
        #expect(update.redraw == ["general"])

        // Text.
        let answer3 = model.set(.text("Sam"), in: "nickname", of: "general")
        used = try #require(answer3)
        #expect(used.did == .set(.text("Sam")))
        #expect(model.settings?.field("nickname", in: "general")?.value == "Sam")
        // The help beside a field is kept when its value is changed.
        #expect(
            model.settings?.field("theme", in: "general")?.help == "Takes effect when Quill is next opened.")
    }

    @Test func aNumberIsBroughtWithinItsBoundsBeforeItIsTold() throws {
        var model = SettingsModel()
        _ = model.show(whole(), remembered: nil)
        let seconds = { (model: SettingsModel) -> Double? in
            guard case let .number(number)? = model.settings?.field("undo_send_seconds", in: "composing")?.kind
            else { return nil }
            return number.value
        }
        let answer4 = model.set(.number(15), in: "undo_send_seconds", of: "composing")
        var used = try #require(answer4)
        #expect(used.did == .set(.number(15)) && seconds(model) == 15)
        // Past the most: the most.
        let answer5 = model.set(.number(600), in: "undo_send_seconds", of: "composing")
        used = try #require(answer5)
        #expect(used.did == .set(.number(60)) && seconds(model) == 60)
        // Below the least: the least.
        let answer6 = model.set(.number(-4), in: "undo_send_seconds", of: "composing")
        used = try #require(answer6)
        #expect(used.did == .set(.number(0)) && seconds(model) == 0)
        // To the places the field shows.
        let answer7 = model.set(.number(7.4), in: "undo_send_seconds", of: "composing")
        used = try #require(answer7)
        #expect(used.did == .set(.number(7)))
        let answer8 = model.set(.number(0.126), in: "cache_gb", of: "composing")
        used = try #require(answer8)
        #expect(used.did == .set(.number(0.13)))
        // With no most there is none.
        let answer9 = model.set(.number(4000), in: "cache_gb", of: "composing")
        used = try #require(answer9)
        #expect(used.did == .set(.number(4000)))
        // What comes to the number it already is tells the core nothing.
        let refused1 = model.set(.number(7.2), in: "undo_send_seconds", of: "composing")
        #expect(refused1 == nil)
        let refused2 = model.set(.number(.nan), in: "undo_send_seconds", of: "composing")
        #expect(refused2 == nil)
    }

    @Test func whatAFieldDoesNotTakeIsNotToldAndChangesNothing() {
        var model = SettingsModel()
        _ = model.show(whole(), remembered: nil)
        let before = model
        // None of the options, or the option it already is.
        let refused3 = model.set(.text("solarized"), in: "theme", of: "general")
        #expect(refused3 == nil)
        let refused4 = model.set(.text("dark"), in: "theme", of: "general")
        #expect(refused4 == nil)
        // The wrong kind of value for the field.
        let refused5 = model.set(.flag(true), in: "theme", of: "general")
        #expect(refused5 == nil)
        let refused6 = model.set(.text("on"), in: "notifications", of: "general")
        #expect(refused6 == nil)
        let refused7 = model.set(.number(1), in: "notifications", of: "general")
        #expect(refused7 == nil)
        let refused8 = model.set(.text("10"), in: "undo_send_seconds", of: "composing")
        #expect(refused8 == nil)
        // The value it already has.
        let refused9 = model.set(.flag(true), in: "notifications", of: "general")
        #expect(refused9 == nil)
        let refused10 = model.set(.text("Robin"), in: "nickname", of: "general")
        #expect(refused10 == nil)
        // A field that is only shown, a list, and one there is not, or
        // not in that pane.
        let refused11 = model.set(.text("That Mac"), in: "where", of: "general")
        #expect(refused11 == nil)
        let refused12 = model.set(.text("work"), in: "accounts", of: "accounts")
        #expect(refused12 == nil)
        let refused13 = model.set(.text("light"), in: "font", of: "general")
        #expect(refused13 == nil)
        let refused14 = model.set(.text("light"), in: "theme", of: "composing")
        #expect(refused14 == nil)
        #expect(model == before)
        // And before the core has said anything there is nothing to set.
        var empty = SettingsModel()
        let refused15 = empty.set(.flag(false), in: "notifications", of: "general")
        #expect(refused15 == nil)
    }

    /// A pane with a long list: one to be searched.
    private func keys() -> SettingsPane {
        SettingsPane(
            id: "keys", title: "Keys", symbol: "keyboard",
            fields: [
                FormField(
                    id: "bindings", label: "",
                    kind: .list(
                        FormList(
                            rows: [
                                FormList.Row(
                                    id: "message_move", title: "Move to a folder", detail: "m", actions: ["change"]),
                                FormList.Row(
                                    id: "view_zoom", title: "Zoom the focused pane", detail: "Z \u{b7} yours",
                                    actions: ["change"]),
                                FormList.Row(
                                    id: "nav_next", title: "Move down", detail: "j, down", actions: ["change"]),
                            ],
                            actions: [FormList.Action(id: "change", title: "Change\u{2026}", row: true)],
                            searchable: true)))
            ])
    }

    @Test func aLongListIsNarrowedByWhatIsTypedOverItAndTheCoreIsNotTold() {
        var model = SettingsModel()
        _ = model.show(settings([general(), keys(), accounts()]), remembered: nil)
        #expect(model.search(in: "bindings", of: "keys") == "")
        #expect(model.shownRows(in: "bindings", of: "keys").count == 3)

        // By a title, or by the keys said under it.
        model.search("move", in: "bindings", of: "keys")
        #expect(model.shownRows(in: "bindings", of: "keys").map(\.id) == ["message_move", "nav_next"])
        model.search("yours", in: "bindings", of: "keys")
        #expect(model.shownRows(in: "bindings", of: "keys").map(\.id) == ["view_zoom"])

        // A row picked stays picked while it is on show, and is let go
        // of when it is not: what cannot be seen is not acted on.
        model.pick(row: "view_zoom", in: "bindings", of: "keys")
        #expect(model.canPress("change", in: "bindings", of: "keys"))
        model.search("zoom", in: "bindings", of: "keys")
        #expect(model.pickedRow(in: "bindings", of: "keys") == "view_zoom")
        model.search("down", in: "bindings", of: "keys")
        #expect(model.pickedRow(in: "bindings", of: "keys") == nil)
        #expect(!model.canPress("change", in: "bindings", of: "keys"))

        // What was typed is kept when the core sends the settings again.
        _ = model.show(settings([general(), keys(), accounts()]), remembered: nil)
        #expect(model.search(in: "bindings", of: "keys") == "down")
        #expect(model.shownRows(in: "bindings", of: "keys").map(\.id) == ["nav_next"])
        // A list that is not one to search is not narrowed by anything.
        model.search("work", in: "accounts", of: "accounts")
        #expect(model.search(in: "accounts", of: "accounts") == "")
        #expect(model.shownRows(in: "accounts", of: "accounts").count == 2)
    }

    @Test func aListsActionsGoByTheRowPicked() throws {
        var model = SettingsModel()
        _ = model.show(whole(), remembered: nil)
        // With no row picked: only what needs none.
        #expect(model.pickedRow(in: "accounts", of: "accounts") == nil)
        #expect(model.canPress("add", in: "accounts", of: "accounts"))
        #expect(!model.canPress("sign_in", in: "accounts", of: "accounts"))
        #expect(model.press("sign_in", in: "accounts", of: "accounts") == nil)
        let answer10 = model.press("add", in: "accounts", of: "accounts")
        var used = try #require(answer10)
        #expect(used == SettingsUsed(pane: "accounts", field: "accounts", did: .action("add", row: nil)))

        // A row that takes the action: it is sent with the row.
        model.pick(row: "work", in: "accounts", of: "accounts")
        #expect(model.canPress("sign_in", in: "accounts", of: "accounts"))
        let answer11 = model.press("sign_in", in: "accounts", of: "accounts")
        used = try #require(answer11)
        #expect(used.did == .action("sign_in", row: "work"))
        // An action that needs no row is sent with none, whatever is
        // picked.
        let answer12 = model.press("add", in: "accounts", of: "accounts")
        used = try #require(answer12)
        #expect(used.did == .action("add", row: nil))
        // A row that does not take it.
        model.pick(row: "home", in: "accounts", of: "accounts")
        #expect(!model.canPress("sign_in", in: "accounts", of: "accounts"))
        #expect(model.press("sign_in", in: "accounts", of: "accounts") == nil)
        // No row, or one the list has not got, is none picked.
        model.pick(row: "nobody", in: "accounts", of: "accounts")
        #expect(model.pickedRow(in: "accounts", of: "accounts") == nil)
        // An action there is not, and one of a field that is no list.
        #expect(model.press("remove", in: "accounts", of: "accounts") == nil)
        #expect(model.press("add", in: "theme", of: "general") == nil)

        // The row picked stays picked while the core still has it, and is
        // let go when it has not; an action the core switches off cannot
        // be pressed.
        model.pick(row: "work", in: "accounts", of: "accounts")
        _ = model.show(settings([general(), accounts(canAdd: false), composing()]), remembered: nil)
        #expect(model.pickedRow(in: "accounts", of: "accounts") == "work")
        #expect(!model.canPress("add", in: "accounts", of: "accounts"))
        #expect(model.press("add", in: "accounts", of: "accounts") == nil)
        let update = model.show(
            settings([general(), accounts(rows: [FormList.Row(id: "home", title: "Home")]), composing()]),
            remembered: nil)
        #expect(update.redraw == ["accounts"])
        #expect(model.pickedRow(in: "accounts", of: "accounts") == nil)
        // Closing the window lets go of what was picked, and keeps what
        // the core said, to show when it is opened again.
        model.pick(row: "home", in: "accounts", of: "accounts")
        model.close()
        #expect(model.pickedRow(in: "accounts", of: "accounts") == nil)
        #expect(model.settings != nil)
    }

    @Test func settingsThatCannotBeReadAreNothingButWhatIsWrong() throws {
        var model = SettingsModel()
        _ = model.show(whole(), remembered: nil)
        #expect(!model.isUnreadable)
        let broken = try #require(SettingsShow(panes: [], problem: "The settings file has a mistake in it."))
        let update = model.show(broken, remembered: "general")
        #expect(update.panesChanged && update.redraw.isEmpty && update.pane == nil)
        #expect(model.isUnreadable && model.pane == nil)
        #expect(model.problem == "The settings file has a mistake in it.")
        // There is nothing in it to change.
        let refused16 = model.set(.flag(false), in: "notifications", of: "general")
        #expect(refused16 == nil)
        #expect(model.press("add", in: "accounts", of: "accounts") == nil)
        let chosen = model.select(pane: "general")
        #expect(!chosen)
        // Put right: the panes are back, with the one remembered on show.
        let back = model.show(whole(), remembered: "composing")
        #expect(back.pane == "composing" && !model.isUnreadable)
    }

    @Test func aButtonTellsTheCoreItWasPressedAndChangesNothing() throws {
        var model = SettingsModel()
        _ = model.show(whole(), remembered: nil)
        let before = model
        let used = try #require(model.pressButton("import_theme", of: "general"))
        #expect(used == SettingsUsed(pane: "general", field: "import_theme", did: .action("press", row: nil)))
        // Nothing on show is another thing for it.
        #expect(model == before)
        // What is no button is not pressed, nor one that is off, nor one
        // that is not there.
        #expect(model.pressButton("theme", of: "general") == nil)
        #expect(model.pressButton("import_theme", of: "accounts") == nil)
        #expect(model.pressButton("nothing", of: "general") == nil)
        #expect(model.press("press", in: "import_theme", of: "general") == nil)
        #expect(model.set(.flag(true), in: "import_theme", of: "general") == nil)
        _ = model.show(settings([general(canImport: false)]), remembered: nil)
        #expect(model.pressButton("import_theme", of: "general") == nil)
        #expect(model.summary.contains("settings field general.import_theme [button] \"Import Theme\u{2026}\" (off)"))
    }

    @Test func theDumpSaysWhatEachFieldHolds() {
        var model = SettingsModel()
        _ = model.show(settings([general(), accounts(), composing()], problem: "Not changed."), remembered: nil)
        model.pick(row: "work", in: "accounts", of: "accounts")
        let summary = model.summary
        #expect(summary[0] == #"settings problem: "Not changed.""#)
        #expect(summary[1] == "settings pane: general of general accounts composing")
        #expect(summary.contains(#"settings general "General" [gearshape] note: "#))
        #expect(summary.contains("settings field general.theme [choice] = dark (dark|light)"))
        #expect(summary.contains(#"settings field general.notifications [toggle] = true "Show a notification""#))
        #expect(summary.contains(#"settings field general.nickname [text] = "Robin""#))
        #expect(summary.contains(#"settings field general.where [static] = "This Mac""#))
        #expect(summary.contains("settings field general.import_theme [button] \"Import Theme\u{2026}\""))
        #expect(summary.contains(#"settings more general "More is set in the settings file.""#))
        #expect(
            summary.contains(
                #"settings field accounts.accounts [list] rows: *"Work" "Microsoft 365"; "Home" "IMAP" actions: +add +sign_in"#
            ))
        #expect(summary.contains("settings field composing.undo_send_seconds [number] = 10 (0 to 60, seconds)"))
        #expect(summary.contains("settings field composing.cache_gb [number] = 5 (0 to any, GB)"))
    }

    @Test func thePaneLastOnShowIsKeptInTheDefaultsAndOnlyThere() throws {
        let suite = "dev.quill.tests.settings-pane-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        #expect(DefaultsSettingsPaneMemory(defaults: defaults).pane == nil)
        DefaultsSettingsPaneMemory(defaults: defaults).pane = "composing"
        // Another run: another object over the same defaults.
        #expect(DefaultsSettingsPaneMemory(defaults: defaults).pane == "composing")
        #expect(defaults.string(forKey: DefaultsSettingsPaneMemory.key) == "composing")
        // What could not be a pane's name is not kept, and what is found
        // there that could not be one is nothing.
        let memory = DefaultsSettingsPaneMemory(defaults: defaults)
        memory.pane = "The Composing Pane"
        #expect(defaults.object(forKey: DefaultsSettingsPaneMemory.key) == nil)
        for junk in ["Composing", "", 7, ["composing"]] as [Any] {
            defaults.set(junk, forKey: DefaultsSettingsPaneMemory.key)
            #expect(memory.pane == nil, "\(junk)")
        }
        memory.pane = "general"
        memory.pane = nil
        #expect(defaults.object(forKey: DefaultsSettingsPaneMemory.key) == nil)

        // A scripted run starts with none, whatever the defaults hold, and
        // what it is given goes no further.
        DefaultsSettingsPaneMemory(defaults: defaults).pane = "composing"
        let transient = TransientSettingsPaneMemory()
        #expect(transient.pane == nil)
        transient.pane = "privacy"
        #expect(transient.pane == "privacy")
        #expect(DefaultsSettingsPaneMemory(defaults: defaults).pane == "composing")
    }

    @Test func aFontIsChosenByItsFamilysNameAndTheTerminalsOwnByNone() {
        let pane = SettingsPane(
            id: "appearance", title: "Appearance", symbol: "paintpalette",
            fields: [FormField(id: "font", label: "Font:", kind: .font(value: ""))])
        var model = SettingsModel()
        _ = model.show(settings([pane]), remembered: nil)
        #expect(model.summary.contains("settings field appearance.font [font] = \"\""))
        let used = model.set(.text("JetBrains Mono"), in: "font", of: "appearance")
        #expect(used == SettingsUsed(pane: "appearance", field: "font", did: .set(.text("JetBrains Mono"))))
        // What the window shows is changed at once, as the pop-up has been.
        #expect(model.settings?.field("font", in: "appearance")?.kind == .font(value: "JetBrains Mono"))
        // The same again says nothing new; the terminal's own is no name.
        #expect(model.set(.text("JetBrains Mono"), in: "font", of: "appearance") == nil)
        #expect(model.set(.text(""), in: "font", of: "appearance") != nil)
        // It takes a name and nothing else, and not one longer than a name.
        #expect(model.set(.flag(true), in: "font", of: "appearance") == nil)
        #expect(model.set(.number(12), in: "font", of: "appearance") == nil)
        #expect(model.set(.text(String(repeating: "m", count: 129)), in: "font", of: "appearance") == nil)
    }
}
