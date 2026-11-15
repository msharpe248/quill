//! Quill without its screen: `quill list`, `read`, `search`, `send` and
//! `sync`, for scripts and for a quick look from a shell.
//!
//! These read the same store the UI does and go through the same doors.
//! Mail to send is checked and built as the compose screen's is and put in
//! the same outbox; nothing here talks to a server but `sync`, which runs
//! the same engine once and stops.
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
use quill_core::types::LocalMessageId;
use quill_mime::BodyFormat;
use quill_store::{DraftContent, DraftState, Mailbox, MessageRow, Store};
use quill_sync::{SyncEvent, SyncOptions, SyncRequest};
use quill_tui::data::DataEvent;
use quill_tui::outgoing::{self, SendRequest};
use quill_tui::text;
use serde_json::{Value, json};

/// The folder and account names a row is printed with.
struct Names {
    accounts: Vec<(quill_core::types::LocalAccountId, String)>,
    folders: Vec<(quill_core::types::LocalFolderId, String)>,
}

impl Names {
    fn of(store: &Store) -> anyhow::Result<Self> {
        Ok(Self {
            accounts: store
                .accounts()?
                .into_iter()
                .map(|account| (account.id, account.name))
                .collect(),
            folders: store
                .folders()?
                .into_iter()
                .map(|folder| (folder.id, folder.name))
                .collect(),
        })
    }

    fn account(&self, row: &MessageRow) -> &str {
        self.accounts
            .iter()
            .find(|(id, _)| *id == row.account)
            .map_or("", |(_, name)| name.as_str())
    }

    fn folder(&self, row: &MessageRow) -> &str {
        self.folders
            .iter()
            .find(|(id, _)| *id == row.folder)
            .map_or("", |(_, name)| name.as_str())
    }
}

fn when(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds)
        .map(|at| at.to_string())
        .unwrap_or_default()
}

fn people(list: &[quill_core::types::Address]) -> Vec<Value> {
    list.iter()
        .map(|person| json!({ "name": person.name, "address": person.email }))
        .collect()
}

fn row_json(row: &MessageRow, names: &Names) -> Value {
    json!({
        "id": row.id.0,
        "account": names.account(row),
        "folder": names.folder(row),
        "thread": row.thread,
        "date": when(row.headers.date),
        "from": people(&row.headers.from),
        "to": people(&row.headers.to),
        "cc": people(&row.headers.cc),
        "subject": row.headers.subject,
        "message_id": row.headers.message_id,
        "unread": !row.flags.seen,
        "starred": row.flags.flagged,
        "answered": row.flags.answered,
        "attachments": row.has_attachments,
    })
}

/// One line for a message: its id, its marks, when, who and what.
fn row_line(row: &MessageRow) -> String {
    let marks = format!(
        "{}{}{}",
        if row.flags.seen { ' ' } else { '*' },
        if row.flags.flagged { '!' } else { ' ' },
        if row.has_attachments { '@' } else { ' ' },
    );
    let who = row
        .headers
        .from
        .first()
        .map_or_else(String::new, |from| from.display_name().to_owned());
    let date = when(row.headers.date);
    format!(
        "{:>8}  {marks}  {}  {}  {}",
        row.id.0,
        date.get(..16).unwrap_or(&date).replace('T', " "),
        text::fit(&text::sanitize_line(&who), 22),
        text::sanitize_line(&row.headers.subject),
    )
}

fn print_rows(
    out: &mut dyn Write,
    store: &Store,
    rows: &[MessageRow],
    json: bool,
) -> anyhow::Result<()> {
    if json {
        let names = Names::of(store)?;
        let rows: Vec<Value> = rows.iter().map(|row| row_json(row, &names)).collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&rows)?)?;
    } else {
        for row in rows {
            writeln!(out, "{}", row_line(row))?;
        }
    }
    Ok(())
}

