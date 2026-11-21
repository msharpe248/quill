//! What can be done to the mail from outside Quill's window, tried on the
//! demo mailbox: looking, changing, taking back, and what the user's
//! settings refuse.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use jiff::tz::TimeZone;
use quill_core::config::AgentConfig;
use quill_store::Store;
use quill_tui::agent::{self, Call, Caller, Code, Outcome, Settings};
use quill_tui::data::DataEvent;
use serde_json::{Value, json};

const NOW: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z

fn store() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(quill_tui::demo::seed(&mut store, NOW))
        .unwrap();
    store
}

fn settings() -> Settings {
    Settings::new(AgentConfig::default(), TimeZone::UTC)
}

fn outcome(
    store: &mut Store,
    settings: &Settings,
    caller: &Caller,
    tool: &str,
    args: Value,
) -> Outcome {
    let call = Call::parse(tool, &args).unwrap_or_else(|refusal| panic!("{}", refusal.message));
    agent::perform(store, settings, caller, &call, NOW)
}

/// Asks from a shell and returns the answer, or panics with the refusal.
fn ask(store: &mut Store, settings: &Settings, tool: &str, args: Value) -> Value {
    match outcome(store, settings, &Caller::shell(), tool, args).result {
        Ok(answer) => answer.value,
        Err(refusal) => panic!("{tool} refused: {}", refusal.message),
    }
}

/// Asks from a shell and returns the refusal, or panics with the answer.
fn refused(store: &mut Store, settings: &Settings, tool: &str, args: Value) -> agent::Refusal {
    match outcome(store, settings, &Caller::shell(), tool, args).result {
        Ok(answer) => panic!("{tool} was done: {}", answer.value),
        Err(refusal) => refusal,
    }
}

