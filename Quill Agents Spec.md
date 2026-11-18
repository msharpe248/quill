# Quill for agents

Agreed with the owner on 2026-11-17, but for the one open question at the end. Most of it is built; what is, what was decided on the way and what was left is in `DECISIONS.md`, under "Quill for agents". Where the two differ, `DECISIONS.md` says what Quill does. Where this and `Quill Terminal Email Client Spec.md` disagree about the shell commands, this is the newer.

## What it is for

A person's AI agent should be able to work their mail through Quill: find it, read it, file it, answer it. Two ways in, one behaviour behind them:

- **Shell commands** (`quill search`, `quill move`, ...), for skills and scripts.
- **MCP**, for agents whose programs speak it.

What Quill offers that a provider's own MCP server does not:

- One set of commands over every kind of account: Gmail, Microsoft 365, IMAP, POP3, Exchange.
- Mail that is already on this Mac: searching and reading reach no server, use no quota, and pass through nobody else's machine.
- A person in the loop, in a real mail program. What the agent did is said in Quill and can be taken back there; what the agent wants to send can wait there to be read first.

## Principles

1. **One program.** Everything is in the `quill` binary and the Quill that is running. There is no second application and no helper to install.
2. **User X's agents, user X's Quill.** A program reaches Quill only if it runs as the same macOS user. Nothing listens on the network, on this Mac or beyond it.
3. **The user decides what agents may do**, in Settings: which way in is open, and which kinds of action are allowed. MCP is off until the user switches it on. What is switched off is refused, with the reason, whichever way in was used.
4. **One layer does the work.** The shell commands and MCP are two fronts over the same actions, with the same names, arguments, answers, limits and refusals. Neither has behaviour the other lacks, except where this says so.
5. **Mail is untrusted.** What a message says is data for the agent to read and never an instruction to Quill; Quill fetches nothing and opens nothing because a message asked.
6. **Nothing is lost for good.** No action here deletes mail permanently, and everything an agent changed is written down and can be taken back.

## The shape

```
agent ── quill search / move / send ...  ──┐
                                           ├── socket ── the running Quill ── actions ── store, queue, engine
agent's MCP client ── quill mcp (relay) ───┘

agent ── quill search / move / send ...  ── actions ── store, queue      (Quill not running)
```

- **The actions layer** is Rust, in the core. It names every action, checks it against the settings, carries it out through the same queue the UI's own actions use, and writes it in the log.
- **The running Quill serves it.** The app or a terminal's Quill, whichever holds the mailbox (`open.lock`), serves agents for as long as it runs. Closing the app's window does not stop it; quitting does.
- **The server is in the core, not the shell.** The Swift shell shows what the core tells it (an agent is connected, a message waits for an OK) and decides nothing.

### The door

- **A socket file in the data folder** (`agent.sock`), made by the Quill that holds the mailbox, while either way in is switched on. The file is the owner's alone (0600, in a folder that is 0700), and Quill also asks the system who is on the other end of each connection and refuses anyone who is not the same user. No password or token is involved: being the user is the proof.
- **No network port.** A port on this Mac can be reached by every program on it and, without care, by pages in a browser. Left out of this version on purpose; see "Not in this version".
- **Left behind by a crash**, the file is replaced by the next Quill to take the mailbox: the lock says who is alive, not the file.
- **A path too long for a socket** (the system allows about a hundred characters): Quill says so in its notices and serves no agents. Not solved in this version.

### `quill mcp`

An MCP client is told to start `quill mcp`. It is a relay: it carries the agent's messages to the running Quill over the socket and the answers back. It holds no mailbox, reads no mail and reaches no server.

