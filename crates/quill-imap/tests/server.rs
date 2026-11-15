//! The IMAP backend against a real server.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Certificates, Dovecot, PASSWORD, Starttls, TlsFront, backend, fill, message};
use futures_util::TryStreamExt;
use quill_core::config::TlsMode;
use quill_core::types::{Flags, FolderId, FolderRole, Operation, RemoteId, SyncCursor};
use quill_core::{Backend, Error};

fn inbox() -> FolderId {
    FolderId("INBOX".to_owned())
}

/// Syncs a folder to the end, the way the sync engine will.
async fn sync_all(
    backend: &dyn Backend,
    folder: &FolderId,
    mut cursor: Option<SyncCursor>,
) -> (Vec<quill_core::types::SyncDelta>, SyncCursor) {
    let mut deltas = Vec::new();
    loop {
        let delta = backend.sync_folder(folder, cursor.take()).await.unwrap();
        cursor = Some(delta.next_cursor.clone());
        let more = delta.has_more;
        deltas.push(delta);
        if !more {
            return (deltas, cursor.unwrap());
        }
    }
}

#[tokio::test]
async fn every_tls_mode_signs_in_and_lists_folders_with_their_roles() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    for mode in [TlsMode::Implicit, TlsMode::Starttls, TlsMode::None] {
        let backend = server.backend(mode, "modes@example.com");
        let folders = backend
            .list_folders()
            .await
            .unwrap_or_else(|err| panic!("{mode:?}: {err}"));
        let role = |name: &str| {
            folders
                .iter()
                .find(|folder| folder.id.0 == name)
                .unwrap_or_else(|| panic!("{mode:?}: no {name} in {folders:?}"))
                .role
        };
        assert_eq!(role("INBOX"), FolderRole::Inbox);
        assert_eq!(role("Sent"), FolderRole::Sent);
        assert_eq!(role("Drafts"), FolderRole::Drafts);
        assert_eq!(role("Trash"), FolderRole::Trash);
        assert_eq!(role("Archive"), FolderRole::Archive);
        assert_eq!(role("Junk"), FolderRole::Junk);
        backend.disconnect().await;
    }
}

#[tokio::test]
async fn folders_named_in_other_scripts_are_decoded_for_display() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "names@example.com";
    let mut session = server.session(user).await;
    session.create("Entw&APw-rfe").await.unwrap();
    session.create("Projects/&ZeVnLIqe-").await.unwrap();
    session.logout().await.unwrap();

    let folders = server
        .backend(TlsMode::Implicit, user)
        .list_folders()
        .await
        .unwrap();
    let named = |id: &str| folders.iter().find(|folder| folder.id.0 == id).unwrap();
    assert_eq!(named("Entw&APw-rfe").name, "Entwürfe");
    assert_eq!(named("Projects/&ZeVnLIqe-").name, "Projects/日本語");
    assert_eq!(named("Projects/&ZeVnLIqe-").role, FolderRole::Other);
}

