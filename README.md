# Quill

A keyboard-first mail client for macOS: a native app whose main surface is a terminal it creates itself. A Swift shell supplies the window, menus and media viewers; a Rust core draws the mail UI inside an embedded terminal and does all the mail work.

The full design is in [the spec](Quill%20Terminal%20Email%20Client%20Spec.md). Choices that depart from it or fill a gap are in [DECISIONS.md](DECISIONS.md).

## Status

Milestone 0, the core skeleton, is in place: the Rust workspace, the local store with migrations, config loading, themes, keymaps, and a three-pane terminal UI over a demo mailbox.

Milestone 1, the app shell, is in place too. `Quill.app` runs the core in its own libghostty terminal inside the App Sandbox, builds its menus and toolbar from the core's command list, shows files in a Quick Look pane beside the terminal, and draws images inline with the Kitty graphics protocol. App Store validation is deferred until there is a developer account to sign with.

Milestone 2, reading mail over IMAP, is built. Add an account from inside Quill (`A`): type its address, confirm the server Quill finds, give the password, and its mail syncs into the local store and reads offline. HTML mail is drawn as styled text: headings, lists, emphasis, quotes and simple tables, with links numbered and listed at the end of the message. `t` switches a message between its HTML and its plain text when it came as both. `o` lists a message's links to open one in your browser, and in the app or a terminal that supports hyperlinks, cmd-click on a link's text opens it too. Quill opens only `http`, `https` and `mailto` links.

Milestone 3, writing and sending, is built. `c` writes a new message, `r`, `R` and `f` reply, reply to all and forward, and the message is written in the reader's place with an editor that follows macOS text keys. Mail is sent over SMTP from an Outbox that holds it for ten seconds, in which `U` takes it back. `e`, `d` and `m` archive, delete and move, also undoable, and all of it works offline and is carried out when the server is next reachable.

Microsoft accounts with OAuth arrive in M5.

## Try it

You need a stable Rust toolchain (1.98 or later; `rust-toolchain.toml` selects it). Standalone use is meant for Ghostty or Kitty, whose keyboard protocol Quill uses when it is offered; other terminals work with fewer key combinations.

```sh
cargo run --release -- tui --demo
```

`--demo` opens an invented mailbox held in memory. It reads your config, theme and keys if you have them, and writes nothing.

Without `--demo`, `quill tui` opens your own mail. The first time, with no accounts yet, it opens the form for adding one.

## The app

```sh
app/build-app.sh
open target/app/Quill.app
```

The script builds the Rust core and the Swift shell, assembles `target/app/Quill.app`, and signs it ad hoc with the App Sandbox on. The app opens your own mail, and the form for adding an account when there is none; `Quill.app/Contents/MacOS/Quill --demo` opens the demo mailbox instead. The app keeps its config and mail in its own container, apart from those of `quill` run in a terminal.

Around the terminal the app adds a menu bar and toolbar built from the core's commands, the mailbox name and unread count in the title bar, the unread count on the Dock icon, and a second pane beside the terminal for looking at attachments, playing sound and video, and showing a message's HTML. `esc` closes the pane, and `cmd-]` moves focus between the panes.

`SANDBOX=0 app/build-app.sh` signs without the sandbox, to tell a sandbox denial from an ordinary bug.

## Adding an account

Press `A` (or File, Add an Account in the app). Quill asks for:

1. **The address.** Quill looks for the mail server: providers it knows by name, then the domain's DNS records, then the names servers usually have. Nothing is asked of anyone but the domain's own DNS and servers.
2. **The servers**, as found, to confirm or correct: the one mail is read from (its name, port, security and the name to sign in with) and, under "Send via", the one it is sent through. `tab` moves between the fields and `←`/`→` changes the security. Clear "Send via" to add an account that only reads.
3. **The password.** Quill signs in to both servers to check it, and sends nothing. Only when that works is anything saved: the account in `config.toml`, the password in your Keychain and nowhere else. If reading works and sending does not, the form says so and goes back to the sending server.

`enter` goes on, `esc` goes back a step. Many providers (Gmail, iCloud, Fastmail, Yahoo) want an app password here, not your normal one; the form says so when it recognises the provider. Microsoft accounts (Microsoft 365, Outlook.com) do not accept a password at all: see the next section.