- **Set up in one line**, with no secret: for example `claude mcp add quill -- /Applications/Quill.app/Contents/MacOS/quill mcp`. The Settings pane shows the line for this Mac, to copy.
- **When Quill is not running** the relay stays up, answers the client's greeting and lists the tools, and answers every call with "Quill is not running. Open Quill and try again." It connects when Quill appears. (A relay that simply exited would be shown by most clients as a broken server until the client was restarted.)
- **When MCP is switched off**, which is how Quill comes, the relay does the same with other words: "MCP is switched off in Quill's settings (Agents)." Quill refuses an MCP connection itself as well; the relay's word is not what keeps it out.
- **It does not start Quill.** With Quill quit, or the user logged out, there is nothing for an agent to reach.

### The shell commands and the running Quill

- **Quill running:** a command hands its request to Quill over the same socket. There is one writer; the list in the window changes as the agent works; what is sent goes at once.
- **Quill not running:** a command works on the store itself, as `quill send` does today. What it queued is carried out when Quill next runs, or by `quill sync`.
- **`quill sync` while Quill is running** asks the running Quill to check for mail and reports what it said. Today it would start a second engine beside the app's; that is a fault this work removes.
- **MCP has no "not running" mode.** That is the one difference between the two fronts, and it is deliberate: the MCP server is the running Quill.

## What agents may do: the settings

A new pane in the app's Settings window, **Agents**, and `[agent]` in `config.toml`. They hold at once, for connections already open too.

**The two ways in:**

| In the pane | In the file | Default | Off means |
|---|---|---|---|
| Shell commands | `commands` | on | The commands that touch mail refuse. `quill tui`, `account`, `theme` and `keys` are not among them |
| MCP | `mcp` | **off** | No MCP client is served |

**What may be done**, through whichever way in is open:

| In the pane | In the file | Default | What it allows |
|---|---|---|---|
| Read mail | (always) | | List, search, read, count; accounts, folders, conversations, attachments, addresses, status, the log |
| Check for mail | (always) | | `sync`. It refreshes the mail and changes nothing the user did; it is not a change and is never treated as one |
| Organize mail | `organize` | on | Mark read and unread, star, move, copy, archive, snooze, put back; make a folder |
| Delete mail | `delete` | on | Move to Trash. Never more than that |
| Write drafts | `draft` | on | Make, change and discard drafts |
| Send mail | `send` | **off** | Send a message or a draft; answer an invitation; unsubscribe |

**How:**

| In the pane | In the file | Default | Meaning |
|---|---|---|---|
| Wait between messages agents send | `send_gap` | 30 seconds | Seconds from one to the next; `0` is no wait. See "Sending" |
| Hold what agents send for my OK | `hold_sends` | off | See "Sending" |
| Tell me what agents do | `notify` | `changes` | `changes`, `sends` or `none`. See "Telling the user" |
| Accounts | `accounts` | all | Which accounts agents see. The others do not exist for them |
| Most messages in one change | `bulk_limit` | 50 | See "Naming messages" |

So Quill as it comes serves no MCP client and sends nothing for an agent; the shell commands read, check for mail, organize, delete to Trash and write drafts.

- **Sending is decided when it is switched on.** The wait and the hold are under "Send mail" in the pane and are shown only while it is on. Switching it on shows the wait, at 30 seconds, for the user to keep, change or take away, so the choice is made there and then and not found later.
- **This changes a command that exists.** `quill send` works today with nothing set; it will refuse until "Send mail" is on, and say so. The README and the release's notes say it too.

The pane also shows:

- **Whether Quill is serving agents now**, and which are connected, by the name each gave.
- **The last things agents changed**, from the log, and a button for the whole log.
- **The line that adds Quill to an MCP client**, with a Copy button.
- **Stop Agents**, a button. See "Stopping agents".

Rules:

