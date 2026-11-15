# Quill

A keyboard-first mail client for macOS: a native app whose main surface is a terminal it creates itself. A Swift shell supplies the window, menus and media viewers; a Rust core draws the mail UI inside an embedded terminal and does all the mail work.

The full design is in [the spec](Quill%20Terminal%20Email%20Client%20Spec.md). Choices that depart from it or fill a gap are in [DECISIONS.md](DECISIONS.md).

## Status

Milestone 0, the core skeleton, is in place: the Rust workspace, the local store with migrations, config loading, themes, keymaps, and a three-pane terminal UI over a demo mailbox.

Milestone 1, the app shell, is in place too. `Quill.app` runs the core in its own libghostty terminal inside the App Sandbox, builds its menus and toolbar from the core's command list, shows files in a Quick Look pane beside the terminal, and draws images inline with the Kitty graphics protocol. App Store validation is deferred until there is a developer account to sign with.

Milestone 2, reading mail over IMAP, is under way. An IMAP account in `config.toml` syncs into the local store and reads offline; see "Adding an IMAP account" below. HTML mail is drawn as styled text: headings, lists, emphasis, quotes and simple tables, with links numbered and listed at the end of the message. `t` switches a message between its HTML and its plain text when it came as both. `o` lists a message's links to open one in your browser, and in the app or a terminal that supports hyperlinks, cmd-click on a link's text opens it too. Quill opens only `http`, `https` and `mailto` links. Still to come in M2: setting an account up from inside the app, and trusting a self-signed server from a prompt. Sending arrives in M3, and Microsoft accounts with OAuth in M5.

## Try it

You need a stable Rust toolchain (1.98 or later; `rust-toolchain.toml` selects it). Standalone use is meant for Ghostty or Kitty, whose keyboard protocol Quill uses when it is offered; other terminals work with fewer key combinations.

```sh
cargo run --release -- tui --demo
```

`--demo` opens an invented mailbox held in memory. It reads your config, theme and keys if you have them, and writes nothing.

Without `--demo`, `quill tui` opens (and creates) the real database, which is empty until account setup lands.

## The app

```sh
app/build-app.sh
open target/app/Quill.app
```

The script builds the Rust core and the Swift shell, assembles `target/app/Quill.app`, and signs it ad hoc with the App Sandbox on. Until accounts exist the app opens the demo mailbox.

Around the terminal the app adds a menu bar and toolbar built from the core's commands, the mailbox name and unread count in the title bar, the unread count on the Dock icon, and a second pane for previews. Press `P` (or the eye in the toolbar) to preview a sample file; `esc` closes the pane, and `cmd-]` moves focus between the panes. Press `I` to show a sample image in the reader.

`SANDBOX=0 app/build-app.sh` signs without the sandbox, to tell a sandbox denial from an ordinary bug.

## Adding an IMAP account

Until account setup is built into the app, an account is a few lines in `config.toml` and a password in the Keychain:

```toml
[[account]]
name = "Personal"
type = "imap"
address = "you@example.com"
imap = { host = "imap.example.com", port = 993, tls = "implicit" }
```

```sh
quill account password Personal   # asks for the password, saves it in the Keychain
quill tui
```

`tls` is `"implicit"` (the default, port 993), `"starttls"` (port 143) or `"none"`. With `"starttls"`, a server that will not upgrade is refused, never used in the clear. `"none"` sends your password and mail unencrypted and says so every time Quill starts.

Optional keys inside `imap = { ... }`: `username`, when you do not sign in with your address, and `fingerprint`, the SHA-256 of a self-signed server's certificate, to accept that one certificate.

`quill account list` shows each account and whether it has a saved password. `quill account forget NAME` removes one. Passwords are only ever in the Keychain: not in the config, the database or the logs.

Microsoft accounts (Microsoft 365, Outlook.com) do not accept a password over IMAP. They need the OAuth sign-in that arrives in M5.

## Keys

Press `?` at any time for the keys that apply where you are. The bottom bar shows the most useful ones.

| Key | Action | Key | Action |
|---|---|---|---|
| `j` / `k` | Next, previous | `1` `2` `3` | Focus accounts, list, reader |
| `g g` / `G` | Top, bottom | `tab` / `shift-tab` | Next, previous pane |
| `space` / `ctrl-u` | Page down, up | `z` | Zoom the focused pane |
| `enter` | Open | `u` | Mark read or unread |
| `esc` | Back, close, dismiss | `s` | Star or unstar |
| `?` | Help for this context | `q` | Quit |
| `t` | Show as HTML or plain text | `o` | Open a link |
| `P` | Preview a sample file | `I` | Show a sample image |

