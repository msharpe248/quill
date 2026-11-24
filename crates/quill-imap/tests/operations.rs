//! Moving, copying, deleting and saving messages, against a real server.

// Test helpers outside `#[test]` functions may unwrap too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use async_imap::Session;
use common::{Dovecot, PASSWORD, Without, backend, fill, message};
use futures_util::TryStreamExt;
use quill_core::config::TlsMode;
use quill_core::types::{Flags, FolderId, Operation, RemoteId, RemoteMessage};
use quill_core::{Backend, Error};
use quill_imap::ImapBackend;
use tokio::net::TcpStream;

fn folder(name: &str) -> FolderId {
    FolderId(name.to_owned())
}

/// Everything in a folder as the backend sees it, oldest first.
async fn listing(backend: &dyn Backend, name: &str) -> Vec<RemoteMessage> {
    let mut messages = Vec::new();
    let mut cursor = None;
    loop {
        let delta = backend.sync_folder(&folder(name), cursor).await.unwrap();
        messages.extend(delta.added);
        if !delta.has_more {
            break;
        }
        cursor = Some(delta.next_cursor);
    }
    messages.sort_by_key(|message| message.headers.date);
    messages
}

fn subjects(messages: &[RemoteMessage]) -> Vec<&str> {
    messages
        .iter()
        .map(|message| message.headers.subject.as_str())
        .collect()
}

fn id_of(messages: &[RemoteMessage], subject: &str) -> RemoteId {
    messages
        .iter()
        .find(|message| message.headers.subject == subject)
        .unwrap_or_else(|| panic!("no {subject:?} among {:?}", subjects(messages)))
        .id
        .clone()
}

