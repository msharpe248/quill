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

## M2, IMAP read

Built and accepted on 2026-11-15 by its two tests: the 50,000-message run below, and the same mail showing with the server stopped. Not verified, because they need the owner: a real provider's account from inside `Quill.app` (tried only against a local Dovecot, in the sandbox and out), and the provider table's entries, which are from memory and are checked by connecting at setup, not in advance.

Left for later milestones: CONDSTORE and QRESYNC (a folder is re-listed by UID each pass), IDLE (M6), a custom CA bundle and a client certificate in config, folder-role overrides in config, and the Autodiscover part of server discovery, which needs the HTTP client that arrives with M5.

- **Mail is parsed with `mail-parser` 1.0.1**, the spec's candidate. It needs Rust 1.98, so the workspace's minimum Rust version rises from 1.89 to 1.98.
- **One function reads every message**: `quill_mime::parse` takes raw RFC 5322 bytes from any backend and returns headers, a plain-text body, the HTML when it is worth showing, and the attachment list.
- **HTML mail is drawn as styled text now, not in M4.** The owner tried a real account on 2026-11-15, found that nearly all of it was HTML, and asked for it to be rendered as well as possible. So the "HTML to text" part of M4 is built early; the web view pane, sanitizer, remote-content rules and inline images stay in M4. See "HTML" below.
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
- **A folder's role comes from SPECIAL-USE, then from common names** (`Sent Items`, `Deleted Items`, `Spam` and so on) for top-level folders on servers without it. A role the server declares for one folder is never also guessed for another, and only the first folder with a telling name gets a guessed role: a `Junk` beside the server's declared junk folder is an ordinary folder. Overrides in config are still to add.
- **A server that keeps every folder inside the inbox loses that prefix in the names shown.** Courier-style servers list `INBOX.Sent`, `INBOX.Trash`; when every folder but the inbox starts with `INBOX` and the delimiter, the sidebar shows `Sent`, `Trash`. This is worked out from the LIST response instead of the NAMESPACE command, which `async-imap` 0.12 has no call for. The folder id sent to the server is unchanged.
- **Levels of a folder path are shown with `/`** whatever the server's delimiter, and the inbox is shown as `Inbox`.
- **Mailbox names are decoded from modified UTF-7 for display** with a small decoder here, and kept encoded as the folder id.
- **One connection per account, reopened after any error.** IDLE, and with it a second connection, arrives in M6.
- **Moving, copying and deleting return "not supported"** until the operation queue in M3.
- **Client certificates (mutual TLS) and TLS 1.0/1.1 are not supported.** The spec lists the first as a per-account extra; it has no milestone yet.

### HTML

