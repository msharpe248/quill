//! The POP3 backend against a real server.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::{BTreeSet, HashSet};

use common::{Certificates, Dovecot, PASSWORD, Stls, TlsFront, backend, message, numbered};
use quill_core::config::TlsMode;
use quill_core::types::{
    Capabilities, Flags, FolderId, FolderRole, Operation, RemoteId, SyncCursor, SyncDelta,
};
use quill_core::{Backend, Error};
use quill_pop3::MailboxStatus;
use sha2::{Digest, Sha256};

fn inbox() -> FolderId {
    FolderId("INBOX".to_owned())
}

/// Syncs the mailbox to the end, the way the sync engine will.
async fn sync_all(
    backend: &dyn Backend,
    mut cursor: Option<SyncCursor>,
) -> (Vec<SyncDelta>, SyncCursor) {
    let mut deltas = Vec::new();
    loop {
        let delta = backend.sync_folder(&inbox(), cursor.take()).await.unwrap();
        cursor = Some(delta.next_cursor.clone());
        let more = delta.has_more;
        deltas.push(delta);
        if !more {
            return (deltas, cursor.unwrap());
        }
    }
}

fn subjects(delta: &SyncDelta) -> Vec<&str> {
    delta
        .added
        .iter()
        .map(|message| message.headers.subject.as_str())
        .collect()
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn every_tls_mode_signs_in_and_syncs() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "modes@example.com";
    server.fill(user, 1, 2);
    let bytes = (numbered(1).len() + numbered(2).len()) as u64;
    for mode in [TlsMode::Implicit, TlsMode::Starttls, TlsMode::None] {
        let backend = server.backend(mode, user);
        let status = backend
            .status()
            .await
            .unwrap_or_else(|err| panic!("{mode:?}: {err}"));
        assert_eq!(status, MailboxStatus { messages: 2, bytes });
        let delta = backend.sync_folder(&inbox(), None).await.unwrap();
        assert_eq!(subjects(&delta), ["Message 2", "Message 1"], "{mode:?}");
    }
}

#[tokio::test]
async fn stls_sends_nothing_but_the_upgrade_request_in_the_clear() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let backend = server.backend(TlsMode::Starttls, "clear@example.com");
    backend.sync_folder(&inbox(), None).await.unwrap();
    let cleartext = server.front.cleartext.lock().unwrap().clone();
    assert_eq!(cleartext, ["STLS"]);
}