/// Every UID in a folder and its flags, as another mail program sees them.
async fn flags_by_uid(session: &mut Session<TcpStream>, name: &str) -> BTreeMap<u32, String> {
    session.select(name).await.unwrap();
    let fetches: Vec<_> = session
        .uid_fetch("1:*", "(UID FLAGS)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    fetches
        .iter()
        .map(|fetch| {
            let mut flags: Vec<String> = fetch.flags().map(|flag| format!("{flag:?}")).collect();
            flags.retain(|flag| flag != "Recent");
            flags.sort();
            (fetch.uid.unwrap(), flags.join(" "))
        })
        .collect()
}

/// Marks messages `\Deleted` without expunging, as a mail program set to
/// "mark as deleted" does.
async fn mark_deleted(session: &mut Session<TcpStream>, name: &str, uids: &str) {
    session.select(name).await.unwrap();
    let _: Vec<_> = session
        .uid_store(uids, "+FLAGS (\\Deleted)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
}

fn invoice() -> String {
    message(
        "Omar Haddad <omar.haddad@example.com>",
        "Invoice 114",
        "Thu, 12 Nov 2026 16:20:00 +0000",
        "Please find the invoice below.\r\n\r\nOmar",
    )
}

/// An inbox with three numbered messages and, after them, a flagged invoice.
async fn arrange(server: &Dovecot, user: &str) -> Session<TcpStream> {
    let mut session = server.session(user).await;
    fill(&mut session, "INBOX", 3).await;
    session
        .append(
            "INBOX",
            Some("(\\Seen \\Flagged $Important)"),
            None,
            invoice().as_bytes(),
        )
        .await
        .unwrap();
    session
}

const SEEN_AND_FLAGGED: Flags = Flags {
    seen: true,
    flagged: true,
    answered: false,
    draft: false,
};

#[tokio::test]
async fn a_move_takes_the_message_its_flags_and_its_body_and_says_where_it_went() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "move@example.com";
    let mut other_client = arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let old = id_of(&listing(&backend, "INBOX").await, "Invoice 114");

    let new = backend
        .move_message(&old, &folder("Archive"))
        .await
        .unwrap()
        .expect("Dovecot has UIDPLUS, so it says where the message went");

    let inbox = listing(&backend, "INBOX").await;
    assert_eq!(subjects(&inbox), ["Message 1", "Message 2", "Message 3"]);
    let archive = listing(&backend, "Archive").await;
    assert_eq!(subjects(&archive), ["Invoice 114"]);
    assert_eq!(archive[0].id, new);
    assert_eq!(archive[0].flags, SEEN_AND_FLAGGED);
    assert_eq!(
        backend.fetch_raw(&new).await.unwrap().as_ref(),
        invoice().as_bytes()
    );
    // The keyword Quill does not track went along too.
    let flags = flags_by_uid(&mut other_client, "Archive").await;
    assert_eq!(flags.len(), 1);
    assert!(
        flags.values().all(|flags| flags.contains("$Important")),
        "{flags:?}"
    );
    // The old id names nothing now.
    assert!(matches!(
        backend.fetch_raw(&old).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn the_id_a_move_returns_moves_the_message_back() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "back@example.com";
    arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let first = id_of(&listing(&backend, "INBOX").await, "Invoice 114");

    let there = backend
        .move_message(&first, &folder("Archive"))
        .await
        .unwrap()
        .unwrap();
    let back = backend
        .move_message(&there, &folder("INBOX"))
        .await
        .unwrap()
        .unwrap();

    assert_ne!(back, first, "a moved message gets a new UID");
    assert!(listing(&backend, "Archive").await.is_empty());
    let inbox = listing(&backend, "INBOX").await;
    assert_eq!(inbox.len(), 4);
    assert_eq!(id_of(&inbox, "Invoice 114"), back);
    assert_eq!(
        backend.fetch_raw(&back).await.unwrap().as_ref(),
        invoice().as_bytes()
    );
}

#[tokio::test]
async fn moving_a_message_that_was_already_moved_is_ok_and_makes_no_duplicate() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "replay@example.com";
    arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let id = id_of(&listing(&backend, "INBOX").await, "Message 2");
    let to = folder("Archive");

    assert!(backend.move_message(&id, &to).await.unwrap().is_some());
    // The replay after a crash: same id, same destination.
    assert_eq!(backend.move_message(&id, &to).await.unwrap(), None);
    backend
        .apply(&Operation::Move {
            id: id.clone(),
            to: to.clone(),
        })
        .await
        .unwrap();

    assert_eq!(subjects(&listing(&backend, "Archive").await), ["Message 2"]);
    assert_eq!(listing(&backend, "INBOX").await.len(), 3);

    // A message whose whole folder has gone is as moved as it will ever be.
    let mut other_client = server.session(user).await;
    other_client.create("Passing").await.unwrap();
    fill(&mut other_client, "Passing", 1).await;
    let passing = listing(&backend, "Passing").await[0].id.clone();
    other_client.delete("Passing").await.unwrap();
    assert_eq!(backend.move_message(&passing, &to).await.unwrap(), None);
    backend
        .apply(&Operation::Delete { id: passing })
        .await
        .unwrap();
    assert_eq!(listing(&backend, "Archive").await.len(), 1);
}

#[tokio::test]
async fn moving_a_message_to_the_folder_it_is_in_leaves_it_alone() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "same@example.com";
    arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let id = id_of(&listing(&backend, "INBOX").await, "Message 1");
    assert_eq!(
        backend.move_message(&id, &folder("INBOX")).await.unwrap(),
        Some(id.clone())
    );
    assert_eq!(id_of(&listing(&backend, "INBOX").await, "Message 1"), id);
}

#[tokio::test]
async fn moving_to_a_folder_that_is_not_there_is_not_found_and_the_message_stays() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "nowhere@example.com";
    let mut other_client = arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let before = flags_by_uid(&mut other_client, "INBOX").await;
    let id = id_of(&listing(&backend, "INBOX").await, "Invoice 114");

    for missing in ["No Such Folder", "Archive/Deeper"] {
        match backend.move_message(&id, &folder(missing)).await {
            Err(Error::NotFound(text)) => assert!(text.contains(missing), "{text}"),
            other => panic!("{missing}: {other:?}"),
        }
        assert!(matches!(
            backend
                .apply(&Operation::Copy {
                    id: id.clone(),
                    to: folder(missing),
                })
                .await,
            Err(Error::NotFound(_))
        ));
    }

    // Nothing moved, nothing was marked, and no folder was made on the way.
    assert_eq!(flags_by_uid(&mut other_client, "INBOX").await, before);
    assert_eq!(
        backend.fetch_raw(&id).await.unwrap().as_ref(),
        invoice().as_bytes()
    );
    let folders = backend.list_folders().await.unwrap();
    assert!(
        folders
            .iter()
            .all(|folder| folder.id.0 != "No Such Folder" && !folder.id.0.contains("Deeper"))
    );
}

