//! Quill without its screen: `quill list`, `search`, `move`, `send`,
//! `sync` and the rest, for scripts, for skills and for a quick look from
//! a shell.
//!
//! What these do to the mail is done by `quill_tui::agent`, the same
//! actions an agent's program is given over MCP: checked against the
//! user's settings for agents, carried out through the queue the UI uses,
//! and written in the log. Nothing here talks to a server but `sync`,
//! which runs the engine once and stops.
//!
//! Every command can print JSON (`--json`): one value, stable field names,
//! times in RFC 3339. The plain output is for reading, and is made safe to
//! print: mail is untrusted, and a terminal obeys what it is sent.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use quill_auth::SecretStore;
use quill_core::config::{ComposeFormat, Config};
use quill_store::{DraftState, Store};
use quill_sync::{SyncEvent, SyncOptions, SyncRequest};
use quill_tui::agent::{self, Call, Caller, Code, Refusal, Settings};
use quill_tui::text;
use serde_json::json;

/// What `quill` exits with when the user's settings refused a command, so
/// that a script can tell a refusal from a fault.
pub const REFUSED_STATUS: u8 = 3;
/// What it exits with when a command was not written right: the same as
/// for an argument it does not know.
pub const MISUSE_STATUS: u8 = 2;

/// A command that was refused: what to say, and what to exit with.
#[derive(Debug)]
pub struct Turned {
    pub refusal: Refusal,
}

impl Turned {
    /// What `quill` exits with for it.
    pub fn status(&self) -> u8 {
        match self.refusal.code {
            Code::SwitchedOff => REFUSED_STATUS,
            Code::Invalid => MISUSE_STATUS,
            _ => 1,
        }
    }
}

impl std::fmt::Display for Turned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.refusal.message.trim_end_matches('.'))
    }
}

impl std::error::Error for Turned {}

/// The user's settings for what may be done from outside Quill's window,
/// as an action needs them.
pub fn settings(config: &Config) -> Settings {
    Settings {
        from_names: config
            .accounts
            .iter()
            .filter_map(|account| Some((account.name.clone(), account.full_name.clone()?)))
            .collect(),
        markdown: config.compose.format == ComposeFormat::Markdown,
        reply_above: config.compose.reply_position == quill_core::config::ReplyPosition::Above,
        ..Settings::new(config.agent.clone(), jiff::tz::TimeZone::system())
    }
}

/// Does one thing to the mail and prints what came of it: as JSON, or a
/// line at a time. A refusal is printed as JSON too, when JSON was asked
/// for, and is the error either way.
pub fn ask(
    out: &mut dyn Write,
    store: &mut Store,
    settings: &Settings,
    call: &Call,
    json: bool,
) -> anyhow::Result<()> {
    let now = jiff::Timestamp::now().as_second();
    let result = agent::perform(store, settings, &Caller::shell(), call, now).result;
    print(out, result, json)
}

/// Prints what came of a request, whoever carried it out: this command on
/// the store, or the Quill that is running.
pub fn print(
    out: &mut dyn Write,
    result: Result<agent::Answer, Refusal>,
    json: bool,
) -> anyhow::Result<()> {
    match result {
        Ok(answer) => {
            if json {
                writeln!(out, "{}", serde_json::to_string_pretty(&answer.value)?)?;
            } else {
                for line in &answer.lines {
                    writeln!(out, "{line}")?;
                }
            }
            Ok(())
        }
        Err(refusal) => {
            if json {
                writeln!(out, "{}", serde_json::to_string_pretty(&refusal.value())?)?;
            }
            Err(Turned { refusal }.into())
        }
    }
}

/// `quill read --raw`: one message exactly as it arrived. It is asked for
/// as any reading is, so the settings and the log hold for it too.
pub fn raw(
    out: &mut dyn Write,
    store: &mut Store,
    settings: &Settings,
    id: i64,
) -> anyhow::Result<()> {
    let call = Call::Read {
        id,
        headers: false,
        max_chars: Some(1),
        from_char: 0,
    };
    ask(&mut std::io::sink(), store, settings, &call, false)?;
    let bytes = store
        .raw(quill_core::types::LocalMessageId(id))?
        .with_context(|| {
            format!("message {id} has not been downloaded. Open it in Quill, or run `quill sync`")
        })?;
    out.write_all(&bytes)?;
    Ok(())
}

