# Quill rules

Proposed on 2026-11-18, from the owner's list and their answers the same day. Nothing here is built yet. What is settled is listed near the end, and what is still open after it. Once built, `DECISIONS.md` says what Quill does, and wins where the two differ.

## What it is for

Mail that sorts itself. A rule says which mail it is about and what to do with it: file it in a folder, delete it, mark it. Rules are written in a file in a few plain lines, can be tried before they are trusted, run on mail as it comes in, and can be switched off with one key.

## What there is already

Rules exist today as `[[rule]]` in `config.toml` (see "Rules for arriving mail" in `docs/manual.md`): a search in the search box's words, and `move_to`, `mark_read`, `star`, `notify`. They are asked about mail new to an inbox, by the sync engine, and what they do reaches the server through the same queue as what the user does.

What they lack, and this adds:

- the words of a message's body;
- deleting, said as deleting;
- a switch for all of them, and for each;
- running on mail that is already here, within a limit;
- a preview of what a rule would match;
- a file of their own, read again without restarting;
- a record of what they did, and a way to take it back;
- a menu.

## Principles

1. **A rule is a search and something to do.** What a rule matches is what the search box would find with the same words. There is one grammar, and anything added to it for rules is added to search.
2. **Nothing is lost for good.** A rule's delete is a move to the Trash, from which Put Back returns it. No rule deletes permanently, sends mail, or runs a program.
3. **Bounded.** Rules are asked about inboxes only. A run over mail already here stops at the rule's look-back, asks before it changes more than a set number of messages, and without being told to never asks about a message twice.
4. **Written down.** Everything a rule did is in the log, by the rule's name, and can be taken back.
5. **The user's alone.** No agent can read, change, switch or run rules. That is already so and stays so.
6. **Tried before trusted.** Running a rule by hand always shows what it would match first.

## The file

`rules.toml`, beside `config.toml` and `keys.toml`.

```toml
[[rule]]
name = "Receipts"
match = "from:orders@example.com subject:receipt"
move_to = "Receipts"
look_back = "1w"

[[rule]]
name = "Build mail"
match = ["from:buildbot", "list:builds.example.com"]   # either
move_to = "archive"
mark_read = true

[[rule]]
name = "Promotions"
match = "body:unsubscribe from:deals"
delete = true
enabled = false
```

| Key | Means |
|---|---|
| `name` | What the rule is called: in the menu's list, the log, notices and `quill rules`. Needed, and no two the same |
| `match` | The search box's words. A list means any one of them |
| `account` | Only this account's mail |
| `move_to` | A folder of the message's account, by its name, or `archive`, `trash`, `junk` |
| `delete` | `true`: to the Trash. Not with `move_to` |
| `mark_read`, `star` | Marks |
| `notify` | As now: whether what it matches is announced |
| `enabled` | `false` switches this rule off. Left out, it is on |
| `look_back` | How far back a run over mail already here goes: `1d`, `1w`, `30d`. Left out, the setting's (one week) |

A rule needs a name, something to match and something to do. `delete` with `move_to`, a name used twice, or a look-back Quill cannot read is a mistake.

**A mistake in the file switches no rule on.** Quill says which rule and what is wrong, and runs none of them until it is put right: half a set of rules is not what the user wrote.

**Read again without restarting**: when rules are switched on, before any run by hand, and when a sync pass finds the file changed.

**The rules in `config.toml` still work.** They are asked first, in their order, called `rule 1`, `rule 2` where they have no name. Quill does not move them. The manual says to.

**Quill writes the file in one case**: New Rule from This Message adds a rule at its end, keeping what is there and its comments, and never to a file it could not read back.

The switch for all rules is not in this file. It is a setting, `enabled` under `[rules]` in `config.toml`, with the default look-back beside it (`look_back`), so that the Settings window and the menu change it as they change any other.

## Matching

The search grammar as it stands (`from:`, `to:`, `cc:`, `subject:`, `has:attachment`, `is:`, `older:`, `newer:`, `larger:`, `smaller:`, `-` for not, quotes for a phrase), and two more filters, for search as well:

- **`body:`** words in the message's text. Bare words already look everywhere, body included; this looks only there.
- **`list:`** the mailing list a message came through, from its `List-Id`.

The body is matched as search matches it: whole words and quoted phrases, in the text a reader would see, without regard to case. Not patterns, and not the HTML behind the text.

**A rule that needs the body waits for it.** When any rule that is on has bare words or `body:`, the bodies of arriving mail are downloaded before the rules are asked, and the new mail is announced after. That is a few seconds later than now. A message whose body cannot be had yet (too large to fetch ahead, or the download failed) is asked what can be told without it, and asked again once the body is here.

## When rules run