#[tokio::test]
async fn an_id_from_before_a_folder_was_renumbered_touches_nothing() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "stale@example.com";
    let mut other_client = arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let before = flags_by_uid(&mut other_client, "INBOX").await;
    let real = id_of(&listing(&backend, "INBOX").await, "Message 2");

    // The same UID under another UIDVALIDITY: on the server that UID is now
    // Message 2, which this id never meant.
    let (uid, rest) = real.0.split_once(';').unwrap();
    let (validity, name) = rest.split_once(';').unwrap();
    let stale = RemoteId(format!(
        "{uid};{};{name}",
        validity.parse::<u32>().unwrap() + 1
    ));

    assert_eq!(
        backend
            .move_message(&stale, &folder("Archive"))
            .await
            .unwrap(),
        None
    );
    backend
        .apply(&Operation::Delete { id: stale.clone() })
        .await
        .unwrap();
    assert!(matches!(
        backend
            .apply(&Operation::Copy {
                id: stale,
                to: folder("Archive"),
            })
            .await,
        Err(Error::NotFound(_))
    ));

    assert_eq!(flags_by_uid(&mut other_client, "INBOX").await, before);
    assert!(listing(&backend, "Archive").await.is_empty());
}

#[tokio::test]
async fn a_copy_leaves_the_original_and_adds_one_like_it() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "copy@example.com";
    arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let id = id_of(&listing(&backend, "INBOX").await, "Invoice 114");

    backend
        .apply(&Operation::Copy {
            id: id.clone(),
            to: folder("Archive"),
        })
        .await
        .unwrap();

    assert_eq!(listing(&backend, "INBOX").await.len(), 4);
    assert_eq!(
        backend.fetch_raw(&id).await.unwrap().as_ref(),
        invoice().as_bytes()
    );
    let archive = listing(&backend, "Archive").await;
    assert_eq!(subjects(&archive), ["Invoice 114"]);
    assert_eq!(archive[0].flags, SEEN_AND_FLAGGED);
    assert_eq!(
        backend.fetch_raw(&archive[0].id).await.unwrap().as_ref(),
        invoice().as_bytes()
    );

    // A message that is gone cannot be copied, and unlike a move nothing
    // says it already was.
    backend
        .apply(&Operation::Delete { id: id.clone() })
        .await
        .unwrap();
    assert!(matches!(
        backend
            .apply(&Operation::Copy {
                id,
                to: folder("Archive"),
            })
            .await,
        Err(Error::NotFound(_))
    ));
    assert_eq!(listing(&backend, "Archive").await.len(), 1);
}

#[tokio::test]
async fn a_delete_removes_its_own_message_for_good_and_nobody_elses() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "delete@example.com";
    let mut other_client = arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let inbox = listing(&backend, "INBOX").await;
    let id = id_of(&inbox, "Message 2");

    // Another mail program has marked two messages and not expunged them.
    mark_deleted(&mut other_client, "INBOX", "1,4").await;

    backend
        .apply(&Operation::Delete { id: id.clone() })
        .await
        .unwrap();

    let left = flags_by_uid(&mut other_client, "INBOX").await;
    assert_eq!(left.keys().copied().collect::<Vec<_>>(), [1, 3, 4]);
    assert!(left[&1].contains("Deleted"), "{left:?}");
    assert!(!left[&3].contains("Deleted"), "{left:?}");
    assert!(left[&4].contains("Deleted"), "{left:?}");
    assert!(matches!(
        backend.fetch_raw(&id).await,
        Err(Error::NotFound(_))
    ));
    // It is not in Trash or anywhere else either.
    for name in ["Trash", "Archive"] {
        assert!(listing(&backend, name).await.is_empty(), "{name}");
    }

    // Deleting it again is the replay after a crash, and is fine.
    backend.apply(&Operation::Delete { id }).await.unwrap();
    assert_eq!(flags_by_uid(&mut other_client, "INBOX").await, left);
}

