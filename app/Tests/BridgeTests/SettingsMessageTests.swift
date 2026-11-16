import Foundation
import Testing

@testable import Bridge

/// The settings window's messages: `settings.show`, `settings.opened`,
/// `settings.closed` and `settings.used`, and the kinds of field they added
/// to a form's. The examples under `schema/examples` are the contract the
/// Rust core is tested against too.
@Suite struct SettingsMessageTests {
    private static let examples = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // BridgeTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // app
        .deletingLastPathComponent()  // repository root
        .appendingPathComponent("schema/examples")

    private func example(_ path: String) throws -> String {
        try String(contentsOf: Self.examples.appendingPathComponent(path), encoding: .utf8)
    }

    private func toggle(_ id: String, _ value: Bool = true, title: String = "Show a notification") -> FormField {
        FormField(id: id, label: "", kind: .toggle(title: title, value: value))
    }

    private func number(
        _ value: Double, min: Double = 0, max: Double? = nil, step: Double = 1, decimals: Int = 0
    ) -> FormField {
        FormField(
            id: "seconds", label: "Undo send:",
            kind: .number(FormNumber(value: value, min: min, max: max, step: step, decimals: decimals)))
    }

    private func list(rows: [FormList.Row] = [], actions: [FormList.Action] = []) -> FormField {
        FormField(id: "accounts", label: "", kind: .list(FormList(rows: rows, actions: actions)))
    }

    private func pane(_ fields: [FormField], id: String = "general", symbol: String = "gearshape") -> SettingsPane {
        SettingsPane(id: id, title: "General", symbol: symbol, fields: fields)
    }