**On arriving mail.** As now: mail new to an inbox, in the pass that found it. A periodic sync asks only about what that sync brought. Mail that came while Quill was closed is new at the next sync, and is asked then.

**On catching up.** When rules are switched on, every rule that is on is asked about the mail in each inbox dated within its look-back, leaving out mail the rules have already been asked about. Each message is marked once asked, so switching rules off and on again does nothing to it, and a message the user put back in the inbox stays there.

**By hand.** One rule, or all of them, over the inbox mail within the look-back, whether or not it was asked before. This is how a new rule or a changed one is applied to mail already here. It always shows what it would match first.

**The cap.** A catch-up or a run by hand that would change more than 50 messages asks first, saying how many. Arriving mail is not capped: three hundred newsletters after a holiday should be filed without a question.

**Order.** Rules are asked in the order written. Every rule that matches makes its marks. The first that files or deletes a message is the one that does.

## What a rule does

- **`move_to`**: files the message, on the server as well.
- **`delete`**: moves it to the account's Trash and notes where it was, so Put Back returns it. Where an account's delete would be for good (no Trash folder), the message is left alone and Quill says so.
- **`mark_read`, `star`, `notify`**: as now.

## Preview

A rule is a search, so its preview is the message list: the mail the rule would match, in the inbox, within its look-back. A bar over the list says what the rule would do and to how many ("Receipts would move these 14 to Receipts"), with Run and Cancel. The messages can be opened and read before deciding. Nothing has changed until Run.

## The log, and taking it back

What rules do is written in the store's log, the one `quill log` and Settings > Agents already show, under the rule's name. A pass over arriving mail is one line for each rule that did something. A run by hand is one line, and one undo: `U` takes it back while its time lasts, and `quill undo` after.

The bottom bar says what rules did as it happens ("Rules filed 3"). A rule that cannot do what it says (no such folder) is a notice naming the rule, as now.

## In the window

**A Rules menu** in the app's menu bar, between Message and Help. In a terminal the same commands are in the palette and on keys.

| Item | Does |
|---|---|
| Rules On | Switches all rules on or off. Checked while on. Switching on catches up |
| Run a Rule… | Choose a rule; see what it would match; Run or Cancel |
| Run All Rules… | The same, for every rule that is on |
| New Rule from This Message… | Starts a rule from the message the list is on |
| Edit Rules | Opens `rules.toml` |
| What Rules Did | The log's lines for rules |

Choosing a rule is a picker, drawn in the terminal like the folder picker: it is chosen with the keyboard. A rule that is switched off is in the picker, said to be off, and can still be run by hand.

**New Rule from This Message** is a short form, a sheet in the app and drawn in a terminal: the rule's name; who it is from, filled in from the message; words in the subject, empty; what to do (move to a folder, delete, mark read). Continue shows the preview. Save adds the rule to `rules.toml`; Save and Run does both.

**Settings** has the switch and the default look-back.

A new menu is a bridge change: the schema, its examples and both sides together.

## From a shell

- `quill rules list` says the rules, in order, and which are on.
- `quill rules test NAME` says what a rule would match, and changes nothing.

Both take `--json`. Running and switching rules from a shell is an open question, below.

## Not in this version

- Rules on the server (Sieve, Gmail's filters, Graph's inbox rules).
- Rules about folders other than inboxes, or about mail sent.
- Patterns (regular expressions).
- Forwarding, answering, or running a hook from a rule.
- A form for editing any rule in Settings: rules are edited in the file.
- A menu item for each rule.

## Order of building

1. `rules.toml`: read, checked, read again; the switch and its setting; the old rules still working.
2. The grammar: `body:`, `list:`, a list of matches.
3. Bodies before rules, and the announcement after.
4. `delete`; the log; undo.
5. Asked-once marks; catching up; look-back; the cap.
6. Running by hand, and the preview.
7. The Rules menu and its bridge change; keys in both presets.
8. New Rule from This Message.
9. `quill rules list` and `test`; the manual; the README.

## Settled (2026-11-18)

- Switching rules on catches up, within the look-back, on inboxes, on mail not asked before.
- Rules live in `rules.toml`.
- A Rules menu among the app's main menus.
- A rule that needs the body may hold a message's announcement for the few seconds the body takes.

## Open questions

1. **Running rules from a shell.** `quill rules run` and `quill rules on|off` would be useful in scripts, but a shell command cannot tell the user from an agent, and "no agent touches rules" is a promise Quill makes. Proposed: only `list` and `test` from a shell, and running and switching in the window.
2. **`list:` on mail already here.** Quill may not have kept `List-Id` for mail it synced before this. If not, `list:` matches new mail at once and older mail only once it has been fetched again. To be checked when building.
3. **A new account's first sync.** Proposed: treated as a catch-up, so its inbox within the look-back is asked, under the cap.
4. **Keys** for the switch and for running a rule, in both presets.
