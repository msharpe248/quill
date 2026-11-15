# Quill: Terminal Email Client Spec

Nov 14, 2026 · @Michael Sharpe

## Summary

Yes, this is buildable. Quill (working name, rename freely) is a native macOS mail app whose main surface is a terminal it creates itself. A Swift shell supplies the window, menus, toolbar and media viewers. A Rust core draws the keyboard-first mail UI inside an embedded libghostty terminal and does all the mail work.

Owning the terminal removes the need to cope with every terminal's quirks, and leaves room to run a bundled Vim in its own pane later. The price is a dependency on libghostty's embedding API, which is not yet stable, so the spec isolates it in one module and proves it in the second milestone.

| Decision | Choice | Reason |
| --- | --- | --- |
| App form | Native macOS app with an embedded terminal | Menus, toolbar, notifications, `mailto:` and native viewers around a terminal UI |
| Terminal | libghostty through the GhosttyKit Swift package, version pinned | Full Kitty graphics and keyboard protocol in a terminal Quill controls ([libghostty-spm](https://lakr233.github.io/libghostty-spm/)) |
| Split of work | Swift and AppKit shell; Rust core with Ratatui | The shell holds no mail logic; the core stays testable and still runs in Ghostty or Kitty |
| Compose | Built-in editor first; bundled Vim as a later option | Both work inside the App Store sandbox; running the user's installed editor does not |
| Exchange Online / Microsoft 365 | Microsoft Graph with OAuth 2.0 | EWS is disabled by default in Exchange Online from Oct 1, 2026 and removed Apr 1, 2027 ([Petri](https://petri.com/microsoft-counts-down-to-end-of-exchange-web-services/)) |
| Exchange Server on-premises | EWS, optional late milestone | The EWS retirement does not touch on-prem servers, and Graph is not available there |
| Inline images | Kitty graphics protocol only | The embedded terminal always supports it, so no fallbacks are needed |
| Video, PDF, Office files | Quick Look and AVKit panes inside the app window | Native viewers, no handoff to another app |
| HTML mail | Styled text in the terminal; sanitized web view pane one key away | Readable by default, faithful on demand |
| Storage | Local SQLite with FTS5, offline-first | Instant UI, full-text search, works on a plane |
| Secrets | macOS Keychain only | No passwords or tokens in config files |
| Distribution | Mac App Store, sandboxed | Simple install and updates; the sandbox is designed in from the first shell commit |

## Goals and non-goals

Goals:

- Feel as fast and as discoverable as lazygit: every action one or two keys away, with on-screen hints and a command palette.
- Be a real Mac app: menu bar, toolbar, tabs, notifications, dock badge, `mailto:` handling, drag and drop.
- Keep the keyboard first. Every menu item and button runs a command that also has a key; nothing requires the mouse.
- Compose in a capable built-in editor, with a bundled Vim as a later option.
- Several accounts of mixed types in one unified inbox.
- Full offline use: read, search, compose, file and delete without a connection, then reconcile.
- Media viewed inside the app window: images inline in the terminal, everything else in a native pane beside it.
- Safe by default: remote content blocked, HTML sanitized, hostile escape sequences stripped.
- Scriptable: a headless CLI with JSON output so shell scripts and coding agents can drive it.

Non-goals for version 1:

- Running well in arbitrary terminals. The core runs standalone in Ghostty or Kitty for development and tests only.
- Running the user's installed editor or any other program outside the app bundle. The App Store sandbox rules that out.
- Linux and Windows. Keep platform code in the shell and behind the core's `Host` trait so Linux can follow.
- Calendar and contacts management beyond showing invites and completing addresses.
- Acting as a mail server, or syncing to Maildir for other clients.
- Pixel-faithful HTML rendering inside the terminal. The web view pane covers that.

## Tech stack

Two languages: Swift 6 with AppKit for the shell, and Rust (stable toolchain, 2024 edition) for the core. The packages and crates below are starting candidates, not pins: check each one's current version, maintenance state and API before adopting it.

| Area | Candidate | Notes |
| --- | --- | --- |
| Shell | Swift 6, AppKit, Swift Package Manager | SwiftUI is fine for settings and simple panels |
| Terminal surface | GhosttyKit and GhosttyTerminal from `libghostty-spm`, or an XCFramework built from Ghostty source | Pin an exact version; macOS 13 or later |
| Native viewers | QuickLookUI, AVKit, WebKit | `QLPreviewView`, `AVPlayerView`, `WKWebView` |
| Shell-core bridge | JSON-RPC 2.0, newline-delimited, over a Unix domain socket | `Codable` in Swift, `serde_json` in Rust |
| Updates | Mac App Store | App Sandbox and hardened runtime; TestFlight for betas |
| TUI | `ratatui`, `crossterm` | Kitty keyboard protocol, mouse, bracketed paste, focus events |
| Async runtime | `tokio` | All network and disk work off the UI task |
| IMAP | `async-imap` or `io-imap` | Needs IDLE, CONDSTORE, QRESYNC, XOAUTH2 |
| POP3 | Small in-house client | The protocol is tiny; needs UIDL, TOP, STLS |
| SMTP | `lettre` | Submission on 465 or 587, XOAUTH2 |
| Graph and EWS | `reqwest` with typed `serde` models | No heavyweight SDK |
| MIME | `mail-parser`, `mail-builder` | Parse and build, charset handling |
| OAuth | `oauth2` | Authorization code with PKCE, loopback redirect |
| Keychain | `keyring` or `security-framework` | The core binary is signed with the app's identity so both see the same items |
| Store | `rusqlite` with bundled SQLite and FTS5 | WAL mode |
| HTML to text | `html2text`, `ammonia` | Render and sanitize |
| Images | `image`, plus `ratatui-image` in Kitty mode or a small Kitty encoder | Kitty protocol only |
| Config | `serde`, `toml` |  |
| Tests | `insta`, `wiremock`, XCTest | Snapshot, HTTP fixture and shell tests |
| Built-in editor | tui-textarea or edtui, or an in-house widget | Judge on grapheme and emoji handling, large pastes, undo, and input-method composition |

The Pimalaya project (Himalaya CLI) already ships IMAP, SMTP, JMAP, Gmail API and Microsoft Graph backends in Rust ([himalaya](https://docs.rs/crate/himalaya/2.0.0)). Read it as prior art before writing the backends, and reuse its crates where the licence and API fit.

## Application shell

Quill.app is a thin Swift shell around terminal surfaces. It owns windows, menus, the toolbar, native viewers and macOS integration. It never touches mail, the store or the network.

```
+---------------------------------------------------------------------------+
| Quill   File  Edit  View  Mailbox  Message  Window  Help                  |  menu bar
+---------------------------------------------------------------------------+
| [Compose] [Reply] [Archive] [Delete]       Work v       [ Search       ]  |  toolbar
+---------------------------------------+-----------------------------------+
|                                       |                                   |
|  main terminal surface                |  second pane, when open:          |
|  Rust core: sidebar, list, reader,    |  Quick Look, video, HTML web      |
|  compose                              |  view; later, a Vim terminal      |
|                                       |                                   |
+---------------------------------------+-----------------------------------+
```

What the shell provides:

- A window whose main surface is a libghostty terminal running the bundled core (`quill tui`) in a PTY. Native tabs and several windows are supported.
- A menu bar and toolbar built from the core's command registry. Each item shows its command's key and is enabled or disabled from core state. The toolbar can be hidden.
- A second pane, split right or below and resizable, or opened as a tab. It hosts a Quick Look preview, an AVKit player or a web view, and later a Vim terminal.
- `cmd-]` and `cmd-[` move focus between panes. These are shell shortcuts, so they never collide with editor keys.
- Notifications with actions (Archive, Mark read, Reply), a dock badge with the unread count, the `mailto:` handler, and registration as the default mail app.
- Drag files onto the window to attach them, native open and save panels, and the Share menu.
- Terminal font, size and colours come from Quill's config and theme. The user's Ghostty config is not read.

The bridge is JSON-RPC over a Unix socket in a private directory. The shell passes its path to the core in `QUILL_SHELL_SOCKET`.

| Direction | Message | Purpose |
| --- | --- | --- |
| Core to shell | `commands.list` | Names, titles, keys and menu placement for every command, sent at startup |
| Core to shell | `state.update` | Focused context, enabled commands, unread count, window title |
| Core to shell | `pane.editor` | Later milestone: run the bundled Vim on a draft file; the reply carries the exit status |
| Core to shell | `pane.preview`, `pane.play`, `pane.web` | Show a file in Quick Look, AVKit or the web view |
| Core to shell | `pane.close` | Close the second pane and return focus |
| Core to shell | `thumbnail.make` | Ask for a poster frame or thumbnail as a PNG file |
| Core to shell | `notify`, `files.pick`, `files.save` | Notifications and native file panels |
| Shell to core | `command.run` | A menu item, toolbar button or notification action was used |
| Shell to core | `url.open`, `files.dropped` | A `mailto:` link or dropped files arrived |
| Shell to core | `appearance.changed`, `app.will_quit` | Light or dark mode changed; flush and exit cleanly |
| Core to shell | spell.check | Send text, get back misspelled ranges and suggestions from the system spell checker |

Bridge rules:

- The shell knows command names, not mail semantics. A new feature is a new command in the core; menus pick it up from `commands.list`.
- The message schema lives in `schema/bridge.json`. Swift and Rust types are both checked against it in CI.
- Without the socket the core runs standalone. A `Host` trait has two implementations: `ShellHost` uses the bridge, and `StandaloneHost` uses `qlmanage` and `open` for media.

libghostty isolation:

- All libghostty calls live in one Swift module, `TerminalKit`, behind a `TerminalSurface` protocol: start a command, send text, resize, focus, set font and theme, report exit.
- No other module imports GhosttyKit. The embedding API is flagged as unstable upstream ([Ghostty](https://github.com/ghostty-org/ghostty)), so upgrades are deliberate, one version at a time, against a surface test suite.

Distribution is the Mac App Store, so the app is sandboxed from the first shell commit:

- Entitlements: App Sandbox, outgoing network connections, user-selected file read and write, Keychain. Record the reason for any addition in `DECISIONS.md`.
- The core binary ships inside the bundle and inherits the sandbox. Nothing outside the bundle is ever launched; links and files open through `NSWorkspace`.
- Files outside the container are reached only through open and save panels, drops onto the window, and security-scoped bookmarks.
- Config, the database and caches live in the app's container. "Edit Config" and "Reveal Config in Finder" menu items make the config easy to reach.
- The App Store handles updates. There is no in-app updater.
- M1 must confirm that the pinned libghostty build passes App Store validation. Ghostty itself is not distributed there, so this is unproven.
- The headless CLI is a helper inside the bundle that shares data through an app group container. Confirm it is workable before promising it.

## Architecture

Two processes. The Swift shell hosts the terminal, and the Rust core runs inside it with one Tokio runtime. In the core, the UI task owns the terminal and never waits on the network or disk; everything else runs as background tasks that talk to it over channels.

```
quill/
  app/                       Swift package and Xcode project for Quill.app
    Sources/QuillApp/        windows, menus, toolbar, panes, notifications
    Sources/TerminalKit/     the only module that imports GhosttyKit
    Sources/Bridge/          JSON-RPC client and message types
    Resources/vim/           Vim plugin shipped with the app
  crates/
    quill-core/    domain types, Backend trait, sync engine, operation queue
    quill-store/   SQLite schema, migrations, FTS5, blob cache
    quill-mime/    parse, build, HTML to text, sanitizer, threading
    quill-auth/    OAuth flows, Keychain access
    quill-imap/    Backend for IMAP
    quill-pop3/    Backend for POP3
    quill-smtp/    Sender for SMTP submission
    quill-graph/   Backend and Sender for Microsoft Graph
    quill-ews/     Backend and Sender for on-prem Exchange (feature flag)
    quill-host/    Host trait: ShellHost (bridge) and StandaloneHost
    quill-media/   image decoding, Kitty graphics, thumbnail cache
    quill-tui/     Ratatui app: state, views, keymap, themes, command registry
    quill-cli/     the `quill` binary: `tui` plus headless subcommands
  schema/bridge.json         bridge message schema
```

Data flow:

1. The TUI reads only from the local store. It renders from an in-memory view model refreshed by store change events.
2. A user action becomes an `Operation` (mark read, move, send). It is applied to the store at once, then queued.
3. The sync engine replays queued operations against the account's backend and pulls remote changes into the store.
4. Failures roll the local change back and raise a visible, dismissible notice.

The UI follows a unidirectional loop: `Event -> Action -> update(State) -> view(State)`. No view mutates state directly. Target a 16 ms frame budget and redraw only on change.

Every protocol implements the same trait, so the sync engine and UI stay protocol-agnostic:

```rust
#[async_trait]
pub trait Backend: Send + Sync {
    fn capabilities(&self) -> Capabilities; // folders, flags, push, server search, threads
    async fn list_folders(&self) -> Result<Vec<RemoteFolder>>;
    async fn sync_folder(&self, folder: &FolderId, cursor: Option<SyncCursor>)
        -> Result<SyncDelta>;            // added, changed, removed, next cursor
    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes>; // full RFC 5322 message
    async fn apply(&self, op: &Operation) -> Result<()>;       // flag, move, copy, delete
    async fn watch(&self, tx: Sender<RemoteEvent>) -> Result<()>; // IDLE, polling, or no-op
}

#[async_trait]
pub trait Sender: Send + Sync {
    async fn send(&self, raw: &[u8], envelope: &Envelope) -> Result<()>;
}
```

`Capabilities` lets the UI degrade honestly. POP3 reports no folders and no remote flags, so Quill keeps those locally for that account.

## Accounts, protocols and auth

| Account type | Read | Send | Auth | Change detection |
| --- | --- | --- | --- | --- |
| IMAP (generic, Fastmail, iCloud) | IMAP4rev1 | SMTP | Password or app password | IDLE, CONDSTORE/QRESYNC when offered |
| Gmail | IMAP | SMTP | OAuth 2.0 (XOAUTH2) or app password | IDLE |
| Microsoft 365 / Exchange Online | Graph | Graph `sendMail` | OAuth 2.0 with PKCE | Delta queries, polled |
| Exchange Server on-prem | EWS | EWS | Basic, NTLM or OAuth as the server allows | `SyncFolderItems`, polled |
| POP3 | POP3 | SMTP | Password; STLS or implicit TLS | Poll with UIDL |

IMAP:

- Track `UIDVALIDITY` per folder and resync the folder when it changes.
- Fetch envelopes and structure first, bodies on demand, and prefetch bodies for the newest 30 days.
- Use `SPECIAL-USE` to find Sent, Drafts, Trash, Archive and Junk; allow overrides in config.
- Hold one IDLE connection per watched folder, capped per account, with reconnect and backoff.
- Map Gmail labels as folders in version 1. Note it as a known limitation.

Microsoft Graph:

- Sync each folder with `messages/delta`, storing the delta link as the cursor.
- Request immutable IDs so a message keeps its ID when moved.
- Fetch the raw MIME with `messages/{id}/$value` so one MIME pipeline serves every backend.
- Scopes: `Mail.ReadWrite`, `Mail.Send`, `offline_access`, `User.Read`.
- Honour `Retry-After` on throttling.

POP3:

- Use `UIDL` to detect new mail and `TOP` for header previews when the server supports it.
- Per-account setting: leave on server, delete after N days, or delete on download.
- Folders, flags and archive exist only locally for these accounts.

OAuth:

- Authorization code with PKCE. Open the system browser, receive the redirect on a loopback port, store refresh tokens in the Keychain.
- Offer device code flow as a fallback for SSH sessions.
- Client IDs come from config. Ship a setup guide for registering a public-client app in Microsoft Entra and a Google Cloud project.

Account setup is a guided flow inside the TUI: type an address, try autodiscovery (well-known provider table, DNS SRV, Autodiscover), confirm, authenticate, start syncing.

## SMTP sending and transport security

Every IMAP and POP3 account sends through SMTP submission, and every protocol supports all three connection modes below. The mode is set per server in config as `tls = "implicit" | "starttls" | "none"`.

| Protocol | Implicit TLS | STARTTLS upgrade | Plaintext (opt-in only) |
| --- | --- | --- | --- |
| IMAP | 993 | 143 with `STARTTLS` | 143 |
| POP3 | 995 | 110 with `STLS` | 110 |
| SMTP submission | 465 | 587 with `STARTTLS` (25 allowed) | 587 or 25 |
| Graph, EWS | HTTPS 443 | n/a | Never |

TLS rules:

- Ports are defaults; any port can be set. Autodiscovery prefers implicit TLS, then STARTTLS, and never picks plaintext.
- STARTTLS is strict. If the server does not advertise it, or the upgrade fails, the connection fails. No silent fallback to plaintext.
- Discard any capabilities and buffered input received before the upgrade, then re-issue `CAPABILITY`, `CAPA` or `EHLO` over TLS.
- Never send credentials before the channel is encrypted unless the account is set to `tls = "none"`.
- TLS 1.2 and 1.3 through `rustls`, with SNI and hostname verification against the system trust store.
- Per-account extras: a custom CA bundle, a pinned certificate fingerprint for self-signed servers, and a client certificate and key for mutual TLS.
- An untrusted certificate shows its subject, issuer, validity dates and fingerprint, and offers reject, trust once, or pin.
- Servers limited to TLS 1.0 or 1.1 are not supported in version 1. Record this in the README.

SMTP:

- Authentication mechanisms: `PLAIN`, `LOGIN`, `XOAUTH2`, `OAUTHBEARER`, `SCRAM-SHA-256`. Choose the strongest the server offers; allow an override.
- Honour `SIZE` before sending and report an over-limit message before upload starts.
- Use `8BITMIME` and `SMTPUTF8` when offered; otherwise encode to 7-bit and refuse internationalised addresses with a clear error.
- `Bcc` recipients go in the envelope and are stripped from the transmitted headers. The Sent copy keeps them.
- A 4xx reply keeps the message in the Outbox and retries with backoff. A 5xx reply stops, keeps the message, and shows the server's text.
- Per-recipient rejections are reported individually; the message is not marked sent unless every recipient was accepted or the user confirms a partial send.
- An SMTP server can be shared by several identities, and an identity can use a different SMTP server from its IMAP or POP3 host.

Tests: the Docker mail server runs every protocol in all three modes, plus cases for a missing STARTTLS capability, an expired certificate, a hostname mismatch and a self-signed certificate.

## Local store and sync

SQLite in WAL mode is the single source of truth for the UI. Raw messages and attachments live as files beside it, named by content hash.

| Location (inside the app container) | Contents |
| --- | --- |
| `Library/Application Support/Quill/config/` | `config.toml`, `keys.toml`, `themes/` |
| `Library/Application Support/Quill/data/` | `quill.db`, raw message blobs |
| `Library/Caches/` | Thumbnails, decoded images, temp HTML |
| macOS Keychain | Passwords, refresh tokens |

Core tables: `accounts`, `folders`, `messages` (envelope, flags, thread id, remote id, blob hash), `threads`, `attachments`, `operations` (the outbound queue), `sync_state` (cursor per folder), `contacts` (harvested addresses with frequency), and an FTS5 table over subject, participants and body text.

Rules:

- Migrations are numbered, forward-only and run at startup inside a transaction.
- Every remote mutation is an `operations` row first. The queue survives restarts and replays in order per account.
- Operations are idempotent. A replay after a crash must not duplicate a send or a move.
- Sending writes to a local Outbox, then delivers. A message leaves the Outbox only after the server accepts it.
- Body text is extracted and indexed when a body is first downloaded.
- A cache budget (default 5 GB) evicts the oldest unpinned blobs. Envelopes are never evicted.

## UI and UX

Three panes, lazygit-style: sidebar, message list, reader. The focused pane has a highlighted border, and a bottom bar always shows the keys that apply right now.

```
+- Accounts ------+- Inbox (unified) - 12 unread ----------+- Reader ---------------------+
| * Unified       | * Ana Ruiz      Q4 roadmap        09:14 | From: Ana Ruiz                |
|   Work          |   GitHub        [quill] PR #42    08:51 | Subject: Q4 roadmap           |
|     Inbox   8   | @ Sam Lee       Photos from Sat   Fri   |                               |
|     Sent        |   +- Sam Lee    Re: Photos        Fri   | Hi Michael, ...               |
|   Personal      |                                         | [image: chart.png]            |
|     Inbox   4   |                                         | -- 2 attachments (a) --       |
+-----------------+-----------------------------------------+-------------------------------+
  j/k move  enter open  r reply  e archive  / search  : commands  ? help
```

Layout:

- Under 100 columns the reader replaces the list when a message opens. Under 60 the sidebar becomes an overlay.
- `1`, `2`, `3` focus panes; `tab` cycles; `z` zooms the focused pane to full screen.
- Mouse works everywhere: click to focus and select, wheel to scroll, click links and attachments.

Default keys (Vim-flavoured; a `mutt` preset ships as an alternative):

| Key | Action | Key | Action |
| --- | --- | --- | --- |
| `j` / `k` | Next, previous | `c` | Compose |
| `g g` / `G` | Top, bottom | `r` / `R` | Reply, reply all |
| `enter` | Open | `f` | Forward |
| `e` | Archive | `a` | Attachments |
| `d` | Delete | `v` | View HTML in the web view pane |
| `s` | Star | `o` | Open link or attachment |
| `u` | Toggle unread | `/` | Search |
| `m` | Move to folder (fuzzy) | `:` or `ctrl-k` | Command palette |
| `x` | Select; `V` for range | `U` | Undo last action |
| `b` | Snooze | `?` | Help for this context |

Interaction rules:

- Every action is a named command. Keys, the palette, mouse and the CLI all call the same commands.
- The palette fuzzy-matches commands, folders, accounts and saved searches, and shows each command's key.
- Destructive actions are undoable for 10 seconds instead of asking for confirmation.
- Nothing blocks. Long work shows a spinner in the status bar and can be cancelled with `esc`.
- Search runs as you type against the local index. `ctrl-enter` extends it to the server.
- Search syntax: `from:ana has:attachment after:2026-10-01 is:unread in:work/inbox "exact phrase"`.
- Themes are TOML files with true colour and a 256-colour fallback. Ship dark, light and a terminal-palette theme.
- Honour `NO_COLOR`, and never rely on colour alone to show state.

## Message rendering and media

The reader shows the best in-terminal version of a message, and the second pane shows the faithful version without leaving the app.

| Content | In the terminal | In the second pane |
| --- | --- | --- |
| Plain text | Wrapped, quote levels coloured, signatures dimmed, links numbered | `o` opens a link in the default browser |
| HTML | Converted to styled text: headings, lists, emphasis, simple tables, numbered links | `v` shows sanitized HTML in a web view; `V` opens the default browser |
| Images (PNG, JPEG, GIF, WebP, HEIC) | Inline at pane width through Kitty graphics | `space` for Quick Look; `O` for Preview |
| Animated GIF | Animated inline | Quick Look |
| Video | Poster frame with duration and a play badge | `space` or `enter` plays in an AVKit pane with scrubbing, full screen and picture-in-picture |
| Audio | File row with duration | AVKit pane with transport controls |
| PDF, Office, other | File row with type and size; first-page thumbnail for PDF | Quick Look; `O` opens the default app |
| Calendar invite (`.ics`) | Summary card: what, when, where, attendees | Accept, tentative, decline; `O` opens Calendar |

Images:

- Kitty graphics protocol only. There are no iTerm2, Sixel or half-block code paths.
- Ghostty 1.4 implements the whole Kitty graphics specification, including animation ([release notes](https://ghostty.org/docs/install/release-notes/1-4-0)). Confirm the pinned libghostty build includes it.
- Decode and scale off the UI task, cache by content hash and cell size, and clear placements when a pane scrolls, resizes or closes.
- In standalone mode, query for Kitty graphics at startup. Without it, show file rows in place of images.

Native viewers:

- Quick Look uses `QLPreviewView` in the second pane. Arrow keys step through a message's attachments, and `esc` closes the pane and returns focus to the terminal.
- Video and audio use `AVPlayerView`. `space` toggles play, and arrow keys seek.
- The web view is a `WKWebView` with JavaScript off, a non-persistent data store, and a content rule list that blocks every network load unless remote content is allowed for that message. Links open in the default browser, never in the pane.
- Poster frames and thumbnails come from `QLThumbnailGenerator` in the shell, requested over the bridge and cached by the core.
- Files are written to a private temp directory (mode 0700) before viewing and removed on exit.
- Standalone mode falls back to `qlmanage -p` for previews and `open` for video and HTML.

Remote content and safety:

- Remote images are blocked by default, with a per-message key (`i`) and a per-sender allow list.
- HTML is sanitized before it reaches the web view or the browser: scripts, forms, external CSS and remote loads removed unless allowed. `cid:` images are rewritten to local files.
- Strip C0 and C1 control characters and escape sequences from every header and body before drawing. Mail is untrusted terminal input.
- Show the real target next to any link whose text looks like a different URL.

## Compose and send

Compose opens in the main terminal surface in place of the reader, so the message list stays visible. `z` zooms it to the full window.

```
+- New message -------------------------------------------------+
| From:    Michael <me@example.com>  (Work)                     |
| To:      ana@example.com,                                     |
| Cc:                                                           |
| Subject: Q4 roadmap                                           |
| Attach:  roadmap.pdf (214 KB)                                 |
+---------------------------------------------------------------+
| Hi Ana,                                                       |
|                                                               |
| Here is the draft roadmap.                                    |
|                                                               |
+---------------------------------------------------------------+
  cmd-enter send   cmd-s save draft   cmd-shift-a attach   esc close
```

Built-in editor (version 1):

- It lives in `quill-tui` behind an `Editor` trait, so a second implementation can be added without touching compose logic.
- Keys follow macOS text conventions: arrows, `opt-arrow` by word, `cmd-arrow` to line and document ends, shift to select, `cmd-z` and `cmd-shift-z` for undo and redo, `cmd-x`, `cmd-c`, `cmd-v`, `cmd-f` to find.
- The shell passes `cmd` combinations to the core through the Kitty keyboard protocol, except those claimed by a menu item.
- Soft wrap at pane width. Click places the cursor, drag selects, and the wheel scrolls.
- Multi-level undo that survives a save, and crash recovery from the autosaved draft.
- Correct handling of grapheme clusters, emoji, wide characters and input-method composition.
- `tab` and `shift-tab` move between header fields and the body. Address fields show a completion popup ranked by frequency and recency. The From field cycles identities.
- Light Markdown highlighting, with quote levels coloured as in the reader.
- Commands for paste as quotation, reflow paragraph, insert signature and insert template.
- Spell check through the system checker over the bridge: misspellings are underlined, and a key lists suggestions.

Vim option (later milestone):

- A Vim binary ships inside the app bundle and inherits the sandbox. `compose.editor = "vim"` selects it; the built-in editor stays the default.
- It runs in a second-pane terminal on a mutt-style draft file: headers, a blank line, then the body.
- A bundled plugin adds address completion, `:QuillSend`, `:QuillPostpone`, `:QuillDiscard` and `:QuillAttach`, and talks to the core over a Vim channel, so the toolbar Send button works too.
- `:wq` hands the draft back for a send, edit, postpone or discard prompt. `:cq` cancels and keeps the draft.
- "Import vimrc" copies a chosen config file into the container. Vim plugins that run external programs will not work in the sandbox; say so in the import dialog.
- It implements the same `Editor` trait and produces the same draft model as the built-in editor.

Message handling:

- The body is written as Markdown and sent as `multipart/alternative` (plain text plus generated HTML). A setting switches to plain text only.
- Attach with the native file panel, a drop onto the window, a pasted path, or an image pasted from the clipboard.
- Warn before sending when the body mentions an attachment and none is present, or when the subject is empty.
- Reply quotes the original with attribution and puts the cursor above or below the quote per `compose.reply_position`. Forward offers inline or as an `.eml` attachment.
- Set `In-Reply-To` and `References` correctly so threads hold together in other clients.
- Identities: several From addresses per account, each with its own signature and Sent folder.
- Drafts autosave locally every few seconds and sync to the server's Drafts folder.
- Several drafts can be open at once; each is a tab.
- Undo send: mail waits in the Outbox for a configurable delay (default 10 seconds).
- Send later: pick a time; the message stays in the Outbox and goes out when Quill is next running after that time. Say so in the UI.
- After a successful send, save to Sent unless the server already does (Gmail, Graph).

## Bells and whistles

P0 is the daily-driver core, P1 makes it better than the alternatives, P2 is the stretch list.

| Priority | Feature | Notes |
| --- | --- | --- |
| P0 | Unified inbox across accounts | Per-account colour marker in the list |
| P0 | Conversation threading | `References` / `In-Reply-To` (JWZ algorithm); Graph `conversationId` as a hint |
| P0 | Full-text search, offline | FTS5 with the query syntax above |
| P0 | Offline queue with undo | All actions work without a connection |
| P0 | Push for new mail | IMAP IDLE; polling elsewhere |
| P0 | macOS notifications | Sender and subject; respects Focus modes |
| P0 | Command palette and contextual help |  |
| P0 | Configurable keys and themes |  |
| P1 | Snooze | Hide until a time, then return to the top |
| P1 | Send later and undo send |  |
| P1 | Templates and snippets | With `{{placeholders}}` |
| P1 | Saved searches as virtual folders | Shown in the sidebar |
| P1 | One-key unsubscribe | `List-Unsubscribe` and one-click POST (RFC 8058) |
| P1 | Calendar invite cards with RSVP |  |
| P1 | Local rules | Match on search syntax; actions: move, label, mark, notify |
| P1 | Bulk actions | Select by search, then act |
| P1 | Headless CLI | `quill list`, `read`, `send`, `search`, `sync`, all with `--json` |
| P1 | Hooks | Pipe a message to an external command; bind it to a key |
| P2 | PGP and S/MIME | Verify first, then sign and encrypt |
| P2 | JMAP backend | Fastmail |
| P2 | macOS Contacts lookup |  |
| P2 | Server-side rules | Sieve, Graph inbox rules |
| P2 | Split view and tabs | Two messages side by side |
| P2 | Linux support |  |
| P2 | Vim compose | Bundled Vim in a second-pane terminal, with vimrc import |

## Config, theming and security

Config is TOML, hot-reloaded on save. A bad edit keeps the last good config and shows the parse error in the status bar.

```toml
[ui]
theme = "dark"
keymap = "vim"            # navigation keys; or "mutt"
reader_width = 0.5
toolbar = true
font = "JetBrains Mono"
font_size = 13

[compose]
editor = "builtin"        # "vim" once the bundled Vim milestone lands
format = "markdown"       # or "plain"
reply_position = "above"  # or "below"
spell_check = true
undo_send_seconds = 10

[privacy]
remote_images = "ask"     # "ask" | "never" | "allowlist"

[[account]]
name = "Work"
type = "graph"            # "imap" | "pop3" | "graph" | "ews"
address = "me@example.com"
oauth_client_id = "..."

[[account]]
name = "Personal"
type = "imap"
address = "me@fastmail.com"
imap = { host = "imap.fastmail.com", port = 993, tls = "implicit" }
smtp = { host = "smtp.fastmail.com", port = 465, tls = "implicit" }
```

Security requirements:

- Passwords and tokens live only in the Keychain. Config, logs and crash reports never contain them.
- TLS is mandatory, using `rustls` with system roots. Plaintext connections need an explicit per-account opt-in and show a permanent warning.
- Certificate errors are never bypassed silently.
- Attachments open only on an explicit key press. Saved files carry the macOS quarantine attribute.
- Logs go to a file, redact addresses and subjects at the default level, and rotate.
- No telemetry and no network calls other than to configured mail servers and OAuth endpoints.

## Testing strategy

| Layer | Method |
| --- | --- |
| MIME and rendering | Unit tests over a corpus of real-world messages: odd charsets, broken multipart, nested forwards, huge headers, hostile escape sequences |
| Store and operation queue | Unit and property tests, including crash-and-replay at every step |
| IMAP, POP3, SMTP | Integration tests against a local mail server in Docker (GreenMail or Stalwart) |
| Graph and EWS | Recorded HTTP fixtures served by `wiremock`, including throttling and expired tokens |
| TUI | Snapshot tests with Ratatui's `TestBackend` and `insta` at several terminal sizes |
| Performance | Benchmarks: cold start under 150 ms, 100,000-message folder scrolls with no dropped frames, search under 50 ms |
| Real accounts | Manual checklist per provider, gated behind environment variables, never in CI |
| Bridge | Schema checks on both sides; contract tests run the core against a scripted fake shell, and the shell against a scripted fake core |
| Shell | XCTest for menu building and pane management; a UI test that launches the app on the demo mailbox, composes a message in the built-in editor, and checks the queued message |
| Terminal surface | Screenshot tests of TerminalKit: text, colour, a Kitty image, resize, and key input including cmd, option and control combinations and input-method composition |

CI runs on a macOS runner: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo deny` for licences and advisories. Tests must never send mail to a real address.

The shell builds and tests with `xcodebuild test` on the same runner, and `swift-format` lints it.

## Milestones

Each milestone ends with a working binary and is one or more pull requests. Do not start a milestone until the previous one meets its acceptance test.

1. **M0, Core skeleton.** Cargo workspace, CI, config loading, store with migrations, three-pane TUI over a fixture backend, command registry, themes, keymap, help overlay. Accept: `quill tui --demo` shows a navigable fake mailbox in Ghostty; snapshot tests pass at 80x24 and 200x60.
2. **M1, Shell spike.** Sandboxed Quill.app with one libghostty surface running the core, the bridge, menus and toolbar built from the command registry, and a Quick Look pane on a sample file. Accept: a menu item runs a core command; `cmd` key combinations reach the core; a Kitty image draws; the archive passes App Store validation and installs through TestFlight.
3. **M2, IMAP read.** Keychain auth, account setup flow, all three TLS modes, folder and envelope sync, bodies on demand, plain-text reader, flags, offline reading. Accept: a 50,000-message account syncs envelopes without freezing the UI; quitting and reopening offline shows the same mail.
4. **M3, Compose and send.** SMTP, the built-in editor, header fields with address completion, reply, forward, attachments out, drafts, Outbox, undo send, operation queue for move, archive and delete. Accept: write, reply and send without touching the mouse; emoji and input-method text survive a round trip; send and archive while offline, and everything reconciles on reconnect with no duplicates.
5. **M4, Rendering and media.** HTML to text, Kitty images, attachment list, Quick Look, AVKit and web view panes, remote content blocking, control-character stripping. Accept: the MIME corpus renders without panics; a video plays in the pane; the web view makes zero network requests while remote content is blocked.
6. **M5, OAuth and Microsoft Graph.** PKCE and device code flows, token refresh, Graph backend and sender, Gmail over XOAUTH2. Accept: a Microsoft 365 mailbox syncs, sends and files mail; tokens refresh unattended for a week.
7. **M6, Daily driver.** Threading, FTS search, unified inbox, IDLE push, notifications with actions, dock badge, `mailto:` handling, command palette, bulk actions, spell check, App Store submission. Accept: all P0 rows in the feature table work; performance budgets hold; the app passes App Review.
8. **M7, POP3.** Client, UIDL tracking, local folders, leave-on-server policies. Accept: repeated polls never duplicate or lose a message under each policy.
9. **M8, P1 features.** Snooze, send later, templates, saved searches, unsubscribe, invites, rules, headless CLI, hooks.
10. **M9, Vim option.** Bundled Vim in a second-pane terminal, the Quill plugin, the draft-file format, vimrc import. Accept: write and send a reply entirely in Vim with `:QuillSend`; the build still passes App Review with Vim inside.
11. **M10, Optional.** EWS for on-prem Exchange, then P2 items in whatever order is wanted.

M1 comes before any mail work on purpose: it retires the libghostty risk while a change of approach is still cheap. If Microsoft 365 is the main account, run a one-day spike of M5's OAuth and Graph sign-in alongside it, for the same reason.

## Working rules for Claude Code

- Read this spec at the start of every session. Work on one milestone at a time and write a short plan before coding.
- Before adding a crate or Swift package, check its current version, last release date, licence and API. Do not rely on memory for APIs.
- Before calling a libghostty or GhosttyKit API, read the pinned version's header and how Ghostty's own macOS app uses it. The API is unstable and recalled signatures will be wrong.
- Only `TerminalKit` imports GhosttyKit. Never bump its version inside an unrelated change.
- The shell holds no mail logic. If Swift code needs to know what a message or folder is, the design is wrong; add a command or a bridge message instead.
- Any bridge change updates `schema/bridge.json`, both sides and the contract tests in the same pull request.
- Keep `DECISIONS.md`. Record every choice that departs from or fills a gap in this spec, with one line of reasoning.
- If the spec turns out to be wrong or contradictory, stop and ask instead of silently diverging.
- No network or disk work on the core's UI task or the shell's main thread. No `unwrap` or `expect` outside tests and startup invariants; no force-unwraps in Swift outside tests.
- Every feature lands with tests. Every bug fix lands with a test that failed before the fix.
- A milestone is done when `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` and `xcodebuild test` pass and its acceptance test has been demonstrated.
- For any visible change, launch the app, check it, and attach a screenshot to the pull request.
- Never commit credentials, tokens, client secrets, signing identities or real mail. Fixtures use invented people at `example.com`.
- Never send mail to a real address from tests or experiments.
- Keep protocol crates free of UI dependencies and the TUI crate free of protocol dependencies.
- macOS-specific behaviour belongs in the shell or in `quill-host`, nowhere else in the core.
- Update `README.md`, the menus and the built-in help whenever a key or command changes.

* The shell is sandboxed from its first commit. Never launch a program outside the app bundle, and never add an entitlement without recording the reason in `DECISIONS.md`.

## Risks and open questions

| Risk | Effect | Mitigation |
| --- | --- | --- |
| A work tenant blocks third-party or unverified apps | Graph sign-in fails with no code fix | Spike early; ask IT for consent, or fall back to IMAP with OAuth if the tenant allows it |
| Google restricts Gmail OAuth scopes for unverified apps | Short-lived tokens or a consent warning | Use an app password, or a personal Cloud project; verify current Google policy during M4 |
| HTML-heavy marketing mail reads poorly as text | Ugly reader output | Browser view on `v`; per-sender "always open in browser" |
| Kitty image placements go stale on scroll or resize | Artifacts, stale images | Clear and re-place on every layout change; screenshot tests in the shell |
| Sync bugs lose or duplicate mail | Loss of trust | Idempotent operations, crash-replay tests, never expunge without a server confirmation |
| Graph has no API for Exchange public folders | Feature gap against Outlook | Out of scope; document it |
| libghostty's embedding API changes or breaks | The shell stops building or rendering after an upgrade | Pin the version, isolate it in TerminalKit, prove it in M1, keep standalone mode working as a fallback |
| Key handling differs between the embedded terminal and the user's usual one | Editor shortcuts misfire in compose | Key-input tests in TerminalKit; shell shortcuts limited to cmd-key combinations |
| App Review rejects the app, or libghostty fails App Store validation | No App Store distribution | Validate in M1; fall back to a notarized direct download from the same code |

Open questions:

- [ ] Which account matters most on day one: Microsoft 365, Gmail or plain IMAP? It sets the milestone order.
- [ ] Is on-prem Exchange actually needed, or only Exchange Online?
- [ ] Open source from the start, and under which licence?
- [ ] Final name. "Quill" is a placeholder and likely collides with existing projects.

* [ ] Bundle Vim or Neovim for the later editor option? Vim is assumed.
* [ ] Offer a direct-download build beside the App Store one?

## Sources

- [Microsoft Starts the Countdown to the End of Exchange Web Services](https://petri.com/microsoft-counts-down-to-end-of-exchange-web-services/) (Petri)
- [Ghostty 1.4.0 release notes](https://ghostty.org/docs/install/release-notes/1-4-0)
- [ratatui-image](https://docs.rs/crate/ratatui-image/latest)
- [himalaya 2.0.0](https://docs.rs/crate/himalaya/2.0.0)

* [libghostty-spm (GhosttyKit Swift package)](https://lakr233.github.io/libghostty-spm/)
* [Ghostty repository and libghostty notes](https://github.com/ghostty-org/ghostty)
