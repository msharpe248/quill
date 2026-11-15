# Decisions

Choices that depart from the spec or fill a gap in it, each with its reason.
Newest milestone last. The spec is `Quill Terminal Email Client Spec.md`.

## M0, core skeleton

### Scope

- **Four crates exist so far**: `quill-core`, `quill-store`, `quill-tui`, `quill-cli`. The protocol, auth, MIME, host and media crates are created by the milestone that first needs them, so nothing in the tree is an empty stub.
- **The schema holds only what M0 reads and writes**: accounts, folders, messages, bodies, sync_state. Threads, attachments, operations, contacts and FTS5 arrive as later forward-only migrations.
- **The unified inbox is already in the sidebar**, though the spec lists it under M6. It is one query over inbox-role folders and the spec's own sketch of the UI shows it.
- **Only commands that work are registered.** Keys the spec reserves for later features (`c`, `r`, `e`, `d`, `/`, `:` and so on) stay unbound until those features land, so help never advertises a key that does nothing.
- **Mouse, bracketed paste and focus events are not switched on yet.** No milestone names them; they fit M3 (compose needs paste) and M6. Text selection with the terminal's own mouse handling works meanwhile.
- **Config is read once at startup.** Hot reload is not in any milestone's list; it is assumed to belong to M6 with the other P0 "configurable keys and themes" work.

### Types and traits

- **The header summary type is `Headers`, not `Envelope`.** The spec uses "envelope" both for the IMAP-style header summary and for the SMTP envelope that `Sender::send` takes. `Envelope` is left free for the latter in M3.
- **A `RemoteId` is unique within an account, not within a folder.** `Backend::fetch_raw` takes only an id, so the id must be enough to find the message. `Operation` therefore names the message by id alone.
- **`Sender` is not defined yet.** Nothing in M0 can exercise it; it arrives with SMTP in M3.
- **Timestamps are Unix seconds (`i64`) in the core and the store.** The UI formats them with `jiff` in the local zone. This keeps date arithmetic out of SQL and the domain types.

### Config and files

- **Config types live in `quill-core::config`.** The spec lists no config crate, and both the core and the UI read it.
- **File locations are resolved in `quill-cli`** (`--config-dir`, `--data-dir`, `QUILL_CONFIG_DIR`, `QUILL_DATA_DIR`, then `$HOME/Library/Application Support/Quill`). This moves behind `quill-host` in M1. In the sandbox `HOME` is the container, so the layout matches the spec's table.
- **Unknown keys in `config.toml` are errors.** A typo should be reported, not silently ignored.
- **A file that cannot be used never stops startup.** A bad `config.toml`, theme or `keys.toml` produces a notice in the status bar and the defaults are used, extending the spec's hot-reload rule ("a bad edit keeps the last good config") to the first load.
- **`keys.toml` maps a command name to a key or a list of keys**, replacing that command's preset bindings; an empty list unbinds. If any entry is wrong the whole file is ignored, so the keys are never half-applied.
- **A theme file may leave colours out**; they are taken from the terminal-palette theme. A file in `themes/` wins over a built-in of the same name, so the shipped themes can be customised by copying them.
- **The 256-colour fallback is computed** (nearest entry of the colour cube or grey ramp) rather than written twice in every theme.
- **The data directory is created with mode 0700.** It holds mail.
- **`quill tui --demo` uses an in-memory database** and writes nothing to disk.

### Store

- **Address lists are stored as JSON text columns.** They are always read and written whole with the row. The FTS5 participants index in M6 is built separately.
- **Body text lives in a `bodies` table for now.** The content-addressed blob cache and the FTS5 table that will hold body text belong to M2 and M6.
- **Flag changes in M0 are written straight to the store.** The operations queue is M3 work, and the fixture mailbox has no server to replay against.

### UI

