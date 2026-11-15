//! The POP3 backend against a scripted server, for what a real one cannot
//! be made to do on demand: lack a command, stall, talk nonsense, hold the
//! mailbox locked, or drop the connection at the worst moment.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use common::fake::{Fake, Say};
use common::{PASSWORD, backend, numbered};
use quill_core::config::TlsMode;
use quill_core::types::{FolderId, Operation, RemoteId, SyncDelta};
use quill_core::{Backend, Error};
use quill_pop3::Pop3Backend;

fn inbox() -> FolderId {
    FolderId("INBOX".to_owned())
}

fn id(id: &str) -> RemoteId {
    RemoteId(id.to_owned())
}

fn text(reply: &str) -> Say {
    Say::Text(reply.to_owned())
}

/// A server holding messages 1 to `count`, with the ids `id-1`, `id-2`...
async fn fake_with(count: usize) -> Fake {
    let messages: Vec<(String, String)> = (1..=count)
        .map(|n| (format!("id-{n}"), numbered(n)))
        .collect();
    let borrowed: Vec<(&str, &str)> = messages
        .iter()
        .map(|(id, raw)| (id.as_str(), raw.as_str()))
        .collect();
    Fake::start(&borrowed).await
}

async fn sync(backend: &Pop3Backend) -> quill_core::Result<SyncDelta> {
    backend.sync_folder(&inbox(), None).await
}

fn subjects(delta: &SyncDelta) -> Vec<&str> {
    delta
        .added
        .iter()
        .map(|message| message.headers.subject.as_str())
        .collect()
}

fn count(fake: &Fake, verb: &str) -> usize {
    fake.verbs().iter().filter(|heard| *heard == verb).count()
}

#[tokio::test]
async fn the_fake_server_behaves_like_a_pop3_server() {
    // The other tests lean on this one: the scripted server, left
    // unscripted, serves a mailbox the way the real one does.
    let fake = fake_with(3).await;
    let backend = fake.backend();
    assert_eq!(backend.status().await.unwrap().messages, 3);
    let delta = sync(&backend).await.unwrap();
    assert_eq!(subjects(&delta), ["Message 3", "Message 2", "Message 1"]);
    for (n, message) in (1..=3).rev().zip(&delta.added) {
        assert_eq!(message.id, id(&format!("id-{n}")));
        assert_eq!(message.size, numbered(n).len() as u64);
    }
    assert_eq!(
        backend.fetch_raw(&id("id-2")).await.unwrap().as_ref(),
        numbered(2).as_bytes()
    );
    // Each call was one session, begun with a sign-in and ended with QUIT.
    assert_eq!(fake.sessions(), 3);
    assert_eq!(count(&fake, "PASS"), 3);
    assert_eq!(count(&fake, "QUIT"), 3);
    assert_eq!(fake.verbs().last().unwrap(), "QUIT");
    // Nothing that reads the mailbox asks to delete from it.
    assert_eq!(count(&fake, "DELE"), 0);
    assert_eq!(fake.uidls(), ["id-1", "id-2", "id-3"]);
}

#[tokio::test]
async fn listing_the_folders_does_not_contact_the_server() {
    let fake = fake_with(1).await;
    let backend = fake.backend();
    assert_eq!(backend.list_folders().await.unwrap().len(), 1);
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    backend.watch(tx).await.unwrap();
    assert_eq!(fake.sessions(), 0);
}