#[tokio::test]
async fn a_saved_message_keeps_its_bytes_and_gets_its_flags() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "save@example.com";
    let backend = server.backend(TlsMode::Implicit, user);
    let sent = message(
        "Robin Vale <robin@example.com>",
        "Re: Trail run Saturday",
        "Sat, 14 Nov 2026 08:02:00 +0000",
        "I'll be there.\r\n\r\n-- \r\nRobin",
    );
    let draft = message(
        "Robin Vale <robin@example.com>",
        "Notes for Thursday",
        "Sat, 14 Nov 2026 08:10:00 +0000",
        "Tom\u{e1}s, \u{9648}\u{9759}: agenda to follow.",
    );

    let cases = [
        (
            "Sent",
            &sent,
            Flags {
                seen: true,
                ..Flags::default()
            },
        ),
        (
            "Drafts",
            &draft,
            Flags {
                seen: true,
                flagged: true,
                answered: false,
                draft: true,
            },
        ),
        // No flags at all: an unread message, with no flag list sent.
        ("Archive", &sent, Flags::default()),
        (
            "INBOX",
            &sent,
            Flags {
                answered: true,
                ..Flags::default()
            },
        ),
    ];
    for (name, raw, flags) in cases {
        let id = backend
            .append(&folder(name), raw.as_bytes(), flags)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{name}: Dovecot says what a saved message is called"));
        assert_eq!(
            backend.fetch_raw(&id).await.unwrap().as_ref(),
            raw.as_bytes(),
            "{name}"
        );
        let saved = listing(&backend, name).await;
        assert_eq!(saved.len(), 1, "{name}");
        assert_eq!(saved[0].id, id, "{name}");
        assert_eq!(saved[0].flags, flags, "{name}");
    }

    // Saved now, by the server's clock: not at the Date the message claims.
    let mut other_client = server.session(user).await;
    other_client.select("Sent").await.unwrap();
    let fetches: Vec<_> = other_client
        .uid_fetch("1", "(INTERNALDATE)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let saved_at = fetches[0].internal_date().unwrap().timestamp();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!((now - saved_at).abs() < 120, "{saved_at} against {now}");

    // Nothing is not a message.
    assert!(matches!(
        backend.append(&folder("Sent"), b"", Flags::default()).await,
        Err(Error::Backend(_))
    ));
}