- **One set of switches for every agent and both ways in.** The name an agent gives is its own claim and cannot tell one from another, so there are no settings per agent. A skill runs `quill send` exactly as the user would at a prompt, and Quill cannot tell those apart either, so the shell commands come under the same switches: with "Send mail" off, `quill send` refuses for the user too.
- **A tool that is switched off is not listed** to an MCP client, and the list is sent again when the settings change. Called anyway, or run from a shell, it is refused in words that say which setting: "Sending is switched off in Quill's settings (Agents)."
- **Agents cannot change these settings.** Nothing in either front writes `[agent]`.
- **In a terminal's Quill** the settings are the file's, as every setting is there.

What the settings are and are not: they stop an agent that has been misled, which is the case that matters, and for an agent that has only MCP they are a wall. They are not a wall against a program that runs as the user and means harm: such a program, or an agent with a shell, can read Quill's files and edit `config.toml` without asking Quill anything. That was true before this feature and is not changed by it. Watching what their agents do stays the user's to do; the log and the notifications are what Quill gives them to do it with, and the README says so in these words.

## Naming messages

A command that acts on mail is told which mail in one of three ways:

- **Ids**, as `list` and `search` print them.
- **A message's own Message-ID** (`msgid:<...>`), which does not change when the message moves.
- **A search** (`--where 'from:news before:2026-01-01'`), in the search box's own words.

Rules:

- **An id can change when a message moves on its server.** Every change answers with the ids the messages have now.
- **A change says what it touched**: how many, and which.
- **`--dry-run` changes nothing** and says what would have been changed. Always allowed.
- **A search that finds more than `bulk_limit` messages changes none of them** and says how many it found. `--yes` (in MCP, `confirm: true`) does it anyway, up to a hard ceiling of 1,000 a call. Ids named one by one are not held to `bulk_limit`, only to the ceiling.
- **Nothing found is not an error**: the answer says zero.

### What the search needs that it lacks

Today: words and phrases, `from:`, `to:`, `subject:`, `has:attachment`, `is:unread|read|starred|unstarred|answered|unanswered`, `after:`, `before:`, `in:`. To add, in the search box as well, since it is one grammar:

- `-` before a word or a filter: not. (`-from:ana`, `-is:starred`)
- `older:30d`, `newer:2w`: by age.
- `larger:5m`, `smaller:100k`: by size.
- `list:NAME`: mail from a mailing list, by its `List-Id`.
- `account:NAME`: an account, without the guessing `in:` does.
- `thread:ID`: the conversation a message is in.

## The commands

Each is a shell command and an MCP tool of the same name (`draft reply` is `draft_reply`). Every command takes `--json`; MCP always answers as JSON. "Have" means it exists today.

### Look (needs: read)

| Command | Does |
|---|---|
| `accounts` | The accounts agents may see, with each one's address and state |
| `folders [--account A]` | Folders, with how many messages and how many unread; local ones said to be on this Mac |
| `list [--in F] [--unread] [--limit N]` | Have. The unified inbox, or a folder |
| `search QUERY` | Have. Gains the grammar above |
| `count QUERY` | How many, and nothing else |
| `read ID` | Have. Gains `--headers`, `--max-chars`, `--from-char` for reading a long message in pieces |
| `thread ID` | The conversation a message is in, oldest first: who, when, and each message's id |
| `attachments ID` | A message's files: name, type, size |
| `attachment save ID N` | Writes one file out. From a shell, `--to DIR`; through MCP, into a folder Quill makes for that agent, and the answer says where |
| `contacts WORDS` | Addresses Quill knows, for a name |
| `status` | Unread counts, what is in the Outbox, when each account last synced, and anything wrong |
| `drafts`, `draft show ID` | Drafts there are; one in full |
| `outbox` | What is waiting to go, held, or refused |
| `log [--limit N] [--all]` | What agents have changed; with `--all`, what they looked at as well |
| `watch QUERY` | Shell only. Waits, and prints one JSON line for each arriving message the query finds. Needs Quill running |

### Check for mail (always allowed)

| Command | Does |
|---|---|
| `sync [--account A]` | Have. Fetches new mail and carries out what is already queued. It makes no change of its own: nothing is moved, marked or deleted because `sync` was run |