#[tokio::test]
async fn a_server_that_refuses_stls_never_sees_the_password() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let front = TlsFront::start(server.pop3_port, &server.certificates, Stls::Refused).await;
    let mut config = server.config(TlsMode::Starttls, "robin@example.com");
    config.port = front.stls_port;
    let backend = backend(config, PASSWORD);
    for _ in 0..2 {
        match backend.sync_folder(&inbox(), None).await {
            Err(Error::Insecure(reason)) => assert!(reason.contains("STLS"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
    // There is no fallback to plaintext: all the server ever heard in the
    // clear was the request to upgrade.
    let cleartext = front.cleartext.lock().unwrap().clone();
    assert_eq!(cleartext, ["STLS", "STLS"]);
}

#[tokio::test]
async fn what_is_slipped_in_before_the_upgrade_is_thrown_away() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "tampered@example.com";
    server.fill(user, 1, 3);
    let front = TlsFront::start(server.pop3_port, &server.certificates, Stls::Tampered).await;
    let mut config = server.config(TlsMode::Starttls, user);
    config.port = front.stls_port;
    // The forged replies sent in the clear would fail the sign-in if they
    // were read as answers. They are dropped with everything else from
    // before TLS, and the real server's answers are the ones heard.
    let delta = backend(config, PASSWORD)
        .sync_folder(&inbox(), None)
        .await
        .unwrap();
    assert_eq!(
        subjects(&delta),
        ["Message 3", "Message 2", "Message 1"],
        "{delta:?}"
    );
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "robin@example.com";
    let wrong = backend(server.config(TlsMode::Implicit, user), "wrong");
    match wrong.sync_folder(&inbox(), None).await {
        Err(Error::Auth(reason)) => {
            // The server's words, without the password or the code.
            assert!(
                !reason.contains("wrong") && !reason.contains('['),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
    // The account is not harmed: the right password still works.
    server
        .backend(TlsMode::Implicit, user)
        .status()
        .await
        .unwrap();
}

#[tokio::test]
async fn an_untrusted_certificate_is_refused_and_reported_with_its_fingerprint() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    for mode in [TlsMode::Implicit, TlsMode::Starttls] {
        // No extra roots: the test CA is unknown to the system.
        let mut config = server.config(mode, "robin@example.com");
        config.trust.extra_roots.clear();
        match backend(config, PASSWORD).sync_folder(&inbox(), None).await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(problem.host, "localhost");
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(server.certificates.leaf_der.as_slice())
                );
                assert_eq!(
                    problem.fingerprint,
                    Some(hex_sha256(&server.certificates.leaf_der))
                );
            }
            other => panic!("{mode:?}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_self_signed_certificate_is_refused_until_it_is_pinned() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "pin@example.com";
    server.fill(user, 1, 1);
    let own = Certificates::self_signed();
    let front = TlsFront::start(server.pop3_port, &own, Stls::Offered).await;
    for (mode, port) in [
        (TlsMode::Implicit, front.implicit_port),
        (TlsMode::Starttls, front.stls_port),
    ] {
        let mut config = server.config(mode, user);
        config.port = port;
        config.trust.extra_roots.clear();
        let fingerprint = match backend(config.clone(), PASSWORD)
            .sync_folder(&inbox(), None)
            .await
        {
            Err(Error::Certificate(problem)) => {
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(own.leaf_der.as_slice())
                );
                problem.fingerprint.clone().unwrap()
            }
            other => panic!("{mode:?}: {other:?}"),
        };
        assert_eq!(fingerprint, hex_sha256(&own.leaf_der));

        // Pinned, the same certificate is accepted.
        config.trust.pinned_sha256 = Some(Sha256::digest(&own.leaf_der).into());
        let delta = backend(config.clone(), PASSWORD)
            .sync_folder(&inbox(), None)
            .await
            .unwrap_or_else(|err| panic!("{mode:?}: {err}"));
        assert_eq!(delta.added.len(), 1);

        // A pin for some other certificate: refused, and it says why.
        config.trust.pinned_sha256 = Some([7; 32]);
        match backend(config, PASSWORD).sync_folder(&inbox(), None).await {
            Err(Error::Certificate(problem)) => {
                assert!(problem.reason.contains("pinned"), "{}", problem.reason);
                assert_eq!(problem.fingerprint, Some(hex_sha256(&own.leaf_der)));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
    }
    // Refused certificates never got as far as a sign-in.
    let cleartext = front.cleartext.lock().unwrap().clone();
    assert!(cleartext.iter().all(|line| line == "STLS"), "{cleartext:?}");
}

#[tokio::test]
async fn an_expired_certificate_is_refused() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    // Valid for thirty days, starting sixty days ago.
    let expired = Certificates::issue(&["localhost"], 60, 30);
    let front = TlsFront::start(server.pop3_port, &expired, Stls::Offered).await;
    for (mode, port) in [
        (TlsMode::Implicit, front.implicit_port),
        (TlsMode::Starttls, front.stls_port),
    ] {
        let mut config = server.config(mode, "robin@example.com");
        config.port = port;
        config.trust.extra_roots = vec![expired.ca_der.clone()];
        match backend(config, PASSWORD).sync_folder(&inbox(), None).await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(expired.leaf_der.as_slice())
                );
                assert_eq!(problem.fingerprint, Some(hex_sha256(&expired.leaf_der)));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_certificate_for_another_host_is_refused() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    // A certificate that names some other host.
    let other = Certificates::issue(&["mail.example.com"], 1, 90);
    let front = TlsFront::start(server.pop3_port, &other, Stls::Offered).await;
    for (mode, port) in [
        (TlsMode::Implicit, front.implicit_port),
        (TlsMode::Starttls, front.stls_port),
    ] {
        let mut config = server.config(mode, "robin@example.com");
        config.port = port;
        config.trust.extra_roots = vec![other.ca_der.clone()];
        match backend(config, PASSWORD).sync_folder(&inbox(), None).await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(problem.fingerprint, Some(hex_sha256(&other.leaf_der)));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
    }
    // And the usual certificate, which names "localhost", reached by
    // address instead.
    let mut config = server.config(TlsMode::Implicit, "robin@example.com");
    config.host = "127.0.0.1".to_owned();
    assert!(matches!(
        backend(config, PASSWORD).sync_folder(&inbox(), None).await,
        Err(Error::Certificate(_))
    ));
}

#[tokio::test]
async fn an_unreachable_server_is_a_connection_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = quill_pop3::Pop3Config {
        host: "127.0.0.1".to_owned(),
        port,
        tls: TlsMode::None,
        username: "robin@example.com".to_owned(),
        trust: quill_pop3::Trust::default(),
    };
    assert!(matches!(
        backend(config, PASSWORD).sync_folder(&inbox(), None).await,
        Err(Error::Connection(_))
    ));
}

#[tokio::test]
async fn an_empty_mailbox_syncs_to_nothing() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let backend = server.backend(TlsMode::Implicit, "empty@example.com");
    let first = backend.sync_folder(&inbox(), None).await.unwrap();
    assert!(first.added.is_empty() && first.removed.is_empty() && first.changed.is_empty());
    assert!(!first.has_more && !first.reset);
    let again = backend
        .sync_folder(&inbox(), Some(first.next_cursor.clone()))
        .await
        .unwrap();
    assert!(again.added.is_empty() && again.removed.is_empty() && !again.has_more);
    assert_eq!(again.next_cursor, first.next_cursor);
    assert_eq!(
        backend.status().await.unwrap(),
        MailboxStatus {
            messages: 0,
            bytes: 0
        }
    );
}

#[tokio::test]
async fn a_first_sync_brings_everything_newest_first_in_batches() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "batches@example.com";
    server.fill(user, 1, 12);

    let backend = server.backend(TlsMode::Implicit, user).with_batch(5);
    let (deltas, cursor) = sync_all(&backend, None).await;
    let sizes: Vec<usize> = deltas.iter().map(|delta| delta.added.len()).collect();
    assert_eq!(sizes, [5, 5, 2]);
    assert!(deltas[0].has_more && deltas[1].has_more && !deltas[2].has_more);
    assert!(
        deltas
            .iter()
            .all(|delta| !delta.reset && delta.removed.is_empty() && delta.changed.is_empty())
    );
    // The first batch is the newest mail, and each batch carries on from
    // the one before.
    let all: Vec<&str> = deltas.iter().flat_map(subjects).collect();
    let expected: Vec<String> = (1..=12).rev().map(|n| format!("Message {n}")).collect();
    assert_eq!(all, expected);

    // Every message exactly once, under the id the server gives it, with
    // its headers and its size.
    let on_server = server.uidls(user).await;
    assert_eq!(on_server.len(), 12);
    for (n, message) in (1..=12)
        .rev()
        .zip(deltas.iter().flat_map(|delta| &delta.added))
    {
        assert_eq!(message.id.0, on_server[n - 1]);
        assert_eq!(message.size, numbered(n).len() as u64);
        assert_eq!(message.headers.from[0].email, "ana.ruiz@example.com");
        assert_eq!(message.headers.to[0].email, "robin@example.com");
        assert_eq!(
            message.headers.message_id.as_deref(),
            Some(format!("<message-{n}@example.com>").as_str())
        );
        assert_eq!(message.headers.date, 1_794_614_400 + n as i64);
        assert_eq!(message.flags, Flags::default());
        assert!(!message.has_attachments);
    }

    // Nothing has changed, so syncing again from the cursor reports nothing
    // and hands back the same cursor.
    let again = backend
        .sync_folder(&inbox(), Some(cursor.clone()))
        .await
        .unwrap();
    assert!(again.added.is_empty() && again.removed.is_empty() && again.changed.is_empty());
    assert!(!again.has_more && !again.reset);
    assert_eq!(again.next_cursor, cursor);

    // Syncing took nothing off the server.
    assert_eq!(server.uidls(user).await, on_server);
}

#[tokio::test]
async fn a_synced_message_carries_decoded_headers_size_and_attachment_marker() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "headers@example.com";
    let plain = message(
        "Ana Ruiz <ana.ruiz@example.com>",
        "No attachment",
        "Fri, 13 Nov 2026 10:00:00 +0000",
        "Just text.",
    );
    let invoice = "From: =?UTF-8?B?6ZmI6Z2Z?= <chen.jing@example.com>\r\n\
                   To: Robin Vale <robin@example.com>\r\n\
                   Cc: Omar Haddad <omar.haddad@example.com>\r\n\
                   Subject: =?utf-8?Q?Tom=C3=A1s_sent_an_invoice?=\r\n\
                   Date: Sat, 14 Nov 2026 09:14:00 +0000\r\n\
                   Message-ID: <invoice.114@example.com>\r\n\
                   MIME-Version: 1.0\r\n\
                   Content-Type: multipart/mixed;\r\n\
                   \tboundary=\"b\"\r\n\
                   \r\n\
                   --b\r\n\
                   Content-Type: text/plain; charset=utf-8\r\n\
                   \r\n\
                   Invoice attached.\r\n\
                   --b\r\n\
                   Content-Type: application/pdf; name=\"invoice.pdf\"\r\n\
                   Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\
                   Content-Transfer-Encoding: base64\r\n\
                   \r\n\
                   JVBERi0xLjQK\r\n\
                   --b--\r\n";
    server.deliver(user, &plain);
    server.deliver(user, invoice);

    let backend = server.backend(TlsMode::Starttls, user);
    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    assert_eq!(delta.added.len(), 2);
    let synced = &delta.added[0];
    assert_eq!(synced.headers.subject, "Tomás sent an invoice");
    assert_eq!(synced.headers.from[0].name.as_deref(), Some("陈静"));
    assert_eq!(synced.headers.from[0].email, "chen.jing@example.com");
    assert_eq!(synced.headers.to[0].email, "robin@example.com");
    assert_eq!(synced.headers.cc[0].email, "omar.haddad@example.com");
    assert_eq!(
        synced.headers.message_id.as_deref(),
        Some("<invoice.114@example.com>")
    );
    assert_eq!(synced.headers.date, 1_794_647_640);
    assert_eq!(synced.size, invoice.len() as u64);
    assert!(synced.has_attachments);
    // POP3 has no flags: everything arrives unread.
    assert_eq!(synced.flags, Flags::default());

    let synced = &delta.added[1];
    assert_eq!(synced.headers.subject, "No attachment");
    assert_eq!(synced.size, plain.len() as u64);
    assert!(!synced.has_attachments);
}