- **HTML becomes styled text with `html2text` 0.17.4** (MIT), the spec's candidate, with its `css` feature so that `display: none` is honoured: marketing mail hides a preview line that way. Colours in the mail are ignored; they assume a white page.
- **The reader shows the HTML when a message has it**, and the plain part only when the HTML renders to nothing or to less than a quarter of the plain part's text. Senders write the HTML; the plain alternative is generated, and on the owner's mail it was often a stub ("your mail program cannot show this") or raw tracking links. Of 1,172 real messages, 1,159 are shown from their HTML. `Parsed::body` stays the plain text, for search and quoting.
- **The HTML is read from the raw message each time it is opened**, not stored a second time in the database. The store hands the UI the raw message (`Store::raw`), the store worker parses it, and the reader lays it out for the pane's width, again on every resize. On the owner's mail the slowest layout took 6 ms, so it runs in `update`; HTML over 2 MB is not rendered and the plain text is shown.
- **Blobs can live in memory** (`BlobStore::in_memory`), so the demo mailbox keeps raw messages like a real one and still writes nothing. `Store::open_in_memory` has them by default; a file store reads blobs only when told where they are (`with_blobs`).
- **Links are numbered after their text and listed at the end**, each target once. A link with nothing to read (around an image with no description, say) gets no number, and anchors and `javascript:` links are never numbered. The list is drawn dim and clipped to the pane, one line per link.
- **`t` switches a message between its HTML and its plain text** (`message.toggle_html`), at the owner's request. It applies only when the message came as both, the reader's bottom border then says which is showing, and the choice lasts while that message is open. `t` is not in the spec's key table and was free.
- **`o` opens a list of the message's links** (`message.open_link`): move or type a number, `enter` opens the link in the default browser. The list shows each address in full before anything is opened, which is the spec's "show the real target" rule applied to every link. The spec gives `o` to "open link or attachment"; attachments join it in M4. In plain text the addresses written out in the text are the links.
- **Only `http`, `https` and `mailto` addresses in printable ASCII are ever opened** (`quill_core::links::is_openable`). The core checks before asking, the bridge type cannot hold anything else, and the shell checks again before opening, so a message cannot open a file or start a program by naming another scheme. Other links are listed, dimmed, and opening one says why not.
- **The shell opens a link with `NSWorkspace.open`**, on a new bridge message `link.open`. That is the sandbox's own way to reach the default browser and needs no entitlement. Standalone, the core runs `/usr/bin/open` with the address as its one argument.
- **Link text is cmd-clickable through terminal hyperlinks (OSC 8).** A cell cannot hold a hyperlink in Ratatui, so, as with images, the reader says where its link text is and the run loop writes that text again with the address attached. Only underlined link text is marked, never the numbers or the list, and every piece is rewritten on every frame: a cell that stops being link text changes style, so Ratatui redraws it without the link, and no cell can keep a stale one. In the app, libghostty reports the click and the shell opens it under the same rule as above; libghostty's own opener is never used.
- **Layout tables are read as blocks; data tables are drawn as grids.** A message with one table inside another, or a table marked `role="presentation"`, is treated as laid out with tables. This is a guess made per message, since `html2text` cannot be told per table.
- **Headings keep a `#` marker and are bold; an image with a description shows as `[description]`**; images without one, which are mostly spacers and tracking pixels, show nothing.
- **Every span of rendered HTML goes through the same control-character stripping as plain text** before it is drawn.

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

### The 50,000-message acceptance run

Run on 2026-11-15 against a local Dovecot holding 50,000 invented messages (a third of them HTML), through the release build in a terminal:

| Measure | Result |
|---|---|
| First 500 headers in the store | 1.9 s |
| All 50,000 headers in the store | 6.5 s |
| Bodies fetched ahead (the newest 2,000 of the last 30 days) | done before the sync indicator cleared |
| Jump to the bottom of the list, then back to the top | on screen within 0.05 s |
| Reopened with the server stopped | the same list, 0.4 s after launch; a fetched body opens |
| Memory, reopened offline | 56 MB |
| Memory during the first sync | 455 MB resident at the end, 689 MB peak footprint |
| Data directory | 31 MB |

The UI stayed usable throughout, which is the acceptance. This was a server on the same machine, so it measures Quill and not a network.

- **The first sync uses far more memory than the result needs.** Every batch of 500 reloads the whole list, so 100 lists of up to 50,000 rows pass through while it runs. It falls back once the process restarts (56 MB). Loading only the rows near the cursor is the fix and is not done yet.
- **An unread count is never cut short.** The run showed 15,055 as `150…` in an 80-column window. The count column now widens to fit and the folder name gives way.

### Account setup