Arrow keys, `home`, `end`, `pageup` and `pagedown` work too. `ctrl-d` also pages down and `ctrl-c` also quits.

Set `keymap = "mutt"` in the config for mutt-flavoured keys.

In the message list, `>` marks the cursor, `*` an unread message, `!` a starred one and `@` one with attachments.

## Configuration

Quill looks in `~/Library/Application Support/Quill/config/`:

| File | Purpose |
|---|---|
| `config.toml` | Settings. See the example in the spec. |
| `keys.toml` | Key overrides, on top of the chosen preset. |
| `themes/NAME.toml` | Your own themes, selected with `theme = "NAME"`. |

Use `--config-dir` and `--data-dir`, or the `QUILL_CONFIG_DIR` and `QUILL_DATA_DIR` environment variables, to point elsewhere.

A file Quill cannot use never stops it from starting. The problem is shown in the bottom bar and the defaults are used instead.

### Keys

`keys.toml` maps a command name to a key or a list of keys. The names are the ones the help overlay's commands go by:

```toml
"nav.next" = ["j", "down", "ctrl-n"]
"view.zoom" = "Z"
"message.toggle_star" = []   # unbind
```

Keys are written as the help overlay shows them: `j`, `G`, `enter`, `space`, `ctrl-k`, `shift-tab`, `cmd-enter`, and `g g` for two keys in a row.

| Command | Does |
|---|---|
| `nav.next`, `nav.prev` | Move down or up; scroll in the reader |
| `nav.top`, `nav.bottom` | Jump to the first or last |
| `nav.page_down`, `nav.page_up` | Move by a screenful |
| `nav.open` | Open the message or mailbox |
| `nav.back` | Go back, close help, dismiss a notice |
| `focus.sidebar`, `focus.list`, `focus.reader` | Focus a pane |
| `focus.next`, `focus.prev` | Cycle the panes |
| `view.zoom` | Zoom the focused pane |
| `message.toggle_unread`, `message.toggle_star` | Change a message's flags |
| `message.toggle_html` | Switch a message between its HTML and its plain text |
| `message.open_link` | List the message's links, to open one in the browser |
| `debug.preview_sample` | Preview a sample file (temporary) |
| `debug.image_sample` | Show a sample image in the reader (temporary) |
| `help.toggle` | Show or hide help |
| `app.quit` | Quit |

### Themes

Three themes ship: `dark` (the default), `light`, and `terminal`, which uses only your terminal's own sixteen colours. To make your own, copy [`dark.toml`](crates/quill-tui/themes/dark.toml) into `themes/` under a new name and edit it. Colours you leave out come from the `terminal` theme.

Hex colours fall back to the nearest of 256 colours when the terminal does not report true colour. Setting `NO_COLOR` turns colour off; the cursor, focus and message state stay visible through markers, borders and text attributes.

## Development

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

The app can be driven without a person, for checks that need the real window: `Quill --script "wait:2;menu:message.toggle_star;press:cmd-j" --dump-screen out.txt` performs the steps, then writes the terminal's text, the shell's state and a picture of the window (`out.txt.png`). Inside the sandbox the file must be in the app's container. `app/Sources/QuillApp/DebugScript.swift` lists the steps.

The UI is covered by snapshot tests of whole screens at 80x24 and 200x60 (`crates/quill-tui/tests/snapshots.rs`). When a change to the UI is intended, regenerate them with `INSTA_UPDATE=always cargo test -p quill-tui` or review them with `cargo insta review`, and read the diff before committing.

Layout of the workspace:

| Crate | Holds |
|---|---|
| `quill-core` | Domain types, the `Backend` trait, config, the fixture backend |
| `quill-store` | The SQLite store: schema, migrations, queries, raw message blobs |
| `quill-tui` | The terminal UI: state, views, keymap, themes, command registry |
| `quill-host` | What hosts the core: the app shell over the bridge, or nothing |
| `quill-media` | Images: the Kitty graphics protocol |
| `quill-mime` | Reading mail: RFC 5322 and MIME parsing, HTML as styled text |
| `quill-imap` | The IMAP backend |
| `quill-sync` | The sync engine: runs each account's backend into the store |
| `quill-auth` | Secrets: the macOS Keychain |
| `quill-cli` | The `quill` binary |
| `app/Sources/TerminalKit` | The terminal surface. The only Swift module that imports libghostty |
| `app/Sources/Bridge` | The shell's end of the bridge: message types and the socket server |
| `app/Sources/QuillApp` | Windows, menus, toolbar and panes |

Known limits: servers that only speak TLS 1.0 or 1.1 will not be supported in version 1.