#[tokio::test]
async fn later_syncs_report_only_arrivals_and_removals() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "changes@example.com";
    server.fill(user, 1, 4);
    let backend = server.backend(TlsMode::Implicit, user);
    let (first, cursor) = sync_all(&backend, None).await;
    assert_eq!(first[0].added.len(), 4);
    let id_of = |subject: &str| {
        first[0]
            .added
            .iter()
            .find(|message| message.headers.subject == subject)
            .unwrap()
            .id
            .clone()
    };

    // New mail arrives: it alone is added.
    server.fill(user, 5, 1);
    let arrival = backend.sync_folder(&inbox(), Some(cursor)).await.unwrap();
    assert_eq!(subjects(&arrival), ["Message 5"]);
    assert!(arrival.removed.is_empty() && !arrival.has_more && !arrival.reset);

    // Another mail program deletes two messages: they alone are removed.
    let (two, three) = (id_of("Message 2"), id_of("Message 3"));
    server.delete_elsewhere(user, &[&two.0, &three.0]).await;
    let removal = backend
        .sync_folder(&inbox(), Some(arrival.next_cursor))
        .await
        .unwrap();
    assert!(removal.added.is_empty(), "{:?}", removal.added);
    assert_eq!(removal.removed, [two.clone(), three]);
    assert!(removal.changed.is_empty() && !removal.reset);

    // Both at once, and the removal is not reported a second time.
    server.fill(user, 6, 2);
    let one = id_of("Message 1");
    server.delete_elsewhere(user, &[&one.0]).await;
    let both = backend
        .sync_folder(&inbox(), Some(removal.next_cursor))
        .await
        .unwrap();
    assert_eq!(subjects(&both), ["Message 7", "Message 6"]);
    assert_eq!(both.removed, [one]);

    // A message's id is the same in every session, whatever number the
    // server gives it this time: the survivor of the first four still
    // answers to the id it had then.
    let fetched = backend.fetch_raw(&id_of("Message 4")).await.unwrap();
    assert_eq!(fetched.as_ref(), numbered(4).as_bytes());
    assert!(matches!(
        backend.fetch_raw(&two).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn fetching_a_message_returns_its_exact_bytes() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "fetch@example.com";
    // Lines that start with a dot travel byte-stuffed, and a line that is
    // only a dot would end the reply if it were not.
    let dotted = message(
        "Maya Okafor <maya.okafor@example.com>",
        "Trail run Saturday",
        "Sat, 14 Nov 2026 04:30:00 +0000",
        "Seven o'clock at the north car park.\r\n.\r\n..\r\n.hidden file\r\n\
         ...and so on.\r\n\r\n.\r\nMaya",
    );
    server.fill(user, 1, 1);
    server.deliver(user, &dotted);
    server.fill(user, 2, 1);

    let backend = server.backend(TlsMode::Implicit, user);
    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    assert_eq!(
        subjects(&delta),
        ["Message 2", "Trail run Saturday", "Message 1"]
    );
    assert_eq!(delta.added[1].size, dotted.len() as u64);
    let id = delta.added[1].id.clone();
    let fetched = backend.fetch_raw(&id).await.unwrap();
    assert_eq!(
        String::from_utf8_lossy(&fetched),
        dotted,
        "the message came back changed"
    );
    let parsed = quill_mime::parse(&fetched, 0).unwrap();
    assert_eq!(
        parsed.body,
        "Seven o'clock at the north car park.\n.\n..\n.hidden file\n...and so on.\n\n.\nMaya\n"
    );

    // Several in one session, in the order asked, with a gap where a
    // message is not on the server.
    let nowhere = RemoteId("no-such-message".to_owned());
    let many = backend
        .fetch_many(&[
            delta.added[2].id.clone(),
            nowhere.clone(),
            id.clone(),
            delta.added[0].id.clone(),
        ])
        .await
        .unwrap();
    assert_eq!(many[0].as_deref(), Some(numbered(1).as_bytes()));
    assert_eq!(many[1], None);
    assert_eq!(many[2].as_deref(), Some(dotted.as_bytes()));
    assert_eq!(many[3].as_deref(), Some(numbered(2).as_bytes()));
    assert!(backend.fetch_many(&[]).await.unwrap().is_empty());

    // A message that was never there, and one that has gone since.
    assert!(matches!(
        backend.fetch_raw(&nowhere).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        backend.fetch_raw(&RemoteId(String::new())).await,
        Err(Error::NotFound(_))
    ));
    server.delete_elsewhere(user, &[&id.0]).await;
    match backend.fetch_raw(&id).await {
        Err(Error::NotFound(reason)) => assert!(reason.contains(&id.0), "{reason}"),
        other => panic!("{other:?}"),
    }

    // Fetching took nothing off the server.
    assert_eq!(server.uidls(user).await.len(), 2);
}

#[tokio::test]
async fn deleting_removes_the_message_from_the_server_and_can_be_repeated() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "delete@example.com";
    server.fill(user, 1, 3);
    let backend = server.backend(TlsMode::Implicit, user);
    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    let before = server.uidls(user).await;
    assert_eq!(before.len(), 3);
    let doomed = delta.added[1].id.clone();
    assert_eq!(delta.added[1].headers.subject, "Message 2");

    backend
        .apply(&Operation::Delete { id: doomed.clone() })
        .await
        .unwrap();
    // A fresh session, not the backend's word for it: the message is gone
    // and the others are untouched.
    let after = server.uidls(user).await;
    assert_eq!(after, [before[0].clone(), before[2].clone()]);
    assert!(!after.contains(&doomed.0));

    // Deleting is idempotent: a message that is not there is deleted
    // already. A replayed operation succeeds and changes nothing.
    for _ in 0..2 {
        backend
            .apply(&Operation::Delete { id: doomed.clone() })
            .await
            .unwrap();
    }
    assert_eq!(server.uidls(user).await, after);
    backend
        .apply(&Operation::Delete {
            id: RemoteId("never-existed".to_owned()),
        })
        .await
        .unwrap();
    assert_eq!(server.uidls(user).await, after);

    // The next sync reports it removed, and only it.
    let next = backend
        .sync_folder(&inbox(), Some(delta.next_cursor))
        .await
        .unwrap();
    assert_eq!(next.removed, [doomed]);
    assert!(next.added.is_empty());
}

