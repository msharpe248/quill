import Bridge
import Testing

@testable import Forms

/// The form in the sheet, without a window: what is drawn for what the core
/// sends, what is kept of the user's typing, and what goes back to the core.
struct FormModelTests {
    private let kinds = [
        FormOption(id: "imap", title: "IMAP (password)"), FormOption(id: "microsoft", title: "Microsoft (sign in)"),
    ]

    private let stepButtons = [
        FormButton(id: "cancel", title: "Cancel", role: .cancel),
        FormButton(id: "back", title: "Back"),
        FormButton(id: "continue", title: "Continue", role: .confirm),
    ]

    private func address(_ value: String = "", error: String? = nil, busy: String? = nil) -> FormShow {
        FormShow(
            step: "address", title: "Add an account", note: "Type the email address of the account.", error: error,
            busy: busy,
            fields: [FormField(id: "address", label: "Address:", kind: .text(value: value), focus: true)],
            buttons: [
                FormButton(id: "cancel", title: "Cancel", role: .cancel),
                FormButton(id: "continue", title: "Continue", role: .confirm, enabled: busy == nil),
            ])!
    }

    /// The server step of a password account.
    private func server(
        name: String = "Example", port: String = "993", security: String = "tls", focus: String = "name"
    )
        -> FormShow
    {
        FormShow(
            step: "server", title: "Add an account: the server",
            fields: [
                FormField(id: "type", label: "Type:", kind: .choice(value: "imap", options: kinds)),
                FormField(id: "name", label: "Name:", kind: .text(value: name), focus: focus == "name"),
                FormField(id: "host", label: "Server:", kind: .text(value: "imap.example.com")),
                FormField(id: "port", label: "Port:", kind: .text(value: port), focus: focus == "port"),
                FormField(
                    id: "security", label: "Security:",
                    kind: .choice(
                        value: security,
                        options: [FormOption(id: "tls", title: "TLS"), FormOption(id: "starttls", title: "STARTTLS")])),
            ], buttons: stepButtons)!
    }

    /// The server step of an account that signs in with Microsoft.
    private func microsoft(name: String = "Example", error: String? = nil, focus: String = "application") -> FormShow {
        FormShow(
            step: "server", title: "Add an account: Microsoft", error: error,
            fields: [
                FormField(id: "type", label: "Type:", kind: .choice(value: "microsoft", options: kinds)),
                FormField(id: "name", label: "Name:", kind: .text(value: name), focus: focus == "name"),
                FormField(id: "address", label: "Address:", kind: .fixed(value: "robin@example.com")),
                FormField(
                    id: "application", label: "Application id:", kind: .text(value: ""),
                    focus: focus == "application"),
                FormField(id: "tenant", label: "Tenant:", kind: .text(value: ""), help: "May be left empty."),
            ], buttons: stepButtons)!
    }

    private func password(error: String? = nil, busy: String? = nil) -> FormShow {
        FormShow(
            step: "password", title: "Add an account: sign in", error: error, busy: busy,
            fields: [FormField(id: "password", label: "Password:", kind: .secret, focus: true)],
            buttons: [
                FormButton(id: "cancel", title: "Cancel", role: .cancel),
                FormButton(id: "back", title: "Back", enabled: busy == nil),
                FormButton(id: "continue", title: "Continue", role: .confirm, enabled: busy == nil),
            ])!
    }

    private func text(_ model: FormModel, _ id: String) -> String? {
        guard case let .text(text)? = model.rows.first(where: { $0.id == id })?.content else { return nil }
        return text
    }

    private func chosen(_ model: FormModel, _ id: String) -> String? {
        guard case let .choice(selected, _)? = model.rows.first(where: { $0.id == id })?.content else { return nil }
        return selected
    }

    // MARK: - What is drawn

    @Test func nothingIsUpAtFirst() {
        let model = FormModel()
        #expect(!model.isUp)
        #expect(model.rows.isEmpty)
        #expect(model.summary == ["sheet: none"])
    }

