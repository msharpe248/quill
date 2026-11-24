//! What is asked from outside Quill's window that takes more than the
//! store to do: a message shown in the window, a folder made on a server,
//! a mailing list's server asked, mail watched for as it arrives. The
//! store's side checks, writes down and says what is still to be done
//! ([`Then`]); with no Quill running, it says that instead. And the rest of
//! what writes mail: a forward, an answer to an invitation.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use jiff::tz::TimeZone;
use quill_core::config::AgentConfig;
use quill_core::types::{
    Address, Flags, FolderRole, Headers, LocalMessageId, RemoteId, RemoteMessage, SyncCursor,
    SyncDelta,
};
use quill_store::{BlobStore, DraftId, DraftState, Store};
use quill_tui::agent::{self, Call, Caller, Code, Outcome, Settings, Then, Way};
use serde_json::{Value, json};

const NOW: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z

fn store() -> Store {
    let mut store = Store::open_in_memory()
        .unwrap()
        .with_blobs(BlobStore::in_memory());
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(quill_tui::demo::seed(&mut store, NOW))
        .unwrap();
    store
}

/// As a command finds things with no Quill running.
fn alone() -> Settings {
    Settings::new(AgentConfig::default(), TimeZone::UTC)
}

/// As the Quill that is running has them, with its engine and service.
fn running() -> Settings {
    Settings {
        running: true,
        online: true,
        ..alone()
    }
}

fn sending(mut settings: Settings) -> Settings {
    settings.agent.send = true;
    settings.agent.send_gap = 0;
    settings
}

fn outcome(store: &mut Store, settings: &Settings, tool: &str, args: Value) -> Outcome {
    let call = Call::parse(tool, &args).unwrap_or_else(|refusal| panic!("{}", refusal.message));
    agent::perform(store, settings, &Caller::shell(), &call, NOW)
}

fn done(store: &mut Store, settings: &Settings, tool: &str, args: Value) -> (Value, Option<Then>) {
    let outcome = outcome(store, settings, tool, args);
    match outcome.result {
        Ok(answer) => (answer.value, outcome.then),
        Err(refusal) => panic!("{tool} refused: {}", refusal.message),
    }
}

fn refused(store: &mut Store, settings: &Settings, tool: &str, args: Value) -> agent::Refusal {
    let outcome = outcome(store, settings, tool, args);
    // What is refused leaves nothing to be done by anyone.
    assert_eq!(outcome.then, None);
    match outcome.result {
        Ok(answer) => panic!("{tool} was done: {}", answer.value),
        Err(refusal) => refusal,
    }
}

/// Puts a message of our own at the top of the Work inbox, downloaded.
fn arrive(store: &mut Store, name: &str, subject: &str, raw: &str) -> i64 {
    let inbox = store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    let remote = RemoteId(name.to_owned());
    store
        .apply_delta(
            inbox,
            &SyncDelta {
                reset: false,
                added: vec![RemoteMessage {
                    id: remote.clone(),
                    headers: Headers {
                        message_id: Some(format!("<{name}@example.com>")),
                        subject: subject.to_owned(),
                        from: vec![Address::new("Ana Ruiz", "ana.ruiz@example.com")],
                        to: vec![Address::new("Robin Vale", "robin@example.com")],
                        cc: Vec::new(),
                        date: NOW + 60,
                    },
                    flags: Flags::default(),
                    size: raw.len() as u64,
                    has_attachments: false,
                    thread: Vec::new(),
                }],
                changed: Vec::new(),
                removed: Vec::new(),
                has_more: false,
                next_cursor: SyncCursor(name.to_owned()),
            },
        )
        .unwrap();
    let id = store.local_message_id(inbox, &remote).unwrap().unwrap();
    let text = quill_mime::parse(raw.as_bytes(), NOW).unwrap().body;
    store.put_message(id, &text, raw.as_bytes()).unwrap();
    id.0
}

/// A message from a mailing list, with the header that says how to leave.
fn from_a_list(leave: &str) -> String {
    format!(
        "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
         To: Robin Vale <robin@example.com>\r\n\
         Subject: The weekly digest\r\n\
         Message-ID: <digest-1@example.com>\r\n\
         {leave}\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         News of the week.\r\n"
    )
}

