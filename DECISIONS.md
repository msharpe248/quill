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

### IMAP backend

- **`async-imap` 0.12 is the IMAP library**, over `io-imap`. Both are maintained; `async-imap` is what Delta Chat runs against a wide range of real servers, and that field experience is worth more here than a newer design.
- **TLS is `rustls` with the `ring` provider, verified by the system trust store** through `rustls-platform-verifier`. On macOS that means Apple's own rules apply, including ones stricter than the standard: a server certificate must carry the server-auth usage and be valid for at most 398 days.
- **STARTTLS is all or nothing, and the server's advertisement is not consulted.** The client asks for the upgrade and fails if it is not granted. Whether STARTTLS appeared in the capability list is beside the point, since that list arrives in the clear. Nothing learned before the upgrade is kept.
- **A pinned fingerprint names one exact certificate and is checked alone.** Its name, dates and issuer are not, because the user chose that certificate; the handshake signature still is. A pin beats the trust store: a different certificate is refused even if a trusted authority signed it.
- **A refused certificate is carried in the error** (the certificate, its SHA-256 and the verifier's reason), so the UI can show it and offer reject, trust once or pin. That prompt itself is still to build.
- **A folder syncs in batches of 500, newest first**, and `SyncDelta` gained `has_more`. A large mailbox is readable after the first batch instead of after the last.
- **`SyncDelta` gained `reset`** for a changed UIDVALIDITY: the store drops what it held for the folder and takes the delta as the whole truth.
- **The cursor is the UIDVALIDITY plus the set of UIDs already delivered**, written as ranges. UIDs are mostly contiguous, so fifty thousand of them are a few dozen bytes. Each sync compares that set with the server's (`UID SEARCH ALL`) to find arrivals and removals, then refreshes the flags of everything already held.
- **CONDSTORE and QRESYNC are not used yet.** The comparison above is correct on every server; the extensions only make it cheaper. They are an optimisation to add once sync is measured on a large mailbox.
- **A message's `RemoteId` is `uid;uidvalidity;folder`.** The id alone finds the message, as `fetch_raw` requires, and an id from before a renumbering is refused rather than misread.
- **List headers are fetched as `BODY.PEEK[HEADER.FIELDS (...)]` and read by `quill-mime`**, not taken from IMAP's `ENVELOPE`. One parser then handles every encoding, for every backend.
- **Flags are added and removed by name** (`+FLAGS`/`-FLAGS`), never replaced wholesale, so keywords Quill does not track survive.
- **Reading a message uses `BODY.PEEK[]`**, so downloading it does not mark it read on the server. Marking read is an explicit flag change.
- **A folder's role comes from SPECIAL-USE, then from common names** (`Sent Items`, `Deleted Items`, `Spam` and so on) for top-level folders on servers without it. Overrides in config are still to add.
- **Mailbox names are decoded from modified UTF-7 for display** with a small decoder here, and kept encoded as the folder id.
- **One connection per account, reopened after any error.** IDLE, and with it a second connection, arrives in M6.
- **Moving, copying and deleting return "not supported"** until the operation queue in M3.
- **Client certificates (mutual TLS) and TLS 1.0/1.1 are not supported.** The spec lists the first as a per-account extra; it has no milestone yet.

### Testing IMAP

- **Dovecot from Homebrew stands in for the spec's Docker mail server.** Docker is not installed on the development machine. Each test starts its own Dovecot, unprivileged, in a temporary directory, and stops it afterwards; nothing is left running and nothing is installed as a service.
- **The tests terminate TLS themselves, in front of Dovecot's plaintext port.** Run unprivileged on macOS, Dovecot's own TLS relay drops every session straight after sign-in. Owning the TLS end also lets a test choose the certificate a client is shown: expired, for another host, unknown authority, or a server that refuses STARTTLS.
- **Dovecot's memory limit is set to the largest value macOS accepts** in the test configuration. macOS rejects every ordinary limit, and Dovecot's "unlimited" is a larger number than macOS allows.
- **Without Dovecot the IMAP tests skip with a message**; CI sets `QUILL_REQUIRE_DOVECOT` so that there they fail instead.

### Dependencies

- **Licences added to the allow-list**: ISC and BSD-3-Clause (the TLS stack: `ring`, `rustls-webpki`, `subtle`) and CDLA-Permissive-2.0 (the bundled certificate-authority list that the platform verifier carries for other platforms). All are permissive.
- **`RUSTSEC-2026-0441` is ignored**: `pin-utils`, a two-macro crate that `async-imap` uses, was archived because the standard library replaced it. It is unmaintained, not vulnerable.

### Sync engine

- **The sync engine is its own crate, `quill-sync`**, not part of `quill-core` as the spec's layout has it. The engine needs the store and the parser, and both of those depend on `quill-core`; putting it in the core would turn that dependency round or hide it behind traits with one implementation each.
- **The engine has its own database connection.** The UI's store worker has another. SQLite's WAL mode lets both work at once, and a ten-second busy timeout makes a writer wait for the other instead of failing.
- **Each account syncs in its own task; requests from the UI run in theirs.** A body the user opens during a long first sync waits for one batch, not for the whole mailbox.
- **A pass that finds nothing new emits no change event**, so an idle mailbox redraws nothing. `Store::apply_delta` now reports whether anything really changed.
- **The same failure is reported once**, not at every interval, and again only after a pass has succeeded in between.
- **Bodies of the last 30 days are fetched ahead**, at most 2,000 a pass, as the spec's IMAP notes ask. Older bodies are fetched when opened.
- **Raw messages are filed by SHA-256 under `data/blobs/`**, written under a temporary name and renamed, with their hash recorded on the message (migration 0002). The cache budget and eviction are still to build.
- **A flag change is written to the store first, then sent to the server; if the server refuses, the store is put back** and the user is told. This is the spec's data flow without the durable queue: a change made offline is rolled back rather than queued until M3 adds the operations table.
- **Folders that disappear from the server are removed locally**, with their mail.

### Accounts and secrets

- **The Keychain is reached through `security-framework`**, the smaller of the spec's two candidates. `quill-auth` puts it behind a `SecretStore` trait, so tests use an in-memory store and another platform can supply its own.
- **A password is filed under the service `Quill` as `imap:<username>@<host>`**, so two accounts never share one by accident, and changing an account's display name does not orphan its password.
- **Until the setup flow exists, an account is added by hand**: a `[[account]]` in `config.toml` and `quill account password NAME`, which reads the password without echo (or from a pipe) and never takes it as an argument, where it would show in the process list and shell history.
- **`imap = { ... }` gained `username` and `fingerprint`.** The spec's example has neither; a sign-in name that differs from the address and a pinned certificate both need somewhere to go.
- **An account the config no longer names is removed from the store**, with its cached mail. Never when the config failed to load: an unreadable file must not be taken for "every account was removed".
- **A plaintext account raises a warning at every start.** The spec asks for a permanent warning; a notice at each launch is what exists so far, and a standing marker on the account is still to add.
- **An account Quill cannot use yet (POP3, Graph, EWS, or IMAP with no server set) is a notice**, never a failure to start.
- **The UI's store worker retries when the database is momentarily busy**, since the sync engine writes through its own connection.