- **`A` opens the form** (`account.add`, File menu in the app), and a first run with no accounts opens it by itself, once. The spec names no key for it.
- **Where there is typing to do, keys are typing first.** In the form a printable key goes into the field with the cursor; only a key the field has no use for (`enter`, `esc`, `tab`, arrows up and down) reaches a command. `?` is therefore typed, not help.
- **The UI does none of the work itself.** It asks an accounts service for discovery, sign-in, saving and trusting, over channels like the store and the sync engine. `quill-cli` supplies the service; `quill-tui` still reaches no server, Keychain or file.
- **Discovery order: known providers, the domain's SRV records (RFC 6186), its mail exchanger, then `imap.` and `mail.` at the domain and at its mail host**, each of the last tried by connecting, implicit TLS before STARTTLS. Plaintext is never chosen; the user can pick it. DNS goes through the system's resolver settings (`hickory-resolver` 0.26.3, MIT/Apache-2.0) and no other resolver is asked. Mozilla's ISPDB is not consulted: that would tell a third party which domains the user has mail at.
- **A provider that needs another kind of password says so in the form** (Gmail, iCloud, Fastmail, Yahoo, AOL: app passwords; Microsoft: no password sign-in at all).
- **Nothing is saved until sign-in has worked.** Then the account is appended to `config.toml` with `toml_edit` 0.25.19 (MIT/Apache-2.0), which leaves the user's comments and layout alone; a result Quill could not read back is not written. If the file cannot be written, the password just saved is removed from the Keychain again.
- **An account added while Quill runs starts syncing at once.** The engine now always runs, even with no accounts, and takes accounts as they come (`SyncHandle::set_account`); setting one again replaces it, which is how a changed trust decision takes effect.
- **An untrusted certificate is put to the user with its subject, issuer, dates and fingerprint** (read with `x509-parser` 0.18.1, MIT/Apache-2.0), at setup and whenever an existing account's server presents one. Reject, trust once (held in memory for this run) or pin (the fingerprint goes into the account's `imap` table). A certificate that could not be read has no fingerprint to hold the server to and can only be rejected. Not answering is rejecting.
- **The app opens the user's own mail by default now**, and `--demo` asks for the demo mailbox; before there was a way to add an account it was the other way round.
- **`quill-net` holds TCP, TLS and trust for every protocol**, taken out of `quill-imap` when SMTP and POP3 needed the same.
- **The bridge message for opening a link is `link.open`**, since the spec keeps `url.open` for the shell telling the core that a `mailto:` link arrived.

### Accounts and secrets

- **The Keychain is reached through `security-framework`**, the smaller of the spec's two candidates. `quill-auth` puts it behind a `SecretStore` trait, so tests use an in-memory store and another platform can supply its own.
- **A password is filed under the service `Quill` as `imap:<username>@<host>`**, so two accounts never share one by accident, and changing an account's display name does not orphan its password.
- **Until the setup flow exists, an account is added by hand**: a `[[account]]` in `config.toml` and `quill account password NAME`, which reads the password without echo (or from a pipe) and never takes it as an argument, where it would show in the process list and shell history.
- **`imap = { ... }` gained `username` and `fingerprint`.** The spec's example has neither; a sign-in name that differs from the address and a pinned certificate both need somewhere to go.
- **An account the config no longer names is removed from the store**, with its cached mail. Never when the config failed to load: an unreadable file must not be taken for "every account was removed".
- **A plaintext account raises a warning at every start.** The spec asks for a permanent warning; a notice at each launch is what exists so far, and a standing marker on the account is still to add.
- **An account Quill cannot use yet (POP3, Graph, EWS, or IMAP with no server set) is a notice**, never a failure to start.
- **The UI's store worker retries when the database is momentarily busy**, since the sync engine writes through its own connection.

## M3, compose and send (in progress)

### SMTP