- **Keys the spec does not list**: `q` and `ctrl-c` quit; `esc` goes back, closes help or dismisses a notice; `space`, `ctrl-d` and `pagedown` page down; `ctrl-u` and `pageup` page up; `shift-tab` cycles panes backwards; arrow keys, `home` and `end` mirror `j`, `k`, `g g` and `G`.
- **`q` is bound to both quit and back.** The earlier command in the registry wins where both apply, and quit does not apply in the help overlay, so `q` closes help and quits only from a pane.
- **The mutt preset** differs from vim in `N` (unread), `F` (star), `=` and `*` (top and bottom), `-` (page up) and `i` (back). The spec asks for the preset without defining it.
- **State is never shown by colour alone**, as the spec requires, by these means: the focused pane has a thick border; the cursor row starts with `>`; list rows carry `*` unread, `!` starred and `@` attachment markers; the unified list marks each row with its account's initial; the reader's border names the message's flags in words.
- **Pane titles carry the key that focuses them** (`[1] Accounts`), for discoverability.
- **The reader previews the message under the list cursor** without marking it read. `enter` opens it: that marks it read and moves focus to the reader.
- **Below 100 columns the sidebar is 18 columns wide; at 100 and above it is a sixth of the width, between 18 and 28.** The spec gives the breakpoints but not the widths.
- **The date column** shows the time for today, the weekday within the last week, day and month within the year, and month and year before that.
- **Escape sequences and control characters are stripped already**, although the spec lists that under M4, because M0 draws message text. Headers are cleaned when rows load and bodies when they load.
- **The demo's body extraction is a split at the first blank line.** `quill-mime` replaces it in M2. The fixture serves only unencoded UTF-8 plain text.
- **Only the "disambiguate escape codes" level of the Kitty keyboard protocol is requested.** It is enough to tell `ctrl-i` from `tab` and to receive `cmd`. Reporting key releases or alternate keys is left until compose needs it.

### Tooling

- **Licences allowed for dependencies**: MIT, Apache-2.0, Unicode-3.0, Zlib. Every current dependency offers one of them.
- **Quill's own crates have no licence field yet.** That is the spec's open question about open-sourcing; `cargo deny` is told to skip private crates.
- **CI installs `cargo-deny` with Homebrew.** The official action is a Linux container and the spec asks for a macOS runner.

## M1, shell spike

**Accepted on 2026-11-14 with one item deferred.** Everything in M1's acceptance test has been shown on the sandboxed, ad-hoc-signed app except "the archive passes App Store validation and installs through TestFlight". The owner chose to put off getting an Apple Developer account until the very end, so that item moves to the App Store submission work in M6, and later milestones go ahead without it.

What this leaves open, knowingly:

- **Whether a sandboxed libghostty app passes App Store validation is still unproven.** The spec put M1 first to retire that risk early. The sandbox half is retired (the app runs sandboxed here); the validation half is not. The spec's fallback, a notarized direct download from the same code, also needs a developer account.
- **There is no stable signing identity until then.** An ad-hoc signature changes with every build, and macOS ties Keychain items to the signature. M2's Keychain storage has to work within that during development; see M2.
- **The Xcode project and archive are deferred with it.** The bundle stays assembled by `app/build-app.sh`.

### Terminal

- **libghostty comes from `libghostty-spm` 2.2.2026111302**, pinned exactly. It is the spec's first candidate, MIT, and ships libghostty as a prebuilt XCFramework built from Ghostty commit `cf5519b22f91`. The binary is a third party's build, held to a checksum in that package's manifest. Building the XCFramework from Ghostty source (the spec's alternative) needs Zig and is the fallback if that trust is not acceptable for a shipping build.
- **The app owns the PTY; libghostty only draws and handles input.** libghostty can start a process itself, but on macOS it always goes through `/usr/bin/login`, which the App Sandbox does not permit. `TerminalKit` spawns the program on its own PTY and joins the two with libghostty's host-managed backend. This also gives the shell the program's exit status and full say over its environment.
- **The shell sets the terminal's modes and size, not the program in it.** Inside the App Sandbox a process may read its terminal's settings but every "set" request on the program's side of the PTY is refused (`tcsetattr` fails, and the size `openpty` applies is silently dropped), and the program is never told about a resize. The side that owns the PTY is allowed all three, so `TerminalKit` puts the PTY in raw mode, sets its size, and sends `SIGWINCH` itself on every resize.
- **The core accepts a terminal that is already raw.** When it cannot switch raw mode on, it checks whether its terminal is raw already and carries on if so. In that case it also turns on the Kitty keyboard protocol without asking the terminal first, because asking needs raw mode it set itself; the host that made the terminal raw is Quill's own shell, which has the protocol.
- **Risk for M9 (bundled Vim):** Vim will meet the same sandbox rule. Starting it on a raw PTY should work the same way, but Vim changes terminal modes during a session and that needs proving before M9 is promised.
- **`TERM` is `xterm-256color` for the core.** The core does not use terminfo. `xterm-ghostty` and the bundled terminfo matter once another program (Vim) runs in a pane.

### Bridge

- **Every message so far is a JSON-RPC notification.** None of the M1 messages needs a reply. Requests with ids arrive with the first message that does (`files.pick`, `thumbnail.make`).
- **Both ends are strict**: an unknown method, a missing field or an extra field is an error, surfaced as a notice. The two ends ship in one bundle, so a mismatch is a bug, not a version to tolerate.
- **`schema/examples` is the shared contract.** The Rust tests validate every example against `schema/bridge.json` and round-trip it through the Rust types. The Swift tests round-trip the same files through the Swift types. Swift has no JSON Schema validator to hand, so the schema itself is checked on the Rust side only.
- **The socket lives at `$TMPDIR/q-<pid>/s`**, in a directory only the user can open. The name is short because a socket path has barely a hundred bytes and the sandbox container's path uses most of them.
- **An unreachable shell is not fatal.** The core runs standalone and says why in the status bar.
- **`update` returns `Effects`**: store requests and host requests. The run loop performs them; `update` still touches nothing itself.