#[tokio::test]
async fn a_wrong_password_is_an_auth_error() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let backend = backend(
        server.config(TlsMode::Implicit, "robin@example.com"),
        "wrong",
    );
    match backend.list_folders().await {
        Err(Error::Auth(_)) => {}
        other => panic!("{other:?}"),
    }
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
        match backend(config, PASSWORD).list_folders().await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(problem.host, "localhost");
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(server.certificates.leaf_der.as_slice())
                );
                assert_eq!(problem.fingerprint.as_ref().map(String::len), Some(64));
            }
            other => panic!("{mode:?}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_pinned_certificate_is_accepted_and_any_other_refused() {
    use sha2::{Digest, Sha256};
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let mut config = server.config(TlsMode::Implicit, "pin@example.com");
    config.trust.extra_roots.clear();
    config.trust.pinned_sha256 = Some(Sha256::digest(&server.certificates.leaf_der).into());
    backend(config.clone(), PASSWORD)
        .list_folders()
        .await
        .unwrap();

    // A pin for some other certificate, even with the CA trusted: the pin wins.
    config.trust.extra_roots = vec![server.certificates.ca_der.clone()];
    config.trust.pinned_sha256 = Some([7; 32]);
    match backend(config, PASSWORD).list_folders().await {
        Err(Error::Certificate(problem)) => {
            assert!(problem.reason.contains("pinned"), "{}", problem.reason);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_certificate_for_another_host_is_refused() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    // The certificate names "localhost"; connect by address instead.
    let mut config = server.config(TlsMode::Implicit, "robin@example.com");
    config.host = "127.0.0.1".to_owned();
    assert!(matches!(
        backend(config, PASSWORD).list_folders().await,
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
    let config = quill_imap::ImapConfig {
        host: "127.0.0.1".to_owned(),
        port,
        tls: TlsMode::None,
        username: "robin@example.com".to_owned(),
        trust: quill_imap::Trust::default(),
    };
    assert!(matches!(
        backend(config, PASSWORD).list_folders().await,
        Err(Error::Connection(_))
    ));
}

#[tokio::test]
async fn a_folder_syncs_newest_first_in_batches() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "batches@example.com";
    let mut session = server.session(user).await;
    fill(&mut session, "INBOX", 12).await;
    session.logout().await.unwrap();

    let backend = server.backend(TlsMode::Implicit, user).with_batch(5);
    let (deltas, cursor) = sync_all(&backend, &inbox(), None).await;
    let sizes: Vec<usize> = deltas.iter().map(|delta| delta.added.len()).collect();
    assert_eq!(sizes, [5, 5, 2]);
    assert!(
        deltas
            .iter()
            .all(|delta| !delta.reset && delta.removed.is_empty())
    );
    // The first batch is the newest mail.
    let subjects: Vec<&str> = deltas[0]
        .added
        .iter()
        .map(|message| message.headers.subject.as_str())
        .collect();
    assert_eq!(
        subjects,
        [
            "Message 12",
            "Message 11",
            "Message 10",
            "Message 9",
            "Message 8"
        ]
    );
    assert_eq!(deltas[2].added[1].headers.subject, "Message 1");

    // Every message exactly once.
    let mut ids: Vec<&str> = deltas
        .iter()
        .flat_map(|delta| &delta.added)
        .map(|message| message.id.0.as_str())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 12);

    // Nothing has changed, so syncing again from the cursor adds nothing.
    let again = backend.sync_folder(&inbox(), Some(cursor)).await.unwrap();
    assert!(again.added.is_empty() && again.removed.is_empty() && !again.has_more);
    assert_eq!(again.changed.len(), 12);
}

#[tokio::test]
async fn a_synced_message_carries_decoded_headers_flags_size_and_attachment_marker() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "headers@example.com";
    let raw = "From: =?UTF-8?B?6ZmI6Z2Z?= <chen.jing@example.com>\r\n\
               To: Robin Vale <robin@example.com>\r\n\
               Cc: Omar Haddad <omar.haddad@example.com>\r\n\
               Subject: =?utf-8?Q?Tom=C3=A1s_sent_an_invoice?=\r\n\
               Date: Sat, 14 Nov 2026 09:14:00 +0000\r\n\
               Message-ID: <invoice.114@example.com>\r\n\
               MIME-Version: 1.0\r\n\
               Content-Type: multipart/mixed; boundary=\"b\"\r\n\
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
    let plain = message(
        "Ana Ruiz <ana.ruiz@example.com>",
        "No attachment",
        "Fri, 13 Nov 2026 10:00:00 +0000",
        "Just text.",
    );
    let mut session = server.session(user).await;
    session
        .append("INBOX", Some("(\\Seen \\Flagged)"), None, raw.as_bytes())
        .await
        .unwrap();
    session
        .append("INBOX", None, None, plain.as_bytes())
        .await
        .unwrap();
    session.logout().await.unwrap();

    let backend = server.backend(TlsMode::Starttls, user);
    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    assert_eq!(delta.added.len(), 2);
    let invoice = &delta.added[0];
    assert_eq!(invoice.headers.subject, "Tomás sent an invoice");
    assert_eq!(invoice.headers.from[0].name.as_deref(), Some("陈静"));
    assert_eq!(invoice.headers.from[0].email, "chen.jing@example.com");
    assert_eq!(invoice.headers.cc[0].email, "omar.haddad@example.com");
    assert_eq!(
        invoice.headers.message_id.as_deref(),
        Some("<invoice.114@example.com>")
    );
    assert_eq!(invoice.headers.date, 1_794_647_640);
    assert_eq!(
        invoice.flags,
        Flags {
            seen: true,
            flagged: true,
            answered: false,
            draft: false
        }
    );
    assert_eq!(invoice.size, raw.len() as u64);
    assert!(invoice.has_attachments);

    let plain = &delta.added[1];
    assert_eq!(plain.headers.subject, "No attachment");
    assert!(!plain.has_attachments);
    assert_eq!(plain.flags, Flags::default());
}

#[tokio::test]
async fn later_syncs_report_arrivals_flag_changes_and_removals() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "changes@example.com";
    let mut other_client = server.session(user).await;
    fill(&mut other_client, "INBOX", 4).await;

    let backend = server.backend(TlsMode::Implicit, user);
    let (first, cursor) = sync_all(&backend, &inbox(), None).await;
    let by_subject = |subject: &str| {
        first[0]
            .added
            .iter()
            .find(|message| message.headers.subject == subject)
            .unwrap()
            .id
            .clone()
    };
    let (one, two) = (by_subject("Message 1"), by_subject("Message 2"));

    // Meanwhile, elsewhere: one message read, one deleted, one arrives.
    other_client.select("INBOX").await.unwrap();
    let _: Vec<_> = other_client
        .uid_store("1", "+FLAGS (\\Seen)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let _: Vec<_> = other_client
        .uid_store("2", "+FLAGS (\\Deleted)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let _: Vec<_> = other_client
        .expunge()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    fill(&mut other_client, "INBOX", 1).await;

    let delta = backend.sync_folder(&inbox(), Some(cursor)).await.unwrap();
    assert!(!delta.reset && !delta.has_more);
    assert_eq!(delta.added.len(), 1);
    assert_eq!(delta.removed, [two]);
    let seen_now: Vec<&RemoteId> = delta
        .changed
        .iter()
        .filter(|change| change.flags.seen)
        .map(|change| &change.id)
        .collect();
    assert_eq!(seen_now, [&one]);
    // The removed message and the new one are not among the flag reports.
    assert_eq!(delta.changed.len(), 3);
}

#[tokio::test]
async fn a_renumbered_folder_resets() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "reset@example.com";
    let folder = FolderId("Projects".to_owned());
    let mut session = server.session(user).await;
    session.create("Projects").await.unwrap();
    fill(&mut session, "Projects", 3).await;

    let backend = server.backend(TlsMode::Implicit, user);
    let (_, cursor) = sync_all(&backend, &folder, None).await;

    // Deleting and recreating a folder gives it a new UIDVALIDITY. Dovecot
    // derives it from the clock, so let the clock move first.
    session.delete("Projects").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    session.create("Projects").await.unwrap();
    fill(&mut session, "Projects", 1).await;

    let delta = backend
        .sync_folder(&folder, Some(cursor.clone()))
        .await
        .unwrap();
    assert!(delta.reset, "{delta:?}");
    assert_eq!(delta.added.len(), 1);
    // A reset replaces everything, so there is nothing to list as removed.
    assert!(delta.removed.is_empty() && delta.changed.is_empty());
    assert_ne!(delta.next_cursor, cursor);
}

#[tokio::test]
async fn a_cursor_from_another_backend_means_a_fresh_sync_not_a_reset() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "foreign@example.com";
    let mut session = server.session(user).await;
    fill(&mut session, "INBOX", 2).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let delta = backend
        .sync_folder(&inbox(), Some(SyncCursor("fixture-v1".to_owned())))
        .await
        .unwrap();
    assert_eq!(delta.added.len(), 2);
    assert!(!delta.reset);
}