fn subjects(value: &Value) -> Vec<String> {
    value["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["subject"].as_str().unwrap().to_owned())
        .collect()
}

/// The id of the one message a search finds.
fn id_of(store: &mut Store, query: &str) -> i64 {
    let found = ask(store, &settings(), "search", json!({ "query": query }));
    let messages = found["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{query} found {messages:?}");
    messages[0]["id"].as_i64().unwrap()
}

fn folder_of(store: &mut Store, id: i64) -> String {
    ask(store, &settings(), "read", json!({ "id": id }))["folder"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn the_accounts_and_their_folders_are_listed() {
    let mut store = store();
    let accounts = ask(&mut store, &settings(), "accounts", json!({}));
    assert_eq!(accounts["schema"], json!(1));
    let names: Vec<&str> = accounts["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|account| account["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Work", "Personal"]);

    let folders = ask(
        &mut store,
        &settings(),
        "folders",
        json!({ "account": "work" }),
    );
    let folders = folders["folders"].as_array().unwrap();
    assert!(folders.iter().all(|folder| folder["account"] == "Work"));
    let inbox = folders
        .iter()
        .find(|folder| folder["role"] == "inbox")
        .unwrap();
    assert_eq!(inbox["unread"], json!(8));
    assert_eq!(inbox["on_this_mac"], json!(false));
    assert_eq!(
        refused(
            &mut store,
            &settings(),
            "folders",
            json!({ "account": "nobody" })
        )
        .code,
        Code::NotFound
    );
}

#[test]
fn mail_is_listed_searched_counted_and_read() {
    let mut store = store();
    let listed = ask(&mut store, &settings(), "list", json!({ "limit": 3 }));
    assert_eq!(listed["messages"].as_array().unwrap().len(), 3);
    assert_eq!(listed["more"], json!(true));
    let unread = ask(&mut store, &settings(), "list", json!({ "unread": true }));
    assert_eq!(unread["messages"].as_array().unwrap().len(), 12);
    assert!(
        unread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["unread"] == true)
    );
    let sent = ask(
        &mut store,
        &settings(),
        "list",
        json!({ "in": "work/sent" }),
    );
    assert!(
        sent["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["folder"] == "Sent" && message["account"] == "Work")
    );

    let found = ask(
        &mut store,
        &settings(),
        "search",
        json!({ "query": "from:peter dinner" }),
    );
    assert_eq!(subjects(&found), ["Sunday dinner"]);
    let message = &found["messages"][0];
    assert_eq!(message["account"], "Personal");
    assert_eq!(message["from"][0]["address"], "peter.vale@example.com");
    let count = ask(
        &mut store,
        &settings(),
        "count",
        json!({ "query": "from:peter dinner" }),
    );
    assert_eq!(count["count"], json!(1));
    assert_eq!(
        refused(&mut store, &settings(), "search", json!({ "query": "  " })).code,
        Code::Invalid
    );

    let id = message["id"].as_i64().unwrap();
    let read = ask(&mut store, &settings(), "read", json!({ "id": id }));
    assert_eq!(read["subject"], "Sunday dinner");
    assert_eq!(read["downloaded"], json!(true));
    let body = read["body"].as_str().unwrap();
    assert!(body.contains("drill"), "{body}");
    assert_eq!(read["remaining_chars"], json!(0));
    assert_eq!(read["next_from_char"], Value::Null);
    assert_eq!(
        refused(&mut store, &settings(), "read", json!({ "id": 999_999 })).code,
        Code::NotFound
    );
}

#[test]
fn a_long_message_is_read_in_pieces_and_its_headers_when_asked() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    let whole = ask(
        &mut store,
        &settings(),
        "read",
        json!({ "id": id, "max_chars": 0 }),
    );
    let text = whole["body"].as_str().unwrap().to_owned();
    let total = text.chars().count();
    assert_eq!(whole["body_chars"], json!(total));

    let first = ask(
        &mut store,
        &settings(),
        "read",
        json!({ "id": id, "max_chars": 10 }),
    );
    assert_eq!(first["body"].as_str().unwrap().chars().count(), 10);
    assert_eq!(first["remaining_chars"], json!(total - 10));
    assert_eq!(first["next_from_char"], json!(10));
    let rest = ask(
        &mut store,
        &settings(),
        "read",
        json!({ "id": id, "max_chars": 0, "from_char": 10 }),
    );
    assert_eq!(
        format!(
            "{}{}",
            first["body"].as_str().unwrap(),
            rest["body"].as_str().unwrap()
        ),
        text
    );
    // Past the end there is nothing, and nothing fails.
    let past = ask(
        &mut store,
        &settings(),
        "read",
        json!({ "id": id, "from_char": 1_000_000 }),
    );
    assert_eq!(past["body"], "");

    let with = ask(
        &mut store,
        &settings(),
        "read",
        json!({ "id": id, "headers": true }),
    );
    let headers = with["headers"].as_str().unwrap();
    assert!(headers.contains("Subject: Sunday dinner"), "{headers}");
    assert!(!headers.contains("drill"));
}

#[test]
fn a_conversation_its_files_and_people_are_found() {
    let mut store = store();
    let id = id_of(&mut store, "subject:\"Offsite agenda\" from:priya");
    let thread = ask(&mut store, &settings(), "thread", json!({ "id": id }));
    let messages = thread["messages"].as_array().unwrap();
    assert!(!messages.is_empty());
    assert!(messages.iter().any(|message| message["id"] == json!(id)));
    // Oldest first.
    let dates: Vec<&str> = messages
        .iter()
        .map(|message| message["date"].as_str().unwrap())
        .collect();
    let mut sorted = dates.clone();
    sorted.sort_unstable();
    assert_eq!(dates, sorted);

    let with_files = ask(
        &mut store,
        &settings(),
        "search",
        json!({ "query": "has:attachment", "limit": 1 }),
    );
    let with_files = with_files["messages"][0]["id"].as_i64().unwrap();
    let files = ask(
        &mut store,
        &settings(),
        "attachments",
        json!({ "id": with_files }),
    );
    let files = files["attachments"].as_array().unwrap();
    assert!(!files.is_empty());
    assert!(
        files[0]["name"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
    );
    assert!(files[0]["size"].as_u64().unwrap() > 0);

    let dir = tempfile::tempdir().unwrap();
    let mut with_folder = settings();
    with_folder.files = Some(dir.path().join("files"));
    let part = files[0]["part"].as_u64().unwrap();
    let saved = ask(
        &mut store,
        &with_folder,
        "attachment_save",
        json!({ "id": with_files, "part": part }),
    );
    let path = std::path::PathBuf::from(saved["path"].as_str().unwrap());
    assert!(path.starts_with(dir.path()), "{}", path.display());
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        files[0]["size"].as_u64().unwrap()
    );
    // With nowhere to write, it says so.
    assert_eq!(
        refused(
            &mut store,
            &settings(),
            "attachment_save",
            json!({ "id": with_files, "part": part })
        )
        .code,
        Code::Invalid
    );

    let people = ask(
        &mut store,
        &settings(),
        "contacts",
        json!({ "words": "ana" }),
    );
    assert!(
        people["contacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|person| person["address"] == "ana.ruiz@example.com")
    );

    let status = ask(&mut store, &settings(), "status", json!({}));
    assert_eq!(
        status["accounts"][0],
        json!({ "name": "Work", "unread": 8 })
    );
    assert_eq!(
        status["accounts"][1],
        json!({ "name": "Personal", "unread": 4 })
    );
    assert_eq!(status["queued_changes"], json!(0));
}

#[test]
fn marks_are_given_logged_and_taken_back() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    let marked = ask(
        &mut store,
        &settings(),
        "mark",
        json!({ "ids": [id], "as": "read" }),
    );
    assert_eq!(marked["changed"], json!(1));
    assert_eq!(marked["messages"][0]["unread"], json!(false));
    let entry = marked["log"].as_i64().unwrap();

    // Asked again, there is nothing to do, and it says why.
    let again = ask(
        &mut store,
        &settings(),
        "mark",
        json!({ "ids": [id], "as": "read" }),
    );
    assert_eq!(again["changed"], json!(0));
    assert!(
        again["skipped"][0]["why"]
            .as_str()
            .unwrap()
            .contains("already")
    );
    assert_eq!(again["log"], Value::Null);

    let log = ask(&mut store, &settings(), "log", json!({}));
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["entry"], json!(entry));
    assert_eq!(entries[0]["action"], "mark");
    assert_eq!(entries[0]["can_undo"], json!(true));
    assert_eq!(entries[0]["way"], "shell");

    let undone = ask(&mut store, &settings(), "undo", json!({}));
    assert_eq!(undone["entry"], json!(entry));
    assert_eq!(undone["messages"][0]["unread"], json!(true));
    // Once, and not twice.
    assert_eq!(
        refused(&mut store, &settings(), "undo", json!({ "entry": entry })).code,
        Code::CannotUndo
    );
    assert_eq!(
        refused(&mut store, &settings(), "undo", json!({})).code,
        Code::CannotUndo
    );
    assert_eq!(
        refused(&mut store, &settings(), "undo", json!({ "entry": 999 })).code,
        Code::NotFound
    );
}

#[test]
fn mail_is_moved_by_a_search_and_put_back_by_undo() {
    let mut store = store();
    let before = ask(
        &mut store,
        &settings(),
        "count",
        json!({ "query": "in:work/inbox from:ana" }),
    );
    let wanted = before["count"].as_u64().unwrap();
    assert!(wanted >= 1);

    // Looked at first: nothing changes.
    let dry = ask(
        &mut store,
        &settings(),
        "move",
        json!({ "where": "in:work/inbox from:ana", "to": "archive", "dry_run": true }),
    );
    assert_eq!(dry["changed"], json!(0));
    assert_eq!(dry["would_change"], json!(wanted));
    assert_eq!(dry["log"], Value::Null);
    // A folder is found by the whole name one of Google's own has on
    // Google's servers too, which is how it was shown once.
    let whole = ask(
        &mut store,
        &settings(),
        "move",
        json!({ "where": "in:work/inbox from:ana", "to": "[Gmail]/Archive", "dry_run": true }),
    );
    assert_eq!(whole["would_change"], json!(wanted));
    let still = ask(
        &mut store,
        &settings(),
        "count",
        json!({ "query": "in:work/inbox from:ana" }),
    );
    assert_eq!(still["count"], json!(wanted));

    let moved = ask(
        &mut store,
        &settings(),
        "move",
        json!({ "where": "in:work/inbox from:ana", "to": "archive" }),
    );
    assert_eq!(moved["changed"], json!(wanted));
    assert!(
        moved["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["folder"] == "Archive")
    );
    let gone = ask(
        &mut store,
        &settings(),
        "count",
        json!({ "query": "in:work/inbox from:ana" }),
    );
    assert_eq!(gone["count"], json!(0));
    let status = ask(&mut store, &settings(), "status", json!({}));
    assert_eq!(status["queued_changes"], json!(wanted));

    let undone = ask(
        &mut store,
        &settings(),
        "undo",
        json!({ "entry": moved["log"] }),
    );
    assert_eq!(undone["changed"], json!(wanted));
    let back = ask(
        &mut store,
        &settings(),
        "count",
        json!({ "query": "in:work/inbox from:ana" }),
    );
    assert_eq!(back["count"], json!(wanted));

    // A folder the account does not have moves nothing, and says so.
    let nowhere = ask(
        &mut store,
        &settings(),
        "move",
        json!({ "where": "in:work/inbox from:ana", "to": "receipts" }),
    );
    assert_eq!(nowhere["changed"], json!(0));
    assert!(
        nowhere["skipped"][0]["why"]
            .as_str()
            .unwrap()
            .contains("no folder")
    );
    // Another account's folder is not one of this message's.
    let other = ask(
        &mut store,
        &settings(),
        "move",
        json!({ "where": "in:work/inbox from:ana", "to": "personal/archive" }),
    );
    assert_eq!(other["changed"], json!(0));
}

#[test]
fn deleting_goes_to_the_trash_and_never_further() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    let deleted = ask(&mut store, &settings(), "delete", json!({ "ids": [id] }));
    assert_eq!(deleted["changed"], json!(1));
    assert_eq!(folder_of(&mut store, id), "Trash");

    // In the trash, deleting would be for good: it is left alone.
    let again = ask(&mut store, &settings(), "delete", json!({ "ids": [id] }));
    assert_eq!(again["changed"], json!(0));
    assert!(
        again["skipped"][0]["why"]
            .as_str()
            .unwrap()
            .contains("for good")
    );
    assert_eq!(folder_of(&mut store, id), "Trash");

    // Put back where it came from.
    let restored = ask(&mut store, &settings(), "restore", json!({ "ids": [id] }));
    assert_eq!(restored["changed"], json!(1));
    assert_eq!(folder_of(&mut store, id), "Inbox");
    // Taking the putting back back leaves it in the trash, and where it
    // came from is still known.
    ask(
        &mut store,
        &settings(),
        "undo",
        json!({ "entry": restored["log"] }),
    );
    assert_eq!(folder_of(&mut store, id), "Trash");
    ask(&mut store, &settings(), "restore", json!({ "ids": [id] }));
    assert_eq!(folder_of(&mut store, id), "Inbox");

    // What is not in a trash folder is not put back.
    let not = ask(&mut store, &settings(), "restore", json!({ "ids": [id] }));
    assert_eq!(not["changed"], json!(0));

    // Deleted and taken back by undo.
    let deleted = ask(&mut store, &settings(), "delete", json!({ "ids": [id] }));
    ask(
        &mut store,
        &settings(),
        "undo",
        json!({ "entry": deleted["log"] }),
    );
    assert_eq!(folder_of(&mut store, id), "Inbox");
}

#[test]
fn a_search_that_finds_too_many_changes_none_until_told_to() {
    let mut store = store();
    let mut careful = settings();
    careful.agent.bulk_limit = 3;
    let found = ask(
        &mut store,
        &careful,
        "count",
        json!({ "query": "is:unread" }),
    );
    let unread = found["count"].as_u64().unwrap();
    assert!(unread > 3);

    let refusal = refused(
        &mut store,
        &careful,
        "mark",
        json!({ "where": "is:unread", "as": "read" }),
    );
    assert_eq!(refusal.code, Code::OverLimit);
    assert!(
        refusal.message.contains(&unread.to_string()),
        "{}",
        refusal.message
    );
    let still = ask(
        &mut store,
        &careful,
        "count",
        json!({ "query": "is:unread" }),
    );
    assert_eq!(still["count"], json!(unread));

    // A dry run is always allowed, and says how many.
    let dry = ask(
        &mut store,
        &careful,
        "mark",
        json!({ "where": "is:unread", "as": "read", "dry_run": true }),
    );
    assert_eq!(dry["would_change"], json!(unread));

    let done = ask(
        &mut store,
        &careful,
        "mark",
        json!({ "where": "is:unread", "as": "read", "confirm": true }),
    );
    assert_eq!(done["changed"], json!(unread));
    assert_eq!(done["ids"].as_array().unwrap().len() as u64, unread);

    // Named one by one, messages are not held to the limit.
    let ids = done["ids"].clone();
    let back = ask(
        &mut store,
        &careful,
        "mark",
        json!({ "ids": ids, "as": "unread" }),
    );
    assert_eq!(back["changed"], json!(unread));

    // Nothing named is a mistake, not every message there is.
    assert_eq!(
        refused(&mut store, &careful, "delete", json!({})).code,
        Code::Invalid
    );
    assert_eq!(
        refused(&mut store, &careful, "delete", json!({ "where": "  " })).code,
        Code::Invalid
    );
    // An id that names nothing is said, and is not an error.
    let none = ask(&mut store, &careful, "delete", json!({ "ids": [999_999] }));
    assert_eq!(none["changed"], json!(0));
    assert_eq!(none["missing"], json!(["999999"]));
}

#[test]
fn messages_are_named_by_message_id() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    let read = ask(&mut store, &settings(), "read", json!({ "id": id }));
    let msgid = read["message_id"].as_str().unwrap().to_owned();
    let starred = ask(
        &mut store,
        &settings(),
        "mark",
        json!({ "msgids": [msgid], "as": "starred" }),
    );
    assert_eq!(starred["ids"], json!([id]));
    assert_eq!(starred["messages"][0]["starred"], json!(true));
    let none = ask(
        &mut store,
        &settings(),
        "mark",
        json!({ "msgids": ["<nobody@example.com>"], "as": "starred" }),
    );
    assert_eq!(none["missing"], json!(["<nobody@example.com>"]));
}

#[test]
fn mail_is_snoozed_until_a_time_said_in_words() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    let refusal = refused(
        &mut store,
        &settings(),
        "snooze",
        json!({ "ids": [id], "until": "yesterday" }),
    );
    assert_eq!(refusal.code, Code::Invalid);
    assert!(refusal.message.contains("Such as"), "{}", refusal.message);

    let snoozed = ask(
        &mut store,
        &settings(),
        "snooze",
        json!({ "ids": [id], "until": "2h" }),
    );
    assert_eq!(snoozed["changed"], json!(1));
    assert_eq!(snoozed["until"], "2026-11-14T11:30:00Z");
    assert_eq!(
        snoozed["messages"][0]["snoozed_until"],
        "2026-11-14T11:30:00Z"
    );

    let back = ask(&mut store, &settings(), "unsnooze", json!({ "ids": [id] }));
    assert_eq!(back["changed"], json!(1));
    assert_eq!(back["messages"][0]["snoozed_until"], Value::Null);
    let not = ask(&mut store, &settings(), "unsnooze", json!({ "ids": [id] }));
    assert_eq!(not["changed"], json!(0));

    // Taking the bringing back back snoozes it again, until when it was.
    ask(
        &mut store,
        &settings(),
        "undo",
        json!({ "entry": back["log"] }),
    );
    let read = ask(&mut store, &settings(), "read", json!({ "id": id }));
    assert_eq!(read["snoozed_until"], "2026-11-14T11:30:00Z");
}