#[tokio::test]
async fn many_messages_are_deleted_in_one_session() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "sweep@example.com";
    server.fill(user, 1, 40);
    let backend = server.backend(TlsMode::Starttls, user);
    let (deltas, cursor) = sync_all(&backend, None).await;
    let ids: Vec<RemoteId> = deltas[0].added.iter().map(|m| m.id.clone()).collect();
    assert_eq!(ids.len(), 40);

    // Everything but the five newest, as "delete after N days" would, with
    // an id that is already gone among them.
    let mut old: Vec<RemoteId> = ids[5..].to_vec();
    old.insert(3, RemoteId("gone-already".to_owned()));
    assert_eq!(backend.delete_many(&old).await.unwrap(), 35);
    let left: Vec<RemoteId> = server
        .uidls(user)
        .await
        .into_iter()
        .rev()
        .map(RemoteId)
        .collect();
    assert_eq!(left, ids[..5]);

    // Again: nothing left to do, and that is success.
    assert_eq!(backend.delete_many(&old).await.unwrap(), 0);
    assert_eq!(backend.delete_many(&[]).await.unwrap(), 0);
    assert_eq!(server.uidls(user).await.len(), 5);

    let next = backend.sync_folder(&inbox(), Some(cursor)).await.unwrap();
    let removed: HashSet<&RemoteId> = next.removed.iter().collect();
    assert_eq!(removed, ids[5..].iter().collect());
    assert!(next.added.is_empty());
}