- **The SMTP client is written directly on `quill-net`, not on `lettre`**, the spec's candidate. `lettre` 0.11.23 has no SCRAM-SHA-256 or OAUTHBEARER, builds its own TLS so Quill's trust rules, pinning and certificate report cannot be used, stops at the first refused recipient, and does not expose the server's SIZE limit. Each of those is a requirement of the spec.
- **Each message goes over a connection of its own**: connect, secure, sign in, send, quit.
- **Sign-in uses the strongest mechanism the server offers for the credential**: for a password SCRAM-SHA-256, then PLAIN, then LOGIN; for an OAuth token OAUTHBEARER, then XOAUTH2. An account can name one instead, or none for a relay that takes mail unsigned; mail is never sent unsigned by accident. SCRAM uses `ring` (already in the tree through rustls) and `stringprep` 0.1.5 (MIT/Apache-2.0).
- **STARTTLS is always asked for, whatever the server advertised**, as in IMAP: the advertisement arrives unprotected. Anything but a go-ahead is a failed connection, and nothing learned before the upgrade is kept.
- **A message goes to everyone or to no one.** If any recipient is refused nothing is sent and each refusal is reported. Sending to the rest after asking the user is not built yet.
- **A message that needs 8BITMIME or SMTPUTF8 from a server without it is refused before anything is sent**, with the reason. The spec says to re-encode to 7-bit; rewriting someone's MIME is not done. Quill's own composer will write 7-bit-clean mail.
- **The only changes made to a message are removing `Bcc` and `Resent-Bcc` and making every line end in CRLF.** A bare CR or LF is how one message is smuggled inside another.
- **Checks that need no server run before sign-in**: envelope addresses with spaces, control characters or angle brackets are refused, and the size is checked against the limit as soon as the server states it.
- **The client says hello as `[127.0.0.1]`**, so the Mac's name is not told to the server.
- **Waits**: 20 seconds for each reply, and ten minutes for the answer after the message's final dot, as RFC 5321 asks.
- **If the connection drops after the whole message was handed over, the server may have taken it.** That is reported as a connection failure, and a retry can deliver twice. `Message-ID` is what lets a server or reader discard the copy.
- **Not built**: a client certificate, pipelining, and SCRAM channel binding.
- **Tested against a scripted server inside the tests**, since no SMTP server is installed and none may be: all three TLS modes, each mechanism (SCRAM against the RFC 7677 vectors), certificates that are expired, misnamed, self-signed and pinned, size limits, refused recipients, 4xx and 5xx.

### The queue, undo and the outbox

- **Every change to mail on a server is made in the store first and replayed from a queue**, as the spec asks: moves, permanent deletions, flag changes and sent copies. Each account's queue runs strictly in the order made.
- **A moved message keeps its row and its body.** The row changes folder and carries a placeholder server id until the server says what the message is called there (IMAP's COPYUID). If the server does not say, the local copy is dropped and the next sync brings the message back under its new id; keeping it would show the message twice.
- **`Backend` gained `move_message` and `append`**, both with default bodies, beside the spec's `apply`. `apply` returns nothing, and without the new id a moved message would have to be downloaded again.
- **Undo is a ten-second hold, not a reversal.** A move or deletion is not sent to the server for ten seconds; `U` within that time takes it back and the server never hears of it. After that the change is the server's. The spec's wording is "undoable for 10 seconds instead of asking for confirmation".
- **`d` moves to the trash; from the trash it deletes for good.** A permanent deletion hides the message at once and removes it when the server confirms. An account with no trash folder deletes for good directly.
- **A sync that runs before the server has heard of a change does not undo it**: a message moved away is not re-listed in its old folder, a renumbered folder keeps messages moved into it, and flags changed here win over the server's until they have been sent.
- **A change the server refuses is taken back with a notice**, along with any later change built on it (a second move of the same message). A change that cannot be sent because the server is unreachable, busy, or refusing sign-in keeps its place and is retried, doubling the wait from 30 seconds to half an hour.
- **Sending builds the message once**, files it among the blobs and records who it goes to. Every attempt sends those same bytes under the same Message-ID.
- **A send interrupted by a crash is never repeated automatically.** Whether the server took it is not known, and the spec says a replay must not duplicate a send, so it is held in the outbox marked as such for the user to check Sent and decide.
- **After a send**, the copy is saved to the account's Sent folder unless the server keeps its own (Gmail's SMTP does), and the message replied to is marked answered. Both go through the queue.
- **Contacts for completing addresses come from message headers as they sync**: senders of received mail, and with five times the weight the recipients of mail in Sent. People the user writes to should come before a newsletter that writes every week.
- **Not built yet**: syncing drafts to the server's Drafts folder (drafts are local), sending to the accepted recipients after some were refused, and send later.