#[test]
fn a_folder_is_made_on_this_mac_and_unmade() {
    let mut store = store();
    assert_eq!(
        refused(
            &mut store,
            &settings(),
            "folder_create",
            json!({ "name": "Receipts" })
        )
        .code,
        Code::Invalid
    );
    let made = ask(
        &mut store,
        &settings(),
        "folder_create",
        json!({ "name": "Receipts", "account": "work" }),
    );
    assert_eq!(made["on_this_mac"], json!(true));
    let folders = ask(
        &mut store,
        &settings(),
        "folders",
        json!({ "account": "Work" }),
    );
    let receipts = folders["folders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|folder| folder["name"] == "Receipts")
        .unwrap();
    assert_eq!(receipts["on_this_mac"], json!(true));

    ask(
        &mut store,
        &settings(),
        "undo",
        json!({ "entry": made["log"] }),
    );
    let folders = ask(
        &mut store,
        &settings(),
        "folders",
        json!({ "account": "Work" }),
    );
    assert!(
        folders["folders"]
            .as_array()
            .unwrap()
            .iter()
            .all(|folder| folder["name"] != "Receipts")
    );
}

#[test]
fn what_the_user_switched_off_is_refused_and_said_plainly() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");

    let mut look_only = settings();
    look_only.agent.organize = false;
    look_only.agent.delete = false;
    let refusal = refused(
        &mut store,
        &look_only,
        "mark",
        json!({ "ids": [id], "as": "read" }),
    );
    assert_eq!(refusal.code, Code::SwitchedOff);
    assert!(
        refusal.message.contains("Organizing"),
        "{}",
        refusal.message
    );
    assert!(refusal.message.contains("(Agents)"));
    let refusal = refused(&mut store, &look_only, "delete", json!({ "ids": [id] }));
    assert!(refusal.message.contains("Deleting"), "{}", refusal.message);
    // Looking goes on.
    ask(&mut store, &look_only, "read", json!({ "id": id }));
    assert_eq!(folder_of(&mut store, id), "Inbox");

    // The way in itself.
    let mut closed = settings();
    closed.agent.commands = false;
    let refusal = refused(&mut store, &closed, "list", json!({}));
    assert_eq!(refusal.code, Code::SwitchedOff);
    assert!(
        refusal.message.contains("from a shell"),
        "{}",
        refusal.message
    );

    // MCP is off as Quill comes, and on when switched on.
    let claude = Caller::mcp("Claude Code");
    let off = outcome(&mut store, &settings(), &claude, "list", json!({}));
    let refusal = off.result.unwrap_err();
    assert_eq!(refusal.code, Code::SwitchedOff);
    assert!(refusal.message.contains("MCP"), "{}", refusal.message);
    let mut open = settings();
    open.agent.mcp = true;
    assert!(
        outcome(&mut store, &open, &claude, "list", json!({}))
            .result
            .is_ok()
    );

    // What was turned away is in the log, for whoever looks.
    let log = ask(&mut store, &settings(), "log", json!({ "all": true }));
    let summaries: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["summary"].as_str().unwrap())
        .collect();
    assert!(
        summaries
            .iter()
            .any(|summary| summary.starts_with("refused:") && summary.contains("Deleting")),
        "{summaries:?}"
    );
}

