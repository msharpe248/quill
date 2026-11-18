# Where Quill is going

Quill is in early access: version 0.1. This page says what is being worked on, what is known to be missing, and what has not yet met a real server. For what Quill does today, see [the README](../README.md).

Nothing here is a promise or a date.

## Next

- **Quill for agents.** A person's AI agent working their mail through Quill: more commands for a shell (`quill move`, `quill delete`, `quill draft reply`, ...), and Quill as an MCP server while it runs, with a Settings pane that says what agents may do. Off until the user switches it on. The design is in [Quill Agents Spec.md](../Quill%20Agents%20Spec.md); nothing of it is built.
- **A download that simply opens.** A disk image signed with a Developer ID and notarized by Apple, and a Homebrew cask. Until then Quill is built from the source.
- **Google's verification of Quill.** Until it is through, signing in with Google as Quill shows a warning and is open to a limited number of people; see [oauth.md](oauth.md).

## After that

- Rules that do more, and that can be run over mail already here.
- Renaming and removing a server's folders, and folders inside folders.
- Keys for making a pane wider or narrower.

## Known gaps

- **Drafts are kept on this Mac only.** They are not copied to the server's Drafts folder, so a draft begun in Quill is not on your phone.
- **Hooks run in a terminal's Quill only**, not yet in the app.
- **Changing your keys, making themes and choosing the font** are in the app's Settings window; in a terminal they are files to edit.
- **A forward is inline**: the original is quoted, not attached as a message.
- **Exchange (EWS) needs its address given**: there is no Autodiscover.
- **Mail servers that speak only TLS 1.0 or 1.1** are not supported.
- **An Intel Mac has not been tried.**

[DECISIONS.md](../DECISIONS.md) has the full account, feature by feature, under "Not built" and "Not done".

## Not yet tried on the real thing

Quill is tested against mail servers run on the Mac it is built on, and against stand-ins written from the providers' documentation. What has met a real provider so far: reading and sending through an IMAP account with a password, and Gmail signed in to with Google. What has not:

- Microsoft 365 and Outlook.com: the sign-in, and reading and sending through Microsoft Graph.
- Exchange on your own servers (EWS).
- A POP3 provider (it is tested against a POP3 server run locally).
- Unsubscribing from a real mailing list, and answering a real invitation.

If you try one of these, an issue saying how it went, either way, is worth a great deal: <https://github.com/msharpe248/quill/issues>.

## The first plan

Quill was first laid out as eleven milestones, M0 to M10, in [the spec](../Quill%20Terminal%20Email%20Client%20Spec.md). All of them are built, and Quill has gone a good way past them since. They are kept here as history.

| Milestone | What it was |
|---|---|
| M0 | The core: the store, the terminal UI, a demo mailbox |
| M1 | The app: a Mac window around a terminal of Quill's own |
| M2 | Reading mail over IMAP |
| M3 | Writing and sending |
| M4 | HTML mail, pictures and attachments |
| M5 | Signing in with Microsoft or Google; Microsoft Graph |
| M6 | Search, the palette, the mouse, push, conversations, notifications, mail links, spell check |
| M7 | POP3 |
| M8 | Snooze, send later, templates, saved searches, unsubscribe, invitations, rules, commands from a shell, hooks |
| M9 | Writing mail in Vim or another editor |
| M10 | Exchange on your own servers (EWS) |

Built since, outside that plan: a web reading pane; a Settings window; themes, light and dark, and importing themes made for Ghostty; your own editor in the message's pane; changing and removing accounts; folders on this Mac; putting back what was deleted; keys, themes and fonts changed in Settings; panes whose edges are dragged. [DECISIONS.md](../DECISIONS.md) records each, with what was verified and what was not.