### Organize (needs: organize)

| Command | Does |
|---|---|
| `mark read\|unread\|starred\|unstarred` | Marks |
| `move DEST` | Moves, to a folder named as after `in:`. A folder on this Mac is moved to as the UI does it: the message is taken off the server, and the answer says so |
| `copy DEST` | Copies |
| `archive` | To the account's archive |
| `snooze WHEN` / `unsnooze` | As `b` does, with the same words for times |
| `restore` | Puts back what was deleted, where it came from |
| `folder create NAME [--account A] [--on-this-mac]` | Makes a folder |
| `undo [N]` | Takes back an entry in the log: the last, or number N |

### Delete (needs: delete)

| Command | Does |
|---|---|
| `delete` | To Trash. In Trash already, or in a folder on this Mac, where the UI's delete would be for good: refused, and the answer says to do it in Quill |

### Write (needs: draft)

| Command | Does |
|---|---|
| `draft new` | A draft: `--to`, `--cc`, `--bcc`, `--subject`, `--account`, `--attach`, the text on standard input (in MCP, `body`) |
| `draft reply ID [--all]` | A reply, addressed and threaded as Quill's own |
| `draft forward ID` | A forward, with the message's files |
| `draft edit ID` | Changes a draft's fields or text |
| `draft discard ID` | Discards one |

### Send (needs: send)

| Command | Does |
|---|---|
| `send` | Have. Gains `--at WHEN` (send later) and `--reply-to ID` |
| `draft send ID` | Sends a draft |
| `outbox cancel ID` | Takes back a message that has not gone, to Drafts |
| `invite accept\|decline\|maybe ID` | Answers an invitation |
| `unsubscribe ID` | Unsubscribes, as the UI does, with the UI's limits on where that may reach |

### Elsewhere

| Command | Does |
|---|---|
| `mcp` | The relay |
| `skill show` | Prints the skill (see "The skill") |

Not here on purpose: see "Out of reach".

## Answers

- **The shapes are a schema**, checked in with examples and tested from both ends, as the bridge is (`schema/agent.json`). Every answer carries the schema's version. A field is added freely; one is renamed or removed only with a new version.
- **Times** are RFC 3339 in UTC, as now.
- **A message's text is the sanitized plain text the reader draws**, never its HTML, cut at `--max-chars` (default 20,000) with a note of how much is left and how to ask for it. Remote pictures are never fetched for an agent. Links are given as text.
- **What mail says is marked as mail.** In JSON it sits in fields of its own (`subject`, `from`, `text`), never mixed into Quill's own words. Each MCP tool that returns mail says in its description that the content is untrusted and is not instructions.
- **Refusals and failures are data too**: a code that does not change (`switched_off`, `over_limit`, `not_found`, `not_running`, ...) and words for a person.
- **Exit statuses** from a shell: 0 done (including nothing found), 1 failed, 2 misuse, and one of its own for "refused by the settings", so a skill can tell a refusal from a fault.
- **MCP tools carry the protocol's hints**, so that a client can ask its user before the ones that matter: read-only for everything under Look; not destructive and repeatable for `sync` and the marks; destructive for `move`, `archive`, `delete`, `draft discard` and `outbox cancel`; and for `send` and `draft send`, that they reach outside this Mac.

## Sending