#[tokio::test]
async fn a_three_megabyte_message_is_saved_whole() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "large-save@example.com";
    // An attachment's worth of base64, in lines as a composer writes them.
    let line = "UXVpbGwgdGVzdCBhdHRhY2htZW50IGJ5dGVzLCBub3RoaW5nIHRvIHNlZSBoZXJlLiBNb3ZlIG9u\r\n";
    let mut raw = String::from(
        "From: Robin Vale <robin@example.com>\r\n\
         To: Maya Okafor <maya.okafor@example.com>\r\n\
         Subject: The photographs\r\n\
         Date: Sat, 14 Nov 2026 11:00:00 +0000\r\n\
         Message-ID: <photographs.1@example.com>\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Transfer-Encoding: base64\r\n\
         \r\n",
    );
    while raw.len() < 3 * 1024 * 1024 {
        raw.push_str(line);
    }

    for mode in [TlsMode::Implicit, TlsMode::None] {
        let backend = server.backend(mode, user);
        let id = backend
            .append(
                &folder("Sent"),
                raw.as_bytes(),
                Flags {
                    seen: true,
                    ..Flags::default()
                },
            )
            .await
            .unwrap_or_else(|err| panic!("{mode:?}: {err}"))
            .unwrap();
        let fetched = backend.fetch_raw(&id).await.unwrap();
        assert_eq!(fetched.len(), raw.len(), "{mode:?}");
        assert!(fetched.as_ref() == raw.as_bytes(), "{mode:?}");
        // The connection is still good for the next thing.
        let moved = backend
            .move_message(&id, &folder("Archive"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(backend.fetch_raw(&moved).await.unwrap().len(), raw.len());
        backend.disconnect().await;
    }
}

#[tokio::test]
async fn saving_into_a_folder_that_is_not_there_is_not_found() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "save-nowhere@example.com";
    // Through the relay, hiding nothing, to see what was sent.
    let relay = Without::start(server.imap_port, &[]).await;
    let mut config = server.config(TlsMode::None, user);
    config.port = relay.port;
    let backend = backend(config, PASSWORD);
    let raw = invoice();

    match backend
        .append(&folder("Sent Items"), raw.as_bytes(), Flags::default())
        .await
    {
        Err(Error::NotFound(text)) => assert!(text.contains("Sent Items"), "{text}"),
        other => panic!("{other:?}"),
    }
    // The server said no when it heard the folder's name, and the message
    // itself was never sent.
    let commands = relay.commands();
    assert!(
        commands
            .iter()
            .any(|command| command.starts_with("APPEND \"Sent Items\" {")),
        "{commands:?}"
    );
    assert!(
        commands.iter().all(|command| !command.contains("Invoice")),
        "{commands:?}"
    );
    // No folder was made, and the backend carries on.
    let folders = backend.list_folders().await.unwrap();
    assert!(folders.iter().all(|folder| folder.id.0 != "Sent Items"));
    assert!(
        backend
            .append(&folder("Sent"), raw.as_bytes(), Flags::default())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn folders_with_awkward_names_take_moves_copies_and_saves() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "awkward@example.com";
    let mut other_client = server.session(user).await;
    // A space, another script (modified UTF-7, as the server lists it), a
    // quote, a backslash, and characters that mean something to LIST or
    // look like the start of a literal.
    let names = [
        "Client Work",
        "Entw&APw-rfe",
        "Projects/&ZeVnLIqe- 2026",
        "Say \"when\"",
        "Back\\slash",
        "100% {5}",
    ];
    for name in names {
        other_client
            .create(name)
            .await
            .unwrap_or_else(|err| panic!("{name}: {err}"));
    }
    fill(&mut other_client, "INBOX", names.len()).await;

    let backend = server.backend(TlsMode::Implicit, user);
    // The ids the backend itself gives these folders are the ones used.
    let listed = backend.list_folders().await.unwrap();
    let inbox = listing(&backend, "INBOX").await;
    for (n, name) in names.iter().enumerate() {
        let to = listed
            .iter()
            .find(|folder| folder.id.0 == *name)
            .unwrap_or_else(|| panic!("{name} is not among {listed:?}"))
            .id
            .clone();
        let subject = format!("Message {}", n + 1);
        let raw = backend.fetch_raw(&id_of(&inbox, &subject)).await.unwrap();

        let moved = backend
            .move_message(&id_of(&inbox, &subject), &to)
            .await
            .unwrap_or_else(|err| panic!("{name}: {err}"))
            .unwrap();
        assert_eq!(backend.fetch_raw(&moved).await.unwrap(), raw, "{name}");

        let saved = backend
            .append(&to, invoice().as_bytes(), Flags::default())
            .await
            .unwrap_or_else(|err| panic!("{name}: {err}"))
            .unwrap();
        assert_eq!(
            backend.fetch_raw(&saved).await.unwrap().as_ref(),
            invoice().as_bytes(),
            "{name}"
        );

        // Out of the awkward folder as well as into it.
        backend
            .apply(&Operation::Copy {
                id: saved.clone(),
                to: folder("Archive"),
            })
            .await
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        backend
            .apply(&Operation::Delete { id: saved })
            .await
            .unwrap_or_else(|err| panic!("{name}: {err}"));
        assert_eq!(
            subjects(&listing(&backend, name).await),
            [subject.as_str()],
            "{name}"
        );
    }
    assert!(listing(&backend, "INBOX").await.is_empty());
    assert_eq!(listing(&backend, "Archive").await.len(), names.len());

    // A name that is a pattern to LIST matches other folders, and is still
    // not mistaken for one that exists.
    assert!(matches!(
        backend
            .append(&folder("Client%"), invoice().as_bytes(), Flags::default())
            .await,
        Err(Error::NotFound(_))
    ));
}

/// A backend for `user` on a server that lacks `hidden`.
async fn lesser(server: &Dovecot, user: &str, hidden: &[&str]) -> (ImapBackend, Without) {
    let relay = Without::start(server.imap_port, hidden).await;
    let mut config = server.config(TlsMode::None, user);
    config.port = relay.port;
    (backend(config, PASSWORD), relay)
}

fn sent(commands: &[String], start: &str) -> bool {
    commands.iter().any(|command| command.starts_with(start))
}

#[tokio::test]
async fn with_move_a_message_is_moved_by_one_command() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "one-command@example.com";
    arrange(&server, user).await;
    let (backend, relay) = lesser(&server, user, &[]).await;
    let id = id_of(&listing(&backend, "INBOX").await, "Invoice 114");
    relay.clear();

    let new = backend.move_message(&id, &folder("Archive")).await.unwrap();
    assert!(new.is_some());

    let commands = relay.commands();
    assert!(sent(&commands, "UID MOVE 4 \"Archive\""), "{commands:?}");
    for never in ["UID COPY", "UID STORE", "UID EXPUNGE", "EXPUNGE"] {
        assert!(!sent(&commands, never), "{never} in {commands:?}");
    }
}