/// Whether the settings let a shell command touch the mail at all.
pub fn may_run(config: &Config) -> anyhow::Result<()> {
    if config.agent.commands {
        return Ok(());
    }
    Err(Turned {
        refusal: Refusal::new(
            Code::SwitchedOff,
            "Commands from a shell are switched off in Quill's settings (Agents).",
        ),
    }
    .into())
}

/// How long `sync` waits for the engine to say anything before giving up.
const QUIET_LIMIT: Duration = Duration::from_secs(120);

/// `quill sync`: one look at every account, and everything in the queue
/// and the outbox that is due, then stop. Returns whether it all went
/// without a problem.
pub async fn sync(
    out: &mut dyn Write,
    config: &Config,
    config_ok: bool,
    database: &std::path::Path,
    blobs: std::path::PathBuf,
    secrets: Arc<dyn SecretStore>,
    json: bool,
) -> anyhow::Result<bool> {
    let open =
        || Store::open(database).with_context(|| format!("cannot open {}", database.display()));
    let mut store = open()?;
    let before = count(&store)?;
    let prepared = crate::accounts::prepare(config, &mut store, &secrets, config_ok)?;
    // What is said about an account that will sync is a warning (it
    // connects without encryption); about one that will not, a problem.
    let syncs = |text: &str| {
        prepared
            .accounts
            .iter()
            .any(|account| text.starts_with(&account.name))
    };
    let (warnings, mut problems): (Vec<String>, Vec<String>) = prepared
        .notices
        .iter()
        .map(|notice| notice.text.clone())
        .partition(|text| syncs(text));
    let mut waiting: Vec<_> = prepared.accounts.iter().map(|account| account.id).collect();
    let synced = waiting.len();
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let handle = quill_sync::spawn_with_rules(
        open()?,
        Some(blobs),
        prepared.accounts,
        SyncOptions {
            // One pass each, started as the engine starts.
            interval: Duration::from_secs(24 * 60 * 60),
            ..SyncOptions::default()
        },
        crate::rules::from_config(config),
        events_tx,
    );
    let mut sent = 0usize;
    loop {
        // Done when every account has been looked at once and nothing is
        // left that is due now.
        if waiting.is_empty() && settled(&store)? {
            break;
        }
        let event = tokio::time::timeout(QUIET_LIMIT, events.recv()).await;
        match event {
            Ok(Some(SyncEvent::Syncing {
                account,
                active: false,
            })) => {
                if waiting.contains(&account) {
                    waiting.retain(|id| *id != account);
                    // What was queued while offline goes now.
                    handle.send(SyncRequest::Queue);
                }
            }
            Ok(Some(SyncEvent::Sent { .. })) => sent += 1,
            Ok(Some(SyncEvent::Problem { account, message })) => {
                problems.push(format!("{account}: {message}"));
            }
            Ok(Some(SyncEvent::Certificate {
                account, problem, ..
            })) => problems.push(format!(
                "{account}: the certificate of {} was not accepted ({}). Open Quill to look at it and decide.",
                problem.host, problem.reason
            )),
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                problems.push("Gave up waiting: nothing was heard for two minutes.".to_owned());
                break;
            }
        }
    }
    drop(handle);
    let after = count(&store)?;
    let new = after.saturating_sub(before);
    if json {
        writeln!(
            out,
            "{}",
            json!({
                "accounts": synced,
                "new": new,
                "sent": sent,
                "warnings": warnings,
                "problems": problems,
            })
        )?;
    } else {
        writeln!(
            out,
            "{synced} account{} looked at, {new} new message{}, {sent} sent.",
            if synced == 1 { "" } else { "s" },
            if new == 1 { "" } else { "s" },
        )?;
        for line in warnings.iter().chain(&problems) {
            writeln!(out, "{}", text::sanitize_line(line))?;
        }
    }
    Ok(problems.is_empty())
}

/// How many messages the store holds, in every folder.
fn count(store: &Store) -> anyhow::Result<usize> {
    Ok(store
        .folders()?
        .iter()
        .map(|folder| folder.total as usize)
        .sum())
}