#[test]
fn accounts_out_of_sight_do_not_exist() {
    let mut store = store();
    let dinner = id_of(&mut store, "from:peter dinner");
    let mut work_only = settings();
    work_only.agent.accounts = Some(vec!["work".to_owned()]);

    let accounts = ask(&mut store, &work_only, "accounts", json!({}));
    assert_eq!(accounts["accounts"].as_array().unwrap().len(), 1);
    let found = ask(
        &mut store,
        &work_only,
        "search",
        json!({ "query": "dinner" }),
    );
    assert!(
        subjects(&found)
            .iter()
            .all(|subject| subject != "Sunday dinner")
    );
    let listed = ask(&mut store, &work_only, "list", json!({ "limit": 500 }));
    assert!(
        listed["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["account"] == "Work")
    );
    assert_eq!(
        refused(&mut store, &work_only, "read", json!({ "id": dinner })).code,
        Code::NotFound
    );
    assert_eq!(
        refused(
            &mut store,
            &work_only,
            "folders",
            json!({ "account": "Personal" })
        )
        .code,
        Code::NotFound
    );
    let deleted = ask(&mut store, &work_only, "delete", json!({ "ids": [dinner] }));
    assert_eq!(deleted["changed"], json!(0));
    assert_eq!(deleted["missing"], json!([dinner.to_string()]));
    assert_eq!(folder_of(&mut store, dinner), "Inbox");
    let status = ask(&mut store, &work_only, "status", json!({}));
    assert_eq!(status["accounts"].as_array().unwrap().len(), 1);
}

#[test]
fn what_was_looked_at_is_logged_without_what_it_said() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    ask(&mut store, &settings(), "read", json!({ "id": id }));
    ask(
        &mut store,
        &settings(),
        "mark",
        json!({ "ids": [id], "as": "starred" }),
    );

    // The changes alone, as the log is asked for.
    let changes = ask(&mut store, &settings(), "log", json!({}));
    assert_eq!(changes["entries"].as_array().unwrap().len(), 1);

    let all = ask(&mut store, &settings(), "log", json!({ "all": true }));
    let entries = all["entries"].as_array().unwrap();
    let actions: Vec<&str> = entries
        .iter()
        .map(|entry| entry["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["mark", "read", "search"]);
    let read = &entries[1];
    assert_eq!(read["change"], json!(false));
    assert_eq!(read["summary"], format!("message {id}"));
    // The search's words are there; nothing of the mail is.
    let whole = all.to_string();
    assert!(whole.contains("from:peter dinner"));
    assert!(!whole.contains("drill"));
    assert!(!whole.contains("Sunday"));
}

#[test]
fn a_change_is_said_in_quill_with_who_made_it_and_a_look_is_not() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    let mut open = settings();
    open.agent.mcp = true;
    let claude = Caller::mcp("Claude Code\u{1b}[31m");
    // An escape sequence in a name is taken out whole.
    assert_eq!(claude.name, "Claude Code");

    let looked = outcome(&mut store, &open, &claude, "read", json!({ "id": id }));
    assert!(looked.events.is_empty());
    assert_eq!(looked.notice, None);

    let moved = outcome(
        &mut store,
        &open,
        &claude,
        "archive",
        json!({ "ids": [id] }),
    );
    assert!(moved.events.contains(&DataEvent::StoreChanged));
    assert!(moved.events.contains(&DataEvent::Queued));
    let said = moved
        .events
        .iter()
        .find_map(|event| match event {
            DataEvent::Flash { text, undoable } => Some((text.clone(), *undoable)),
            _ => None,
        })
        .unwrap();
    assert_eq!(said.0, "Claude Code: archived 1 message.");
    assert!(said.1);
    let notice = moved.notice.unwrap();
    assert_eq!(notice.text, said.0);
    assert!(!notice.send);

    // Nothing changed, nothing said.
    let again = outcome(
        &mut store,
        &open,
        &claude,
        "archive",
        json!({ "ids": [id] }),
    );
    assert!(again.events.is_empty());
    assert_eq!(again.notice, None);
    // The log has who it was.
    let log = ask(&mut store, &settings(), "log", json!({}));
    assert_eq!(log["entries"][0]["agent"], "Claude Code");
    assert_eq!(log["entries"][0]["way"], "mcp");
}

#[test]
fn the_answers_for_a_terminal_are_safe_to_print() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    for (tool, args) in [
        ("accounts", json!({})),
        ("folders", json!({})),
        ("list", json!({})),
        ("read", json!({ "id": id, "headers": true })),
        ("status", json!({})),
        ("archive", json!({ "ids": [id] })),
        ("log", json!({ "all": true })),
    ] {
        let answer = outcome(&mut store, &settings(), &Caller::shell(), tool, args)
            .result
            .unwrap();
        assert!(!answer.lines.is_empty(), "{tool}");
        for line in &answer.lines {
            assert!(
                !line.chars().any(|c| c.is_control()),
                "{tool} would print a control character: {line:?}"
            );
        }
    }
}

