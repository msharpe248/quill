import Foundation
import Testing

@testable import Bridge

/// The form messages: `form.show`, `form.close` and `form.used`. The
/// examples under `schema/examples` are the contract the Rust core is tested
/// against too.
@Suite struct FormMessageTests {
    private static let examples = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // BridgeTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // app
        .deletingLastPathComponent()  // repository root
        .appendingPathComponent("schema/examples")

    private func example(_ path: String) throws -> String {
        try String(contentsOf: Self.examples.appendingPathComponent(path), encoding: .utf8)
    }

    private let securities = [
        FormOption(id: "tls", title: "TLS"), FormOption(id: "starttls", title: "STARTTLS"),
        FormOption(id: "none", title: "None"),
    ]

    @Test func theFormDecodesToWhatItSays() throws {
        guard case let .formShow(form) = try CoreToShell(line: example("core-to-shell/form.show.json")) else {
            Issue.record("not a form.show")
            return
        }
        #expect(form.step == "server")
        #expect(form.title == "Add an account: the server")
        #expect(form.note == "Found in the DNS records of example.com.")
        #expect(form.error == "The port is a number from 1 to 65535.")
        #expect(form.busy == nil)
        #expect(
            form.fields.map(\.id) == [
                "type", "name", "address", "host", "port", "security", "smtp_host", "password", "code",
                "shortcut",
            ])
        #expect(
            form.fields[0]
                == FormField(
                    id: "type", label: "Type:",
                    kind: .choice(
                        value: "imap",
                        options: [
                            FormOption(id: "imap", title: "IMAP (password)"),
                            FormOption(id: "microsoft", title: "Microsoft (sign in)"),
                        ])))
        #expect(form.fields[1] == FormField(id: "name", label: "Name:", kind: .text(value: "Example")))
        #expect(
            form.fields[2] == FormField(id: "address", label: "Address:", kind: .fixed(value: "robin@example.com")))
        #expect(form.fields[4] == FormField(id: "port", label: "Port:", kind: .text(value: "99x"), focus: true))
        #expect(form.fields[5].kind == .choice(value: "tls", options: securities))
        #expect(form.fields[6].help == "Empty: the account will read mail and not send it.")
        // A secret has nothing to say of what it holds.
        #expect(form.fields[7] == FormField(id: "password", label: "Password:", kind: .secret))
        #expect(form.fields[7].value == nil)
        // A code is shown to be read off: it is there for the view to draw,
        // and is no value to type over.
        #expect(
            form.fields[8]
                == FormField(
                    id: "code", label: "Code:", kind: .code(value: FormSecret("HJKL-MNPQ")),
                    help: "The code is good for 15 minutes."))
        #expect(form.fields[8].value == nil)
        // Keys that have been pressed, as Quill writes them: there to be
        // pressed over.
        #expect(
            form.fields[9]
                == FormField(
                    id: "shortcut", label: "New Key:", kind: .keys(value: "g g"),
                    help: "Press the key, or two keys one after the other."))
        #expect(form.fields[9].value == "g g")
        #expect(
            form.fields.map(\.isEditable) == [true, true, false, true, true, true, true, true, false, true])
        #expect(
            form.buttons == [
                FormButton(id: "cancel", title: "Cancel", role: .cancel),
                FormButton(id: "back", title: "Back"),
                FormButton(id: "continue", title: "Continue", role: .confirm),
            ])

        #expect(try CoreToShell(line: example("core-to-shell/form.close.json")) == .formClose)
        #expect(CoreToShell.formClose.line == #"{"jsonrpc":"2.0","method":"form.close","params":{}}"#)
    }

    @Test func whatWasDoneWithTheFormDecodesToWhatItSays() throws {
        guard case let .formUsed(used) = try ShellToCore(line: example("shell-to-core/form.used.json")) else {
            Issue.record("not a form.used")
            return
        }
        #expect(used.step == "password")
        #expect(used.button == "continue")
        #expect(used.values == ["name": "Example", "security": "tls"])
        #expect(used.secrets.keys.sorted() == ["password"])
        #expect(used.secrets["password"]?.revealed == "not-a-real-password")

        // A changed choice has no button, and the field is left out.
        let changed = ShellToCore.formUsed(FormUsed(step: "server", button: nil, values: ["type": "microsoft"])!)
        #expect(
            changed.line
                == #"{"jsonrpc":"2.0","method":"form.used","params":{"secrets":{},"step":"server","values":{"type":"microsoft"}}}"#
        )
        #expect(try ShellToCore(line: changed.line) == changed)
    }

    @Test func whatIsLeftOutIsLeftOutAndAwkwardTextSurvives() throws {
        let plain = FormShow(step: "address", title: "Add an account", fields: [], buttons: [])!
        #expect(
            CoreToShell.formShow(plain).line
                == #"{"jsonrpc":"2.0","method":"form.show","params":{"buttons":[],"fields":[],"step":"address","title":"Add an account"}}"#
        )
        let awkward = FormShow(
            step: "certificate", title: "Größe \"quoted\"", note: "one\ntwo", error: "é \\ /", busy: "Waiting…",
            fields: [
                FormField(id: "sha256", label: "SHA-256:", kind: .fixed(value: "ab:cd\nef:01")),
                FormField(id: "name", label: "", kind: .text(value: "日本語"), focus: true, help: "small print"),
            ],
            buttons: [FormButton(id: "reject", title: "Reject", role: .cancel, enabled: false)])!
        let line = CoreToShell.formShow(awkward).line
        #expect(!line.contains("\n"))
        #expect(try CoreToShell(line: line) == .formShow(awkward))
        // False and missing are the same thing, and neither is written.
        #expect(!CoreToShell.formShow(plain).line.contains("focus"))
        #expect(!CoreToShell.formShow(plain).line.contains("null"))
    }

    @Test func aFormIsOnlyBuiltWhole() {
        let name = FormField(id: "name", label: "Name:", kind: .text(value: ""))
        let build: ([FormField], [FormButton]) -> FormShow? = { fields, buttons in
            FormShow(step: "server", title: "Add an account", fields: fields, buttons: buttons)
        }
        #expect(build([name], []) != nil)
        #expect(FormShow(step: "The Server", title: "x", fields: [], buttons: []) == nil)
        #expect(FormShow(step: "", title: "x", fields: [], buttons: []) == nil)
        #expect(FormShow(step: "server", title: "", fields: [], buttons: []) == nil)
        // Two fields of one name, or a field with no name.
        #expect(build([name, name], []) == nil)
        #expect(build([FormField(id: "", label: "x", kind: .secret)], []) == nil)
        #expect(build([FormField(id: "Pass Word", label: "x", kind: .secret)], []) == nil)
        // Only one field has the focus, and never one that is only shown.
        let focused = { (id: String) in FormField(id: id, label: "x", kind: .text(value: ""), focus: true) }
        #expect(build([focused("a")], []) != nil)
        #expect(build([focused("a"), focused("b")], []) == nil)
        #expect(build([FormField(id: "a", label: "x", kind: .fixed(value: "y"), focus: true)], []) == nil)
        #expect(build([FormField(id: "a", label: "x", kind: .code(value: FormSecret("y")))], []) != nil)
        #expect(build([FormField(id: "a", label: "x", kind: .code(value: FormSecret("y")), focus: true)], []) == nil)
        // A choice is one of its options, each with a name of its own.
        let choice = { (value: String, ids: [String]) in
            FormField(
                id: "type", label: "Type:",
                kind: .choice(value: value, options: ids.map { FormOption(id: $0, title: $0) }))
        }
        #expect(build([choice("imap", ["imap", "microsoft"])], []) != nil)
        #expect(build([choice("pop3", ["imap", "microsoft"])], []) == nil)
        #expect(build([choice("imap", [])], []) == nil)
        #expect(build([choice("imap", ["imap", "imap"])], []) == nil)
        #expect(build([choice("imap", ["imap", "Not A Name"])], []) == nil)
        // Buttons: a name each, a title, one for return and one for esc.
        let button = { (id: String, role: FormRole?) in FormButton(id: id, title: "Go", role: role) }
        #expect(build([], [button("cancel", .cancel), button("back", nil), button("continue", .confirm)]) != nil)
        #expect(build([], [button("continue", nil), button("continue", nil)]) == nil)
        #expect(build([], [button("continue", .confirm), button("save", .confirm)]) == nil)
        #expect(build([], [button("cancel", .cancel), button("reject", .cancel)]) == nil)
        #expect(build([], [FormButton(id: "continue", title: "")]) == nil)
        #expect(build([], (0...FormShow.mostButtons).map { button("b\($0)", nil) }) == nil)
        #expect(
            build((0...FormShow.mostFields).map { FormField(id: "f\($0)", label: "", kind: .secret) }, []) == nil)
    }

    @Test func theNamesInAFormArePlainOnes() {
        for id in ["a", "smtp_host", "sha256", "trust_once", String(repeating: "a", count: 64)] {
            #expect(FormID.isValid(id), "\(id)")
        }
        for id in ["", "Name", "9lives", "_x", "a b", "a-b", "a.b", "é", String(repeating: "a", count: 65)] {
            #expect(!FormID.isValid(id), "\(id)")
        }
        #expect(FormUsed(step: "server", button: nil, values: [:]) != nil)
        #expect(FormUsed(step: "The Server", button: nil, values: [:]) == nil)
        #expect(FormUsed(step: "server", button: "Continue", values: [:]) == nil)
        #expect(FormUsed(step: "server", button: "", values: [:]) == nil)
        #expect(FormUsed(step: "server", button: nil, values: ["Name": "x"]) == nil)
        #expect(FormUsed(step: "server", button: nil, values: [:], secrets: ["pass word": FormSecret("x")]) == nil)
    }

    @Test func formsThatAreNotWholeAreRefusedAndSayWhy() {
        let show = { (params: String) in
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"form.show","params":\#(params)}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: missing step")) {
            try show(#"{"title":"x","fields":[],"buttons":[]}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: missing buttons")) {
            try show(#"{"step":"a","title":"x","fields":[]}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: unexpected html")) {
            try show(#"{"step":"a","title":"x","fields":[],"buttons":[],"html":"<b>"}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: note is not a string")) {
            try show(#"{"step":"a","title":"x","note":null,"fields":[],"buttons":[]}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: a field is text, secret, choice, static, code or keys")) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"html","id":"a","label":"","value":""}],"buttons":[]}"#)
        }
        // The core never sends what a secret field holds.
        #expect(throws: BridgeError.malformed("form.show: unexpected value")) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"secret","id":"a","label":"","value":"not-a-real-password"}],"buttons":[]}"#
            )
        }
        // Nor is a field that is only shown given the keyboard.
        #expect(throws: BridgeError.malformed("form.show: unexpected focus")) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"static","id":"a","label":"","value":"","focus":true}],"buttons":[]}"#
            )
        }
        // A code is only shown too, and what it shows is said, never quoted
        // in what is wrong with the form.
        #expect(throws: BridgeError.malformed("form.show: unexpected focus")) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"code","id":"a","label":"","value":"HJKL-MNPQ","focus":true}],"buttons":[]}"#
            )
        }
        #expect(throws: BridgeError.malformed("form.show: missing value")) {
            try show(#"{"step":"a","title":"x","fields":[{"kind":"code","id":"a","label":""}],"buttons":[]}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: value is not a string")) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"code","id":"a","label":"","value":4711}],"buttons":[]}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: missing options")) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"choice","id":"a","label":"","value":"b"}],"buttons":[]}"#)
        }
        #expect(throws: BridgeError.malformed("form.show: unexpected colour")) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"choice","id":"a","label":"","value":"b","options":[{"id":"b","title":"B","colour":"red"}]}],"buttons":[]}"#
            )
        }
        #expect(throws: BridgeError.malformed("form.show: a button's role is default or cancel")) {
            try show(
                #"{"step":"a","title":"x","fields":[],"buttons":[{"id":"go","title":"Go","role":"destructive","enabled":true}]}"#
            )
        }
        #expect(throws: BridgeError.malformed("form.show: missing enabled")) {
            try show(#"{"step":"a","title":"x","fields":[],"buttons":[{"id":"go","title":"Go"}]}"#)
        }
        #expect(throws: BridgeError.self) {
            try show(
                #"{"step":"a","title":"x","fields":[{"kind":"text","id":"a","label":"","value":""},{"kind":"text","id":"a","label":"","value":""}],"buttons":[]}"#
            )
        }
        #expect(throws: BridgeError.malformed("form.close: unexpected step")) {
            try CoreToShell(line: #"{"jsonrpc":"2.0","method":"form.close","params":{"step":"a"}}"#)
        }
    }

    @Test func aSecretIsWrittenOnTheBridgeAndNowhereElse() throws {
        let password = "not-a-real-password"
        let secret = FormSecret(password)
        let used = FormUsed(
            step: "password", button: "continue", values: ["name": "Example"], secrets: ["password": secret])!
        let message = ShellToCore.formUsed(used)
        // On its way to the core it is a string like any other.
        #expect(message.line.contains(#""secrets":{"password":"not-a-real-password"}"#))
        #expect(try ShellToCore(line: message.line) == message)
        #expect(secret.revealed == password)

        // Printed any way there is, it is not there: by itself, in its
        // message, in a list, described, reflected or dumped.
        var dumped = ""
        dump(secret, to: &dumped)
        dump(used, to: &dumped)
        dump(message, to: &dumped)
        dump(["password": secret], to: &dumped)
        let printed: [String] = [
            "\(secret)", String(describing: secret), String(reflecting: secret),
            "\(used)", String(describing: used), String(reflecting: used),
            "\(message)", String(describing: message), String(reflecting: message),
            "\([secret])", "\(["password": secret])", String(reflecting: Optional(secret)),
            "\(Mirror(reflecting: secret).children.map { "\($0.value)" })",
            dumped,
        ]
        for text in printed {
            #expect(!text.contains(password), "\(text)")
        }
        #expect("\(used)".contains("<secret>"))
        // What is not secret still prints, to be of use.
        #expect("\(used)".contains("Example"))
    }

    @Test func aCodeIsWrittenOnTheBridgeAndDrawnAndNeverPrinted() throws {
        let code = "HJKL-MNPQ"
        let page = "https://login.example.com/device"
        let form = FormShow(
            step: "sign_in", title: "Sign In to Work", note: "On any device, open this page and enter the code:",
            busy: "Quill is waiting, and takes it from there.",
            fields: [
                FormField(id: "page", label: "Page:", kind: .code(value: FormSecret(page))),
                FormField(
                    id: "code", label: "Code:", kind: .code(value: FormSecret(code)),
                    help: "The code is good for 15 minutes."),
            ],
            buttons: [FormButton(id: "cancel", title: "Cancel", role: .cancel)])!
        let message = CoreToShell.formShow(form)
        // It comes from the core as a string like any other, and is there
        // for the view that draws it.
        #expect(message.line.contains(#""kind":"code""#))
        #expect(message.line.contains(#""value":"HJKL-MNPQ""#))
        #expect(try CoreToShell(line: message.line) == message)
        guard case let .code(shown) = form.fields[1].kind else {
            Issue.record("not a code")
            return
        }
        #expect(shown.revealed == code)

        // Printed any way there is, neither the code nor the page is there.
        var dumped = ""
        dump(form, to: &dumped)
        dump(message, to: &dumped)
        dump(form.fields, to: &dumped)
        let printed: [String] = [
            "\(form)", String(describing: form), String(reflecting: form),
            "\(message)", String(describing: message), String(reflecting: message),
            "\(form.fields)", "\(form.fields[1].kind)", String(reflecting: form.fields[0]),
            dumped,
        ]
        for text in printed {
            #expect(!text.contains("HJKL"), "\(text)")
            #expect(!text.contains("login.example.com"), "\(text)")
        }
        #expect("\(form)".contains("<secret>"))
        // What is not kept back still prints, to be of use.
        #expect("\(form)".contains("Sign In to Work"))
    }

    @Test func whyAFormUsedCouldNotBeReadNeverQuotesIt() {
        // Each is wrong in a way that could be described by quoting what
        // was found: a secret where a name or an object belongs.
        let password = "not-a-real-password"
        let broken = [
            #"{"step":"password","values":{},"secrets":"\#(password)"}"#,
            #"{"step":"password","values":{},"secrets":{"password":7,"other":"\#(password)"}}"#,
            #"{"step":"password","values":{"name":["\#(password)"]},"secrets":{}}"#,
            #"{"step":"\#(password) no name","values":{},"secrets":{}}"#,
            #"{"step":"password","button":"\#(password) no name","values":{},"secrets":{}}"#,
            #"{"step":"password","values":{},"secrets":{"pass word":"\#(password)"}}"#,
            #"{"step":"password","values":{}}"#,
        ]
        for params in broken {
            let line = #"{"jsonrpc":"2.0","method":"form.used","params":\#(params)}"#
            do {
                _ = try ShellToCore(line: line)
                Issue.record("accepted \(params)")
            } catch {
                #expect(!"\(error)".contains(password), "\(error)")
                #expect("\(error)".contains("form.used"), "\(error)")
            }
        }
    }
}