/// `quill list`: the unified inbox, newest first, or what `within` names
/// (`work/sent`, as after `in:` in a search).
pub fn list(
    out: &mut dyn Write,
    store: &Store,
    within: Option<&str>,
    unread: bool,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let mut rows = match within {
        None => store.messages(Mailbox::Unified)?,
        Some(place) => {
            let query = quill_core::search::parse(&format!("in:{place}"), |_, _, _| None);
            anyhow::ensure!(
                !query.is_empty(),
                "\"{place}\" does not name a folder. Try an account and a folder, as in work/sent"
            );
            store.search(&query, usize::MAX)?
        }
    };
    if unread {
        rows.retain(|row| !row.flags.seen);
    }
    rows.truncate(limit);
    print_rows(out, store, &rows, json)
}

/// `quill search`: the same words and filters as the search box.
pub fn search(
    out: &mut dyn Write,
    store: &Store,
    words: &str,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let tz = jiff::tz::TimeZone::system();
    let query = quill_core::search::parse(words, |year, month, day| {
        let date = jiff::civil::Date::new(
            i16::try_from(year).ok()?,
            i8::try_from(month).ok()?,
            i8::try_from(day).ok()?,
        )
        .ok()?;
        Some(date.to_zoned(tz.clone()).ok()?.timestamp().as_second())
    });
    anyhow::ensure!(!query.is_empty(), "there is nothing to search for");
    let rows = store.search(&query, limit)?;
    print_rows(out, store, &rows, json)
}