fn sending() -> Settings {
    let mut settings = settings();
    settings.agent.send = true;
    settings
}

fn letter(to: &str, subject: &str) -> Value {
    json!({ "to": [to], "subject": subject, "body": "Yes, Thursday.\n", "account": "Work" })
}

#[test]
fn a_draft_is_written_changed_and_discarded_but_never_the_users_own() {
    let mut store = store();
    let made = ask(
        &mut store,
        &settings(),
        "draft_new",
        json!({ "to": ["ana.ruiz@example.com"], "subject": "Thursday", "body": "Yes." }),
    );
    let id = made["draft"].as_i64().unwrap();
    assert_eq!(made["state"], "draft");
    assert_eq!(made["written_by"], "A shell command");
    assert_eq!(made["account"], "Work");

    let listed = ask(&mut store, &settings(), "drafts", json!({}));
    assert_eq!(listed["drafts"][0]["draft"], json!(id));
    assert!(listed["drafts"][0].get("body").is_none());
    let shown = ask(&mut store, &settings(), "draft_show", json!({ "id": id }));
    assert_eq!(shown["body"], "Yes.");

    let changed = ask(
        &mut store,
        &settings(),
        "draft_edit",
        json!({ "id": id, "subject": "Friday", "cc": ["kit@example.com"] }),
    );
    assert_eq!(changed["subject"], "Friday");
    assert_eq!(changed["cc"], "kit@example.com");
    // What was not named is as it was.
    assert_eq!(changed["to"], "ana.ruiz@example.com");
    let shown = ask(&mut store, &settings(), "draft_show", json!({ "id": id }));
    assert_eq!(shown["body"], "Yes.");

    // A draft the user wrote in Quill is theirs.
    let account = store.accounts().unwrap()[0].id;
    let theirs = store
        .save_draft(
            None,
            account,
            &quill_store::DraftContent {
                subject: "Mine".to_owned(),
                body: "Half written".to_owned(),
                ..Default::default()
            },
            NOW,
        )
        .unwrap()
        .unwrap()
        .0;
    for (tool, args) in [
        ("draft_discard", json!({ "id": theirs })),
        ("draft_edit", json!({ "id": theirs, "body": "gone" })),
    ] {
        let refusal = refused(&mut store, &settings(), tool, args);
        assert!(refusal.message.contains("in Quill"), "{}", refusal.message);
    }
    // But it can be read.
    let read = ask(
        &mut store,
        &settings(),
        "draft_show",
        json!({ "id": theirs }),
    );
    assert_eq!(read["body"], "Half written");
    assert_eq!(read["written_by"], Value::Null);

    ask(
        &mut store,
        &settings(),
        "draft_discard",
        json!({ "id": id }),
    );
    assert_eq!(
        refused(&mut store, &settings(), "draft_show", json!({ "id": id })).code,
        Code::NotFound
    );
    assert_eq!(store.drafts().unwrap().len(), 1);

    // With writing drafts switched off, none of it.
    let mut off = settings();
    off.agent.draft = false;
    let refusal = refused(&mut store, &off, "draft_new", json!({ "body": "x" }));
    assert_eq!(refusal.code, Code::SwitchedOff);
    assert!(refusal.message.contains("drafts"), "{}", refusal.message);
}