#[tokio::test]
async fn without_move_a_message_is_copied_then_expunged_by_uid() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "no-move@example.com";
    let mut other_client = arrange(&server, user).await;
    let (backend, relay) = lesser(&server, user, &["MOVE"]).await;
    let id = id_of(&listing(&backend, "INBOX").await, "Invoice 114");
    // Marked by another mail program, and not ours to remove.
    mark_deleted(&mut other_client, "INBOX", "2").await;
    relay.clear();

    let new = backend
        .move_message(&id, &folder("Archive"))
        .await
        .unwrap()
        .expect("COPY reports COPYUID as MOVE does");

    let commands = relay.commands();
    assert!(!sent(&commands, "UID MOVE"), "{commands:?}");
    assert!(sent(&commands, "UID COPY 4 \"Archive\""), "{commands:?}");
    assert!(
        sent(&commands, "UID STORE 4 +FLAGS.SILENT (\\Deleted)"),
        "{commands:?}"
    );
    assert!(sent(&commands, "UID EXPUNGE 4"), "{commands:?}");
    assert!(!sent(&commands, "EXPUNGE"), "{commands:?}");

    let archive = listing(&backend, "Archive").await;
    assert_eq!(subjects(&archive), ["Invoice 114"]);
    assert_eq!(archive[0].id, new);
    // The copy was made before the original was marked, so it is not
    // marked itself.
    assert_eq!(archive[0].flags, SEEN_AND_FLAGGED);
    let there = flags_by_uid(&mut other_client, "Archive").await;
    assert!(there.values().all(|flags| !flags.contains("Deleted")));
    assert_eq!(
        backend.fetch_raw(&new).await.unwrap().as_ref(),
        invoice().as_bytes()
    );

    let left = flags_by_uid(&mut other_client, "INBOX").await;
    assert_eq!(left.keys().copied().collect::<Vec<_>>(), [1, 2, 3]);
    assert!(left[&2].contains("Deleted"), "{left:?}");

    // The replay finds it gone and copies nothing.
    relay.clear();
    assert_eq!(
        backend.move_message(&id, &folder("Archive")).await.unwrap(),
        None
    );
    assert!(!sent(&relay.commands(), "UID COPY"));
    assert_eq!(listing(&backend, "Archive").await.len(), 1);
    // To a folder that is not there, nothing is marked on the way.
    let two = id_of(&listing(&backend, "INBOX").await, "Message 3");
    assert!(matches!(
        backend.move_message(&two, &folder("Nowhere")).await,
        Err(Error::NotFound(_))
    ));
    assert_eq!(flags_by_uid(&mut other_client, "INBOX").await, left);
}

#[tokio::test]
async fn without_uidplus_a_plain_expunge_is_used_only_when_nothing_else_is_marked() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "bare@example.com";
    let mut other_client = arrange(&server, user).await;
    let (backend, relay) = lesser(&server, user, &["MOVE", "UIDPLUS"]).await;
    let inbox = listing(&backend, "INBOX").await;
    relay.clear();

    // Nothing else is marked: the move and the delete go through.
    let moved = backend
        .move_message(&id_of(&inbox, "Invoice 114"), &folder("Archive"))
        .await
        .unwrap();
    assert_eq!(moved, None, "without UIDPLUS the server does not say");
    backend
        .apply(&Operation::Delete {
            id: id_of(&inbox, "Message 1"),
        })
        .await
        .unwrap();
    let commands = relay.commands();
    assert!(!sent(&commands, "UID MOVE"), "{commands:?}");
    assert!(!sent(&commands, "UID EXPUNGE"), "{commands:?}");
    assert!(sent(&commands, "UID SEARCH DELETED"), "{commands:?}");
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.as_str() == "EXPUNGE")
            .count(),
        2,
        "{commands:?}"
    );
    assert_eq!(
        subjects(&listing(&backend, "Archive").await),
        ["Invoice 114"]
    );
    let left = flags_by_uid(&mut other_client, "INBOX").await;
    assert_eq!(left.keys().copied().collect::<Vec<_>>(), [2, 3]);
    // A saved message is saved, and what it is called is not known.
    assert_eq!(
        backend
            .append(&folder("Sent"), invoice().as_bytes(), Flags::default())
            .await
            .unwrap(),
        None
    );
    assert_eq!(listing(&backend, "Sent").await.len(), 1);

    // Another mail program marks a message. Expunging now would take it
    // too, so both operations are refused and change nothing.
    mark_deleted(&mut other_client, "INBOX", "3").await;
    let before = flags_by_uid(&mut other_client, "INBOX").await;
    relay.clear();
    let two = id_of(&inbox, "Message 2");
    match backend.move_message(&two, &folder("Archive")).await {
        Err(Error::Backend(text)) => assert!(text.contains("marked for deletion"), "{text}"),
        other => panic!("{other:?}"),
    }
    match backend.apply(&Operation::Delete { id: two.clone() }).await {
        Err(Error::Backend(text)) => assert!(text.contains("marked for deletion"), "{text}"),
        other => panic!("{other:?}"),
    }
    let commands = relay.commands();
    for never in ["EXPUNGE", "UID COPY", "UID STORE", "UID EXPUNGE"] {
        assert!(!sent(&commands, never), "{never} in {commands:?}");
    }
    assert_eq!(flags_by_uid(&mut other_client, "INBOX").await, before);
    assert_eq!(listing(&backend, "Archive").await.len(), 1);

    // Once the other program has expunged its own, the replay goes through.
    other_client.select("INBOX").await.unwrap();
    let _: Vec<_> = other_client
        .expunge()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(
        backend
            .move_message(&two, &folder("Archive"))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        subjects(&listing(&backend, "Archive").await),
        ["Invoice 114", "Message 2"]
    );
    assert!(listing(&backend, "INBOX").await.is_empty());
}