/// `quill read`: one message, by the id `list` and `search` print.
pub fn read(
    out: &mut dyn Write,
    store: &Store,
    id: i64,
    raw: bool,
    json: bool,
) -> anyhow::Result<()> {
    let id = LocalMessageId(id);
    let row = store
        .message_rows(&[id])?
        .pop()
        .with_context(|| format!("there is no message {}", id.0))?;
    if raw {
        let bytes = store.raw(id)?.with_context(|| {
            format!(
                "message {} has not been downloaded. Open it in Quill, or run `quill sync`",
                id.0
            )
        })?;
        out.write_all(&bytes)?;
        return Ok(());
    }
    let body = store.body(id)?;
    if json {
        let names = Names::of(store)?;
        let mut value = row_json(&row, &names);
        value["downloaded"] = json!(body.is_some());
        value["body"] = json!(body);
        writeln!(out, "{}", serde_json::to_string_pretty(&value)?)?;
        return Ok(());
    }
    let join = |list: &[quill_core::types::Address]| {
        list.iter()
            .map(|person| text::sanitize_line(&person.to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    writeln!(out, "From:    {}", join(&row.headers.from))?;
    writeln!(out, "To:      {}", join(&row.headers.to))?;
    if !row.headers.cc.is_empty() {
        writeln!(out, "Cc:      {}", join(&row.headers.cc))?;
    }
    writeln!(out, "Date:    {}", when(row.headers.date))?;
    writeln!(
        out,
        "Subject: {}",
        text::sanitize_line(&row.headers.subject)
    )?;
    writeln!(out)?;
    match body {
        Some(body) => writeln!(out, "{}", text::sanitize_body(body.trim_end()))?,
        None => writeln!(
            out,
            "(Not downloaded yet. Open it in Quill, or run `quill sync`.)"
        )?,
    }
    Ok(())
}

/// A message to send, as the command line gives it.
#[derive(Debug, Clone, Default)]
pub struct Message {
    pub account: Option<String>,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attach: Vec<String>,
}

/// `quill send`: checks the message and puts it in the outbox, as the
/// compose screen does. It goes at the next sync.
pub fn send(
    out: &mut dyn Write,
    store: &mut Store,
    config: &Config,
    message: Message,
    json: bool,
) -> anyhow::Result<()> {
    let accounts = store.accounts()?;
    let account = match &message.account {
        Some(name) => accounts
            .iter()
            .find(|account| account.name.eq_ignore_ascii_case(name))
            .with_context(|| format!("there is no account named \"{name}\""))?,
        None => accounts
            .first()
            .context("there is no account to send from. Add one in Quill first")?,
    };
    let mut content = DraftContent {
        to: message.to.join(", "),
        cc: message.cc.join(", "),
        bcc: message.bcc.join(", "),
        subject: message.subject,
        body: message.body,
        ..DraftContent::default()
    };
    for path in &message.attach {
        for event in outgoing::attach(store, path, None) {
            match event {
                DataEvent::Attached(file) => content.attachments.push(file),
                DataEvent::AttachFailed(why) => anyhow::bail!("{why}"),
                _ => {}
            }
        }
    }
    let identity = config
        .accounts
        .iter()
        .find(|configured| configured.name == account.name);
    let request = SendRequest {
        draft: None,
        account: account.id,
        content,
        from_name: identity.and_then(|identity| identity.full_name.clone()),
        format: match config.compose.format {
            ComposeFormat::Markdown => BodyFormat::Markdown,
            ComposeFormat::Plain => BodyFormat::Plain,
        },
        // Typed at a prompt and meant: nothing to take back.
        delay_seconds: 0,
        utc_offset: jiff::Zoned::now().offset().seconds(),
        // Nobody is here to be asked whether a message with no subject is
        // meant. The checks that refuse a message still do.
        confirmed: true,
    };
    for event in outgoing::send(store, &request)? {
        match event {
            DataEvent::SendRefused(why) => anyhow::bail!("{why}"),
            DataEvent::SendDoubts(doubts) => anyhow::bail!("{}", doubts.join(" ")),
            _ => {}
        }
    }
    if json {
        writeln!(
            out,
            "{}",
            json!({ "queued": true, "account": account.name })
        )?;
    } else {
        writeln!(
            out,
            "In the outbox of {}. It goes when Quill next syncs: leave the app running, or run `quill sync`.",
            text::sanitize_line(&account.name)
        )?;
    }
    Ok(())
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
    let handle = quill_sync::spawn(
        open()?,
        Some(blobs),
        prepared.accounts,
        SyncOptions {
            // One pass each, started as the engine starts.
            interval: Duration::from_secs(24 * 60 * 60),
            ..SyncOptions::default()
        },
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
    use quill_core::types::{
        Flags, FolderRole, Headers, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
    };
    use quill_store::BlobStore;

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

    #[tokio::test]
    async fn list_prints_the_inbox_newest_first_or_one_folder() {
        let store = demo().await;
        let all = said(|out| list(out, &store, None, false, 50, false));
        let lines: Vec<&str> = all.lines().collect();
        assert!(
            lines[0].contains("Ana Ruiz") && lines[0].ends_with("Q4 roadmap"),
            "{all}"
        );
        // Unread is marked, and can be asked for alone.
        assert!(lines[0].contains(" * "), "{}", lines[0]);
        let unread = said(|out| list(out, &store, None, true, 50, false));
        assert!(unread.lines().count() < lines.len());
        assert!(unread.lines().all(|line| line.contains(" *")));
        assert_eq!(
            said(|out| list(out, &store, None, false, 3, false))
                .lines()
                .count(),
            3
        );

        let sent = said(|out| list(out, &store, Some("work/sent"), false, 50, false));
        assert!(
            sent.contains("Offsite agenda") && !sent.contains("Q4 roadmap"),
            "{sent}"
        );
        assert!(list(&mut Vec::new(), &store, Some(""), false, 50, false).is_err());
    }

    #[tokio::test]
    async fn json_says_the_same_with_stable_names() {
        let store = demo().await;
        let text = said(|out| list(out, &store, None, false, 2, true));
        let rows: Vec<Value> = serde_json::from_str(&text).unwrap();
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
        let store = demo().await;
        let found = said(|out| search(out, &store, "from:ana roadmap", 50, false));
        assert_eq!(found.lines().count(), 1, "{found}");
        assert!(found.contains("Q4 roadmap"));
        let none = said(|out| search(out, &store, "zeppelin", 50, false));
        assert!(none.is_empty());
        assert!(search(&mut Vec::new(), &store, "  ", 50, false).is_err());
    }

    #[tokio::test]
    async fn read_prints_a_message_its_json_or_its_raw_bytes() {
        let store = demo().await;
        let id = store.messages(Mailbox::Unified).unwrap()[0].id.0;
        let text = said(|out| read(out, &store, id, false, false));
        assert!(
            text.starts_with("From:    Ana Ruiz <ana.ruiz@example.com>\n"),
            "{text}"
        );
        assert!(text.contains("Subject: Q4 roadmap\n\n"));
        assert!(text.contains("draft roadmap"));

        let value: Value =
            serde_json::from_str(&said(|out| read(out, &store, id, false, true))).unwrap();
        assert_eq!(value["downloaded"], true);
        assert!(value["body"].as_str().unwrap().contains("draft roadmap"));

        let raw = said(|out| read(out, &store, id, true, false));
        assert!(raw.contains("Content-Type:") && raw.contains("Subject: Q4 roadmap"));

        let missing = read(&mut Vec::new(), &store, 999_999, false, false).unwrap_err();
        assert_eq!(missing.to_string(), "there is no message 999999");
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
            said(|out| list(out, &store, None, false, 1, false)),
            said(|out| read(out, &store, id, false, false)),
        ] {
            assert!(
                !text.contains('\u{1b}') && !text.contains('\u{7}'),
                "{text:?}"
            );
        }
        // JSON carries it as data, escaped as JSON escapes it.
        let json = said(|out| list(out, &store, None, false, 1, true));
        assert!(!json.contains('\u{1b}') && json.contains("\\u001b"));
    }

    #[tokio::test]
    async fn send_puts_a_checked_message_in_the_outbox() {
        let mut store = demo().await;
        let config = Config::default();
        let message = Message {
            account: Some("personal".to_owned()),
            to: vec!["Ana Ruiz <ana.ruiz@example.com>".to_owned()],
            cc: vec!["kit@example.com".to_owned()],
            subject: "From a script".to_owned(),
            body: "Sent without the screen.\n".to_owned(),
            ..Message::default()
        };
        let text = said(|out| send(out, &mut store, &config, message, false));
        assert!(text.starts_with("In the outbox of Personal."), "{text}");
        let outbox = store.outbox().unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].content.subject, "From a script");
        assert_eq!(outbox[0].state, DraftState::Outbox);
        // Due at once: there is nobody at a prompt to take it back.
        assert!(outbox[0].send_after.unwrap() <= jiff::Timestamp::now().as_second());

        // What the compose screen would refuse is refused here.
        let nobody = send(
            &mut Vec::new(),
            &mut store,
            &config,
            Message::default(),
            false,
        );
        assert!(
            nobody
                .unwrap_err()
                .to_string()
                .contains("nobody to send it to")
        );
        let bad = Message {
            to: vec!["not an address".to_owned()],
            ..Message::default()
        };
        assert!(send(&mut Vec::new(), &mut store, &config, bad, false).is_err());
        let stranger = Message {
            account: Some("Nobody".to_owned()),
            to: vec!["ana@example.com".to_owned()],
            ..Message::default()
        };
        let err = send(&mut Vec::new(), &mut store, &config, stranger, false).unwrap_err();
        assert_eq!(err.to_string(), "there is no account named \"Nobody\"");
        assert_eq!(store.outbox().unwrap().len(), 1);
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
        let message = Message {
            to: vec!["ana.ruiz@example.com".to_owned()],
            subject: "Stuck".to_owned(),
            ..Message::default()
        };
        send(&mut Vec::new(), &mut store, &config, message, false).unwrap();
        drop(store);
        let (fine, text) = run(false).await;
        assert!(!fine);
        assert!(text.contains("no server to send through"), "{text}");
    }
}