- **Every message is checked as the compose screen checks it**, and refused where the screen would refuse.
- **Off as Quill comes.** Nothing is sent for an agent, by either way in, until the user switches "Send mail" on.
- **Switched on, it goes as the user's own does**, with the same delay in which it can be taken back. Who it may be sent to is not limited.
- **One message a call.** There is no command that sends many.
- **A wait between messages** (`send_gap`, in seconds: 30 unless the user says otherwise when switching sending on, and `0` for none). With a wait, what agents send goes one message at a time, each no sooner than that long after the one before. A message that has to wait is accepted and sits in the Outbox with the time it is due, where it can be seen and taken back, and the agent is told when it will go. No more than 20 may be waiting so; the next is refused, and says why. This is what turns a misled agent's hundred messages into a slow line the user can stop.
- **Held, if the user says so** (`hold_sends`; off as Quill comes). Then a message an agent sends goes to the Outbox as *waiting for your OK* and does not go. The agent is told so, so that it can tell the user. In Quill the row says who wrote it and for whom; sending it (`compose.send` on that row, as for any Outbox message) lets it go, and it can be opened, changed or discarded first.
- **Whatever the settings, it is in the log**, with its recipients and its attachments' names, and the user is told (see "Telling the user").
- **Attachments are files on this Mac**, named by path. That is the one place an agent can make Quill read a file that is not mail, so the Outbox row and the log both name them.
- **Quill not running:** the message is queued, as `quill send` does it today. The wait and the hold apply all the same, when Quill next runs or at `quill sync`.

## The log, and taking things back