#[test]
fn a_reply_is_drafted_addressed_and_threaded_with_the_original_quoted() {
    let mut store = store();
    let id = id_of(&mut store, "from:peter dinner");
    let made = ask(
        &mut store,
        &settings(),
        "draft_reply",
        json!({ "id": id, "body": "We are coming, and bringing the drill." }),
    );
    assert_eq!(made["to"], "Peter Vale <peter.vale@example.com>");
    assert_eq!(made["subject"], "Re: Sunday dinner");
    assert_eq!(made["account"], "Personal");
    let draft = store
        .draft(quill_store::DraftId(made["draft"].as_i64().unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(
        draft.content.in_reply_to.as_deref(),
        Some("<personal.inbox-1@example.com>")
    );
    let body = &draft.content.body;
    let answer = body.find("We are coming").unwrap();
    let quoted = body.find("> ").unwrap();
    // Above what it answers, as the user's own replies are.
    assert!(answer < quoted, "{body}");
    assert!(body.contains("drill"));
    assert_eq!(
        refused(
            &mut store,
            &settings(),
            "draft_reply",
            json!({ "id": 999_999, "body": "x" })
        )
        .code,
        Code::NotFound
    );
}

#[test]
fn sending_is_off_until_switched_on_and_then_goes_through_the_outbox() {
    let mut store = store();
    let refusal = refused(
        &mut store,
        &settings(),
        "send",
        letter("ana.ruiz@example.com", "Thursday"),
    );
    assert_eq!(refusal.code, Code::SwitchedOff);
    assert!(refusal.message.contains("Sending"), "{}", refusal.message);
    assert!(store.outbox().unwrap().is_empty());

    let sent = ask(
        &mut store,
        &sending(),
        "send",
        letter("ana.ruiz@example.com", "Thursday"),
    );
    assert_eq!(sent["state"], "waiting");
    assert_eq!(sent["held"], json!(false));
    assert_eq!(sent["send_at"], "2026-11-14T09:30:00Z");
    let outbox = store.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].agent.as_deref(), Some("A shell command"));
    assert_eq!(outbox[0].send_after, Some(NOW));

    // What the compose screen would refuse is refused, and leaves nothing
    // behind.
    let refusal = refused(
        &mut store,
        &sending(),
        "send",
        json!({ "body": "to nobody" }),
    );
    assert_eq!(refusal.code, Code::Invalid);
    assert!(refusal.message.contains("nobody to send it to"));
    assert!(store.drafts().unwrap().is_empty());
    assert_eq!(store.outbox().unwrap().len(), 1);

    // The log says who it went to, and not what it said.
    let log = ask(&mut store, &settings(), "log", json!({}));
    let line = log["entries"][0]["summary"].as_str().unwrap();
    assert!(line.contains("ana.ruiz@example.com"), "{line}");
    assert!(!line.contains("Thursday"), "{line}");
}