#[tokio::test]
async fn what_pop3_cannot_do_is_refused_not_ignored() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "limits@example.com";
    server.fill(user, 1, 1);
    let backend = server.backend(TlsMode::Implicit, user);
    assert_eq!(
        backend.capabilities(),
        Capabilities {
            folders: false,
            flags: false,
            push: false,
            server_search: false,
            threads: false,
        }
    );
    let folders = backend.list_folders().await.unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].id, inbox());
    assert_eq!(folders[0].name, "Inbox");
    assert_eq!(folders[0].role, FolderRole::Inbox);

    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    let id = delta.added[0].id.clone();
    let archive = FolderId("Archive".to_owned());
    for op in [
        Operation::SetFlags {
            id: id.clone(),
            flags: Flags {
                seen: true,
                ..Flags::default()
            },
        },
        Operation::Move {
            id: id.clone(),
            to: archive.clone(),
        },
        Operation::Copy {
            id: id.clone(),
            to: archive.clone(),
        },
    ] {
        assert!(
            matches!(backend.apply(&op).await, Err(Error::Unsupported(_))),
            "{op:?}"
        );
    }
    assert!(matches!(
        backend.move_message(&id, &archive).await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        backend
            .append(&inbox(), numbered(2).as_bytes(), Flags::default())
            .await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        backend.sync_folder(&archive, None).await,
        Err(Error::NotFound(_))
    ));
    // None of that touched the message.
    assert_eq!(server.uidls(user).await, [id.0]);

    // No push: watching returns at once and sends nothing.
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    backend.watch(tx).await.unwrap();
    assert!(rx.recv().await.is_none());
}