#[tokio::test]
async fn a_server_without_uidl_is_unsupported_and_says_so() {
    let fake = fake_with(2).await;
    fake.script("UIDL", text("-ERR Unknown command\r\n"));
    let backend = fake.backend();
    match sync(&backend).await {
        Err(Error::Unsupported(reason)) => assert!(reason.contains("UIDL"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        backend.fetch_raw(&id("id-1")).await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        backend.apply(&Operation::Delete { id: id("id-1") }).await,
        Err(Error::Unsupported(_))
    ));
    assert_eq!(fake.uidls().len(), 2);
    // Signing in still works, so the account can say what is wrong with it.
    assert_eq!(backend.status().await.unwrap().messages, 2);
}

#[tokio::test]
async fn a_server_without_top_is_synced_with_retr() {
    let fake = fake_with(3).await;
    fake.capa(Some(&["USER", "UIDL"]));
    let delta = sync(&fake.backend()).await.unwrap();
    assert_eq!(subjects(&delta), ["Message 3", "Message 2", "Message 1"]);
    assert_eq!(delta.added[0].headers.from[0].email, "ana.ruiz@example.com");
    assert_eq!(delta.added[0].size, numbered(3).len() as u64);
    assert_eq!(count(&fake, "TOP"), 0);
    assert_eq!(count(&fake, "RETR"), 3);
}

#[tokio::test]
async fn a_server_without_capa_is_asked_for_top_and_believed() {
    // Has TOP, though it cannot say so.
    let fake = fake_with(3).await;
    fake.capa(None);
    let delta = sync(&fake.backend()).await.unwrap();
    assert_eq!(subjects(&delta), ["Message 3", "Message 2", "Message 1"]);
    assert_eq!(count(&fake, "TOP"), 3);
    assert_eq!(count(&fake, "RETR"), 0);

    // Has neither: one TOP to find out, then RETR for everything.
    let fake = fake_with(3).await;
    fake.capa(None);
    fake.script("TOP", text("-ERR Unknown command\r\n"));
    let delta = sync(&fake.backend()).await.unwrap();
    assert_eq!(subjects(&delta), ["Message 3", "Message 2", "Message 1"]);
    assert_eq!(count(&fake, "TOP"), 1);
    assert_eq!(count(&fake, "RETR"), 3);
}

#[tokio::test]
async fn a_server_that_claims_top_and_refuses_it_is_still_synced() {
    let fake = fake_with(3).await;
    fake.script("TOP", text("-ERR Not implemented\r\n"));
    let delta = sync(&fake.backend()).await.unwrap();
    assert_eq!(subjects(&delta), ["Message 3", "Message 2", "Message 1"]);
    assert_eq!(count(&fake, "RETR"), 3);
}

#[tokio::test]
async fn a_message_the_server_will_not_show_is_asked_for_again_next_time() {
    let fake = fake_with(3).await;
    fake.script("TOP 2 0", text("-ERR Message is corrupt\r\n"));
    fake.script("RETR 2", text("-ERR Message is corrupt\r\n"));
    let backend = fake.backend();
    let first = sync(&backend).await.unwrap();
    assert_eq!(subjects(&first), ["Message 3", "Message 1"]);
    assert!(!first.has_more);

    // Still refused: nothing new, and no error to stop the rest syncing.
    let second = backend
        .sync_folder(&inbox(), Some(first.next_cursor.clone()))
        .await
        .unwrap();
    assert!(second.added.is_empty() && second.removed.is_empty() && !second.has_more);
    assert_eq!(second.next_cursor, first.next_cursor);

    // The server recovers, and the message arrives.
    fake.unscript("TOP 2 0");
    fake.unscript("RETR 2");
    let third = backend
        .sync_folder(&inbox(), Some(second.next_cursor))
        .await
        .unwrap();
    assert_eq!(subjects(&third), ["Message 2"]);
}

#[tokio::test]
async fn a_batch_that_brings_nothing_does_not_ask_to_be_called_again() {
    // Otherwise the engine would spin on a server that refuses everything.
    let fake = fake_with(5).await;
    fake.script("TOP", text("-ERR No\r\n"));
    fake.script("RETR", text("-ERR No\r\n"));
    let delta = fake
        .backend()
        .with_batch(2)
        .sync_folder(&inbox(), None)
        .await
        .unwrap();
    assert!(delta.added.is_empty());
    assert!(!delta.has_more);
}

#[tokio::test]
async fn a_server_that_stalls_is_given_up_on() {
    for stalled in ["GREETING", "CAPA", "USER", "PASS", "UIDL", "LIST", "TOP"] {
        let fake = fake_with(2).await;
        fake.script(stalled, Say::Silence);
        let started = Instant::now();
        match sync(&fake.backend()).await {
            Err(Error::Connection(reason)) => {
                assert!(reason.contains("timed out"), "{stalled}: {reason}");
            }
            other => panic!("{stalled}: {other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{stalled}: {:?}",
            started.elapsed()
        );
        // Nothing more was said to a server that had stopped answering.
        assert!(!fake.verbs().contains(&"QUIT".to_owned()), "{stalled}");
    }

    // Half a reply, then nothing.
    let fake = fake_with(2).await;
    fake.script("UIDL", text("+OK\r\n1 id-1\r\n"));
    assert!(matches!(
        sync(&fake.backend()).await,
        Err(Error::Connection(reason)) if reason.contains("timed out")
    ));
    let fake = fake_with(2).await;
    fake.script("RETR", text("+OK\r\nSubject: half a mess"));
    assert!(matches!(
        fake.backend().fetch_raw(&id("id-1")).await,
        Err(Error::Connection(reason)) if reason.contains("timed out")
    ));

    // A server that never answers QUIT has still answered everything that
    // mattered to a sync. To a delete, the answer to QUIT is what matters.
    let fake = fake_with(2).await;
    fake.script("QUIT", Say::Silence);
    let backend = fake.backend();
    assert_eq!(sync(&backend).await.unwrap().added.len(), 2);
    assert!(matches!(
        backend.apply(&Operation::Delete { id: id("id-1") }).await,
        Err(Error::Connection(_))
    ));
}

#[tokio::test]
async fn a_server_that_hangs_up_is_a_connection_error() {
    for cut in ["GREETING", "PASS", "UIDL", "TOP"] {
        let fake = fake_with(2).await;
        fake.script(cut, Say::HangUp);
        match sync(&fake.backend()).await {
            Err(Error::Connection(_)) => {}
            other => panic!("{cut}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn nonsense_from_the_server_is_an_error_and_not_a_hang() {
    let binary = Say::Bytes(vec![0xff, 0xfe, 0x00, 0x80, b'\r', b'\n']);
    let cases: Vec<(&str, Say)> = vec![
        // Not a POP3 server at all.
        ("GREETING", text("220 mail.example.com ESMTP ready\r\n")),
        ("GREETING", text("* OK IMAP4rev1 ready\r\n")),
        ("GREETING", text("\r\n")),
        ("GREETING", binary.clone()),
        // A greeting with no end.
        ("GREETING", Say::Text("x".repeat(1024 * 1024))),
        // Out of step part way through.
        ("USER", text("+ \r\n")),
        ("PASS", text("welcome\r\n")),
        ("PASS", binary.clone()),
        ("CAPA", text("+OK\r\nTOP\r\n")),
        ("UIDL", text("maybe\r\n")),
        ("UIDL", text("+OK\r\nthis is not a listing\r\n.\r\n")),
        ("UIDL", text("+OK\r\n0 id-0\r\n.\r\n")),
        ("UIDL", text("+OK\r\n1\r\n.\r\n")),
        // A listing line far longer than any listing line is.
        (
            "UIDL",
            Say::Text(format!("+OK\r\n1 {}\r\n.\r\n", "x".repeat(20_000))),
        ),
        ("TOP", binary),
    ];
    for (when, say) in cases {
        let fake = fake_with(2).await;
        fake.script(when, say.clone());
        let backend = fake.backend();
        let started = Instant::now();
        let shown = format!("{when}: {say:?}");
        let shown: String = shown.chars().take(60).collect();
        match sync(&backend).await {
            // "CAPA" with no end is a stall: the listing never finishes.
            Err(Error::Connection(_)) if when == "CAPA" => {}
            Err(Error::Backend(reason)) => assert!(reason.contains("127.0.0.1"), "{reason}"),
            other => panic!("{shown}: {other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(5), "{shown}");
        // And the next call, to a server that has come to its senses,
        // starts clean.
        fake.unscript(when);
        assert_eq!(sync(&backend).await.unwrap().added.len(), 2, "{shown}");
    }
}

#[tokio::test]
async fn a_size_listing_that_makes_no_sense_costs_only_the_sizes() {
    for capa in [
        &["USER", "UIDL", "TOP"][..],
        &["USER", "UIDL", "TOP", "PIPELINING"],
    ] {
        let fake = fake_with(8).await;
        fake.capa(Some(capa));
        fake.script("LIST", text("-ERR No\r\n"));
        let delta = fake
            .backend()
            .with_batch(2)
            .sync_folder(&inbox(), None)
            .await
            .unwrap();
        assert_eq!(subjects(&delta), ["Message 8", "Message 7"]);
        assert_eq!(delta.added[0].size, 0);
        // Asked once for the whole mailbox, or once for each message when
        // the questions can go out together.
        let asked = if capa.contains(&"PIPELINING") { 2 } else { 1 };
        assert_eq!(count(&fake, "LIST"), asked, "{capa:?}");
    }

    // The whole-mailbox form, which is what a larger batch uses.
    let fake = fake_with(12).await;
    let delta = sync(&fake.backend()).await.unwrap();
    assert_eq!(delta.added.len(), 12);
    assert_eq!(delta.added[0].size, numbered(12).len() as u64);
    assert_eq!(
        fake.heard().iter().filter(|line| *line == "LIST").count(),
        1
    );
    fake.script("LIST", text("+OK\r\nnot sizes\r\n3 12\r\n.\r\n"));
    let delta = sync(&fake.backend()).await.unwrap();
    assert_eq!(delta.added.len(), 12);
    let size_of = |subject: &str| {
        delta
            .added
            .iter()
            .find(|message| message.headers.subject == subject)
            .unwrap()
            .size
    };
    assert_eq!(size_of("Message 3"), 12);
    assert_eq!(size_of("Message 4"), 0);
}

#[tokio::test]
async fn a_mailbox_in_use_is_a_connection_error_worth_retrying() {
    for busy in [
        "-ERR [IN-USE] Do you have another POP session running?\r\n",
        "-ERR maildrop already locked\r\n",
        "-ERR [LOGIN-DELAY] minimum time between mail checks violation\r\n",
        "-ERR [SYS/TEMP] Temporary authentication failure.\r\n",
    ] {
        let fake = fake_with(2).await;
        fake.script("PASS", text(busy));
        let backend = fake.backend();
        match sync(&backend).await {
            Err(Error::Connection(reason)) => {
                if busy.contains("IN-USE") || busy.contains("locked") {
                    assert!(reason.contains("in use"), "{reason}");
                }
            }
            other => panic!("{busy}: {other:?}"),
        }
        // The other program finishes, and the retry goes through.
        fake.unscript("PASS");
        assert_eq!(sync(&backend).await.unwrap().added.len(), 2);
    }

    // Turned away at the door.
    let fake = fake_with(2).await;
    fake.script("GREETING", text("-ERR Too many connections\r\n"));
    assert!(matches!(
        sync(&fake.backend()).await,
        Err(Error::Connection(reason)) if reason.contains("Too many connections")
    ));
    assert_eq!(fake.heard(), Vec::<String>::new());
}

#[tokio::test]
async fn a_refused_sign_in_is_an_auth_error() {
    let fake = fake_with(1).await;
    let wrong = backend(fake.config(TlsMode::None), "wrong");
    assert!(matches!(sync(&wrong).await, Err(Error::Auth(_))));
    // Refused at USER, as servers that know their users do.
    fake.script("USER", text("-ERR No such user\r\n"));
    match sync(&fake.backend()).await {
        Err(Error::Auth(reason)) => assert_eq!(reason, "No such user"),
        other => panic!("{other:?}"),
    }
    assert_eq!(count(&fake, "PASS"), 1);
}

#[tokio::test]
async fn a_server_that_only_takes_sasl_is_signed_in_to_with_auth_plain() {
    use base64::Engine;
    let fake = fake_with(2).await;
    fake.capa(Some(&["SASL PLAIN", "UIDL", "TOP"]));
    let delta = sync(&fake.backend()).await.unwrap();
    assert_eq!(delta.added.len(), 2);
    let heard = fake.heard();
    let token = base64::engine::general_purpose::STANDARD
        .encode(format!("\0robin@example.com\0{PASSWORD}"));
    assert_eq!(heard[..3], ["CAPA", "AUTH PLAIN", token.as_str()]);
    assert_eq!(count(&fake, "USER") + count(&fake, "PASS"), 0);

    let wrong = backend(fake.config(TlsMode::None), "wrong");
    assert!(matches!(sync(&wrong).await, Err(Error::Auth(_))));
}

#[tokio::test]
async fn plaintext_is_used_only_when_the_account_asks_for_it() {
    // Asked for: the sign-in goes out in the clear.
    let fake = fake_with(1).await;
    sync(&fake.backend()).await.unwrap();
    assert!(fake.heard().contains(&format!("PASS {PASSWORD}")));

    // STLS required and not offered: nothing but the request is sent.
    let fake = fake_with(1).await;
    let strict = backend(fake.config(TlsMode::Starttls), PASSWORD);
    for _ in 0..2 {
        match sync(&strict).await {
            Err(Error::Insecure(reason)) => assert!(reason.contains("STLS"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(fake.heard(), ["STLS", "STLS"]);
    assert!(matches!(
        strict.apply(&Operation::Delete { id: id("id-1") }).await,
        Err(Error::Insecure(_))
    ));
    assert_eq!(fake.uidls(), ["id-1"]);
}

#[tokio::test]
async fn a_failed_upgrade_is_insecure_and_sends_no_credentials() {
    // The server agrees to STLS and then does not speak TLS.
    let fake = fake_with(1).await;
    fake.script("STLS", text("+OK Begin TLS negotiation\r\n"));
    let strict = backend(fake.config(TlsMode::Starttls), PASSWORD);
    match sync(&strict).await {
        Err(Error::Insecure(reason)) => assert!(reason.contains("upgrade"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let heard = fake.heard();
    assert_eq!(heard[0], "STLS");
    for line in &heard {
        let upper = line.to_ascii_uppercase();
        assert!(
            !line.contains(PASSWORD)
                && !line.contains("robin@example.com")
                && !upper.starts_with("USER")
                && !upper.starts_with("PASS")
                && !upper.starts_with("AUTH"),
            "{line:?}"
        );
    }

    // Implicit TLS against a server that is not speaking TLS at all.
    let fake = fake_with(1).await;
    let implicit = backend(fake.config(TlsMode::Implicit), PASSWORD);
    assert!(sync(&implicit).await.is_err());
    assert!(fake.heard().iter().all(|line| !line.contains(PASSWORD)));
}

#[tokio::test]
async fn credentials_with_a_line_break_are_never_sent() {
    let fake = fake_with(2).await;
    let sneaky = backend(fake.config(TlsMode::None), "secret\r\nDELE 1");
    assert!(matches!(sync(&sneaky).await, Err(Error::Auth(_))));
    let mut config = fake.config(TlsMode::None);
    config.username = "robin@example.com\nQUIT".to_owned();
    assert!(matches!(
        sync(&backend(config, PASSWORD)).await,
        Err(Error::Auth(_))
    ));
    assert_eq!(count(&fake, "USER") + count(&fake, "PASS"), 0);
    assert_eq!(count(&fake, "DELE"), 0);
}

#[tokio::test]
async fn nothing_is_deleted_unless_the_server_answers_quit() {
    let fake = fake_with(3).await;
    let backend = fake.backend();
    let delete = Operation::Delete { id: id("id-2") };

    // The connection drops after DELE, before QUIT.
    fake.script("QUIT", Say::HangUp);
    assert!(matches!(
        backend.apply(&delete).await,
        Err(Error::Connection(_))
    ));
    assert_eq!(count(&fake, "DELE"), 1);
    assert_eq!(fake.uidls(), ["id-1", "id-2", "id-3"]);
    fake.unscript("QUIT");

    // It drops at the DELE itself.
    fake.script("DELE", Say::HangUp);
    assert!(matches!(
        backend.apply(&delete).await,
        Err(Error::Connection(_))
    ));
    assert_eq!(fake.uidls(), ["id-1", "id-2", "id-3"]);
    fake.unscript("DELE");

    // The server says it could not do it.
    fake.script("QUIT", text("-ERR some deleted messages not removed\r\n"));
    match backend.apply(&delete).await {
        Err(Error::Backend(reason)) => assert!(reason.contains("not removed"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fake.uidls(), ["id-1", "id-2", "id-3"]);
    fake.unscript("QUIT");

    // Each failure left the operation to be tried again, and the retry
    // works: one session, one DELE, one QUIT.
    let before = fake.sessions();
    backend.apply(&delete).await.unwrap();
    assert_eq!(fake.uidls(), ["id-1", "id-3"]);
    assert_eq!(fake.sessions(), before + 1);
    let heard = fake.heard();
    assert_eq!(heard[heard.len() - 2..], ["DELE 2", "QUIT"]);

    // And once more, as a replayed queue would: nothing to do, no DELE.
    let deletes = count(&fake, "DELE");
    backend.apply(&delete).await.unwrap();
    assert_eq!(count(&fake, "DELE"), deletes);
    assert_eq!(fake.uidls(), ["id-1", "id-3"]);
}

#[tokio::test]
async fn one_refused_delete_does_not_hold_up_the_others() {
    let fake = fake_with(4).await;
    fake.script("DELE 2", text("-ERR Message 2 is locked\r\n"));
    let backend = fake.backend();
    let doomed = [id("id-1"), id("id-2"), id("id-4")];
    match backend.delete_many(&doomed).await {
        Err(Error::Backend(reason)) => assert!(reason.contains("locked"), "{reason}"),
        other => panic!("{other:?}"),
    }
    // The two the server accepted are gone; the refused one and the one
    // nobody asked about are not.
    assert_eq!(fake.uidls(), ["id-2", "id-3"]);
    assert_eq!(fake.sessions(), 1);
    assert_eq!(count(&fake, "QUIT"), 1);

    // The repeat finishes the job once the server allows it.
    fake.unscript("DELE 2");
    assert_eq!(backend.delete_many(&doomed).await.unwrap(), 1);
    assert_eq!(fake.uidls(), ["id-3"]);
}

#[tokio::test]
async fn two_messages_with_one_id_are_one_message() {
    // Against the rules, but servers that make ids from the content do it
    // whenever the same message is delivered twice.
    let (first, second, third) = (numbered(1), numbered(2), numbered(3));
    let fake = Fake::start(&[("same", &first), ("other", &second), ("same", &third)]).await;
    let backend = fake.backend();
    let delta = sync(&backend).await.unwrap();
    let ids: Vec<&str> = delta.added.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, ["same", "other"]);
    let again = backend
        .sync_folder(&inbox(), Some(delta.next_cursor))
        .await
        .unwrap();
    assert!(again.added.is_empty() && again.removed.is_empty());

    assert_eq!(backend.delete_many(&[id("same")]).await.unwrap(), 2);
    assert_eq!(fake.uidls(), ["other"]);
}

#[tokio::test]
async fn ids_the_rules_do_not_allow_still_name_their_messages() {
    let (first, second) = (numbered(1), numbered(2));
    let fake = Fake::start(&[("id with spaces", &first), ("naïve-ünïcödé;1:2", &second)]).await;
    let backend = fake.backend();
    let delta = sync(&backend).await.unwrap();
    let ids: Vec<&str> = delta.added.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, ["naïve-ünïcödé;1:2", "id with spaces"]);
    for (n, message) in [(2, &delta.added[0]), (1, &delta.added[1])] {
        assert_eq!(
            backend.fetch_raw(&message.id).await.unwrap().as_ref(),
            numbered(n).as_bytes()
        );
    }
    let again = backend
        .sync_folder(&inbox(), Some(delta.next_cursor))
        .await
        .unwrap();
    assert!(again.added.is_empty() && again.removed.is_empty());
    backend.delete_many(&[id("id with spaces")]).await.unwrap();
    assert_eq!(fake.uidls(), ["naïve-ünïcödé;1:2"]);
}

#[tokio::test]
async fn pipelined_commands_and_their_replies_stay_in_step() {
    let fake = fake_with(100).await;
    fake.capa(Some(&["USER", "UIDL", "TOP", "PIPELINING"]));
    // Twenty at a time out of a hundred: few enough that the sizes are
    // asked for message by message, in groups like everything else.
    let backend = fake.backend().with_batch(20);
    let mut cursor = None;
    let mut seen = Vec::new();
    loop {
        let delta = backend.sync_folder(&inbox(), cursor).await.unwrap();
        for message in &delta.added {
            let n: usize = message.id.0.trim_start_matches("id-").parse().unwrap();
            assert_eq!(message.headers.subject, format!("Message {n}"));
            assert_eq!(message.size, numbered(n).len() as u64);
            seen.push(n);
        }
        cursor = Some(delta.next_cursor);
        if !delta.has_more {
            break;
        }
    }
    assert_eq!(seen, (1..=100).rev().collect::<Vec<_>>());

    // A refusal in the middle of a group does not shift the answers after it.
    fake.script("DELE 50", text("-ERR No\r\n"));
    let all: Vec<RemoteId> = (1..=100).map(|n| id(&format!("id-{n}"))).collect();
    assert!(backend.delete_many(&all).await.is_err());
    assert_eq!(fake.uidls(), ["id-50"]);
}

#[tokio::test]
async fn enormous_headers_and_bodies_are_cut_short_without_losing_step() {
    // Two megabytes of one header, in a message between two ordinary ones.
    let bloated = format!(
        "Subject: Bloated\r\nFrom: Ana Ruiz <ana.ruiz@example.com>\r\nX-Padding: {}\r\n\
         Date: Sat, 14 Nov 2026 09:14:00 +0000\r\n\r\nBody.\r\n",
        "x".repeat(2 * 1024 * 1024)
    );
    // Three megabytes of body, in lines, some starting with dots.
    let heavy = format!(
        "Subject: Heavy\r\nFrom: Ana Ruiz <ana.ruiz@example.com>\r\n\r\n{}",
        "..a line that starts with dots and goes on for a while, again and again\r\n"
            .repeat(45_000)
    );
    let (first, last) = (numbered(1), numbered(4));
    let messages = [
        ("id-1", first.as_str()),
        ("id-2", bloated.as_str()),
        ("id-3", heavy.as_str()),
        ("id-4", last.as_str()),
    ];

    // With TOP, and with RETR standing in for it.
    for capa in [&["USER", "UIDL", "TOP"][..], &["USER", "UIDL"][..]] {
        let fake = Fake::start(&messages).await;
        fake.capa(Some(capa));
        let backend = fake.backend();
        let delta = sync(&backend).await.unwrap();
        assert_eq!(
            subjects(&delta),
            ["Message 4", "Heavy", "Bloated", "Message 1"],
            "{capa:?}"
        );
        assert_eq!(delta.added[2].headers.from[0].email, "ana.ruiz@example.com");
        assert_eq!(delta.added[2].size, bloated.len() as u64);
        // A full download is not cut short.
        let fetched = backend.fetch_raw(&id("id-3")).await.unwrap();
        assert!(fetched.as_ref() == heavy.as_bytes(), "{capa:?}");
        let fetched = backend.fetch_raw(&id("id-2")).await.unwrap();
        assert!(fetched.as_ref() == bloated.as_bytes(), "{capa:?}");
    }
}