#[tokio::test]
async fn a_folder_is_made_on_the_server_listed_and_ready_to_take_mail() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "folders@example.com";
    let _other_client = arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let before = backend.list_folders().await.unwrap();
    assert!(before.iter().all(|folder| folder.name != "Receipts"));

    let made = backend.create_folder("Receipts").await.unwrap();
    assert_eq!(made.name, "Receipts");
    assert_eq!(made.role, quill_core::types::FolderRole::Other);
    // It is among the server's folders from now on, under the same name.
    let after = backend.list_folders().await.unwrap();
    assert_eq!(after.len(), before.len() + 1);
    assert!(after.contains(&made), "{after:?}");

    // And mail can be moved into it at once.
    let invoice = id_of(&listing(&backend, "INBOX").await, "Invoice 114");
    backend.move_message(&invoice, &made.id).await.unwrap();
    assert_eq!(
        subjects(&listing(&backend, &made.id.0).await),
        ["Invoice 114"]
    );

    // A name that is not plain ASCII is written the way a server reads
    // one, and read back as it was typed.
    let named = backend.create_folder("Reçus & Co").await.unwrap();
    assert_eq!(named.name, "Reçus & Co");
    assert_eq!(named.id.0, "Re&AOc-us &- Co");
    assert!(backend.list_folders().await.unwrap().contains(&named));

    // One that is there already is said, and not made twice.
    let again = backend.create_folder("receipts").await.unwrap_err();
    assert!(
        again
            .to_string()
            .contains("already a folder called receipts"),
        "{again}"
    );
    assert_eq!(
        backend.list_folders().await.unwrap().len(),
        before.len() + 2
    );
}

fn names(folders: &[quill_core::types::RemoteFolder]) -> Vec<&str> {
    folders.iter().map(|folder| folder.name.as_str()).collect()
}

#[tokio::test]
async fn a_folder_is_made_inside_another() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "inside@example.com";
    let _other_client = arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);

    // The folder it is inside need not be there first.
    let made = backend.create_folder("Projects/2026").await.unwrap();
    assert_eq!(made.name, "Projects/2026");
    let listed = backend.list_folders().await.unwrap();
    assert!(names(&listed).contains(&"Projects/2026"), "{listed:?}");

    // And one more beside it, inside the same.
    let beside = backend.create_folder("Projects / Reçus").await.unwrap();
    assert_eq!(beside.name, "Projects/Reçus");
    let invoice = id_of(&listing(&backend, "INBOX").await, "Invoice 114");
    backend.move_message(&invoice, &beside.id).await.unwrap();
    assert_eq!(
        subjects(&listing(&backend, &beside.id.0).await),
        ["Invoice 114"]
    );

    // A name with nothing on one side of its slash is refused before the
    // server is asked.
    let refused = backend.create_folder("Projects/").await.unwrap_err();
    assert!(refused.to_string().contains("each side"), "{refused}");
}