#[tokio::test]
async fn a_cursor_from_elsewhere_means_a_fresh_sync_and_never_a_reset() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "foreign@example.com";
    server.fill(user, 1, 2);
    let backend = server.backend(TlsMode::Implicit, user);
    for foreign in ["fixture-v1", "1;7;1:3,9", "", "1;not a cursor"] {
        let delta = backend
            .sync_folder(&inbox(), Some(SyncCursor(foreign.to_owned())))
            .await
            .unwrap();
        assert_eq!(delta.added.len(), 2, "{foreign}");
        assert!(delta.removed.is_empty() && !delta.reset, "{foreign}");
    }
}

#[tokio::test]
async fn a_larger_mailbox_arrives_whole() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "large@example.com";
    server.fill(user, 1, 300);
    let on_server = server.uidls(user).await;
    assert_eq!(on_server.len(), 300);
    // What the test below relies on Dovecot offering.
    let mut session = server.session(user).await;
    let capa = session.listing("CAPA").await;
    session.quit().await;
    for offered in ["TOP", "UIDL", "PIPELINING"] {
        assert!(capa.iter().any(|line| line == offered), "{capa:?}");
    }

    // In one batch, which asks for the sizes with a single LIST, and in
    // batches of fifty, which ask message by message. Both send their
    // commands in groups, since Dovecot allows pipelining.
    for batch in [500, 50] {
        let backend = server.backend(TlsMode::Implicit, user).with_batch(batch);
        let (deltas, cursor) = sync_all(&backend, None).await;
        assert_eq!(deltas.len(), 300_usize.div_ceil(batch), "batch {batch}");
        let added: Vec<_> = deltas.iter().flat_map(|delta| &delta.added).collect();
        assert_eq!(added.len(), 300);
        for (n, message) in (1..=300).rev().zip(&added) {
            assert_eq!(message.headers.subject, format!("Message {n}"));
            assert_eq!(message.id.0, on_server[n - 1]);
            assert_eq!(message.size, numbered(n).len() as u64, "batch {batch}");
        }
        // A cursor for three hundred messages is far smaller than their ids.
        let plain: usize = on_server.iter().map(|id| id.len() + 1).sum();
        assert!(
            cursor.0.len() * 4 < plain,
            "{} bytes of cursor for {plain} bytes of ids",
            cursor.0.len()
        );
    }
}