### Building mail

- **`mail-builder` 2.0.1 (the spec's candidate, MIT/Apache-2.0) writes headers and MIME; `pulldown-cmark` 0.13.6 (MIT) parses the Markdown.** `mail-builder` runs with its defaults off, which keeps the machine's host name out. Quill writes the date (the library always says +0000), the boundaries (the library's come from the clock) and the Message-ID itself, and base64-encodes every attachment (the library would send a text attachment holding a NUL as 7bit).
- **The same draft always builds to the same bytes**, which is what lets every send attempt carry one Message-ID.
- **What is built is 7-bit clean**: encoded headers, quoted-printable or base64 bodies, CRLF everywhere, no line over 998 bytes. A non-ASCII address is the one exception and needs SMTPUTF8 from the server.
- **A message written in Markdown goes as `multipart/alternative`**: the text as typed, and HTML made from it by Quill's own small renderer. Raw HTML the user typed is shown escaped, not passed through; images become links; a link is live only if Quill itself would open it. `format=flowed` is not used.
- **No User-Agent or X-Mailer header**, and nothing in the boundaries or Message-ID names the program, system or host.
- **Text from the user cannot add a header**: line breaks and control characters in subjects, names and file names become spaces, and an address is stripped of anything that could make it two.
- **A forwarded message attached as `.eml`** goes in as `message/rfc822` when it is 7-bit, and as a base64 `application/octet-stream` named `.eml` when it is not, since `message/rfc822` may not be base64-encoded.
- **Replies**: to Reply-To if the original has one, otherwise From. Reply-all keeps the original's To in To and Cc in Cc, without the user's own addresses or duplicates. A reply to mail the user sent goes to its recipients. `Re:` and `Fwd:` are added once; `RE:`, `Re[2]:`, `AW:`, `SV:`, `Antw:`, `Fw:` and `WG:` are recognised as already there.
- **A forward has no In-Reply-To but keeps References**, so it stays in its conversation.
- **Not built**: Reply-To on outgoing mail, punycode for international domains, stripping the original's signature when quoting.

### IMAP changes to mail

- **Move uses `UID MOVE`** where the server has it; otherwise `UID COPY`, mark `\Deleted`, and `UID EXPUNGE` of that one message (UIDPLUS). The new id comes from the server's `COPYUID`, and an appended message's from `APPENDUID`. `async-imap` parses both and then drops them, so these commands are sent through its raw command interface.
- **On a server with neither MOVE nor UIDPLUS, a plain `EXPUNGE` is used only when nothing else in the folder is marked `\Deleted`.** If other mail is marked, the move or deletion is refused before anything changes: a plain `EXPUNGE` would remove that other mail too, and the spec says never to expunge without the server's confirmation of what goes.
- **After any expunge the folder is searched for the message again**, and a message still there is an error, not a success. A shared folder can accept the `\Deleted` flag without setting it.
- **Moving or deleting a message that is already gone succeeds**, and so does one whose folder was renumbered or removed, so a replay after a crash is safe. Copy and append cannot be made safe to repeat; the queue runs each once and a crash mid-append can leave two sent copies.
- **A UID is never acted on under a different UIDVALIDITY** than the one in the message's id.
- **Folder names from the server are unescaped** (`\"` and `\\` inside quoted names), and sent back quoted or as a literal as their characters need. Before this a folder with a quote or backslash in its name could be listed and not opened.
- **Each sign-in asks for `CAPABILITY` once more**, to learn whether the server has MOVE and UIDPLUS. A server that refuses the question is taken to have neither.

### IMAP: refusals, silence and mail marked deleted

- **Every command after sign-in is read to its tagged answer by Quill's own runner**; `async-imap` now only connects, signs in and signs out. Before this a `NO` or `BAD` to SEARCH, FETCH, STORE or LIST was taken for an empty answer, which could report every message in a folder as removed. A refused command now fails the whole call and changes nothing here.
- **A server that goes silent is given 20 seconds** (`quill_net::PATIENCE`), counted from the last byte in either direction, not per command, so a large message on a slow line is not cut off. The connection is then dropped and the next call makes a new one. IDLE, in M6, will need a longer limit or a connection of its own.
- **Mail marked `\Deleted` and not yet expunged is treated as gone**: it is never listed, and a message seen before is reported removed. If the mark is later cleared it comes back as new under the same id. Nothing on the server is touched. The sync searches `UNDELETED` and checks the flags of what it fetches as well, for a message marked between the two.
- **A summary is accepted only for a UID that was asked for**, so a flag update the server volunteers in the middle of a FETCH is not mistaken for new mail.

### The compose screen

- **The editor is in-house** (`quill_tui::editor`, behind the `Editor` trait), not tui-textarea or edtui. The spec's tests for the choice were graphemes and emoji, large pastes, undo, and macOS key conventions; an editor written to them was less work than bending either crate to them. A megabyte paste redraws in well under a frame because wrapping is cached per line. Details of its behaviour (the last column kept for the cursor, undo coalescing as in TextEdit, `ctrl-a`/`ctrl-e`/`ctrl-k` as in Cocoa text fields) are in the module's own comments.
- **Markdown highlighting is a simple scanner**, not CommonMark: quote, heading and fenced lines are marked whole, inline marks do not nest. What is sent is converted by `quill_mime::markdown`, which is stricter.
- **Not built from the spec's editor list**: find (`cmd-f`), paste as quotation, reflow and insert template have code in the editor and no command yet; mouse placement and selection wait for mouse support in M6; spell check waits for the bridge message in M6; several drafts open as tabs is not built (one message is written at a time, and closing it keeps it as a draft).
- **`z` cannot zoom while writing**, since it is a letter. `ctrl-z` zooms in every pane.
- **Keys for a terminal as well as the app.** Each compose command has a `cmd` key and a `ctrl` key that needs no keyboard protocol: `ctrl-x` send, `ctrl-s` save, `ctrl-t` attach, `ctrl-o` Bcc. Hints name the `ctrl` key outside Quill's own window, where `cmd` never arrives.
- **Bcc is hidden until asked for** (`compose.bcc`) or filled in, and `tab` passes it while it is empty, so To, Cc, Subject, body is the path through a new message.
- **Attaching: a path in a terminal, the native panel in the app.** In a terminal `compose.attach` asks for a path, typed or pasted. In the app it asks the shell for its open panel (`files.pick`), and files dropped on the window arrive the same way (`files.dropped`); dropped with nothing being written, they start a new message. The user's choice lets the shell read a file but not the core, a separate process in the same sandbox, so the shell copies each file into the app's temporary folder and the core removes the copy once it has read it. The spec has `files.pick` answered by a reply; here every bridge message is a notification, so the answer is `files.dropped`, which the spec already has for drops. No new entitlement: `files.user-selected.read-write` was already held.
- **Copy goes to the shell in the app (`clipboard.write`, a new bridge message) and through the terminal elsewhere (OSC 52).** Paste arrives from the terminal as a bracketed paste either way.
- **The embedded terminal keeps no key bindings of its own** (`keybind = clear`). libghostty arrives with Ghostty's defaults, which took `cmd-c`, `cmd-a`, `cmd-z`, `cmd-enter` and others before the menus or the core saw them: Send did nothing from the keyboard and undo never reached the editor. Every key is now the menus' or the core's.
- **The Edit menu's Cut, Copy and Select All belong to the core while it says so.** The core has commands `edit.cut`, `edit.copy` and `edit.select_all` with no menu items of their own; the shell's items run them while they are enabled (the cursor is in the body, and for cut and copy something is selected) and otherwise act on the focused view, so text selected with the mouse in the terminal still copies. They are targeted at the window explicitly: an item nobody answers for is disabled, and a disabled item still swallows its key.
- **A reply is started from the downloaded message.** If only its headers are here, the body is fetched first and the reply opens when it lands.
- **The signature goes above the quoted message** when the reply is written above it, and at the end when it is written below. `full_name` and `signature` are per-account keys in `config.toml`; the spec's several identities per account are not built.
- **Closing an untouched reply leaves no draft**, and a draft emptied out is deleted. Anything else written is kept, also when Quill quits: the run loop saves it and waits for the store before exiting.
- **Drafts are local.** They are not yet copied to the server's Drafts folder, so a draft started here is not seen by another client. This is the spec's "sync to the server's Drafts folder", still to do.
- **Drafts and Outbox rows use negative ids** in the message list (`DraftId::as_message_id`), so one list type serves mail and drafts and nothing can confuse the two.
- **An answered message is also marked read**, and shows `r` in the list's first column once read.
- **A saved copy of sent mail makes the engine sync at once**, so Sent shows it without waiting for the next interval.
- **The sending server has its own certificate question.** A certificate the SMTP server presents and Quill does not accept holds the message in the Outbox and asks, as for IMAP; "always trust" pins it under `smtp = { fingerprint = ... }`, and the waiting mail goes as soon as it is answered.
- **The account form asks about the sending server too** ("Send via", with its port and security) and signs in to it before saving, with `SmtpSender::check`, which sends nothing. A failure there goes back to that field and says reading works. An untrusted certificate on the sending server does not hold the account up at setup: it is asked about when the first message is sent. One password serves both servers; an account whose sending server wants another needs `quill account password` and a `username` under `smtp`, by hand.
- **Checked end to end against a scratch Dovecot and a local SMTP sink** that writes what it is given to a file and delivers nothing: reply, new message with a completed address and an attachment, undo send, reopening from Drafts and from the Outbox, the copy in Sent and the answered flag on the server. In a terminal and again in the sandboxed `Quill.app`, where the account was added through the form (both servers signed in to), a file was attached through the panel's path with its temporary copy removed, a message was sent with `cmd-enter`, and cut, copy, select all and undo reached the editor. A scripted run never shows a panel or touches the clipboard: it records what would have been chosen and copied. Dropping a file with the mouse is not checked; it shares the panel's path from the point the files are known. Not checked against any real provider: the owner's accounts are not touched while they are away.
- **Keychain reads block.** The engine reads a password with a synchronous call, and macOS can hold that call while it asks the user about a rebuilt binary. Mail stops until it is answered and nothing says why. To fix with the credential rework in M5.

## M7, POP3 (the protocol crate; the rest of the milestone follows M6)

- **`quill-pop3` is a small in-house client**, as the spec says: CAPA, STLS, USER/PASS (AUTH PLAIN where that is all a server offers), STAT, UIDL, LIST, TOP, RETR, DELE, with pipelining when advertised. Built ahead of its milestone because it stands alone.
- **A message's id is its UIDL**, which stays the same between sessions where message numbers do not. A server without UIDL is not supported and says so.
- **The sync cursor is the list of UIDLs already seen**: sorted, prefix-compressed, deflated (`flate2` 1.1.11, MIT/Apache-2.0, already in the tree) and base64. Full ids, not hashes, so a removed message can be named exactly. 50,000 Dovecot-style ids take 5 KB; 50,000 random 32-character ids take about 1 MB, which is as small as full ids get. An unreadable cursor is treated as none: everything is listed again and nothing is reported removed.
- **Every call is one short session**: sign in, work, QUIT. POP3 locks the mailbox while a session is open.
- **Deleting commits only on a clean QUIT**, and a message already gone counts as deleted, so a replay is safe. The sync itself never deletes. Leave-on-server policies are the sync engine's to apply and are not built yet.
- **Headers come from TOP**, or from RETR for a server or a message TOP is refused for.
- **Flags, folders, moves and saved copies are unsupported** by the backend and say so; the engine will keep them locally for these accounts when the milestone is finished.
- **Tested against Dovecot's POP3 service and a scripted server**, including the spec's acceptance: repeated polls with deliveries and deletions in between never duplicate or lose a message.