    @Test func theSettingsDecodeToWhatTheySay() throws {
        guard case let .settingsShow(settings) = try CoreToShell(line: example("core-to-shell/settings.show.json"))
        else {
            Issue.record("not a settings.show")
            return
        }
        #expect(settings.panes.map(\.id) == ["general", "accounts", "composing"])
        #expect(settings.panes.map(\.title) == ["General", "Accounts", "Composing"])
        #expect(settings.panes.map(\.symbol) == ["gearshape", "at", "square.and.pencil"])
        #expect(settings.problem == "That was not changed: the settings file cannot be written.")

        let general = settings.panes[0]
        #expect(general.note == nil)
        #expect(general.more == "Rules and saved searches are set in the settings file.")
        #expect(general.fields.map(\.id) == ["theme", "notifications", "nickname"])
        #expect(
            general.fields[0].kind
                == .choice(
                    value: "dark",
                    options: [FormOption(id: "dark", title: "Dark"), FormOption(id: "light", title: "Light")]))
        #expect(
            general.fields[1]
                == FormField(
                    id: "notifications", label: "New mail:",
                    kind: .toggle(title: "Show a notification", value: true), help: "Takes effect at once."))
        #expect(general.fields[2] == FormField(id: "nickname", label: "Called:", kind: .text(value: "Robin")))

        let accounts = settings.panes[1]
        #expect(accounts.note == "The accounts in the settings file.")
        #expect(accounts.more == nil)
        guard case let .list(list) = accounts.fields[0].kind else {
            Issue.record("not a list")
            return
        }
        #expect(
            list.rows == [
                FormList.Row(
                    id: "account_1f0e3dad99908345", title: "Work", detail: "Microsoft 365, robin@example.com",
                    actions: ["sign_in"]),
                FormList.Row(id: "account_9c1185a5c5e9fc54", title: "Personal"),
            ])
        #expect(
            list.actions == [
                FormList.Action(id: "add", title: "Add an Account…", row: false),
                FormList.Action(id: "sign_in", title: "Sign In…", row: true, enabled: false),
            ])
        #expect(list.empty == "No accounts yet.")

        let composing = settings.panes[2]
        #expect(
            composing.fields[0].kind
                == .number(
                    FormNumber(value: 10, min: 0, max: 4_294_967_295, step: 1, decimals: 0, unit: "seconds")))
        #expect(
            composing.fields[1]
                == FormField(
                    id: "cache_gb", label: "Keep up to:",
                    kind: .number(FormNumber(value: 0.5, min: 0, step: 0.25, decimals: 2, unit: "GB")),
                    help: "0 is no limit."))
        #expect(composing.fields[2].kind == .fixed(value: "This Mac"))
        #expect(settings.field("cache_gb", in: "composing") == composing.fields[1])
        #expect(settings.field("cache_gb", in: "general") == nil)
        // Which kinds say what the user did as they do it.
        #expect(general.fields.map(\.reportsAsChanged) == [false, true, false])
        #expect(accounts.fields[0].reportsAsChanged && composing.fields[0].reportsAsChanged)
    }

    @Test func whatWasDoneInTheSettingsDecodesToWhatItSays() throws {
        guard case let .settingsUsed(used) = try ShellToCore(line: example("shell-to-core/settings.used.json"))
        else {
            Issue.record("not a settings.used")
            return
        }
        #expect(used.pane == "composing" && used.field == "undo_send_seconds")
        #expect(used.did == .set(.number(15)))
        #expect(try ShellToCore(line: example("shell-to-core/settings.opened.json")) == .settingsOpened)
        #expect(try ShellToCore(line: example("shell-to-core/settings.closed.json")) == .settingsClosed)
        #expect(ShellToCore.settingsOpened.line == #"{"jsonrpc":"2.0","method":"settings.opened","params":{}}"#)
        #expect(ShellToCore.settingsClosed.line == #"{"jsonrpc":"2.0","method":"settings.closed","params":{}}"#)

        // Each kind of value, and an action with and without a row, as it
        // is written and as it is read back.
        let cases: [(SettingsUsed.Did, String)] = [
            (.set(.flag(false)), #""value":false"#),
            (.set(.flag(true)), #""value":true"#),
            (.set(.text("light")), #""value":"light""#),
            (.set(.text("")), #""value":"""#),
            (.set(.number(15)), #""value":15"#),
            (.set(.number(0.5)), #""value":0.5"#),
            (.action("add", row: nil), #""action":"add""#),
            (.action("sign_in", row: "account_1"), #""action":"sign_in","field":"f","pane":"p","row":"account_1""#),
        ]
        for (did, written) in cases {
            let message = ShellToCore.settingsUsed(try #require(SettingsUsed(pane: "p", field: "f", did: did)))
            #expect(message.line.contains(written), "\(message.line)")
            #expect(!message.line.contains("\n") && !message.line.contains("null"))
            #expect(try ShellToCore(line: message.line) == message)
        }
        // A number that is a whole one is written as one, as the core
        // writes it.
        let whole = ShellToCore.settingsUsed(
            try #require(SettingsUsed(pane: "storage", field: "cache_gb", did: .set(.number(2)))))
        #expect(
            whole.line
                == #"{"jsonrpc":"2.0","method":"settings.used","params":{"field":"cache_gb","pane":"storage","value":2}}"#
        )
    }

    @Test func whatWasDoneIsAValueOrAnActionNeverBoth() {
        let read = { (params: String) in
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"settings.used","params":\#(params)}"#)
        }
        for bad in [
            #"{"pane":"general","field":"theme"}"#,
            #"{"pane":"general","field":"theme","value":"light","action":"add"}"#,
            #"{"pane":"general","field":"theme","value":"light","row":"row_1"}"#,
            #"{"pane":"general","field":"theme","row":"row_1"}"#,
            #"{"pane":"General","field":"theme","value":"light"}"#,
            #"{"pane":"general","field":"","value":"light"}"#,
            #"{"pane":"general","field":"theme","value":null}"#,
            #"{"pane":"general","field":"theme","value":["light"]}"#,
            #"{"pane":"general","field":"theme","value":{"is":"light"}}"#,
            #"{"pane":"accounts","field":"accounts","action":"Sign In"}"#,
            #"{"pane":"accounts","field":"accounts","action":"sign_in","row":"Work"}"#,
            #"{"pane":"accounts","field":"accounts","action":7}"#,
            #"{"field":"theme","value":"light"}"#,
            #"{"pane":"general","field":"theme","value":"light","step":"general"}"#,
        ] {
            #expect(throws: BridgeError.self, "\(bad)") { try read(bad) }
        }
        #expect(SettingsUsed(pane: "General", field: "theme", did: .set(.flag(true))) == nil)
        #expect(SettingsUsed(pane: "general", field: "the theme", did: .set(.flag(true))) == nil)
        #expect(SettingsUsed(pane: "general", field: "theme", did: .set(.number(.nan))) == nil)
        #expect(SettingsUsed(pane: "general", field: "theme", did: .set(.number(.infinity))) == nil)
        #expect(SettingsUsed(pane: "accounts", field: "accounts", did: .action("Add", row: nil)) == nil)
        #expect(SettingsUsed(pane: "accounts", field: "accounts", did: .action("add", row: "Row")) == nil)
        let longest = String(repeating: "x", count: SettingsUsed.longestText)
        #expect(SettingsUsed(pane: "general", field: "theme", did: .set(.text(longest))) != nil)
        #expect(SettingsUsed(pane: "general", field: "theme", did: .set(.text(longest + "x"))) == nil)
        for method in ["settings.opened", "settings.closed"] {
            #expect(throws: BridgeError.self) {
                try ShellToCore(line: #"{"jsonrpc":"2.0","method":"\#(method)","params":{"pane":"general"}}"#)
            }
        }
    }

    @Test func theSettingsAreOnlyBuiltWhole() {
        let build = { (panes: [SettingsPane]) in SettingsShow(panes: panes) }
        #expect(build([pane([toggle("on"), number(10, max: 60)])]) != nil)
        // The same field in two panes is two fields.
        #expect(build([pane([toggle("on")]), pane([toggle("on")], id: "privacy")]) != nil)
        // With nothing to show but what is wrong, it says that.
        #expect(SettingsShow(panes: [], problem: "config line 3: expected a value") != nil)
        #expect(build([]) == nil)
        #expect(SettingsShow(panes: [], problem: "") == nil)

        #expect(build([pane([]), pane([])]) == nil)
        #expect(build([pane([], id: "General")]) == nil)
        #expect(build([pane([], symbol: "")]) == nil)
        #expect(build([pane([], symbol: "Gear Shape")]) == nil)
        #expect(build([SettingsPane(id: "general", title: "", symbol: "gearshape", fields: [])]) == nil)
        #expect(build([pane([toggle("on"), toggle("on")])]) == nil)
        #expect(build([pane([toggle("The Switch")])]) == nil)
        #expect(build((0...SettingsShow.mostPanes).map { pane([], id: "p\($0)") }) == nil)
        #expect(build([pane((0...FormShow.mostFields).map { toggle("f\($0)") })]) == nil)
        // No secret, no code, and no field with the focus.
        #expect(build([pane([FormField(id: "password", label: "", kind: .secret)])]) == nil)
        #expect(build([pane([FormField(id: "code", label: "", kind: .code(value: FormSecret("HJKL")))])]) == nil)
        #expect(build([pane([FormField(id: "name", label: "", kind: .text(value: ""), focus: true)])]) == nil)
        #expect(build([pane([FormField(id: "name", label: "", kind: .text(value: ""))])]) != nil)
        // A toggle says what it switches.
        #expect(build([pane([toggle("on", title: "")])]) == nil)
        // A number is within its bounds, steps by something, and has no
        // more places than are shown.
        #expect(build([pane([number(0, max: 60)])]) != nil)
        #expect(build([pane([number(60, max: 60)])]) != nil)
        #expect(build([pane([number(1e9, step: 0.25, decimals: 3)])]) != nil)
        #expect(build([pane([number(-1)])]) == nil)
        #expect(build([pane([number(61, max: 60)])]) == nil)
        #expect(build([pane([number(5, min: 5, max: 1)])]) == nil)
        #expect(build([pane([number(10, step: 0)])]) == nil)
        #expect(build([pane([number(10, step: -1)])]) == nil)
        #expect(build([pane([number(10, decimals: 4)])]) == nil)
        #expect(build([pane([number(10, decimals: -1)])]) == nil)
        #expect(build([pane([number(.nan)])]) == nil)
        #expect(build([pane([number(10, max: .infinity)])]) == nil)
        // A list's rows and actions have names of their own, and a row
        // takes only the list's actions.
        let signIn = FormList.Action(id: "sign_in", title: "Sign In", row: true)
        let work = FormList.Row(id: "work", title: "Work", actions: ["sign_in"])
        #expect(build([pane([list(rows: [work], actions: [signIn])])]) != nil)
        #expect(build([pane([list(rows: [work, work], actions: [signIn])])]) == nil)
        #expect(build([pane([list(rows: [work], actions: [])])]) == nil)
        #expect(build([pane([list(rows: [], actions: [signIn, signIn])])]) == nil)
        #expect(build([pane([list(rows: [FormList.Row(id: "Work", title: "Work")])])]) == nil)
        #expect(build([pane([list(rows: [FormList.Row(id: "work", title: "")])])]) == nil)
        #expect(
            build([pane([list(actions: [FormList.Action(id: "add", title: "", row: false)])])]) == nil)
        let many = (0...FormList.mostActions).map { FormList.Action(id: "a\($0)", title: "Do", row: false) }
        #expect(build([pane([list(actions: many)])]) == nil)
    }

    @Test func aSheetHasNoneOfTheFieldsThatReportAsTheyChange() {
        for field in [toggle("on"), number(10), list()] {
            #expect(FormShow(step: "server", title: "Add an account", fields: [field], buttons: []) == nil)
        }
        // A field that does not hang together is in no form either.
        #expect(
            FormShow(
                step: "server", title: "Add an account",
                fields: [FormField(id: "type", label: "", kind: .choice(value: "imap", options: []))], buttons: [])
                == nil)
    }

    @Test func whatIsLeftOutIsLeftOutAndAwkwardTextSurvives() throws {
        let settings = try #require(
            SettingsShow(
                panes: [
                    SettingsPane(
                        id: "general", title: "Größe \"quoted\"", symbol: "gearshape", note: "one\ntwo",
                        fields: [
                            toggle("on", false, title: "日本語"),
                            number(0.25, min: -5, max: 5.5, step: 0.25, decimals: 2),
                            list(
                                rows: [FormList.Row(id: "row_1", title: "Work \"main\"", detail: "é \\ /")],
                                actions: [FormList.Action(id: "add", title: "Add…", row: false, enabled: false)]),
                        ], more: "more")
                ], problem: "wrong\nthing"))
        let line = CoreToShell.settingsShow(settings).line
        #expect(!line.contains("\n") && !line.contains("null") && !line.contains("focus"))
        #expect(try CoreToShell(line: line) == .settingsShow(settings))
        // What is not there is not written.
        let plain = try #require(SettingsShow(panes: [pane([number(10)])]))
        let written = CoreToShell.settingsShow(plain).line
        for absent in ["problem", "note", "more", "max", "unit", "help", "empty"] {
            #expect(!written.contains("\"\(absent)\""), "\(absent) in \(written)")
        }
        // A field of a kind there is none of, and one with a part too many.
        for bad in [
            #"{"kind":"slider","id":"a","label":"","value":1}"#,
            #"{"kind":"toggle","id":"a","label":"","title":"On","value":true,"focus":true}"#,
            #"{"kind":"toggle","id":"a","label":"","title":"On","value":"true"}"#,
            #"{"kind":"number","id":"a","label":"","value":"10","min":0,"step":1,"decimals":0}"#,
            #"{"kind":"number","id":"a","label":"","value":10,"min":0,"step":1,"decimals":0.5}"#,
            #"{"kind":"number","id":"a","label":"","value":10,"min":0,"step":1}"#,
            #"{"kind":"list","id":"a","label":"","rows":[{"id":"r","title":"R"}],"actions":[]}"#,
            #"{"kind":"list","id":"a","label":"","rows":[],"actions":[{"id":"x","title":"X","row":true}]}"#,
            #"{"kind":"list","id":"a","label":"","rows":[{"id":"r","title":"R","actions":[1]}],"actions":[]}"#,
        ] {
            let line =
                #"{"jsonrpc":"2.0","method":"settings.show","params":{"panes":[{"id":"p","title":"P","symbol":"at","fields":[\#(bad)]}]}}"#
            #expect(throws: BridgeError.self, "\(bad)") { try CoreToShell(line: line) }
        }
    }

    @Test func settingsThatAreNotWholeAreRefusedAndSayWhy() {
        let show = { (pane: String) in
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"settings.show","params":{"panes":[\#(pane)]}}"#)
        }
        let with = { (field: String) in
            #"{"id":"p","title":"P","symbol":"at","fields":[\#(field)]}"#
        }
        #expect(throws: BridgeError.malformed("settings.show: missing symbol")) {
            try show(#"{"id":"p","title":"P","fields":[]}"#)
        }
        #expect(throws: BridgeError.malformed("settings.show: unexpected html")) {
            try show(#"{"id":"p","title":"P","symbol":"at","fields":[],"html":"<b>"}"#)
        }
        // The kinds a pane has, which are not all of a sheet's.
        let kinds = BridgeError.malformed("settings.show: a field is text, choice, static, toggle, number or list")
        #expect(throws: kinds) { try show(with(#"{"kind":"slider","id":"a","label":"","value":1}"#)) }
        #expect(throws: kinds) { try show(with(#"{"kind":"secret","id":"a","label":""}"#)) }
        #expect(throws: kinds) { try show(with(#"{"kind":"code","id":"a","label":"","value":"HJKL"}"#)) }
        // And a sheet has none of the settings' own.
        #expect(throws: BridgeError.malformed("form.show: a field is text, secret, choice, static or code")) {
            try CoreToShell(
                line:
                    #"{"jsonrpc":"2.0","method":"form.show","params":{"step":"a","title":"x","fields":[{"kind":"toggle","id":"a","label":"","title":"On","value":true}],"buttons":[]}}"#
            )
        }
        #expect(throws: BridgeError.malformed("settings.show: value is not true or false")) {
            try show(with(#"{"kind":"toggle","id":"a","label":"","title":"On","value":"true"}"#))
        }
        #expect(throws: BridgeError.malformed("settings.show: value is not a number")) {
            try show(with(#"{"kind":"number","id":"a","label":"","value":"10","min":0,"step":1,"decimals":0}"#))
        }
        #expect(throws: BridgeError.malformed("settings.show: decimals is not a whole number")) {
            try show(with(#"{"kind":"number","id":"a","label":"","value":10,"min":0,"step":1,"decimals":0.5}"#))
        }
        #expect(throws: BridgeError.malformed("settings.show: missing step")) {
            try show(with(#"{"kind":"number","id":"a","label":"","value":10,"min":0,"decimals":0}"#))
        }
        #expect(throws: BridgeError.malformed("settings.show: missing actions")) {
            try show(with(#"{"kind":"list","id":"a","label":"","rows":[{"id":"r","title":"R"}],"actions":[]}"#))
        }
        #expect(throws: BridgeError.malformed("settings.show: unexpected focus")) {
            try show(with(#"{"kind":"toggle","id":"a","label":"","title":"On","value":true,"focus":true}"#))
        }
        // What only the whole of it shows to be wrong is said in one go.
        #expect(throws: BridgeError.self) {
            try show(with(#"{"kind":"number","id":"a","label":"","value":10,"min":0,"step":0,"decimals":0}"#))
        }
        #expect(throws: BridgeError.malformed("settings.used: value is true or false, a number or text")) {
            try ShellToCore(
                line: #"{"jsonrpc":"2.0","method":"settings.used","params":{"pane":"p","field":"f","value":[1]}}"#)
        }
        #expect(
            throws: BridgeError.malformed("settings.used: a field is given a value, or one action is asked of a list")
        ) {
            try ShellToCore(line: #"{"jsonrpc":"2.0","method":"settings.used","params":{"pane":"p","field":"f"}}"#)
        }
    }

    @Test func aNumberIsBroughtWithinItsBoundsAndWrittenAsItIsShown() {
        let seconds = FormNumber(value: 10, min: 0, max: 60, step: 1, decimals: 0, unit: "seconds")
        #expect(seconds.taking(15) == 15)
        #expect(seconds.taking(7.4) == 7)
        #expect(seconds.taking(7.5) == 8)
        #expect(seconds.taking(-3) == 0)
        #expect(seconds.taking(99) == 60)
        #expect(seconds.taking(.nan) == nil)
        #expect(seconds.taking(.infinity) == nil)
        #expect(seconds.written(10) == "10")
        #expect(seconds.written(0) == "0")
        let room = FormNumber(value: 5, min: 0, step: 1, decimals: 2)
        #expect(room.taking(0.126) == 0.13)
        #expect(room.taking(1e12) == 1e12)
        #expect(room.written(5) == "5")
        #expect(room.written(0.5) == "0.5")
        #expect(room.written(0.25) == "0.25")
        #expect(room.written(12.10) == "12.1")
        #expect(room.written(4_294_967_295) == "4294967295")
        #expect(room.with(value: 2).value == 2)
        #expect(room.with(value: 2).decimals == 2)
    }

    @Test func aListSaysWhichOfItsActionsCanBePressed() {
        let list = FormList(
            rows: [
                FormList.Row(id: "work", title: "Work", actions: ["sign_in"]), FormList.Row(id: "home", title: "Home"),
            ],
            actions: [
                FormList.Action(id: "add", title: "Add", row: false),
                FormList.Action(id: "sign_in", title: "Sign In", row: true),
                FormList.Action(id: "export", title: "Export", row: false, enabled: false),
                FormList.Action(id: "remove", title: "Remove", row: true, enabled: false),
            ])
        // One that needs no row, with or without one picked.
        #expect(list.canPress("add", picked: nil) && list.canPress("add", picked: "home"))
        // One done to a row: only with a row that takes it.
        #expect(list.canPress("sign_in", picked: "work"))
        #expect(!list.canPress("sign_in", picked: "home"))
        #expect(!list.canPress("sign_in", picked: nil))
        #expect(!list.canPress("sign_in", picked: "nobody"))
        // One that cannot be pressed at all, and one there is not.
        #expect(!list.canPress("export", picked: nil))
        #expect(!list.canPress("remove", picked: "work"))
        #expect(!list.canPress("rename", picked: "work"))
    }
}