### Shell

- **Only `cmd` chords become menu shortcuts.** Any other key belongs to the terminal: a menu that claimed `j` would take it from the core. Those keys are shown beside the item as a badge (macOS 14 and later; in the title on macOS 13).
- **Quit belongs to the shell.** `app.quit` has no menu item. Quit Quill (cmd-Q) and closing the window send `app.will_quit`, wait briefly for the core to exit, then hang up on it.
- **`debug.preview_sample` (`P`) is temporary.** It writes a one-page PDF and asks the host to preview it, so the Quick Look pane can be exercised before messages have attachments. It goes when M4 lands.
- **The app opens the demo mailbox** until accounts exist (M2); `--no-demo` opens the real, empty one.
- **The bundle is assembled by `app/build-app.sh`** from `swift build` output and signed ad hoc with the sandbox on. An Xcode project, archive and App Store validation wait on two things this machine lacks: Xcode's first-launch components (`xcodebuild -runFirstLaunch`) and a signing identity.
- **The bundle identifier `dev.quill.Quill` is a placeholder** until there is a developer account for it to belong to.
- **Shell tests use Swift Testing through `swift test`**, not XCTest through `xcodebuild test` as the spec says. `xcodebuild` cannot run here until its first-launch components are installed. Revisit then.
- **The app can be driven from the command line** (`--script`, `--dump-screen`; see `DebugScript.swift`). A check can then use the real window, menus and terminal without a person or screen-recording permission, and gets the terminal's text, the shell's state and a picture of the window.
- **`swift-format` is configured** for four-space indentation and 120-column lines (`app/.swift-format`).

### Images

- **An image is not part of the frame.** Views leave the cells under an image empty and return where it goes; the run loop then asks the terminal to draw it. This keeps `view` a pure function of the state and is the mechanism message images will use in M4.
- **Pixels are sent once, then placed.** The terminal keeps the image under an id, so showing it again after a scroll, resize or overlay is a few bytes, not the whole image.
- **An image is taken down while anything overlaps it.** The terminal draws images above text, so one left under the help overlay or the sidebar overlay would show through. It is placed again when the overlay goes.
- **Every graphics command asks for no reply** (`q=2`). A reply arrives on stdin and would be read as typing.
- **Whether the terminal draws images is decided without asking it, for now.** Quill's own shell always does. Standalone, the environment decides (`TERM` naming kitty or ghostty, `TERM_PROGRAM=ghostty`, `KITTY_WINDOW_ID`). The spec's startup query belongs with message rendering in M4; until then a terminal that does support images but does not say so gets the one-line placeholder.
- **Cells are assumed to be twice as tall as wide** when sizing an image. M4 reads the real cell size.
- **`debug.image_sample` (`I`) is temporary**, like `debug.preview_sample`: it shows a generated picture in the reader so the Kitty graphics path can be proved before messages have images.

### Entitlements

| Entitlement | Why |
|---|---|
| `com.apple.security.app-sandbox` | Required for the Mac App Store. |
| `com.apple.security.network.client` | Mail servers and OAuth endpoints. Outgoing only. |
| `com.apple.security.files.user-selected.read-write` | Attaching and saving files through open and save panels. |
| Helper: `app-sandbox` + `inherit` | The core runs inside the app's own sandbox. A helper may hold only these two. |

The Keychain needs no entitlement for the app's own items; an access group shared with the core is decided in M2, when there is a team identifier to name it with.

## M2, IMAP read (in progress)

- **Mail is parsed with `mail-parser` 1.0.1**, the spec's candidate. It needs Rust 1.98, so the workspace's minimum Rust version rises from 1.89 to 1.98.
- **One function reads every message**: `quill_mime::parse` takes raw RFC 5322 bytes from any backend and returns headers, a plain-text body and the attachment list.
- **An HTML-only message is shown through `mail-parser`'s own HTML-to-text conversion** for now. The styled conversion and sanitizer (`html2text`, `ammonia`) are M4.
- **A message's date falls back to when it was received** if the Date header is missing, unreadable, before 1970 or after 2200. A forged or broken date must not send a message to the far end of the list.
- **A forwarded message (`message/rfc822`) is an attachment**, not part of the body.
- **The demo mailbox's bodies now go through the parser**, replacing the split at the first blank line.
