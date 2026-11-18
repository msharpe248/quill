# Building and working on Quill

For what Quill is, see [the README](../README.md); for how it is used, [the manual](manual.md). The design is in [the spec](../Quill%20Terminal%20Email%20Client%20Spec.md), and the choices made since are in [DECISIONS.md](../DECISIONS.md).

## What you need

- A Mac with macOS 13 or later.
- A stable Rust toolchain, 1.98 or later (`rust-toolchain.toml` selects it).
- Xcode's command line tools, for the Swift shell. `swift build` is enough: no Xcode project is involved.
- For the tests: Dovecot (`brew install dovecot`) and `cargo-deny`.

## The terminal program

```sh
cargo build --release          # target/release/quill
cargo run --release -- tui --demo
```

Standalone use is meant for Ghostty or Kitty, whose keyboard protocol Quill uses when it is offered; other terminals work with fewer key combinations.

## The app

```sh
app/build-app.sh
open target/app/Quill.app
```

The script builds the Rust core and the Swift shell, assembles `target/app/Quill.app`, and signs it ad hoc with the hardened runtime. `QUILL_APP_OUT` names another place to put it.

**Signing.** An ad hoc build is a new program to the Keychain every time it is built, so macOS asks about each saved password after every build. Set `QUILL_SIGN_IDENTITY` to a code-signing identity of your own and every build is the same program; the manual says how, under "Adding an account". Quill's scripts never make an identity.

**Signing in to Google as Quill.** Quill's registration with Google is not in the source. `app/build-app.sh` looks for `google-client.json` at the top of the repository (the file Google's console gives for a "Desktop app" OAuth client; git ignores it), or takes `QUILL_GOOGLE_CLIENT_ID` and `QUILL_GOOGLE_CLIENT_SECRET` from the environment, and says which it found. With neither, the build works and the form asks whoever adds a Gmail account for a client of their own. [oauth.md](oauth.md) has the whole of it.

**A build to give out.** `app/release.sh` makes a release build and packs it as a disk image to drag Quill out of into Applications (`target/dist/Quill-VERSION-ARCH.dmg`; the picture behind the icons is `app/dmg/background.png`, and where the icons go is in `app/dmg/layout.py`) and as a zip, with the checksum of each for a Homebrew cask. Signed ad hoc it has to be allowed by hand on anyone else's Mac (System Settings, Privacy & Security); with `QUILL_SIGN_IDENTITY` set to a Developer ID and `QUILL_NOTARY_PROFILE` to a saved notary profile it is signed and notarized, and simply opens.

## Checks

A change is not done until these pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check          # needs cargo-deny

