# Quill

Read `Quill Terminal Email Client Spec.md` at the start of every session. It is the source of truth, and its "Working rules for Claude Code" section applies to all work here. Then read `DECISIONS.md` for the choices made since.

The owner asked on 2026-11-15 for unattended work through M10 without waiting between milestones; acceptance steps that need them (real accounts, a developer account, App Review) are recorded in `DECISIONS.md` as not verified and passed over. While they are away, never run Quill against `~/Library/Application Support/Quill` (their real account and mail): use `--demo`, or scratch `--config-dir` and `--data-dir`.

M0 (core skeleton), M1 (shell spike, App Store validation deferred to the end because it needs an Apple Developer account; do not ask for one before then), M2 (IMAP read), M3 (compose and send), M4 (rendering and media) and M7 (POP3) are built and accepted. The rest is built as far as it can be without the owner, and says in `DECISIONS.md` what was not verified:

- M5 (OAuth and Graph): built, tested against stand-ins only. A real sign-in and a real Microsoft 365 or Gmail mailbox need the owner; never contact a real provider while they are away.
- M6 (daily driver): search, the palette, the mouse, IDLE push, bulk actions, conversations, notifications, `mailto:`, spell check and the performance budgets are in. App Store submission waits for the developer account.
- M8 (P1 features): the headless CLI, saved searches, snooze, send later, rules, unsubscribe, templates, invitations and hooks are in. Hooks run only in a terminal: the sandboxed app launches nothing.
- M9 (Vim): the draft file, the plugin, the shell's pane and the core's side are in. Writing a message in the user's own Vim works in a terminal; in the app `editor` under `[compose]` picks `vim` or `nano` (macOS's own, inside the sandbox) or `script` (the user's own Vim, run outside the sandbox by their `editor-command.sh`); all three were seen working. `builtin` is the default. The app has no editor inside it: the owner dropped the bundled Vim on 2026-11-15. `mg` is a terminal's alone: the Mac's crashes at start in the sandbox.
- M10 (EWS): built, tested against a stand-in only.

What each milestone left undone is listed under it in `DECISIONS.md`.

Each milestone had its own branch off the last up to `m4-rendering-media`, which now carries everything since: the milestones overlapped too much to keep apart. `main` is where the owner last accepted the work; leave it for them to move.

Commit finished, checked work as you go. There is no remote.

## Checks

A change is not done until these pass:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
(cd app && swift test && swift format lint --configuration .swift-format -r Sources Tests Package.swift)
```

For anything visible in the app, also build it and look: `app/build-app.sh`, then run it with `--script` and `--dump-screen` (see `README.md`) and read the text and the `.png` it writes.

## This machine

- Rust comes from Homebrew's keg-only `rustup`; if `cargo` is not found, put `/opt/homebrew/opt/rustup/bin` on `PATH`.
- A scratch local mail server for trying things by hand: see how `crates/quill-imap/tests/common/mod.rs` starts Dovecot unprivileged. Its directory needs a short path (sockets). Always stop it and remove its Keychain entry afterwards (`security delete-generic-password -s Quill -a "imap:USER@HOST"`).
- The POP3 tests use the same Dovecot, through `crates/quill-pop3/tests/common/mod.rs`.
- No SMTP server is installed. To try sending by hand, a few lines of Python that accept one SMTP conversation and write the message to a file are enough, with `tls = "none"` on `127.0.0.1`. Never point a scratch account at a real server.
- A Keychain item belongs to the build that made it. After any rebuild, a `quill` or `Quill.app` that reads an item an older build saved makes macOS put a dialog on the owner's screen, and Quill waits on it silently. For a scratch run: delete the scratch item, then save it again (or add the account again) with the build you are about to run, and do not rebuild in between.
- Scratch files for the sandboxed app go in `~/Library/Containers/dev.quill.Quill/Data/tmp/`, which the app can read; point it at them with `--core-env QUILL_CONFIG_DIR=...`.
- The IMAP tests need Dovecot (`brew install dovecot`, already installed here). They start their own server per test; nothing runs in the background.
- `xcodebuild` fails with "failed to load a required plug-in" until the user runs `xcodebuild -runFirstLaunch`. `swift build` and `swift test` do not need it.
- If a build ever stops with "You have not agreed to the Xcode license agreements", that needs the user (`sudo xcodebuild -license`); `export DEVELOPER_DIR=/Library/Developer/CommandLineTools` builds meanwhile.

## Conventions the code already follows

- `update` in `quill-tui/src/state.rs` is the only place UI state changes. Views take `&State`. It returns `Effects` (store requests and host requests) for the run loop to perform.
- The UI task never touches disk. It sends a `DataRequest` to the store worker (`quill-tui/src/data.rs`) and gets `DataEvent`s back.
- `quill-tui` reads no files. `quill-cli` loads config, theme and keys and passes their contents in.
- Every user action is a command in `quill-tui/src/command.rs`. Adding one means: the registry entry, its keys in both presets in `keymap.rs`, a hint if it deserves one, the README tables, and tests. The app's menus and toolbar pick it up from `commands.list` by themselves.
- The shell holds no mail logic. It knows command names, titles and keys. If Swift needs to know what a message is, add a command or a bridge message instead.
- Only `app/Sources/TerminalKit` imports `GhosttyTerminal`. Everything else uses the `TerminalSurface` protocol.
- A bridge change touches `schema/bridge.json`, `schema/examples`, `crates/quill-host/src/bridge.rs` and `app/Sources/Bridge` in the same change.
- Inside the App Sandbox a program cannot change its terminal's modes or size. `TerminalKit` does it from the PTY's master side. Do not "fix" the core to set raw mode itself.
- The reader draws a message's HTML from the raw message (`Store::raw`), laid out for the pane's width by `quill_mime::html::render`. Mail is untrusted: every span is sanitized before drawing, and nothing is fetched.
- Only `quill_core::links::is_openable` decides what may be opened or written into a terminal hyperlink. The shell has the same rule in `Links.isOpenable`; change them together.
- Link text is marked as a hyperlink by the run loop, not by views: the reader returns `LinkPlacement`s. Only underlined link text is marked; keep it that way, or a cell can keep a stale link.
- Where there is typing to do (`Context::Setup`), a key goes to the field first and to a command only if the field has no use for it. A new typing context follows the same rule in `State::on_key`.
- The form that adds an account, and each question Quill asks before going on (a sign-in, an existing account's certificate, a time to snooze until or send at, unsubscribing), is one state machine in the core with two views: drawn in the terminal, or, where `Env::sheets` is set (the app), described to the shell (`State::form` in `state/sheet.rs`, which asks the state in front for its `form`; `form.show`) and shown in the one sheet, whose buttons come back as `form.used` (`State::on_form`, which answers only the form on show at the step it names, and ends where the terminal's keys do). A change to one goes in its state and both views, with the words shared; nothing about it is decided in Swift, which knows fields by kind and never by name. A new one needs a step name no other shares, and its terminal keys switched off while the sheet has it (`State::answered_in_sheet`). What is chosen from with the keyboard is never a sheet: the palette, the pickers (folders, templates, links, attachments, hooks, spellings) and the help.
- What a form shows to be read off and carried elsewhere (a sign-in code, the page to type it at) is a `code` field, a `bridge::Secret` like a password: neither side prints it or writes it to a dump.
- A password crosses the bridge only in `form.used`, under `secrets`, in a type that does not print (`bridge::Secret`, `FormSecret`). The core never sends one back, and nothing logs or dumps one: keep it so when adding a log line or a dump line near the bridge.
- The UI adds accounts through a service it is handed (`AccountsLink`, implemented in `quill-cli/src/service.rs`); it reaches no server, Keychain or config file itself.
- `config.toml` is changed for the user only through `quill-cli/src/config_edit.rs`, which keeps their comments and refuses to write a file Quill could not read back.
- The app's Settings window is described by the core and drawn by the shell. Which settings it has, their panes, labels, allowed values and help are in `quill-tui/src/settings.rs`, by the names `quill-core/src/config.rs` reads the file with. Adding one means: `Setting` (key, section, pane, whether it holds at once), its field in `describe`, its arms in `change` and `Change::written`, `Settings::of`, `Applied` if it holds at once, the README's table, and tests. The shell knows a field by its kind and nothing more; a new kind is a bridge change.
- The Settings window's file work is the accounts service's (`quill-cli/src/settings.rs`): every request reads `config.toml` from disk, a change goes through `config_edit::set`, and the answer is the file read again. Nothing of the config is kept between requests, so a hand edit is never written over, and a file with a mistake in it is never written at all.
- `Env` is changed in one place: the run loop puts `Effects::settings` into it (`Applied::apply`) between one update and the next, for the settings that hold at once. `update` still takes `&Env`, decides, and changes only the state. A harness that runs `update` does the same (`tests/common/mod.rs`).
- The settings window is resized at once, never animated, and its toolbar is never hidden: either runs AppKit's event loop in the middle of a change, and ended in an exception when the panes were put back (see `DECISIONS.md`).
- While a message is written, keys go to `Compose::key` first; a compose command needs a `ctrl` key as well as a `cmd` one, because a terminal keeps `cmd` for itself. Hints come from `Env::hint_key`, which names the key that works where Quill is running.
- The app's Edit menu items Cut, Copy and Select All run the core's `edit.*` commands while those are enabled and act on the focused view otherwise. A shell menu item that should always work needs an explicit target: a disabled item still swallows its key.
- The embedded terminal has no key bindings of its own (`keybind = clear` in `GhosttySurface`). Keep it so, or libghostty takes `cmd` keys before the menus and the core.
- Files reach the core from the shell as `files.dropped` with copies in the app's temporary folder; the core deletes each copy once read. The clipboard is written through `Host::copy_text`.
- A scripted app run must never show a panel, open a browser or touch the clipboard: `DebugScript` replaces `filePicker`, `linkOpener` and `clipboardWriter` with recorders. Add a recorder with anything new that reaches outside the window.
- Images are placed by the run loop, not drawn by views. A view leaves cells empty and returns an `ImagePlacement`.
- UI changes show up in the insta snapshots under `crates/quill-tui/tests/snapshots/`. Regenerate with `INSTA_UPDATE=always cargo test -p quill-tui`, then read the diff.
- What an account signs in to a mail server with comes through `quill_core::Credentials` (async): a Keychain password read off the async threads, or an OAuth token. Do not add a blocking secret lookup to a backend.
- OAuth lives in `quill-auth`; which provider and application an account uses is decided in `quill-cli/src/oauth.rs`. The core never opens a browser: the sign-in address goes to the UI, which hands it to the host.
- The sync engine goes by `Backend::capabilities`, not by account type. A backend with no folders (POP3) has its mail kept in the store: local folders with ids starting `kept:`, local moves and marks, every body downloaded, and nothing deleted from the server that is not filed here in full.
- An action on several messages is one `DataRequest::Many`, and its queued operations are grouped (`Store::group_operations`) so one undo takes them all back.
- Run tests with `--no-fail-fast`: a failure in one test binary otherwise hides stale snapshots in the next.
- A question drawn over the panes (sign-in, a time, unsubscribe, a picker) is a `Context` of its own with its state in an `Option` on `State`; the bridge has fewer contexts, so `app.rs` maps a new one onto the nearest existing one. Movement commands apply only where there is something to move in.
- Things that wait on the user or the network for long (a sign-in, a one-click unsubscribe) run in the accounts service beside its other requests, and report back as `AccountEvent`s.
- New mail is announced from `SyncEvent::Arrived`; rules for arriving mail run in the engine (`quill-sync/src/rules.rs`) and act through the same queue as the user.
- A scripted run of the app never asks for notification permission, never posts a notification and never shows an alert: recorders stand in (see `DebugScript`). It keeps the settings window's last pane for the run only (`TransientSettingsPaneMemory`), as it does the reading divider.
- When two sessions or agents edit the tree at once: per-crate `cargo fmt -p`, never `--all`; stage by path; and a bridge change is committed with both of its sides.
- A program Quill starts (the terminal's Vim, a hook) is an argv built from constants and paths, never a line for a shell, and nothing from a message is ever part of it. What a message says reaches a program only as a file, standard input, or a cleaned one-line environment value.
- The shell starts what is in its bundle and three things outside it, all for writing mail: `/usr/bin/vim` and `/usr/bin/nano` (`editor = "vim"`, `"nano"`), inside the sandbox, and the user's `editor-command.sh` in the app's scripts folder (`editor = "script"`), which macOS runs outside the sandbox through `NSUserUnixTask`. The paths are constants in the shell (`SystemEditor`, `UserEditorScript`); the bridge carries a word, never a path. Do not widen this without the owner.
- A scripted app run pointed at a config with `editor = "script"` runs the owner's real `editor-command.sh` and their own Vim setup. Use `vim` or `nano` for checks unless the script route is what is being checked.
- An editor that is not Vim has none of Quill's commands in it: it is started with its cursor on the body's first line (`pane.editor`'s `line`), asked to leave in its own keys (`PoliteStop`), and Send and Save are off until the message is back.
- Vim is always started with `vim::NO_MODELINES` (and `VimLaunch.noModelines` in the shell): keep the two the same.
- Web reading (`view.web_reading`, `[ui] web_reading`) is a mode of `State`: the reader's state still follows the list, it is just not drawn. The page for the host's reading pane is asked for in one place, `State::follow_selection`, at the end of every `update`, only once the run loop has said the cursor rested (`Action::SelectionSettled`), and a page is shown only while `Follow::asked` still names it. Nothing else asks for a reading page, except `i` and `I` asking again for the one on show.
- A choice made in the UI that outlasts the run goes to the accounts service as `AccountRequest::Remember`, which writes `config.toml` through `config_edit`. The demo mailbox has a service that does only that (`service::spawn_remembering`).
- A command that is on or off says so through `State::is_on`; the bridge carries it as `checked` in `state.update` and the shell checks its menu item. The shell never works out a mode from anything else.
- Fixtures use invented people at example.com. Never real mail, never real addresses.