If the server's certificate is not one your Mac trusts, as with a self-signed certificate on a home server, Quill sends it nothing and shows you the certificate: who it was issued to and by, its dates and its SHA-256 fingerprint. `r` rejects it, `o` trusts it until Quill quits, `p` trusts that one certificate from now on by writing its fingerprint into the config. The same question is asked if an existing account's certificate stops being trusted.

An account is a few lines in `config.toml`, and can be written by hand:

```toml
[[account]]
name = "Personal"
type = "imap"
address = "you@example.com"
imap = { host = "imap.example.com", port = 993, tls = "implicit" }
```

```sh
quill account password Personal   # asks for the password, saves it in the Keychain
```

`tls` is `"implicit"` (the default, port 993), `"starttls"` (port 143) or `"none"`. With `"starttls"`, a server that will not upgrade is refused, never used in the clear. `"none"` sends your password and mail unencrypted and says so every time Quill starts.

Optional keys inside `imap = { ... }`: `username`, when you do not sign in with your address, and `fingerprint`, the SHA-256 of a self-signed server's certificate, to accept that one certificate.

`quill account list` shows each account and whether it has a saved password. `quill account forget NAME` removes one. Passwords are only ever in the Keychain: not in the config, the database or the logs.

A Keychain item belongs to the program that made it. If you build Quill from source and then rebuild it, macOS will ask whether the new `quill` may read the password the old one saved; "Always Allow" settles it for that build. Until you answer, Quill waits and no mail moves.

## Microsoft 365 and Gmail: signing in with the provider

A Microsoft 365 or Outlook.com mailbox is read and sent through Microsoft Graph, and signs in with OAuth: in your browser, at Microsoft, with Quill never seeing the password. Gmail can sign in the same way, in place of an app password.

Both need an application id that you register with the provider once. [docs/oauth.md](docs/oauth.md) goes through it step by step. The account is then a few lines in `config.toml`:

```toml
[[account]]
name = "Work"
type = "graph"
address = "you@example.com"
oauth_client_id = "the id of the application you registered"
```

Start Quill and press `S`, or choose "Sign in to an account" from the palette or the File menu. Your browser opens at the provider; sign in there and come back. From a terminal it is `quill account sign-in Work`, and over SSH Quill shows a code to type in on any device instead of opening a browser (Microsoft only).

The sign-in is kept in your Keychain. `quill account list` shows which accounts are signed in, and `quill account forget NAME` removes one.

This part has not met the real services yet: it is tested against stand-ins on this Mac only.

## Exchange on your own servers

An Exchange server that is not Exchange Online is reached through Exchange Web Services. (A Microsoft 365 mailbox is a `graph` account, above.)

```toml
[[account]]
name = "Office"
type = "ews"
address = "you@example.com"
ews = { url = "https://mail.example.com/EWS/Exchange.asmx", username = "EXAMPLE\\you" }
```

```sh
quill account password Office
```

`username` is what you sign in with, often `DOMAIN\user`; without it your address is used. The address of the service is given in full: Quill does not look it up. The server must allow Basic authentication for EWS, over `https`; one that offers only NTLM or Kerberos is not supported, and Quill says so. Exchange 2013 SP1 or later.

This is the least tried part of Quill: it is tested against a stand-in only, never a real Exchange server.

## POP3 accounts

A POP3 server only hands mail over. It has no folders and keeps no marks, so for these accounts Quill keeps the mail on this Mac: every message is downloaded whole, and Sent, Archive and Trash are folders that exist only here. Reading, filing, starring and searching work as for any account.

```toml
[[account]]
name = "Old Mail"
type = "pop3"
address = "you@example.com"
pop3 = { host = "pop.example.com", port = 995, tls = "implicit" }
smtp = { host = "smtp.example.com", port = 465, tls = "implicit" }
delete_from_server = "never"
```

```sh
quill account password "Old Mail"
```

`delete_from_server` says what becomes of the server's copy:

| Value | Meaning |
|---|---|
| `"never"` | Left on the server, so other mail programs see the same mail. The default. |
| `"downloaded"` | Deleted once Quill has the whole message. |
| a number, such as `14` | Deleted that many days after Quill first saw it. |

Whatever it says, nothing is deleted from the server until the whole message is safely here, and a message another program deletes from the server stays in Quill. Deleting a message for good in Quill deletes the server's copy too.