fn log_of(store: &mut Store) -> Vec<String> {
    let (log, _) = done(store, &alone(), "log", json!({ "all": true }));
    log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["summary"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_message_is_shown_in_the_window_of_the_quill_that_is_running() {
    let mut store = store();
    let (found, _) = done(
        &mut store,
        &alone(),
        "search",
        json!({ "query": "from:peter dinner" }),
    );
    let id = found["messages"][0]["id"].as_i64().unwrap();

    let (answer, then) = done(&mut store, &running(), "open", json!({ "id": id }));
    assert_eq!(answer["action"], "open");
    let Some(Then::Show { message, folder }) = then else {
        panic!("{then:?}");
    };
    assert_eq!(message, LocalMessageId(id));
    assert_eq!(
        store.message_location(message).unwrap().unwrap().folder,
        folder
    );
    assert!(log_of(&mut store)[0].contains("showed message"));

    // With no Quill running there is no window: said, and nothing done.
    assert_eq!(
        refused(&mut store, &alone(), "open", json!({ "id": id })).code,
        Code::NotRunning
    );
    assert_eq!(
        refused(&mut store, &running(), "open", json!({ "id": 999_999 })).code,
        Code::NotFound
    );
    // The window marks what it opens read, so this is organizing.
    let mut looking = running();
    looking.agent.organize = false;
    assert_eq!(
        refused(&mut store, &looking, "open", json!({ "id": id })).code,
        Code::SwitchedOff
    );
}

#[test]
fn a_folder_is_asked_of_the_server_or_made_on_this_mac() {
    let mut store = store();
    // An account whose server keeps folders, as a real one's does: the
    // demo mailbox's own have no server at all.
    let work = store
        .accounts()
        .unwrap()
        .into_iter()
        .find(|account| account.name == "Work")
        .unwrap();
    assert!(!work.has_server_folders());
    store
        .upsert_account(&work.name, &work.address, "imap")
        .unwrap();
    let folders = |store: &mut Store| -> Vec<String> {
        let (listed, _) = done(store, &alone(), "folders", json!({ "account": "Work" }));
        listed["folders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|folder| folder["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let before = folders(&mut store);

    // On the server: checked and written down here, and made by the
    // engine, so the store has nothing new yet.
    let (answer, then) = done(
        &mut store,
        &running(),
        "folder_create",
        json!({ "name": " Projects/2026 ", "account": "work" }),
    );
    assert_eq!(answer["on_this_mac"], false);
    assert_eq!(answer["name"], "Projects/2026");
    let Some(Then::MakeFolder {
        account_name, name, ..
    }) = then
    else {
        panic!("{then:?}");
    };
    assert_eq!(
        (account_name.as_str(), name.as_str()),
        ("Work", "Projects/2026")
    );
    assert_eq!(folders(&mut store), before);
    assert!(
        log_of(&mut store)
            .iter()
            .any(|line| line.contains("asked the server for the folder Work/Projects/2026"))
    );

    // A name that will not do is refused before any server is asked.
    let refusal = refused(
        &mut store,
        &running(),
        "folder_create",
        json!({ "name": "Projects/", "account": "Work" }),
    );
    assert_eq!(refusal.code, Code::Invalid);

    // With no Quill running there is nobody to ask the server: said, with
    // the other way.
    let refusal = refused(
        &mut store,
        &alone(),
        "folder_create",
        json!({ "name": "Receipts", "account": "Work" }),
    );
    assert_eq!(refusal.code, Code::NotRunning);
    assert!(refusal.message.contains("on_this_mac"));
    // And so with a Quill that has no servers to ask: the demo mailbox.
    let offline = Settings {
        online: false,
        ..running()
    };
    assert_eq!(
        refused(
            &mut store,
            &offline,
            "folder_create",
            json!({ "name": "Receipts", "account": "Work" })
        )
        .code,
        Code::NotRunning
    );

    // An account whose server keeps no folders has its folders here,
    // whatever is said.
    let (answer, then) = done(
        &mut store,
        &alone(),
        "folder_create",
        json!({ "name": "Kept", "account": "Personal" }),
    );
    assert_eq!((answer["on_this_mac"].clone(), then), (json!(true), None));

    // On this Mac it is made at once, by anyone.
    let (answer, then) = done(
        &mut store,
        &alone(),
        "folder_create",
        json!({ "name": "Receipts", "account": "Work", "on_this_mac": true }),
    );
    assert_eq!(answer["on_this_mac"], true);
    assert_eq!(then, None);
    assert!(folders(&mut store).contains(&"Receipts".to_owned()));
}

#[test]
fn watching_is_a_shell_commands_and_finds_what_arrives() {
    let mut store = store();
    let (answer, then) = done(
        &mut store,
        &running(),
        "watch",
        json!({ "query": " from:ana " }),
    );
    assert_eq!(answer["watching"], "from:ana");
    assert_eq!(
        then,
        Some(Then::Watch {
            query: "from:ana".to_owned()
        })
    );
    assert!(log_of(&mut store)[0].contains("watch: from:ana"));
    // Mail arrives in the Quill that is running.
    assert_eq!(
        refused(&mut store, &alone(), "watch", json!({})).code,
        Code::NotRunning
    );
    // No agent's program is told of it over MCP, and none may ask.
    let mut open = running();
    open.agent.mcp = true;
    assert!(
        agent::tools_for(&open)
            .iter()
            .all(|tool| tool["name"] != "watch")
    );
    let call = Call::parse("watch", &json!({})).unwrap();
    let asked = agent::perform(&mut store, &open, &Caller::mcp("Claude Code"), &call, NOW);
    assert_eq!(asked.result.unwrap_err().code, Code::Invalid);
    assert_eq!(asked.then, None);

    // What arrives is found by the watcher's own search, among the mail
    // agents may see.
    let ana = arrive(
        &mut store,
        "arrived-1",
        "Budget, again",
        "From: Ana Ruiz <ana.ruiz@example.com>\r\nSubject: Budget, again\r\n\r\nNumbers.\r\n",
    );
    let ids = [LocalMessageId(ana)];
    let found = agent::arrived(&store, &running(), "from:ana", &ids);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["id"], json!(ana));
    assert_eq!(found[0]["subject"], "Budget, again");
    assert_eq!(found[0]["schema"], json!(agent::SCHEMA));
    assert!(agent::arrived(&store, &running(), "from:peter", &ids).is_empty());
    // With nothing to find, all that arrives.
    assert_eq!(agent::arrived(&store, &running(), "  ", &ids).len(), 1);
    // An account agents do not see has no mail to tell of.
    let mut narrow = running();
    narrow.agent.accounts = Some(vec!["Personal".to_owned()]);
    assert!(agent::arrived(&store, &narrow, "", &ids).is_empty());
    // And a way in that is switched off is not open to be told on.
    let mut shut = running();
    shut.agent.commands = false;
    assert!(!agent::way_open(&shut, Way::Shell));
    assert!(agent::way_open(&running(), Way::Shell));
}

#[test]
fn a_forward_is_drafted_with_what_is_said_and_the_messages_files() {
    let mut store = store();
    let raw = "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
        To: Robin Vale <robin@example.com>\r\n\
        Subject: The figures\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\
        \r\n\
        --b\r\n\
        Content-Type: text/plain; charset=utf-8\r\n\
        \r\n\
        Here are the figures.\r\n\
        --b\r\n\
        Content-Type: text/csv; name=\"figures.csv\"\r\n\
        Content-Disposition: attachment; filename=\"figures.csv\"\r\n\
        \r\n\
        a,b\r\n1,2\r\n\
        --b--\r\n";
    let id = arrive(&mut store, "figures-1", "The figures", raw);

    let (made, then) = done(
        &mut store,
        &alone(),
        "draft_forward",
        json!({ "id": id, "to": ["sam.lee@example.com"], "body": "For your records." }),
    );
    assert_eq!(then, None);
    assert_eq!(made["action"], "draft_forward");
    assert_eq!(made["state"], "draft");
    assert_eq!(made["to"], "sam.lee@example.com");
    assert_eq!(made["subject"], "Fwd: The figures");
    assert_eq!(made["written_by"], "A shell command");
    assert_eq!(made["attachments"][0]["name"], "figures.csv");
    let draft = store
        .draft(DraftId(made["draft"].as_i64().unwrap()))
        .unwrap()
        .unwrap();
    let body = &draft.content.body;
    // What is said, then the message as it was.
    let said = body.find("For your records.").unwrap();
    let was = body.find("Here are the figures.").unwrap();
    assert!(said < was, "{body}");
    assert!(body.contains("ana.ruiz@example.com"));
    // The file is the message's own, to the byte.
    let file = &draft.content.attachments[0];
    assert_eq!(
        store.blob(&file.blob_hash).unwrap().unwrap(),
        b"a,b\r\n1,2".to_vec()
    );
    // A forward does not mark the original answered.
    assert_eq!(draft.content.replied_to, None);

    // To nobody yet, and with nothing said: the user fills it in.
    let (bare, _) = done(&mut store, &alone(), "draft_forward", json!({ "id": id }));
    assert_eq!(bare["to"], "");
    // Asked for without the files, it has the message and none of them.
    let (light, _) = done(
        &mut store,
        &alone(),
        "draft_forward",
        json!({ "id": id, "without_files": true }),
    );
    let light = store
        .draft(DraftId(light["draft"].as_i64().unwrap()))
        .unwrap()
        .unwrap();
    assert!(light.content.attachments.is_empty());
    assert!(light.content.body.contains("Here are the figures."));
    // Writing drafts is what it needs, and a message that is there.
    let mut reading = alone();
    reading.agent.draft = false;
    assert_eq!(
        refused(&mut store, &reading, "draft_forward", json!({ "id": id })).code,
        Code::SwitchedOff
    );
    assert_eq!(
        refused(
            &mut store,
            &alone(),
            "draft_forward",
            json!({ "id": 999_999 })
        )
        .code,
        Code::NotFound
    );
}

#[test]
fn an_invitation_is_answered_through_the_outbox_as_any_message_is_sent() {
    let mut store = store();
    let calendar = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//Cal//EN\r\n\
        METHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:offsite-2026@example.com\r\nSEQUENCE:2\r\n\
        DTSTAMP:20261114T093000Z\r\nDTSTART:20261120T090000Z\r\nDTEND:20261120T160000Z\r\n\
        SUMMARY:Offsite\r\nORGANIZER;CN=Ana Ruiz:mailto:ana.ruiz@example.com\r\n\
        ATTENDEE;CN=Robin Vale;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:robin@example.com\r\n\
        END:VEVENT\r\nEND:VCALENDAR\r\n";
    let raw = format!(
        "From: Ana Ruiz <ana.ruiz@example.com>\r\n\
         To: Robin Vale <robin@example.com>\r\n\
         Subject: Offsite\r\n\
         Message-ID: <invite-1@example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: multipart/alternative; boundary=\"b\"\r\n\
         \r\n\
         --b\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         You are invited to the offsite.\r\n\
         --b\r\n\
         Content-Type: text/calendar; method=REQUEST; charset=utf-8\r\n\
         \r\n\
         {calendar}\r\n\
         --b--\r\n"
    );
    let id = arrive(&mut store, "invite-1", "Offsite", &raw);

    // Answering is sending: off as Quill comes.
    assert_eq!(
        refused(
            &mut store,
            &alone(),
            "invite",
            json!({ "id": id, "answer": "accept" })
        )
        .code,
        Code::SwitchedOff
    );

    let outcome = outcome(
        &mut store,
        &sending(alone()),
        "invite",
        json!({ "id": id, "answer": "maybe" }),
    );
    let notice = outcome.notice.clone().unwrap();
    assert!(notice.send);
    assert!(notice.text.contains("answered an invitation (maybe)"));
    let answer = outcome.result.unwrap().value;
    assert_eq!(answer["action"], "invite");
    assert_eq!(answer["answer"], "maybe");
    assert_eq!(answer["to"], "Ana Ruiz <ana.ruiz@example.com>");
    assert_eq!(answer["subject"], "Tentative: Offsite");
    assert_eq!(answer["attachments"][0]["name"], "invite.ics");
    assert_eq!(answer["state"], "waiting");
    let sent = store
        .draft(DraftId(answer["draft"].as_i64().unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(sent.state, DraftState::Outbox);
    assert_eq!(sent.agent.as_deref(), Some("A shell command"));
    let reply = store
        .blob(&sent.content.attachments[0].blob_hash)
        .unwrap()
        .unwrap();
    let reply = String::from_utf8(reply).unwrap();
    assert!(reply.contains("METHOD:REPLY") && reply.contains("PARTSTAT=TENTATIVE"));

    // Held, when the user holds what agents send: it waits for them.
    let mut held = sending(alone());
    held.agent.hold_sends = true;
    let (answer, _) = done(
        &mut store,
        &held,
        "invite",
        json!({ "id": id, "answer": "decline" }),
    );
    assert_eq!(answer["held"], true);
    assert_eq!(answer["state"], "held");

    // A message with no invitation has nothing to answer.
    let (found, _) = done(
        &mut store,
        &alone(),
        "search",
        json!({ "query": "from:peter dinner" }),
    );
    let plain = found["messages"][0]["id"].as_i64().unwrap();
    let refusal = refused(
        &mut store,
        &sending(alone()),
        "invite",
        json!({ "id": plain, "answer": "accept" }),
    );
    assert_eq!(refusal.code, Code::Invalid);
    // And an answer that is not one is a mistake in the request.
    assert!(Call::parse("invite", &json!({ "id": id, "answer": "perhaps" })).is_err());
}

#[test]
fn unsubscribing_takes_the_lists_own_way_and_opens_no_page() {
    let mut store = store();
    let one_click = arrive(
        &mut store,
        "digest-1",
        "The weekly digest",
        &from_a_list(
            "List-Unsubscribe: <https://lists.example.com/leave?u=1>, \
             <mailto:leave@lists.example.com?subject=leave>\r\n\
             List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        ),
    );
    let by_mail = arrive(
        &mut store,
        "digest-2",
        "The weekly digest",
        &from_a_list("List-Unsubscribe: <mailto:leave@lists.example.com>\r\n"),
    );
    let by_page = arrive(
        &mut store,
        "digest-3",
        "The weekly digest",
        &from_a_list("List-Unsubscribe: <https://lists.example.com/settings>\r\n"),
    );
    let ask = |id: i64| json!({ "id": id });

    // Asking a list's server is reaching outside this Mac: with sending.
    assert_eq!(
        refused(&mut store, &running(), "unsubscribe", ask(one_click)).code,
        Code::SwitchedOff
    );

    // One request to the list's own address, made by whoever has the
    // network: checked and written down here.
    let (answer, then) = done(
        &mut store,
        &sending(running()),
        "unsubscribe",
        ask(one_click),
    );
    assert_eq!(answer["way"], "one_click");
    assert_eq!(answer["host"], "lists.example.com");
    assert_eq!(
        then,
        Some(Then::Unsubscribe {
            url: "https://lists.example.com/leave?u=1".to_owned(),
            host: "lists.example.com".to_owned(),
        })
    );
    assert!(log_of(&mut store)[0].contains("asked lists.example.com to unsubscribe"));
    // With no Quill running nobody can make it: said, and the list's
    // other way is not taken in its place.
    let refusal = refused(&mut store, &sending(alone()), "unsubscribe", ask(one_click));
    assert_eq!(refusal.code, Code::NotRunning);
    let (drafts, _) = done(&mut store, &alone(), "drafts", json!({}));
    assert_eq!(drafts["drafts"], json!([]));

    // A list that wants a message gets a draft, and nothing is sent.
    let (answer, then) = done(&mut store, &sending(alone()), "unsubscribe", ask(by_mail));
    assert_eq!(then, None);
    assert_eq!(answer["way"], "mail");
    assert_eq!(answer["state"], "draft");
    assert_eq!(answer["to"], "leave@lists.example.com");
    assert_eq!(answer["subject"], "unsubscribe");
    assert_eq!(answer["written_by"], "A shell command");
    let (outbox, _) = done(&mut store, &alone(), "outbox", json!({}));
    assert_eq!(outbox["outbox"], json!([]));

    // A list with only a page is left alone: nothing is opened, and the
    // address is given as what it is, the sender's.
    let outcome = outcome(&mut store, &sending(running()), "unsubscribe", ask(by_page));
    assert_eq!(outcome.then, None);
    assert!(outcome.notice.is_none());
    let answer = outcome.result.unwrap();
    assert_eq!(answer.value["way"], "page");
    assert_eq!(answer.value["changed"], 0);
    assert_eq!(answer.value["page"], "https://lists.example.com/settings");
    assert!(answer.lines[0].contains("Nothing was done"));
    assert!(!answer.lines[0].contains("https://"));

    // A message from no list has no way to leave one.
    let (found, _) = done(
        &mut store,
        &alone(),
        "search",
        json!({ "query": "from:peter dinner" }),
    );
    let plain = found["messages"][0]["id"].as_i64().unwrap();
    assert_eq!(
        refused(&mut store, &sending(running()), "unsubscribe", ask(plain)).code,
        Code::NotFound
    );
}
