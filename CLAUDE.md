# Quill

Read `Quill Terminal Email Client Spec.md` at the start of every session. It is the source of truth, and its "Working rules for Claude Code" section applies to all work here. Then read `DECISIONS.md` for the choices made since.

The owner asked on 2026-11-15 for unattended work through M10 without waiting between milestones; acceptance steps that need them (real accounts, a developer account, App Review) are recorded in `DECISIONS.md` as not verified and passed over. While they are away, never run Quill against `~/Library/Application Support/Quill` (their real account and mail): use `--demo`, or scratch `--config-dir` and `--data-dir`.

M0 (core skeleton), M1 (shell spike, App Store validation deferred to the end because it needs an Apple Developer account; do not ask for one before then) and M2 (IMAP read) are built. HTML-to-styled-text from M4 was pulled forward at the owner's request (`quill_mime::html`). M3 (compose and send) is next; `quill-smtp` and `quill-pop3` exist as crates ahead of the UI that uses them.

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
- A scratch local mail server for trying things by hand: see how `crates/quill-imap/tests/common/mod.rs` starts Dovecot unprivileged. Always stop it and remove its Keychain entry afterwards.
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
- The UI adds accounts through a service it is handed (`AccountsLink`, implemented in `quill-cli/src/service.rs`); it reaches no server, Keychain or config file itself.
- `config.toml` is changed for the user only through `quill-cli/src/config_edit.rs`, which keeps their comments and refuses to write a file Quill could not read back.
- Images are placed by the run loop, not drawn by views. A view leaves cells empty and returns an `ImagePlacement`.
- UI changes show up in the insta snapshots under `crates/quill-tui/tests/snapshots/`. Regenerate with `INSTA_UPDATE=always cargo test -p quill-tui`, then read the diff.
- Fixtures use invented people at example.com. Never real mail, never real addresses.