With anything but `"never"`, the mail on this Mac is the only copy. Back it up: it is in `~/Library/Application Support/Quill/data` (or the app's container, for the app).

`tls` is as for IMAP: `"implicit"` (port 995), `"starttls"` (port 110) or `"none"`.

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
| `c` | Write a new message | `r` / `R` | Reply, reply to all |
| `f` | Forward | `e` | Archive |
| `d` | Delete (to Trash; from Trash, for good) | `m` | Move to a folder |
| `U` | Undo the last move, deletion or send | `A` | Add an account |
| `S` | Sign in to an account (Microsoft, Google) | `T` | Group or ungroup conversations |
| `b` | Snooze until a time | | |
| `/` | Search | `a` | List a message's attachments |
| `v` | View the message as a web page | `B` | Open that page in the browser |
| `i` | Load remote pictures for this message | `I` | Always load them from this sender |
| `x` | Select or unselect, and move on | `V` | Start or end a range |
| `X` | Unsubscribe from a mailing list | | |
| `y` / `n` | Accept, decline an invitation | `M` | Answer an invitation with maybe |

Arrow keys, `home`, `end`, `pageup` and `pagedown` work too. `ctrl-d` also pages down and `ctrl-c` also quits.

Set `keymap = "mutt"` in the config for mutt-flavoured keys.

`:` or `ctrl-k` opens the command palette: type a few letters of a command or a mailbox and press `enter`. Each command is listed with its key.

The mouse works too. A click selects, a second click on what is selected opens it, and the wheel scrolls whatever is under the pointer. Links in a message and the line naming its files can be clicked, and in a message being written a click places the cursor and a drag selects. While Quill has the mouse, selecting text the terminal's own way needs `shift` held; `mouse = false` under `[ui]` gives the mouse back to the terminal.

In the message list, `>` marks the cursor, `+` a selected message, `*` an unread one, `r` one you have answered, `!` a starred one and `@` one with attachments.

The messages of a conversation are listed together, where the newest of them would be, in the order they were written; a reply hangs from the message above it with `└`. `T` switches that off and on, and `threads = false` under `[ui]` makes off the default. What a search finds is always listed message by message.

`b` snoozes a message: it leaves its folder until the time you type, then comes back to the top as if it had just arrived, with a notification in the app. Times are typed as you would say them: `30m`, `2h`, `3d`, `tonight`, `tomorrow`, `tomorrow 9`, `fri 14:30`, `2026-12-01 9:00`. Snoozed mail waits in "Snoozed" in the sidebar; `b` there with `now` brings it straight back. Snoozing is kept on this Mac only: on the server, and in other mail programs, the message stays where it was.

While a question is on screen (a certificate to accept, a sign-in, a time to type, whether to unsubscribe), the keys and the wheel are the question's. Nothing behind it moves until it is answered or put away with `esc`.

To act on several messages at once, select them first: `x` on each, or `V`, move, and `V` again for a run of them; `ctrl-a` selects everything listed. Archive, delete, move, read and star then apply to all of them, and one `U` takes the whole lot back. `esc` lets the selection go. From the reader, a command is always about the message being read.

## HTML mail, and what it is allowed to do

The reader shows a message's HTML as styled text, and fetches nothing. For the message as its sender laid it out, `v` opens it as a web page in a pane beside the reader (in a terminal, in your browser; `B` does that from the app too).

That page is never the message's own HTML. Quill writes a copy with scripts, forms, frames and anything that would fetch taken out, and tells the page it may load nothing from the web. Pictures the message carries with it are shown. Pictures it only points at are not: loading one tells the sender that you opened the message, when, and from where. Where one would be, the page shows its description, and the status line says how many were left out.

`i` loads them for the message in front of you. `I` says mail from that sender may always load them. In the config:

```toml
[privacy]
remote_images = "ask"     # or "never": i and I do nothing
```

A link whose text names one site and whose target is another is flagged in the reader, on the line below it.

## Unsubscribing from a mailing list

Mail from a list usually says how to leave it, in a `List-Unsubscribe` header that mail programs read and people never see. When the message in the reader has one that Quill will use, the reader's top border says `X unsubscribes`.

`X` asks first. The question names who sent the message and says exactly what `enter` will do, which is one of three things, in this order of preference:

| The list gives | `enter` does |
|---|---|
| An `https` address, and `List-Unsubscribe-Post: List-Unsubscribe=One-Click` | Quill tells the list's server itself (RFC 8058): one POST to that address, saying `List-Unsubscribe=One-Click` and nothing else. The status line says whether the server took it |
| A `mailto:` address | A message to that address is started, from the account the list wrote to. Nothing is sent until you send it |
| An `https` address alone | Your browser opens that page. Quill sends nothing |

`esc` does nothing at all, and no other key answers the question.

The header is a stranger's, so it is held to rules. Only `https` and `mailto:` addresses are taken; plain `http` is ignored. A web address must name its server plainly: one with a user name in front of the host, a bare number or `localhost` for a host, a backslash, or characters outside printable ASCII is dropped whole, and so is one longer than 2048 characters. The one-click request carries no cookies and no sign-in, follows no redirect, and gives up after 15 seconds. Its answer is not read beyond whether the server said yes. A message to the list goes to the one address the question named: other recipients and copies the header asks for are left off, and a missing subject becomes `unsubscribe`.

Quill does not check the message's DKIM signature, which RFC 8058 asks for before a one-click request. The question on screen, naming the server, stands in for it: read the name before pressing `enter`. In the demo mailbox, which has nothing to make requests with, one click is not offered.

## Invitations

A message that carries a calendar invitation shows it as a card between its headers and its text: what the event is, when (in your own time zone), where, who is asking, and how many of those invited have said what. An all-day event is shown as its date, and one that repeats says so; how often is not worked out. A time given in a zone Quill does not know, such as one of Windows' names, is shown as the clock reads there, with the zone's name beside it as the invitation wrote it.

The card's last line says what kind of thing it is: an invitation that asks for your answer, a cancellation, someone's answer to an invitation of yours, or an event sent for information.

| Key | Action |
|---|---|
| `y` | Accept |
| `n` | Decline |
| `M` | Maybe (a tentative yes) |

The keys work only on an invitation that asks you for an answer, from the reader or with the cursor on the message in the list. Each sends the organizer a short message ("Accepted: Offsite") with the calendar's own form of the answer attached as `invite.ics`. Like any mail it waits in the Outbox for `undo_send_seconds`, and `U` in that time takes it back; it is then a draft, to throw away with `d`.

Quill is not a calendar. It adds nothing to one, does not remember what you answered, and does not tell you about clashes. The calendar file itself is among the message's attachments (`a`), to open in Calendar.

Not yet checked against Exchange or Google Calendar: whether they take an answer that arrives as an attached calendar file and mark you as coming, or want it as a part of the message's body.

## New mail, and mail links

In the app, new mail in an inbox is announced with a notification: who it is from and its subject, with buttons to archive it, mark it read or reply. Clicking it goes to the message. macOS asks whether Quill may show notifications the first time there is one to show. `notifications = false` under `[ui]` turns them off.

Quill can be your default mail program: choose it in Mail, Settings, General, "Default email reader". A mail link clicked anywhere then starts a new message in Quill, to the people the link names. A link can fill in recipients, a subject and some opening words, and nothing else.

## Searching

`/` opens a search box in the title of the message list, and the list shows what is found as you type, from every account and folder. `enter` moves to the results, which are a list like any other: open, reply, archive. `/` again changes the search; `esc` leaves it.

| Typed | Finds mail |
|---|---|
| `roadmap q4` | with both words, in the subject, the people or the text |
| `"draft roadmap"` | with those words together |
| `from:ana` `to:sam` `subject:invoice` | with the word in that place. `from:"Ana Ruiz"` for a phrase |
| `is:unread` `is:read` `is:starred` `is:answered` | in that state |
| `has:attachment` | with files attached |
| `after:2026-10-01` `before:2026-11` | dated on or after, or before, a day, a month or a year |
| `in:work` `in:sent` `in:work/inbox` | in an account, a folder, or a folder of an account |

Words match whatever their case or accents, and the word you are still typing matches words that start with it. A message's text is searchable once it has been downloaded; its subject and people are from the moment it is listed. The search runs against the mail on this Mac; asking the server as well is not built yet.

### Saved searches

A search you make often can live in the sidebar, as a mailbox of whatever it finds:

```toml
[[search]]
name = "Unread from Ana"
query = "from:ana is:unread"

[[search]]
name = "Invoices"
query = "invoice has:attachment"
```

They are listed under "Searches". Moving onto one runs it, `esc` goes back to the mailbox you came from, and the command palette finds them by name.

## Attachments and pictures

A message's files are named in its header, and `a` lists them with what each is and how big.

| Key in the list | Action |
|---|---|
| `enter` | Look at the file: Quick Look beside the terminal in the app, with `←`/`→` stepping through the message's other files. Sound and video play in a player pane |
| `O` | Open it in the application your Mac opens that kind of file with |
| `s` | Save it. The app asks where; in a terminal it goes to Downloads, never over a file already there |
| `esc` | Close the list. In the app, `esc` in a pane closes the pane |

The first picture among a message's files is drawn above its text, in the app and in terminals that draw pictures (Ghostty, Kitty). In the app that includes kinds only macOS can read: a HEIC photo, the first page of a PDF, a frame of a video.

Files are written to a private folder that is removed when Quill quits, under names cleaned of anything that could place them elsewhere or disguise what they are, and marked as downloaded, so macOS checks them before running anything. Nothing is opened until you press a key for it.

## Writing mail

`c` opens a new message where the reader was; `r`, `R` and `f` start a reply, a reply to all, or a forward of the message under the cursor. `ctrl-z` gives the message the whole window and back.

| Key | Action |
|---|---|
| `tab` / `shift-tab` | Next, previous field. `enter` and the arrows move between the header fields too |
| `cmd-enter` or `ctrl-x` | Send |
| `cmd-s` or `ctrl-s` | Save as a draft and keep writing |
| `cmd-shift-a` or `ctrl-t` | Attach a file. The app shows its file panel; a terminal asks for the path, typed or pasted |
| `cmd-shift-b` or `ctrl-o` | Add blind copies (Bcc) |
| `cmd-shift-t` or `ctrl-r` | Insert a template, when there are any |
| `cmd-shift-g` or `ctrl-g` | Other spellings of the misspelt word at the cursor (in the app) |
| `cmd-;` or `ctrl-y` | Go to the next misspelt word (in the app) |
| `esc` | Close. What you wrote is kept in Drafts |

The `cmd` keys work in the app; a terminal keeps `cmd` for itself, so there the `ctrl` keys are the ones shown.

- **Addresses.** As you type in To, Cc or Bcc, people you have had mail from or written to are offered under the field. `↓`/`↑` choose, `enter` or `tab` takes one, `esc` declines. Separate several with commas.
- **Files.** In the app, files dropped on the window are attached to the message being written, or start a new one.
- **From.** With more than one account, `shift-tab` up to From and `←`/`→` choose which one sends.
- **The body** follows macOS text keys: arrows, `opt-←`/`opt-→` by word, `cmd-←`/`cmd-→` to the ends of the line, `cmd-↑`/`cmd-↓` to the ends of the message, shift with any of them to select, `cmd-a`, `cmd-c`, `cmd-x`, `cmd-z` and `cmd-shift-z` to undo and redo. `ctrl-a`, `ctrl-e`, `ctrl-k`, `ctrl-w` and the other Emacs-style keys of macOS text fields work as well, and are the ones to use in a terminal.
- **Markdown.** The body is written as Markdown and sent as plain text with an HTML version beside it. `format = "plain"` under `[compose]` sends plain text only.
- **Before it goes**, Quill asks once if the subject is empty or the message mentions an attachment and has none. Sending again sends it as it is.
- **Undo send.** A sent message waits in the Outbox for `undo_send_seconds` (10 by default); `U` in that time makes it a draft again. Set it to 0 to send at once.
- **Drafts and the Outbox** appear at the top of the sidebar when there is something in them. `enter` on a draft, or on mail still in the Outbox, opens it to carry on writing; `d` throws a draft away. What you are writing is saved every few seconds and when Quill quits.
- **Mail a server refuses** stays in the Outbox and says why when you open it. Mail that could not be sent because the server was unreachable is tried again by itself.

To send, an account needs an SMTP server. Adding an account inside Quill fills it in when it can be found; by hand it is one more line:

```toml
[[account]]
name = "Personal"
type = "imap"
address = "you@example.com"
full_name = "Your Name"                 # optional: how your name appears on mail you send
signature = "Your Name\nExample Ltd"    # optional: added to new messages
imap = { host = "imap.example.com", port = 993, tls = "implicit" }
smtp = { host = "smtp.example.com", port = 465, tls = "implicit" }
```

The SMTP server is signed in to with the same name and password as the IMAP server unless `smtp = { ... }` has a `username` of its own.

### Spelling

In the app, the body of a message being written is checked by the Mac's own spelling checker. A misspelt word is underlined, and coloured as an error where there is colour. With the cursor on such a word or just after it, `ctrl-g` (or `cmd-shift-g`) lists other spellings: `j`/`k` or the arrows move, `enter` puts the chosen one in the word's place, `esc` leaves the word alone. `cmd-z` takes a replacement back in one step. `ctrl-y` (or `cmd-;`) goes to the next misspelt word, round to the first after the last.

The check runs when you stop typing for a moment, not at each key. Only the body is ever handed to the checker, never the recipients or the subject, and never mail you are reading. Quoted lines are not marked. A body of more than 100,000 bytes is not checked.

```toml
[compose]
spell_check = false    # true by default
```

In a terminal there is no checker to ask, and nothing is underlined.

### Sending later

`ctrl-l` (or `cmd-L` in the app) in a message being written asks when to send it, in the same words as a snooze: `tomorrow 8`, `mon`, `2h`. The message waits in the Outbox, which shows when it will go, and `U` or opening it from the Outbox takes it back. It is sent at that time if Quill is running then, and otherwise when Quill next runs: nothing on a server holds it.

### Templates

Text you write often can be kept in files and put into a message with a key. Each `.md` or `.txt` file in a `templates` folder beside `config.toml` is a template, named after the file without its ending. A first line `Subject: ...` with an empty line under it is the subject the template brings; the rest is its text.

```
Subject: Following up on {{topic}}

Hi {{first_name}},

Thanks for meeting on {{date}}. As promised: {{what}}

{{from_name}}
```

`ctrl-r` (or `cmd-shift-t` in the app) in a message being written lists the templates. Typing narrows the list, `↓`/`↑` choose, `enter` puts the chosen one into the body at the cursor, in place of anything selected there, and `esc` closes the list. A message with no subject yet takes the template's. One `cmd-z` takes the text out again.

Words in double braces are placeholders. Quill fills the ones it knows:

| Placeholder | Becomes |
|---|---|
| `{{to}}` | The first recipient's address |
| `{{to_name}}` | Their name, or the part of their address before the `@` when they are written without one |
| `{{first_name}}` | The first word of their name. Not guessed from an address |
| `{{from_name}}` | Your `full_name` on the account the message is from |
| `{{date}}` | Today, as `14 November 2026`, in your time zone |
| `{{subject}}` | The message's subject |

Any other placeholder, and one of these that has nothing to fill it yet (no recipient, no `full_name`), stays as written, and the first of them is selected in the body so that what you type next replaces it. Braces that are not a placeholder (`{{ two words }}`, `{{#if}}`, a single `{`) are left alone. Filling is substitution and nothing more: a template cannot run a command, include another file or fetch anything, and a value that is put in is not read again for placeholders of its own.

Templates are read once, when Quill starts. A file over 64 KiB, one that is not UTF-8 text, and anything past the first 200 by name are left out, with a notice saying which.

## Rules for arriving mail

A rule does something with mail as it arrives in an inbox: files it, marks it, or keeps quiet about it. Rules are written in the config and asked in order.

```toml
[[rule]]
match = "from:buildbot"          # the search box's words
move_to = "archive"              # a folder's name, or archive, trash, junk
mark_read = true

[[rule]]
name = "Invoices"
match = "subject:invoice has:attachment"
account = "Work"                 # only this account's mail
star = true

[[rule]]
match = "from:newsletter"
notify = false                   # stays in the inbox, with no notification
```

A rule needs `match` and at least one thing to do. Every rule that matches marks the message; the first that files it is the one that files it. Mail a rule files out of the inbox is not announced unless the rule says `notify = true`.

What a rule does is done on the server as well, the same way as when you do it yourself. Rules see who a message is from and to, its subject, its marks and whether it has attachments; the words of its body are not known when it arrives. They apply to mail arriving from now on, not to mail already there, and only while Quill is running or when `quill sync` runs.

## From a shell

Quill's mail can be read and sent without its screen, for scripts and for a quick look. Each command takes `--json`.

```sh
quill list                         # the unified inbox, newest first
quill list --in work/sent --limit 10
quill list --unread --json
quill search from:ana is:unread "exact phrase"
quill read 4127                    # by the id list and search print
quill read 4127 --raw > message.eml
echo "Running late." | quill send --to ana.ruiz@example.com --subject "Today"
quill sync                         # fetch, carry out what is queued, send, stop
```

`list`, `search` and `read` look at the mail already on this Mac and reach no server. `send` checks the message as the compose screen would and puts it in the Outbox; it goes at the next sync, from the app if it is running or from `quill sync`. `--account NAME` chooses which account sends, and `--attach FILE` attaches. `quill sync` exits with an error if anything could not be done, and says what.

JSON times are RFC 3339 in UTC. What mail says is printed safely: control characters and escape sequences never reach your terminal, and in JSON they are escaped as JSON escapes them.

## Configuration

Quill looks in `~/Library/Application Support/Quill/config/`:

| File | Purpose |
|---|---|
| `config.toml` | Settings. See the example in the spec. |
| `keys.toml` | Key overrides, on top of the chosen preset. |
| `themes/NAME.toml` | Your own themes, selected with `theme = "NAME"`. |
| `templates/NAME.md`, `templates/NAME.txt` | Text to put into messages. See "Templates". |

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
| `nav.next`, `nav.prev` | Move down or up; scroll in the reader. Between the fields of the account form |
| `nav.top`, `nav.bottom` | Jump to the first or last |
| `nav.page_down`, `nav.page_up` | Move by a screenful |
| `nav.open` | Open the message or mailbox |
| `nav.back` | Go back, close help, dismiss a notice |
| `focus.sidebar`, `focus.list`, `focus.reader` | Focus a pane |
| `focus.next`, `focus.prev` | Cycle the panes |
| `view.zoom` | Zoom the focused pane |
| `view.threads` | List conversations together, or every message by itself |
| `message.toggle_unread`, `message.toggle_star` | Change a message's flags |
| `message.toggle_html` | Switch a message between its HTML and its plain text |
| `message.open_link` | List the message's links, to open one in the browser |
| `message.archive`, `message.delete`, `message.move` | File the message, or every selected one |
| `message.snooze` | Put the message aside until a time |
| `select.toggle`, `select.range`, `select.all` | Select messages to act on together |
| `message.compose` | Write a new message |
| `message.reply`, `message.reply_all`, `message.forward` | Answer or pass on the message |
| `compose.send`, `compose.save` | Send the message being written, or save it as a draft |
| `compose.send_later` | Send the message being written at a time you type |
| `compose.attach`, `compose.bcc` | Attach a file; add blind copies |
| `compose.template` | Put a template into the message being written |
| `compose.spell_suggest`, `compose.spell_next` | List other spellings of the misspelt word at the cursor; go to the next misspelt word |
| `edit.undo` | Undo the last move, deletion or send |
| `search.start` | Search |
| `palette.open` | The command palette |
| `message.attachments` | List the message's attachments |
| `message.view_html`, `message.open_in_browser` | Show the message as a web page, beside the reader or in the browser |
| `message.load_remote`, `sender.always_remote` | Let this message, or this sender's mail, load remote pictures |
| `message.unsubscribe` | Unsubscribe from the list that sent the message, after asking |
| `invite.accept`, `invite.decline`, `invite.tentative` | Answer the invitation the message carries: yes, no or maybe |
| `attachment.save`, `attachment.open` | Save the chosen attachment, or open it in its application |
| `edit.copy`, `edit.cut`, `edit.select_all` | The app's Edit menu, while a message is being written |
| `account.add` | Add an account |
| `account.sign_in` | Sign an account in to Microsoft or Google |
| `help.toggle` | Show or hide help |
| `app.quit` | Quit |

### Disk space

Messages you have opened, and recent ones, are kept on this Mac so they read offline. They may take up to 5 GB; past that the oldest copies are let go and fetched again when you next open them. The list of your mail and its text for searching are always kept.

Opening a message whose copy was let go shows its text at once, with a line above it saying the rest is being fetched; its layout, pictures, attachments and any invitation appear when the copy arrives. If it cannot be fetched (no connection, say), the text stays and the line says why. Opening the message again tries again.

```toml
[storage]
cache_gb = 5      # 0 for no limit
```

Mail of a POP3 account is never cleared this way: this Mac may have the only copy.

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