#[test]
fn messages_sent_one_after_another_keep_the_wait_between_them() {
    let mut store = store();
    let first = ask(
        &mut store,
        &sending(),
        "send",
        letter("ana.ruiz@example.com", "One"),
    );
    assert_eq!(first["send_at"], "2026-11-14T09:30:00Z");
    // The second no sooner than thirty seconds after the first, and the
    // third after that.
    let second = ask(
        &mut store,
        &sending(),
        "send",
        letter("ana.ruiz@example.com", "Two"),
    );
    assert_eq!(second["send_at"], "2026-11-14T09:30:30Z");
    let third = ask(
        &mut store,
        &sending(),
        "send",
        letter("ana.ruiz@example.com", "Three"),
    );
    assert_eq!(third["send_at"], "2026-11-14T09:31:00Z");

    // One taken back leaves the others where they were.
    ask(
        &mut store,
        &sending(),
        "outbox_cancel",
        json!({ "id": second["draft"] }),
    );
    let outbox = ask(&mut store, &sending(), "outbox", json!({}));
    assert_eq!(outbox["outbox"].as_array().unwrap().len(), 2);
    assert_eq!(store.drafts().unwrap().len(), 1);

    // With no wait set, each is due at once.
    let mut no_wait = sending();
    no_wait.agent.send_gap = 0;
    let fourth = ask(
        &mut store,
        &no_wait,
        "send",
        letter("ana.ruiz@example.com", "Four"),
    );
    assert_eq!(fourth["send_at"], "2026-11-14T09:30:00Z");

    // A time asked for is kept, when it is later than the line.
    let later = ask(
        &mut store,
        &sending(),
        "send",
        json!({ "to": ["ana.ruiz@example.com"], "subject": "Later", "body": "x", "at": "2h" }),
    );
    assert_eq!(later["send_at"], "2026-11-14T11:30:00Z");
    let refusal = refused(
        &mut store,
        &sending(),
        "send",
        json!({ "to": ["ana.ruiz@example.com"], "body": "x", "at": "yesterday" }),
    );
    assert_eq!(refusal.code, Code::Invalid);
}