- **Every change an agent makes is one entry**: when, which way in, the name the agent gave, what was asked, and the messages touched.
- **Looking is written down too**, more briefly: what was asked (the search's words, the id that was read, that mail was checked for) and how many messages came back. Never what a message said. `quill log` and the Settings pane show the changes; `--all`, and the whole log, show both.
- **The log is in the store, on this Mac only**, and is kept for 90 days. It is how a user finds out afterwards what an agent did, so nothing an agent can ask for shortens or empties it.
- **One entry is one undo.** `quill undo` (and `U` in Quill, straight after) takes back the whole of an entry: forty messages moved by one command go back together. This uses the grouping the UI's bulk actions already have, but is not held to their ten seconds: an entry can be undone for as long as it makes sense (the messages are still where the agent put them).
- **What cannot be undone says so**: a message that has gone, a move to this Mac that has already taken the message off its server.

## Telling the user

- **In Quill's window, always**: "Claude Code moved 12 messages to Archive. `U` takes it back." in the bottom bar, as for the user's own actions. No setting turns this off.
- **Every MCP connection is a notification**, in the app: "Claude Code connected to Quill." Each one, every time, whatever `notify` says: it is how the user learns that something has come in, and no setting silences it. A client that connects is not asked about: switching MCP on was the consent.
- **What agents do is a notification by the `notify` setting**, in the app:
  - `changes` (as Quill comes): when an agent changes mail or sends.
  - `sends`: only when an agent sends, or a message is waiting for an OK.
  - `none`: none of these.
- **Gathered, not one each.** An agent that files forty messages one at a time makes one notification, not forty: what an agent did within a short while is said together ("Claude Code: 40 messages moved, 3 deleted"). A send is always said by itself, with who it went to.
- **Looking is never notified.** It is in the log.
- **A click on a notification** opens Quill on the log. Each has a **Stop Agents** button.
- **A terminal's Quill** posts no notifications: it has no way to. It says the same things in its bottom bar, connections among them.
- **A scripted run posts none**: a recorder stands in, as for new mail.

## Stopping agents

One command, **Stop Agents** (`agents.stop`): in the palette, in the app's menus, a button in the Settings pane and on every notification about agents, and on a key in a terminal. At once, and without asking:

- Every connection is dropped, MCP and shell alike, and what was half done is left undone.
- Both ways in are switched off, in the settings file, so nothing connects again and the shell commands refuse, with Quill running or not.
- Agent mail still waiting in the Outbox (for its turn, or for an OK) is taken back to Drafts. Mail that has gone has gone; the log says which.
- Quill says what it stopped and what it took back.

Nothing that was done before is undone by it: that is what the log and `undo` are for. Agents are let back in the Settings pane, by switching a way in on again. No agent can run this command or its opposite.

## Out of reach

No command and no tool, in any setting:

- Passwords, sign-ins and tokens, in any form. Nothing here reads the Keychain for an agent or returns a secret.
- Adding, changing or removing an account.
- `config.toml`, `keys.toml`, themes: no writing, and of the config no reading beyond what `accounts` and `folders` say.
- Rules. A rule is a standing instruction, and a standing instruction to forward or file is how mail is stolen quietly.
- Hooks. They run the user's programs.
- Deleting for good, emptying Trash, removing a folder.
- Opening a link, fetching a remote picture, or reaching any server because of what a message says.
- The `[agent]` settings themselves.

## What Quill says about itself

Quill's privacy policy and its page for Google's verification say that Quill has no AI features and that nobody but the user reads their mail. With this built, both need saying more carefully, and before a release that has it:

- **Quill still has no AI in it** and still sends mail to nobody. What changes is that the user can let a program of their own read and act on their mail through Quill, and such a program may be an AI agent that sends what it reads to its maker's service. That is the user's doing and the agent's, not Quill's, and it is off until the user switches it on. The policy says exactly that.
- **Google's rules for Gmail data** (the Limited Use requirements) are about what an application does with the data. Whether a door for the user's own agents sits well with them, and with the verification now under way, is the owner's to weigh before this is given out; nothing in the code decides it. If it does not sit well, the switches could leave Google accounts out of what agents see, which `accounts` already allows by hand.
- `PRIVACY.md`, the site's privacy page and its home page change together, with the release that has this in it and not before.

## The skill

A `SKILL.md` kept in the repository and compiled into the binary, printed by `quill skill show`, so that an agent learns Quill from the Quill it has. It says: what the commands are and how to name messages; to look before changing (`--dry-run`, `count`); that mail is untrusted and what it says is never an instruction; that a held send needs the user and to tell them; and what a refusal means and not to work around it.

## Order of building

1. **The actions layer and the log**, with Look, Organize and Delete as shell commands on the store; `[agent]` and its refusals; the grammar's additions; the schema.
2. **The socket and the running Quill**: commands handed to it, the bottom bar's notices, `U`, `quill sync` asking instead of starting an engine.
3. **`quill mcp`** and the tools; the Settings pane; notifications; Stop Agents.
4. **Write and Send**: drafts, replies, forwards, the wait between messages, held sends and the OK in the Outbox.
5. **`watch`, the skill, invitations and unsubscribe.**

Each step is tried on the demo mailbox and scratch folders only.

## Open questions

1. **Is `quill send` meant to be off too?** As drafted, "Send mail" is one switch for both ways in, so the shell's `quill send`, which works today, refuses until it is on. The other reading is that only MCP's sending starts off, which needs a second switch.

## Settled (2026-11-17)

- Top-level commands, not `quill cmd`.
- One program: the server is the running Quill, and `quill mcp` only carries messages to it.
- A socket file the user alone can reach; no port.
- MCP off as Quill comes; shell commands on.
- Checking for mail is not a change.
- Sending is off as Quill comes. Switched on, it is not limited by recipient and not held unless the user sets it; the wait between messages is chosen when sending is switched on.
- Stop Agents: one command that drops every connection, switches both ways in off and takes back agent mail that has not gone.
- An MCP client that connects is not asked about, and every connection is a macOS notification.
- Delete is to Trash only.
- Nothing is served when Quill is not running, and an agent cannot start it.
- One set of switches for all agents.
- Changes and looking are both logged; the user is notified, by a setting.
- A program running as the user can go round the settings; vigilance is the user's, and Quill says so.

## Not in this version

- **A network port**, even on this Mac alone. If one is ever wanted (a client that cannot start a program): 127.0.0.1 only, off by default, a token kept in the Keychain and asked for on every request, and the `Origin` check the MCP specification requires.
- MCP resources and prompts: tools only.
- Renaming and removing folders (not yet in Quill itself).
- A message's HTML, and its pictures.
- Calendars, beyond answering an invitation.
- Reading or changing rules, saved searches and templates.