    @Test func theFirstFormOpensTheSheetWithTheKeyboardWhereTheCoreSays() {
        var model = FormModel()
        let update = model.show(address())
        #expect(update == FormUpdate(opens: true, sameStep: false, focus: "address"))
        #expect(model.isUp)
        #expect(
            model.rows == [FormRow(id: "address", label: "Address:", content: .text(""), help: nil, enabled: true)])
        // The same form again opens nothing and moves nothing.
        #expect(model.show(address()) == FormUpdate(opens: false, sameStep: true, focus: nil))
    }

    @Test func theFieldsDrawnAreAlwaysTheOnesTheCoreNames() {
        var model = FormModel()
        _ = model.show(server())
        #expect(model.rows.map(\.id) == ["type", "name", "host", "port", "security"])
        #expect(model.rows[0].content == .choice(selected: "imap", options: kinds))
        // Another kind of account: other fields, in the core's order. None
        // of the old ones is kept, and none is added.
        _ = model.show(microsoft())
        #expect(model.rows.map(\.id) == ["type", "name", "address", "application", "tenant"])
        #expect(model.rows[2].content == .fixed("robin@example.com"))
        #expect(model.rows[4].help == "May be left empty.")
        #expect(chosen(model, "type") == "microsoft")
        // And a field of the old kind is no longer there to type in.
        model.type("mail.example.com", in: "host")
        #expect(model.press("continue", secrets: [:])?.values["host"] == nil)
    }

    @Test func whileTheCoreIsBusyNothingIsTypedOrChosen() {
        var model = FormModel()
        _ = model.show(address("robin@example.com"))
        let update = model.show(address("robin@example.com", busy: "Looking for the mail server of example.com…"))
        // The keyboard is not moved into a field that takes no typing.
        #expect(update.focus == nil)
        #expect(model.isBusy)
        #expect(model.rows.allSatisfy { !$0.enabled })
        model.type("someone@example.org", in: "address")
        #expect(text(model, "address") == "robin@example.com")
        // The core says which buttons can be pressed meanwhile.
        #expect(model.press("continue", secrets: [:]) == nil)
        #expect(model.press("cancel", secrets: [:]) != nil)
    }

    // MARK: - What is kept

    @Test func whatWasTypedIsKeptWhenTheSameStepIsSentAgain() {
        var model = FormModel()
        _ = model.show(microsoft())
        model.type("Work", in: "name")
        model.type("11111111-2222", in: "application")
        // The core says something is wrong with another field, and its own
        // idea of these has not changed: they stay as typed.
        _ = model.show(microsoft(error: "A tenant is an id or a domain name."))
        #expect(text(model, "name") == "Work")
        #expect(text(model, "application") == "11111111-2222")
    }

    @Test func theCoresValueIsShownWhenItHasChanged() {
        var model = FormModel()
        _ = model.show(server())
        model.type("1143", in: "port")
        model.type("Work", in: "name")
        // The port follows the kind of connection: the core changed it.
        _ = model.show(server(port: "143", security: "starttls"))
        #expect(text(model, "port") == "143")
        #expect(chosen(model, "security") == "starttls")
        // What it did not change is still what was typed.
        #expect(text(model, "name") == "Work")
        // Told the same again, what is typed since is kept.
        model.type("2143", in: "port")
        _ = model.show(server(port: "143", security: "starttls"))
        #expect(text(model, "port") == "2143")
    }

    @Test func aNewStepStartsFromWhatTheCoreSays() {
        var model = FormModel()
        _ = model.show(address())
        model.type("robin@example.com", in: "address")
        _ = model.show(server())
        model.type("Work", in: "name")
        // Back at the address, with the core's own value for it, which is
        // what was typed and sent; a field of the same name at another
        // step is another field.
        let update = model.show(address("robin@example.com"))
        #expect(update == FormUpdate(opens: false, sameStep: false, focus: "address"))
        #expect(text(model, "address") == "robin@example.com")
        _ = model.show(server())
        #expect(text(model, "name") == "Example")
    }

    @Test func aChoiceTheFormNoLongerOffersIsNotKept() {
        var model = FormModel()
        _ = model.show(server())
        #expect(model.choose("starttls", in: "security") != nil)
        // The core takes the option away without changing its own value.
        let fewer = FormShow(
            step: "server", title: "Add an account: the server",
            fields: [
                FormField(
                    id: "security", label: "Security:",
                    kind: .choice(value: "tls", options: [FormOption(id: "tls", title: "TLS")]))
            ], buttons: stepButtons)!
        _ = model.show(fewer)
        #expect(chosen(model, "security") == "tls")
    }

    @Test func closingKeepsNothing() {
        var model = FormModel()
        _ = model.show(address())
        model.type("robin@example.com", in: "address")
        model.close()
        #expect(model == FormModel())
        // Opened again, it is a new form.
        #expect(model.show(address()).opens)
        #expect(text(model, "address") == "")
    }

    // MARK: - The keyboard

    @Test func theKeyboardGoesWhereTheCoreSaysAfterAButtonAndStaysAfterAChoice() {
        var model = FormModel()
        _ = model.show(microsoft(focus: "name"))
        // A choice changed: the user is still looking at it.
        #expect(model.choose("imap", in: "type") != nil)
        #expect(model.show(server()).focus == nil)
        // A button pressed, and the core answers with a complaint about a
        // field: the keyboard goes to it.
        _ = model.show(microsoft(focus: "name"))
        #expect(model.press("continue", secrets: [:]) != nil)
        #expect(model.show(microsoft(error: "Enter the application id.", focus: "application")).focus == "application")
        // Once answered, a form sent again moves nothing.
        #expect(model.show(microsoft(error: "Enter the application id.", focus: "application")).focus == nil)
    }

    @Test func theKeyboardWaitsOutAWaitAndThenGoesWhereTheCoreSays() {
        var model = FormModel()
        _ = model.show(address())
        #expect(model.press("continue", secrets: [:]) != nil)
        #expect(model.show(address(busy: "Looking…")).focus == nil)
        // Not found: the same step, typed in again.
        #expect(model.show(address(error: "That does not look like an email address.")).focus == "address")
    }

    @Test func withNoFieldSaidToHaveTheFocusTheFirstThatTakesTypingHasIt() {
        var model = FormModel()
        let form = FormShow(
            step: "server", title: "x",
            fields: [
                FormField(id: "address", label: "Address:", kind: .fixed(value: "robin@example.com")),
                FormField(id: "name", label: "Name:", kind: .text(value: "")),
            ], buttons: [])!
        #expect(model.show(form).focus == "name")
        // And a form with nothing to type in gives it to nothing.
        var other = FormModel()
        let shown = FormShow(
            step: "certificate", title: "x",
            fields: [FormField(id: "sha256", label: "SHA-256:", kind: .fixed(value: "ab:cd"))], buttons: [])!
        #expect(other.show(shown).focus == nil)
    }

    // MARK: - What goes back

    @Test func aButtonSendsWhatEveryFieldHolds() {
        var model = FormModel()
        _ = model.show(server())
        model.type("Work", in: "name")
        model.type("1143", in: "port")
        let used = model.press("continue", secrets: [:])
        #expect(
            used
                == FormUsed(
                    step: "server", button: "continue",
                    values: [
                        "type": "imap", "name": "Work", "host": "imap.example.com", "port": "1143",
                        "security": "tls",
                    ]))
        // What is only shown is not sent back.
        _ = model.show(microsoft())
        #expect(
            model.press("continue", secrets: [:])?.values.keys.sorted() == ["application", "name", "tenant", "type"])
    }

    @Test func aChangedChoiceIsSentAtOnceWithNoButton() {
        var model = FormModel()
        _ = model.show(server())
        model.type("Work", in: "name")
        let used = model.choose("microsoft", in: "type")
        #expect(used?.button == nil)
        #expect(used?.step == "server")
        #expect(used?.values["type"] == "microsoft")
        // What was typed goes too, so that the core has it when the fields
        // change.
        #expect(used?.values["name"] == "Work")
        #expect(chosen(model, "type") == "microsoft")
        // The same option again is no change, and an option that is not
        // one of the field's is not chosen.
        #expect(model.choose("microsoft", in: "type") == nil)
        #expect(model.choose("pop3", in: "type") == nil)
        #expect(model.choose("imap", in: "name") == nil)
        #expect(model.choose("imap", in: "nowhere") == nil)
        #expect(chosen(model, "type") == "microsoft")
    }

    @Test func aButtonTheFormDoesNotHaveOrCannotPressSendsNothing() {
        var model = FormModel()
        #expect(model.press("continue", secrets: [:]) == nil)
        _ = model.show(password(busy: "Signing in to imap.example.com…"))
        #expect(model.press("continue", secrets: ["password": "not-a-real-password"]) == nil)
        #expect(model.press("back", secrets: [:]) == nil)
        #expect(model.press("save", secrets: [:]) == nil)
    }

    // MARK: - Secrets

    @Test func aSecretIsSentWithItsButtonAndNeverKept() {
        var model = FormModel()
        _ = model.show(password())
        // The row says there is a secret field, and nothing of what is in
        // it.
        #expect(model.rows == [FormRow(id: "password", label: "Password:", content: .secret, help: nil, enabled: true)])
        // Typing in it is not the model's to hear of.
        model.type("not-a-real-password", in: "password")
        #expect(model.press("back", secrets: [:])?.secrets.isEmpty == true)

        _ = model.show(password())
        let used = model.press("continue", secrets: ["password": "not-a-real-password", "other": "not-a-field"])
        #expect(used?.secrets.keys.sorted() == ["password"])
        #expect(used?.secrets["password"]?.revealed == "not-a-real-password")
        #expect(used?.values.isEmpty == true)
        // Nothing of it is left in the model, however it is looked at.
        var dumped = ""
        dump(model, to: &dumped)
        for text in ["\(model)", String(reflecting: model), dumped, model.summary.joined(separator: "\n")] {
            #expect(!text.contains("not-a-real-password"), "\(text)")
        }
        // Nor in what was sent, printed.
        #expect(!"\(String(describing: used))".contains("not-a-real-password"))
    }

    @Test func theButtonThatEscPressesSendsNothingThatWasTyped() {
        var model = FormModel()
        _ = model.show(password())
        let used = model.press("cancel", secrets: ["password": "not-a-real-password"])
        #expect(used == FormUsed(step: "password", button: "cancel", values: [:]))

        _ = model.show(server())
        model.type("Work", in: "name")
        #expect(model.press("cancel", secrets: [:]) == FormUsed(step: "server", button: "cancel", values: [:]))
    }

    @Test func aChangedChoiceSendsNoSecret() {
        var model = FormModel()
        let form = FormShow(
            step: "password", title: "x",
            fields: [
                FormField(id: "type", label: "Type:", kind: .choice(value: "imap", options: kinds)),
                FormField(id: "password", label: "Password:", kind: .secret),
            ], buttons: [])!
        _ = model.show(form)
        #expect(model.choose("microsoft", in: "type")?.secrets.isEmpty == true)
    }

    // MARK: - The dump

    /// A question that only waits: a sign-in with a code to read off.
    private func signIn() -> FormShow {
        FormShow(
            step: "sign_in", title: "Sign In to Work", note: "On any device, open this page and enter the code:",
            busy: "Quill is waiting, and takes it from there.",
            fields: [
                FormField(
                    id: "page", label: "Page:", kind: .code(value: FormSecret("https://login.example.com/device"))),
                FormField(
                    id: "code", label: "Code:", kind: .code(value: FormSecret("HJKL-MNPQ")),
                    help: "The code is good for 15 minutes."),
            ],
            buttons: [FormButton(id: "cancel", title: "Cancel", role: .cancel)])!
    }

    /// A question that asks for a time: a line to type on, and times to
    /// choose ready-made.
    private func time(_ typed: String = "", chosen: String = "typed", error: String? = nil) -> FormShow {
        FormShow(
            step: "snooze", title: "Snooze Until", error: error,
            fields: [
                FormField(id: "when", label: "When:", kind: .text(value: typed), focus: true, help: "Such as: 2h"),
                FormField(
                    id: "offered", label: "Preset:",
                    kind: .choice(
                        value: chosen,
                        options: [
                            FormOption(id: "typed", title: "None"), FormOption(id: "in_2h", title: "In 2 Hours"),
                            FormOption(id: "tomorrow", title: "Tomorrow"),
                        ])),
            ],
            buttons: [
                FormButton(id: "cancel", title: "Cancel", role: .cancel),
                FormButton(id: "confirm", title: "Snooze", role: .confirm),
            ])!
    }

    @Test func oneQuestionTakesAnothersPlaceInTheSheetThatIsUp() {
        var model = FormModel()
        #expect(model.show(microsoft(focus: "name")).opens)
        model.type("Work", in: "name")
        // Another question altogether: the sheet is not opened again, and
        // nothing of the form before it is kept or drawn.
        let update = model.show(signIn())
        #expect(update == FormUpdate(opens: false, sameStep: false, focus: nil))
        #expect(model.isUp)
        #expect(model.rows.map(\.id) == ["page", "code"])
        #expect(model.isBusy)
        // It has one thing to press, and that sends nothing but itself.
        #expect(model.press("continue", secrets: [:]) == nil)
        #expect(model.press("cancel", secrets: [:]) == FormUsed(step: "sign_in", button: "cancel", values: [:]))
        // And back: the form starts from what the core says, with the
        // keyboard where the core says.
        let back = model.show(microsoft(name: "Example", focus: "name"))
        #expect(back == FormUpdate(opens: false, sameStep: false, focus: "name"))
        #expect(text(model, "name") == "Example")
    }

    @Test func aCodeIsDrawnAndIsNoFieldToTypeInOrSend() {
        var model = FormModel()
        _ = model.show(signIn())
        // It is there for the view to draw, with what is said under it.
        guard case let .code(shown)? = model.rows.last?.content else {
            Issue.record("not a code")
            return
        }
        #expect(shown.revealed == "HJKL-MNPQ")
        #expect(model.rows.last?.help == "The code is good for 15 minutes.")
        // It takes no typing and no choosing, and is nothing a button
        // sends back.
        model.type("0000", in: "code")
        #expect(model.choose("0000", in: "code") == nil)
        guard case let .code(still)? = model.rows.last?.content else {
            Issue.record("not a code")
            return
        }
        #expect(still.revealed == "HJKL-MNPQ")
        #expect(model.press("cancel", secrets: ["code": "0000"])?.values == [:])

        // Nothing that prints the model or sums it up has the code or the
        // page: the dump of a scripted run has a marker for each.
        var dumped = ""
        dump(model, to: &dumped)
        for text in ["\(model)", String(reflecting: model), dumped, model.summary.joined(separator: "\n")] {
            #expect(!text.contains("HJKL"), "\(text)")
            #expect(!text.contains("login.example.com"), "\(text)")
        }
        #expect(
            model.summary == [
                "sheet: up",
                "sheet step: sign_in",
                #"sheet title: "Sign In to Work""#,
                #"sheet note: "On any device, open this page and enter the code:""#,
                "sheet error: ",
                #"sheet busy: "Quill is waiting, and takes it from there.""#,
                "sheet field page [code] = <secret>",
                "sheet field code [code] = <secret>",
                #"sheet buttons: cancel "Cancel" (esc)"#,
            ])
    }

    @Test func aTimeChosenIsSentAtOnceAndTheCoresLineIsShownInPlaceOfWhatWasTyped() {
        var model = FormModel()
        #expect(model.show(time()).focus == "when")
        model.type("fri 9", in: "when")
        // Choosing a ready-made time tells the core, with what the line
        // held and no button.
        #expect(
            model.choose("tomorrow", in: "offered")
                == FormUsed(step: "snooze", button: nil, values: ["when": "fri 9", "offered": "tomorrow"]))
        // The core types it on the line: its value for the line has
        // changed, so that is what is shown, and the keyboard stays put.
        #expect(model.show(time("tomorrow", chosen: "tomorrow")).focus == nil)
        #expect(text(model, "when") == "tomorrow")
        #expect(chosen(model, "offered") == "tomorrow")
        // Typed over, the button sends what the line holds now.
        model.type("tomorrow 9", in: "when")
        #expect(
            model.press("confirm", secrets: [:])
                == FormUsed(
                    step: "snooze", button: "confirm", values: ["when": "tomorrow 9", "offered": "tomorrow"]))
        // Refused: what was typed stays on the line to correct, the core
        // says none of its times is chosen, and the keyboard is back on
        // the line.
        let refused = model.show(time("tomorrow 9", chosen: "typed", error: "That is not a time."))
        #expect(refused == FormUpdate(opens: false, sameStep: true, focus: "when"))
        #expect(text(model, "when") == "tomorrow 9")
        #expect(chosen(model, "offered") == "typed")
    }

    @Test func theSummarySaysWhatIsOnScreenAndMarksASecret() {
        var model = FormModel()
        _ = model.show(microsoft(error: "Enter the application id.", focus: "application"))
        model.type("Work", in: "name")
        #expect(
            model.summary == [
                "sheet: up",
                "sheet step: server",
                #"sheet title: "Add an account: Microsoft""#,
                "sheet note: ",
                #"sheet error: "Enter the application id.""#,
                "sheet busy: ",
                "sheet field type [choice] = microsoft (imap|microsoft)",
                #"sheet field name [text] = "Work""#,
                #"sheet field address [static] = "robin@example.com""#,
                #"sheet field application [text] = "" (focus)"#,
                #"sheet field tenant [text] = """#,
                #"sheet buttons: cancel "Cancel" (esc) | back "Back" | continue "Continue" (return)"#,
            ])

        _ = model.show(password(busy: "Signing in to imap.example.com…"))
        #expect(
            model.summary == [
                "sheet: up",
                "sheet step: password",
                #"sheet title: "Add an account: sign in""#,
                "sheet note: ",
                "sheet error: ",
                #"sheet busy: "Signing in to imap.example.com…""#,
                "sheet field password [secret] = <secret> (focus)",
                #"sheet buttons: cancel "Cancel" (esc) | back "Back" (disabled) | continue "Continue" (return, disabled)"#,
            ])
        #expect(FormModel.secretMarker == "<secret>")
    }

    /// The question about a command's keys.
    private func keysForm(_ keys: String = "", error: String? = nil) -> FormShow {
        FormShow(
            step: "keys_change", title: "Keys for Move to a Folder", note: "It has: m.", error: error,
            fields: [
                FormField(id: "keys", label: "New Key:", kind: .keys(value: keys), focus: true),
                FormField(
                    id: "how", label: "Do:",
                    kind: .choice(
                        value: "replace",
                        options: [
                            FormOption(id: "replace", title: "Use it in place of the keys it has"),
                            FormOption(id: "add", title: "Add it to the keys it has"),
                        ])),
            ],
            buttons: [
                FormButton(id: "cancel", title: "Cancel", role: .cancel),
                FormButton(id: "confirm", title: "Save", role: .confirm, enabled: !keys.isEmpty),
            ])!
    }

    @Test func aKeyThatIsPressedIsToldToTheCoreAtOnceAndKept() {
        var model = FormModel()
        let update = model.show(keysForm())
        #expect(update.focus == "keys")
        #expect(model.rows[0].content == .keys(""))

        // Pressed: the core is told what the form holds, with no button.
        let used = model.record("ctrl-k", in: "keys")
        #expect(used == FormUsed(step: "keys_change", button: nil, values: ["keys": "ctrl-k", "how": "replace"]))
        #expect(model.rows[0].content == .keys("ctrl-k"))
        // The same key again says nothing new.
        #expect(model.record("ctrl-k", in: "keys") == nil)
        // The core's answer, with the key in it, moves nothing: the field
        // keeps the keyboard, for the second key of a pair.
        let again = model.show(keysForm("ctrl-k"))
        #expect(again.focus == nil)
        #expect(model.rows[0].content == .keys("ctrl-k"))
        #expect(model.record("g g", in: "keys")?.values["keys"] == "g g")
        #expect(model.summary.contains("sheet field keys [keys] = \"g g\" (focus)"))

        // Only a field that records keys takes them, and not more than a
        // field's keys come to.
        #expect(model.record("x", in: "how") == nil)
        #expect(model.record("x", in: "nothing") == nil)
        #expect(model.record(String(repeating: "x ", count: 100), in: "keys") == nil)
        // Save sends what was recorded with the rest.
        #expect(model.press("confirm", secrets: [:])?.values == ["keys": "g g", "how": "replace"])
    }
}