#[test]
fn no_more_than_twenty_messages_wait_their_turn() {
    let mut store = store();
    let mut slow = sending();
    slow.agent.send_gap = 600;
    for number in 0..21 {
        ask(
            &mut store,
            &slow,
            "send",
            letter("ana.ruiz@example.com", &format!("Number {number}")),
        );
    }
    // The first went at once; twenty wait; the next is turned away.
    let refusal = refused(
        &mut store,
        &slow,
        "send",
        letter("ana.ruiz@example.com", "Too many"),
    );
    assert_eq!(refusal.code, Code::OverLimit);
    assert!(
        refusal.message.contains("20 messages"),
        "{}",
        refusal.message
    );
    assert_eq!(store.outbox().unwrap().len(), 21);
    assert!(store.drafts().unwrap().is_empty());
}

#[test]
fn held_mail_waits_for_the_user_and_goes_when_they_say() {
    let mut store = store();
    let mut holding = sending();
    holding.agent.hold_sends = true;
    let claude = Caller::mcp("Claude Code");
    holding.agent.mcp = true;
    let outcome = outcome(
        &mut store,
        &holding,
        &claude,
        "send",
        letter("ana.ruiz@example.com", "Thursday"),
    );
    let answer = outcome.result.unwrap();
    assert_eq!(answer.value["state"], "held");
    assert_eq!(answer.value["held"], json!(true));
    assert!(answer.lines[0].contains("waiting for the user's OK"));
    // The user is told, as of a message sent.
    let notice = outcome.notice.unwrap();
    assert!(notice.send);
    assert!(
        notice.text.contains("waiting in the Outbox for your OK"),
        "{}",
        notice.text
    );

    let draft = quill_store::DraftId(answer.value["draft"].as_i64().unwrap());
    let row = store.draft(draft).unwrap().unwrap();
    assert_eq!(row.state, quill_store::DraftState::Held);
    assert_eq!(row.agent.as_deref(), Some("Claude Code"));
    // The engine never takes it, however long it waits.
    assert!(
        store
            .claim_send(row.account, NOW + 86_400)
            .unwrap()
            .is_none()
    );
    // A held message keeps no place in the line: the next is not made to
    // wait for it.
    let mut not_holding = sending();
    not_holding.agent.send_gap = 30;
    let next = ask(
        &mut store,
        &not_holding,
        "send",
        letter("ana.ruiz@example.com", "Other"),
    );
    assert_eq!(next["send_at"], "2026-11-14T09:30:00Z");
    let status = ask(&mut store, &settings(), "status", json!({}));
    assert_eq!(status["outbox"]["held"], json!(1));

    // The user lets it go, from the Outbox.
    assert!(store.send_now(draft, NOW + 5).unwrap());
    assert_eq!(
        store.draft(draft).unwrap().unwrap().state,
        quill_store::DraftState::Outbox
    );
    assert!(store.claim_send(row.account, NOW + 5).unwrap().is_some());
}

#[test]
fn a_message_sent_is_taken_back_while_it_has_not_gone_and_stopping_takes_all_back() {
    let mut store = store();
    let first = ask(
        &mut store,
        &sending(),
        "send",
        letter("ana.ruiz@example.com", "One"),
    );
    let second = ask(
        &mut store,
        &sending(),
        "send",
        letter("ana.ruiz@example.com", "Two"),
    );
    // Undo takes back the last.
    let undone = ask(&mut store, &sending(), "undo", json!({}));
    assert_eq!(undone["draft"], second["draft"]);
    assert_eq!(store.outbox().unwrap().len(), 1);
    // One that has gone cannot be.
    let account = store.accounts().unwrap()[0].id;
    let going = store.claim_send(account, NOW).unwrap().unwrap();
    store.sent(going.id).unwrap();
    let refusal = refused(
        &mut store,
        &sending(),
        "undo",
        json!({ "entry": first["log"] }),
    );
    assert_eq!(refusal.code, Code::CannotUndo);

    // The user stops the agents: what had not gone is a draft again, and
    // their own mail in the outbox is left to go.
    let mut holding = sending();
    holding.agent.hold_sends = true;
    ask(
        &mut store,
        &holding,
        "send",
        letter("ana.ruiz@example.com", "Held"),
    );
    ask(
        &mut store,
        &sending(),
        "send",
        letter("ana.ruiz@example.com", "Waiting"),
    );
    let own = store
        .save_draft(
            None,
            account,
            &quill_store::DraftContent {
                to: "ana.ruiz@example.com".to_owned(),
                ..Default::default()
            },
            NOW,
        )
        .unwrap()
        .unwrap();
    assert!(
        store
            .queue_send(
                own,
                "hash",
                "robin@example.com",
                &["ana.ruiz@example.com".to_owned()],
                NOW + 60,
                NOW
            )
            .unwrap()
    );
    let events = agent::stop(&mut store, NOW).unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        DataEvent::Flash { text, .. } if text.starts_with("2 messages that agents sent")
    )));
    let outbox = store.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].id, own);
    // Nothing to take back is nothing to say.
    assert!(agent::stop(&mut store, NOW).unwrap().is_empty());
}