#[tokio::test]
async fn a_missing_folder_is_not_found() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let backend = server.backend(TlsMode::Implicit, "robin@example.com");
    assert!(matches!(
        backend
            .sync_folder(&FolderId("No Such Folder".to_owned()), None)
            .await,
        Err(Error::NotFound(_))
    ));
    // The backend recovers: the next call signs in again and works.
    assert!(backend.list_folders().await.is_ok());
}

#[tokio::test]
async fn fetching_a_message_returns_its_exact_bytes_and_leaves_it_unread() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "fetch@example.com";
    let raw = message(
        "Maya Okafor <maya.okafor@example.com>",
        "Trail run Saturday",
        "Sat, 14 Nov 2026 04:30:00 +0000",
        "Seven o'clock at the north car park.\r\n\r\nMaya",
    );
    let mut session = server.session(user).await;
    session
        .append("INBOX", None, None, raw.as_bytes())
        .await
        .unwrap();

    let backend = server.backend(TlsMode::Implicit, user);
    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    let id = delta.added[0].id.clone();
    let fetched = backend.fetch_raw(&id).await.unwrap();
    assert_eq!(fetched.as_ref(), raw.as_bytes());
    let parsed = quill_mime::parse(&fetched, 0).unwrap();
    assert_eq!(
        parsed.body,
        "Seven o'clock at the north car park.\n\nMaya\n"
    );

    // Reading it through Quill did not mark it read on the server.
    let again = backend
        .sync_folder(&inbox(), Some(delta.next_cursor))
        .await
        .unwrap();
    assert!(!again.changed[0].flags.seen);

    // A message that is gone, and an id from a renumbered folder.
    let gone = RemoteId(id.0.replacen('1', "999", 1));
    assert!(matches!(
        backend.fetch_raw(&gone).await,
        Err(Error::NotFound(_))
    ));
    let stale = RemoteId("1;12345;INBOX".to_owned());
    assert!(matches!(
        backend.fetch_raw(&stale).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn flags_are_set_and_cleared_on_the_server_without_touching_other_keywords() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "flags@example.com";
    let mut session = server.session(user).await;
    let raw = message(
        "Lena Fischer <lena.fischer@example.com>",
        "Lunch on Thursday?",
        "Mon, 09 Nov 2026 12:00:00 +0000",
        "Are you around?",
    );
    session
        .append(
            "INBOX",
            Some("(\\Answered $Important)"),
            None,
            raw.as_bytes(),
        )
        .await
        .unwrap();

    let backend = server.backend(TlsMode::Implicit, user);
    let delta = backend.sync_folder(&inbox(), None).await.unwrap();
    let id = delta.added[0].id.clone();
    assert!(delta.added[0].flags.answered);

    let wanted = Flags {
        seen: true,
        flagged: true,
        answered: false,
        draft: false,
    };
    backend
        .apply(&Operation::SetFlags { id, flags: wanted })
        .await
        .unwrap();
    let after = backend
        .sync_folder(&inbox(), Some(delta.next_cursor))
        .await
        .unwrap();
    assert_eq!(after.changed[0].flags, wanted);

    // The keyword another client set is still there.
    session.select("INBOX").await.unwrap();
    let fetches: Vec<_> = session
        .uid_fetch("1", "(FLAGS)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let flags = format!("{:?}", fetches[0].flags().collect::<Vec<_>>());
    assert!(flags.contains("$Important"), "{flags}");
}

#[tokio::test]
async fn a_large_folder_syncs_completely() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "large@example.com";
    let mut session = server.session(user).await;
    fill(&mut session, "INBOX", 1200).await;
    // Holes in the UID sequence, as a lived-in mailbox has.
    session.select("INBOX").await.unwrap();
    let _: Vec<_> = session
        .uid_store("100:150,400,777:800", "+FLAGS (\\Deleted)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let _: Vec<_> = session
        .expunge()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();

    let backend = server.backend(TlsMode::Implicit, user);
    let (deltas, cursor) = sync_all(&backend, &inbox(), None).await;
    let total: usize = deltas.iter().map(|delta| delta.added.len()).sum();
    assert_eq!(total, 1200 - 51 - 1 - 24);
    assert_eq!(deltas.len(), 3);
    // The cursor stays small however many messages it covers.
    assert!(cursor.0.len() < 100, "{}", cursor.0);
}

#[tokio::test]
async fn an_expired_certificate_is_refused() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    // Valid for thirty days, starting sixty days ago.
    let expired = Certificates::issue(&["localhost"], 60, 30);
    let front = TlsFront::start(server.imap_port, &expired, Starttls::Offered).await;
    for (mode, port) in [
        (TlsMode::Implicit, front.implicit_port),
        (TlsMode::Starttls, front.starttls_port),
    ] {
        let mut config = server.config(mode, "robin@example.com");
        config.port = port;
        config.trust.extra_roots = vec![expired.ca_der.clone()];
        match backend(config, PASSWORD).list_folders().await {
            Err(Error::Certificate(problem)) => {
                assert_eq!(
                    problem.certificate.as_deref(),
                    Some(expired.leaf_der.as_slice())
                );
            }
            other => panic!("{mode:?}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_certificate_naming_another_host_is_refused() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let other = Certificates::issue(&["mail.example.com"], 1, 90);
    let front = TlsFront::start(server.imap_port, &other, Starttls::Offered).await;
    let mut config = server.config(TlsMode::Implicit, "robin@example.com");
    config.port = front.implicit_port;
    config.trust.extra_roots = vec![other.ca_der.clone()];
    assert!(matches!(
        backend(config, PASSWORD).list_folders().await,
        Err(Error::Certificate(_))
    ));
}

#[tokio::test]
async fn a_server_that_refuses_starttls_never_sees_the_password() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let front = TlsFront::start(server.imap_port, &server.certificates, Starttls::Refused).await;
    let mut config = server.config(TlsMode::Starttls, "robin@example.com");
    config.port = front.starttls_port;
    let backend = backend(config, PASSWORD);
    for _ in 0..2 {
        match backend.list_folders().await {
            Err(Error::Insecure(reason)) => assert!(reason.contains("STARTTLS"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
    // There is no fallback to plaintext: all the server ever heard in the
    // clear was the request to upgrade.
    let cleartext = front.cleartext.lock().unwrap().clone();
    assert_eq!(cleartext.len(), 2, "{cleartext:?}");
    for line in &cleartext {
        assert!(line.ends_with("STARTTLS"), "{line}");
        assert!(!line.contains(PASSWORD) && !line.to_uppercase().contains("LOGIN"));
    }
}

#[tokio::test]
async fn starttls_sends_nothing_but_the_upgrade_request_in_the_clear() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let backend = server.backend(TlsMode::Starttls, "clear@example.com");
    backend.list_folders().await.unwrap();
    let cleartext = server.front.cleartext.lock().unwrap().clone();
    assert_eq!(cleartext.len(), 1, "{cleartext:?}");
    assert!(cleartext[0].ends_with("STARTTLS"));
}