cd app
swift test
swift format lint --configuration .swift-format -r Sources Tests Package.swift
```

The IMAP backend's tests run against a real server: install Dovecot (`brew install dovecot`) and they start their own copy, unprivileged, in a temporary directory. Without Dovecot those tests skip, saying so.

The shell and the core talk over a bridge defined in `schema/bridge.json`. The example messages in `schema/examples` are tested from both sides; change the schema, both sets of types and the examples together.

A scripted run that installs the command line tool (`menu:app.install_command_line`) must say where the link goes, with `--core-env QUILL_LINK_DIR=...`: without it the link is made in the real `/usr/local/bin`, `/opt/homebrew/bin` or `~/.local/bin`.

The app can be driven without a person, for checks that need the real window: `Quill --script "wait:2;menu:message.toggle_star;press:cmd-j" --dump-screen out.txt` performs the steps, then writes the terminal's text, the shell's state and a picture of the window (`out.txt.png`). A run like this opens your own mailbox unless it is told otherwise: give it `--demo`, or scratch folders with `--core-env QUILL_CONFIG_DIR=...` and `--core-env QUILL_DATA_DIR=...`, and a home folder of its own (`--core-env HOME=...`, and `HOME` set for the app itself) if it is not to run your editor on your own settings. `app/Sources/QuillApp/DebugScript.swift` lists the steps. A sheet on the window is in the dump too, whichever question it is asking (its step, what it says, and each field with what it holds; a password, and a code to sign in with, as `<secret>`), is drawn into the picture where it lies and saved by itself as `out.txt.sheet.png`; `sheetset:FIELD=VALUE` and `sheetpress:BUTTON` work it. `snap:NAME` writes the dump as it stands in the middle of a script (`out.txt.NAME`, and its picture), so one launch can check several states; the dump says whether the web reading pane is on and what it shows, and gives the widths to check its place by: the terminal's columns, the columns the window has room for, where the core says the pane goes, where the divider was left, and the width of each pane drawn in the terminal. `divider:COLUMNS` drags the divider and `divider:reset` double-clicks it. `notice:COMMAND` clicks a button of the bar above the page in the reading pane, by its command. `clicktext:TEXT` clicks the terminal where that text first is on its screen (`clicktext:2,TEXT` for the second place). `clickeditor:` clicks the editor that has the text of the message being written, `typeeditor:TEXT` types into it, and the dump says where the core put it, the cells it came to and which terminal has the keys. A scripted run starts at the same window size every time and keeps a dragged divider for the run only; it neither reads nor writes your saved window or divider. The settings window is worked with `settings:open`, `settingspane:PANE`, `settingsset:PANE.FIELD=VALUE`, `settingsstep:PANE.FIELD=up`, `settingspick:PANE.FIELD=TITLE`, `settingspress:PANE.FIELD.ACTION`, `settingskey:KEY` and `settings:close`; the dump says what it shows and what the core was told, its picture is `out.txt.settings.png`, and the pane it was last on is kept for the run only. The window of keys comes up with `menu:help.keys`; `keysearch:TEXT` types in its search field and `keys:close` closes it, the dump has every row it shows, and its picture is `out.txt.keys.png`.

The UI is covered by snapshot tests of whole screens at 80x24 and 200x60 (`crates/quill-tui/tests/snapshots.rs`). When a change to the UI is intended, regenerate them with `INSTA_UPDATE=always cargo test -p quill-tui` or review them with `cargo insta review`, and read the diff before committing.

## Layout

| Crate | Holds |
|---|---|
| `quill-core` | Domain types, the `Backend` trait, config, search, the fixture backend |
| `quill-store` | The SQLite store: schema, migrations, queries, the queue of changes, raw message blobs |
| `quill-mime` | Reading mail: RFC 5322 and MIME parsing, HTML as styled text |
| `quill-auth` | Secrets: the macOS Keychain, and signing in with a provider (OAuth) |
| `quill-net` | Connections: TLS, and the certificates a user has chosen to trust |
| `quill-imap`, `quill-pop3`, `quill-smtp` | Mail servers: reading over IMAP and POP3, sending over SMTP |
| `quill-graph`, `quill-ews` | Microsoft 365 (Graph) and Exchange on your own servers (EWS) |
| `quill-sync` | The sync engine: runs each account's backend into the store, and the rules for arriving mail |
| `quill-media` | Images: the Kitty graphics protocol |
| `quill-host` | What hosts the core: the app shell over the bridge, or nothing |
| `quill-tui` | The terminal UI: state, views, keymap, themes, command registry |
| `quill-cli` | The `quill` binary: the UI, the commands for a shell, and the service that changes accounts and settings |

| In `app/Sources` | Holds |
|---|---|
| `TerminalKit` | The terminal surface. The only Swift module that imports libghostty |
| `Bridge` | The shell's end of the bridge: message types and the socket server |
| `QuillApp` | Windows, menus, toolbar |
| `Panes`, `Editor`, `Forms` | The reading and viewing panes, starting your editor, the sheets and the settings window as the core describes them |
| `Themes`, `Spelling`, `Notifications` | The theme chooser, the Mac's spell checker, notifications |

The shell holds no mail logic: it knows commands by name, and everything about mail is the core's. `CLAUDE.md` lists the conventions the code keeps to.

## Reporting a problem

Open an issue at <https://github.com/msharpe248/quill/issues>. The README says what is worth putting in it. Never paste a password, a token or mail you would not publish.