#[tokio::test]
async fn a_folder_is_renamed_with_what_is_inside_it_and_its_mail_is_found_by_the_new_name() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    let user = "rename@example.com";
    let _other_client = arrange(&server, user).await;
    let backend = server.backend(TlsMode::Implicit, user);
    let projects = backend.create_folder("Projects").await.unwrap();
    let year = backend.create_folder("Projects/2026").await.unwrap();
    let invoice = id_of(&listing(&backend, "INBOX").await, "Invoice 114");
    let moved = backend
        .move_message(&invoice, &year.id)
        .await
        .unwrap()
        .expect("the server says where it went");

    let renamed = backend.rename_folder(&projects.id, "Work").await.unwrap();
    let mut pairs: Vec<(&str, &str)> = renamed
        .iter()
        .map(|renamed| (renamed.from.0.as_str(), renamed.to.name.as_str()))
        .collect();
    pairs.sort_unstable();
    assert_eq!(
        pairs,
        [("Projects", "Work"), ("Projects/2026", "Work/2026")]
    );
    let listed = backend.list_folders().await.unwrap();
    let listed = names(&listed);
    assert!(listed.contains(&"Work") && listed.contains(&"Work/2026"));
    assert!(!listed.iter().any(|name| name.starts_with("Projects")));

    // The message is the one it was, under its folder's new name.
    let now = renamed
        .iter()
        .find(|renamed| renamed.from == year.id)
        .unwrap();
    let there = backend.message_in(&moved, &now.to.id).unwrap();
    let raw = backend.fetch_raw(&there).await.unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("Invoice 114"));
    assert_eq!(
        id_of(&listing(&backend, &now.to.id.0).await, "Invoice 114"),
        there
    );

    // A name that is taken is refused, and nothing is renamed.
    let taken = backend
        .rename_folder(&now.to.id, "archive")
        .await
        .unwrap_err();
    assert!(taken.to_string().contains("already a folder"), "{taken}");
    // The same name again changes nothing, and says so by saying nothing.
    let same = backend.rename_folder(&now.to.id, "Work/2026").await;
    assert_eq!(same.unwrap(), []);
    // The inbox keeps its name, and a folder that is not there has none
    // to change.
    assert!(backend.rename_folder(&folder("INBOX"), "In").await.is_err());
    assert!(matches!(
        backend.rename_folder(&folder("Nowhere"), "Somewhere").await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn a_folder_is_deleted_only_once_its_mail_is_in_the_trash() {
    let Some(server) = Dovecot::start().await else {
        return;
    };
    for (user, hidden) in [
        ("delete@example.com", &[][..]),
        ("delete-plainly@example.com", &["MOVE", "UIDPLUS"][..]),
    ] {
        arrange(&server, user).await;
        let (backend, relay) = lesser(&server, user, hidden).await;
        let receipts = backend.create_folder("Receipts").await.unwrap();
        let inside = backend.create_folder("Receipts/Old").await.unwrap();
        for subject in ["Invoice 114", "Message 1"] {
            let id = id_of(&listing(&backend, "INBOX").await, subject);
            backend.move_message(&id, &receipts.id).await.unwrap();
        }
        let trash = folder("Trash");

        // Not with a folder inside it.
        let refused = backend
            .delete_folder(&receipts.id, Some(&trash))
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("folders inside"), "{refused}");
        // An empty one goes, and there was no mail to speak of.
        assert_eq!(
            backend
                .delete_folder(&inside.id, Some(&trash))
                .await
                .unwrap(),
            quill_core::types::FolderMail::None
        );

        // Not with mail in it and nowhere for the mail to go.
        let refused = backend.delete_folder(&receipts.id, None).await.unwrap_err();
        assert!(refused.to_string().contains("no trash folder"), "{refused}");
        assert_eq!(listing(&backend, &receipts.id.0).await.len(), 2);

        relay.clear();
        assert_eq!(
            backend
                .delete_folder(&receipts.id, Some(&trash))
                .await
                .unwrap(),
            quill_core::types::FolderMail::Trashed
        );
        let commands = relay.commands();
        assert_eq!(
            sent(&commands, "UID MOVE 1:* \"Trash\""),
            hidden.is_empty(),
            "{commands:?}"
        );
        let listed = backend.list_folders().await.unwrap();
        assert!(!names(&listed).contains(&"Receipts"), "{listed:?}");
        let mut trashed = listing(&backend, "Trash").await;
        trashed.sort_by(|a, b| a.headers.subject.cmp(&b.headers.subject));
        assert_eq!(subjects(&trashed), ["Invoice 114", "Message 1"]);
        // What was not in the folder is where it was.
        assert_eq!(listing(&backend, "INBOX").await.len(), 2);

        // Asked again, it is gone already, which is what was wanted.
        assert_eq!(
            backend
                .delete_folder(&receipts.id, Some(&trash))
                .await
                .unwrap(),
            quill_core::types::FolderMail::None
        );
        // And the inbox is never deleted.
        assert!(
            backend
                .delete_folder(&folder("INBOX"), Some(&trash))
                .await
                .is_err()
        );
    }
}