/// The acceptance test for POP3: however mail comes and goes between
/// polls, what the polls have reported adds up to exactly what is on the
/// server. No message is reported twice and none is missed.
#[tokio::test]
async fn repeated_polls_never_duplicate_or_lose_a_message() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "polls@example.com";
    // Small batches, so that syncs are often caught half way.
    let backend = server.backend(TlsMode::Implicit, user).with_batch(7);

    /// What the engine would hold after applying a delta.
    fn apply(held: &mut BTreeSet<String>, delta: &SyncDelta) {
        assert!(!delta.reset && delta.changed.is_empty());
        for message in &delta.added {
            assert!(
                held.insert(message.id.0.clone()),
                "{} was added twice",
                message.id.0
            );
        }
        for id in &delta.removed {
            assert!(held.remove(&id.0), "{} was removed but never held", id.0);
        }
    }
    let on_server = async || -> BTreeSet<String> { server.uidls(user).await.into_iter().collect() };

    let mut held = BTreeSet::new();
    let mut delivered = 0;
    let mut deliver = |count: usize| {
        server.fill(user, delivered + 1, count);
        delivered += count;
    };

    // A first sync of thirty messages, interrupted after each batch by
    // mail arriving and by another mail program deleting: one message
    // already synced, and one not yet reached.
    deliver(30);
    let mut cursor = None;
    let mut batches = 0;
    loop {
        let delta = backend.sync_folder(&inbox(), cursor.take()).await.unwrap();
        apply(&mut held, &delta);
        cursor = Some(delta.next_cursor.clone());
        batches += 1;
        if !delta.has_more {
            break;
        }
        if batches <= 2 {
            deliver(2);
            let listed = server.uidls(user).await;
            let synced = held.iter().next().unwrap().clone();
            let unsynced = listed.iter().find(|id| !held.contains(*id)).unwrap();
            server.delete_elsewhere(user, &[&synced, unsynced]).await;
        }
    }
    assert!(batches >= 4, "{batches}");
    assert_eq!(held, on_server().await);
    assert_eq!(held.len(), 30);

    // Then many polls, with something different between each pair.
    for round in 0..24 {
        let arriving = (round * 7) % 5;
        deliver(arriving);
        let listed = server.uidls(user).await;
        if round % 3 == 1 && listed.len() > 4 {
            // Deleted by another mail program: the oldest, and one from
            // the middle.
            server
                .delete_elsewhere(user, &[&listed[0], &listed[listed.len() / 2]])
                .await;
        }
        if round % 4 == 2 {
            // Deleted through the backend, as a leave-on-server policy
            // would: two that are held, and a repeat of one long gone.
            let doomed: Vec<RemoteId> = held.iter().take(2).cloned().map(RemoteId).collect();
            backend.delete_many(&doomed).await.unwrap();
            backend.delete_many(&doomed[..1]).await.unwrap();
        }
        if round == 10 {
            // A burst larger than a batch.
            deliver(20);
        }
        if round % 5 == 4 {
            // A poll with nothing new at all.
            let quiet = backend.sync_folder(&inbox(), cursor.clone()).await.unwrap();
            apply(&mut held, &quiet);
            cursor = Some(quiet.next_cursor);
        }

        loop {
            let delta = backend.sync_folder(&inbox(), cursor.take()).await.unwrap();
            apply(&mut held, &delta);
            cursor = Some(delta.next_cursor.clone());
            if !delta.has_more {
                break;
            }
        }
        assert_eq!(held, on_server().await, "after round {round}");
    }
    assert!(delivered > 80, "{delivered}");
    assert!(!held.is_empty());

    // A restart: a new backend with the stored cursor carries on where the
    // old one stopped, and finds nothing to report.
    let restarted = server.backend(TlsMode::Starttls, user);
    let delta = restarted.sync_folder(&inbox(), cursor).await.unwrap();
    assert!(delta.added.is_empty() && delta.removed.is_empty() && !delta.has_more);
}