/// Whether nothing is waiting that is due: no queued change, and nothing
/// in the outbox still to go. Mail a server refused stays in the outbox
/// and is not waited for.
fn settled(store: &Store) -> anyhow::Result<bool> {
    let now = jiff::Timestamp::now().as_second();
    let going = store.outbox()?.iter().any(|draft| {
        matches!(draft.state, DraftState::Outbox | DraftState::Sending)
            && draft.send_after.is_none_or(|at| at <= now)
    });
    Ok(store.queued_operations()? == 0 && !going)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use quill_auth::MemorySecrets;
    use quill_core::types::LocalMessageId;
    use quill_core::types::{
        Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
    };
    use quill_store::{BlobStore, Mailbox};
    use serde_json::Value;

    use super::*;
    use crate::pop3_test_server::{Dovecot, PASSWORD};

    /// 2026-11-14T09:30:00Z.
    const NOW: i64 = 1_794_648_600;

    /// The demo mailbox, in a store of its own.
    async fn demo() -> Store {
        let mut store = Store::open_in_memory()
            .unwrap()
            .with_blobs(BlobStore::in_memory());
        quill_tui::demo::seed(&mut store, NOW).await.unwrap();
        store
    }

    fn said(work: impl FnOnce(&mut dyn Write) -> anyhow::Result<()>) -> String {
        let mut out = Vec::new();
        work(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn everything() -> Settings {
        Settings::new(
            quill_core::config::AgentConfig::default(),
            jiff::tz::TimeZone::UTC,
        )
    }

    fn list(limit: usize, unread: bool, within: Option<&str>) -> Call {
        Call::List {
            within: within.map(str::to_owned),
            unread,
            limit: Some(limit),
        }
    }

    fn read(id: i64) -> Call {
        Call::Read {
            id,
            headers: false,
            max_chars: None,
            from_char: 0,
        }
    }

    #[tokio::test]
    async fn list_prints_the_inbox_newest_first_or_one_folder() {
        let mut store = demo().await;
        let all = said(|out| {
            ask(
                out,
                &mut store,
                &everything(),
                &list(50, false, None),
                false,
            )
        });
        let lines: Vec<&str> = all.lines().collect();
        assert!(
            lines[0].contains("Ana Ruiz") && lines[0].ends_with("Q4 roadmap"),
            "{all}"
        );
        // Unread is marked, and can be asked for alone.
        assert!(lines[0].contains(" * "), "{}", lines[0]);
        let unread = said(|out| ask(out, &mut store, &everything(), &list(50, true, None), false));
        assert!(unread.lines().count() < lines.len());
        assert!(unread.lines().all(|line| line.contains(" *")));
        assert_eq!(
            said(|out| ask(out, &mut store, &everything(), &list(3, false, None), false))
                .lines()
                .count(),
            3
        );

        let sent = said(|out| {
            ask(
                out,
                &mut store,
                &everything(),
                &list(50, false, Some("work/sent")),
                false,
            )
        });
        assert!(
            sent.contains("Offsite agenda") && !sent.contains("Q4 roadmap"),
            "{sent}"
        );
        assert!(
            ask(
                &mut Vec::new(),
                &mut store,
                &everything(),
                &list(50, false, Some("")),
                false
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn json_says_the_same_with_stable_names() {
        let mut store = demo().await;
        let text = said(|out| ask(out, &mut store, &everything(), &list(2, false, None), true));
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["more"], true);
        let rows = value["messages"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let first = &rows[0];
        assert_eq!(first["subject"], "Q4 roadmap");
        assert_eq!(first["account"], "Work");
        assert_eq!(first["folder"], "Inbox");
        assert_eq!(first["from"][0]["address"], "ana.ruiz@example.com");
        assert_eq!(first["unread"], true);
        assert_eq!(first["attachments"], true);
        assert!(first["date"].as_str().unwrap().ends_with('Z'));
        assert!(first["id"].is_i64());
    }

    #[tokio::test]
    async fn search_takes_the_search_boxs_words() {
        let mut store = demo().await;
        let search = |words: &str| Call::Search {
            query: words.to_owned(),
            limit: Some(50),
        };
        let found = said(|out| {
            ask(
                out,
                &mut store,
                &everything(),
                &search("from:ana roadmap"),
                false,
            )
        });
        assert_eq!(found.lines().count(), 1, "{found}");
        assert!(found.contains("Q4 roadmap"));
        let none = said(|out| ask(out, &mut store, &everything(), &search("zeppelin"), false));
        assert!(none.is_empty());
        let nothing = ask(
            &mut Vec::new(),
            &mut store,
            &everything(),
            &search("  "),
            false,
        )
        .unwrap_err();
        // Nothing to search for is a mistake in the command.
        assert_eq!(
            nothing.downcast_ref::<Turned>().unwrap().status(),
            MISUSE_STATUS
        );
    }

    #[tokio::test]
    async fn read_prints_a_message_its_json_or_its_raw_bytes() {
        let mut store = demo().await;
        let id = store.messages(Mailbox::Unified).unwrap()[0].id.0;
        let text = said(|out| ask(out, &mut store, &everything(), &read(id), false));
        assert!(
            text.starts_with("From:    Ana Ruiz <ana.ruiz@example.com>\n"),
            "{text}"
        );
        assert!(text.contains("Subject: Q4 roadmap\n\n"));
        assert!(text.contains("draft roadmap"));

        let value: Value = serde_json::from_str(&said(|out| {
            ask(out, &mut store, &everything(), &read(id), true)
        }))
        .unwrap();
        assert_eq!(value["downloaded"], true);
        assert!(value["body"].as_str().unwrap().contains("draft roadmap"));

        let whole = said(|out| raw(out, &mut store, &everything(), id));
        assert!(whole.contains("Content-Type:") && whole.contains("Subject: Q4 roadmap"));

        let missing = ask(
            &mut Vec::new(),
            &mut store,
            &everything(),
            &read(999_999),
            false,
        )
        .unwrap_err();
        assert_eq!(missing.to_string(), "There is no message 999999");
        assert_eq!(missing.downcast_ref::<Turned>().unwrap().status(), 1);
        assert!(raw(&mut Vec::new(), &mut store, &everything(), 999_999).is_err());
    }

    #[tokio::test]
    async fn a_refusal_is_json_when_json_was_asked_for_and_has_a_status_of_its_own() {
        let mut store = demo().await;
        let id = store.messages(Mailbox::Unified).unwrap()[0].id.0;
        let mut off = everything();
        off.agent.delete = false;
        let call = Call::Delete {
            which: quill_tui::agent::Which {
                ids: vec![id],
                ..Default::default()
            },
        };
        let mut out = Vec::new();
        let refused = ask(&mut out, &mut store, &off, &call, true).unwrap_err();
        let turned = refused.downcast_ref::<Turned>().unwrap();
        assert_eq!(turned.status(), REFUSED_STATUS);
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["refused"]["code"], "switched_off");
        assert!(
            value["refused"]["message"]
                .as_str()
                .unwrap()
                .contains("Deleting")
        );
        // Without JSON nothing is printed: the words are the error's.
        let mut out = Vec::new();
        assert!(ask(&mut out, &mut store, &off, &call, false).is_err());
        assert!(out.is_empty());
        // Raw bytes are a reading, and are refused as one is.
        off.agent.commands = false;
        assert!(raw(&mut Vec::new(), &mut store, &off, id).is_err());
    }

    #[tokio::test]
    async fn what_mail_says_cannot_reach_the_terminal() {
        let mut store = demo().await;
        let inbox = store
            .folders()
            .unwrap()
            .into_iter()
            .find(|folder| folder.role == FolderRole::Inbox)
            .unwrap()
            .id;
        store
            .apply_delta(
                inbox,
                &SyncDelta {
                    reset: false,
                    added: vec![RemoteMessage {
                        id: RemoteId("sly".to_owned()),
                        headers: Headers {
                            message_id: None,
                            subject: "Hi\u{1b}]0;owned\u{7}\u{1b}[2J there".to_owned(),
                            from: Vec::new(),
                            to: Vec::new(),
                            cc: Vec::new(),
                            date: NOW + 60,
                        },
                        flags: Flags::default(),
                        size: 1,
                        has_attachments: false,
                        thread: Vec::new(),
                    }],
                    changed: Vec::new(),
                    removed: Vec::new(),
                    has_more: false,
                    next_cursor: SyncCursor("c".to_owned()),
                },
            )
            .unwrap();
        let id = store.messages(Mailbox::Unified).unwrap()[0].id.0;
        store
            .put_body(LocalMessageId(id), "Body\u{1b}[31m in red\u{7}")
            .unwrap();
        for text in [
            said(|out| ask(out, &mut store, &everything(), &list(1, false, None), false)),
            said(|out| ask(out, &mut store, &everything(), &read(id), false)),
        ] {
            assert!(
                !text.contains('\u{1b}') && !text.contains('\u{7}'),
                "{text:?}"
            );
        }
        // JSON carries it as data, escaped as JSON escapes it.
        let json = said(|out| ask(out, &mut store, &everything(), &list(1, false, None), true));
        assert!(!json.contains('\u{1b}') && json.contains("\\u001b"));
    }

    fn letter(account: Option<&str>, to: &[&str], subject: &str, body: &str) -> Call {
        Call::Send {
            to: to.iter().map(|to| (*to).to_owned()).collect(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: subject.to_owned(),
            body: body.to_owned(),
            account: account.map(str::to_owned),
            attach: Vec::new(),
            at: None,
        }
    }

    #[tokio::test]
    async fn send_puts_a_checked_message_in_the_outbox_once_sending_is_allowed() {
        let mut store = demo().await;
        let message = letter(
            Some("personal"),
            &["Ana Ruiz <ana.ruiz@example.com>"],
            "From a script",
            "Sent without the screen.\n",
        );
        // Off as Quill comes: refused, with a status of its own.
        let refused = ask(&mut Vec::new(), &mut store, &everything(), &message, false).unwrap_err();
        assert_eq!(
            refused.downcast_ref::<Turned>().unwrap().status(),
            REFUSED_STATUS
        );
        assert!(refused.to_string().contains("Sending is switched off"));
        assert!(store.outbox().unwrap().is_empty());

        let mut sending = everything();
        sending.agent.send = true;
        let text = said(|out| ask(out, &mut store, &sending, &message, false));
        assert!(text.starts_with("In the outbox of Personal."), "{text}");
        let outbox = store.outbox().unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].content.subject, "From a script");
        assert_eq!(outbox[0].state, DraftState::Outbox);
        // The first is due at once: there is nobody at a prompt to take
        // it back.
        assert!(outbox[0].send_after.unwrap() <= jiff::Timestamp::now().as_second());

        // What the compose screen would refuse is refused here, as a
        // command written wrong, and leaves nothing behind.
        let nobody = ask(
            &mut Vec::new(),
            &mut store,
            &sending,
            &letter(None, &[], "", ""),
            false,
        )
        .unwrap_err();
        assert!(nobody.to_string().contains("nobody to send it to"));
        assert_eq!(
            nobody.downcast_ref::<Turned>().unwrap().status(),
            MISUSE_STATUS
        );
        assert!(
            ask(
                &mut Vec::new(),
                &mut store,
                &sending,
                &letter(None, &["not an address"], "", ""),
                false
            )
            .is_err()
        );
        let err = ask(
            &mut Vec::new(),
            &mut store,
            &sending,
            &letter(Some("Nobody"), &["ana@example.com"], "", ""),
            false,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "There is no account named \"Nobody\"");
        assert_eq!(store.outbox().unwrap().len(), 1);
        assert!(store.drafts().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_looks_once_sends_what_is_waiting_and_says_what_went_wrong() {
        let Some(server) = Dovecot::start().await else {
            return;
        };
        let user = "script@example.com";
        server.fill(user, 1, 3);
        let config = Config::parse(&format!(
            "[[account]]\nname = \"Old Mail\"\ntype = \"pop3\"\naddress = \"{user}\"\npop3 = {{ host = \"localhost\", port = {}, tls = \"none\" }}\n",
            server.pop3_port
        ))
        .unwrap();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::default());
        secrets
            .set(&format!("pop3:{user}@localhost"), PASSWORD)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("quill.db");
        let blobs = dir.path().join("blobs");
        let run = |json: bool| {
            let (config, database, blobs, secrets) = (
                config.clone(),
                database.clone(),
                blobs.clone(),
                secrets.clone(),
            );
            async move {
                let mut out = Vec::new();
                let fine = sync(&mut out, &config, true, &database, blobs, secrets, json)
                    .await
                    .unwrap();
                (fine, String::from_utf8(out).unwrap())
            }
        };
        let (fine, text) = run(false).await;
        assert!(fine, "{text}");
        assert_eq!(
            text,
            "1 account looked at, 3 new messages, 0 sent.\nOld Mail connects without encryption. Its password and mail cross the network in the clear.\n"
        );
        // Again, with nothing new, as JSON.
        server.fill(user, 4, 1);
        let (fine, text) = run(true).await;
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(fine);
        assert_eq!(value["new"], 1);
        assert_eq!(value["accounts"], 1);

        // Mail waiting to go, from an account with no server to send
        // through: the sync says so and is not a success.
        let mut store = Store::open(&database)
            .unwrap()
            .with_blobs(BlobStore::new(blobs.clone()));
        let mut sending = settings(&config);
        sending.agent.send = true;
        ask(
            &mut Vec::new(),
            &mut store,
            &sending,
            &letter(None, &["ana.ruiz@example.com"], "Stuck", ""),
            false,
        )
        .unwrap();
        drop(store);
        let (fine, text) = run(false).await;
        assert!(!fine);
        assert!(text.contains("no server to send through"), "{text}");
    }
}
