//! One message at a time: its raw bytes, its flags, moving, copying,
//! deleting, and saving a draft.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::types::{Flags, FolderId, Operation, RemoteId};
use quill_core::{Backend, Error};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wiremock::ResponseTemplate;

use crate::common::{Rig, header, large_message, message, ok, refusal};

fn id(text: &str) -> RemoteId {
    RemoteId(text.to_owned())
}

fn folder(text: &str) -> FolderId {
    FolderId(text.to_owned())
}

fn not_found() -> ResponseTemplate {
    refusal(
        404,
        "ErrorItemNotFound",
        "The specified object was not found in the store.",
    )
}

/// What Graph says a message's read mark and flag are.
fn marks(read: bool, flag: &str) -> ResponseTemplate {
    ok(json!({
        "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users('robin%40example.com')/messages(isRead,flag)/$entity",
        "@odata.etag": "W/\"CQAAABYAAAB1\"",
        "id": "m1",
        "isRead": read,
        "flag": {"flagStatus": flag}
    }))
}

const READ: Flags = Flags {
    seen: true,
    flagged: false,
    answered: false,
    draft: false,
};

#[tokio::test]
async fn a_large_message_is_downloaded_byte_for_byte() {
    let rig = Rig::start().await;
    let raw = large_message();
    assert!(raw.len() > 2 * 1024 * 1024);
    rig.on("GET", "/me/messages/m1/$value")
        .respond_with(ResponseTemplate::new(200).set_body_raw(raw.clone(), "text/plain"))
        .mount(&rig.server)
        .await;

    let fetched = rig.graph.fetch_raw(&id("m1")).await.unwrap();
    assert!(fetched == raw, "the message changed on the way");
    assert_eq!(rig.check().await, ["GET /v1.0/me/messages/m1/$value"]);
}

#[tokio::test]
async fn an_id_with_awkward_characters_is_one_part_of_the_path() {
    let rig = Rig::start().await;
    rig.on("GET", "/me/messages/AAMk%2Fb+c=%3Fd/$value")
        .respond_with(ResponseTemplate::new(200).set_body_raw("Subject: x\r\n\r\n", "text/plain"))
        .mount(&rig.server)
        .await;

    let fetched = rig.graph.fetch_raw(&id("AAMk/b+c=?d")).await.unwrap();
    assert_eq!(&fetched[..], b"Subject: x\r\n\r\n");
    rig.check().await;
}

#[tokio::test]
async fn a_message_that_is_gone_cannot_be_downloaded() {
    let rig = Rig::start().await;
    rig.on("GET", "/me/messages/m1/$value")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;

    // The sync engine takes this to mean the message has left the server.
    let err = rig.graph.fetch_raw(&id("m1")).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    rig.check().await;
}

#[tokio::test]
async fn marking_read_writes_the_read_mark_and_nothing_else() {
    let rig = Rig::start().await;
    rig.on("GET", "/me/messages/m1")
        .respond_with(marks(false, "notFlagged"))
        .mount(&rig.server)
        .await;
    rig.on("PATCH", "/me/messages/m1")
        .respond_with(marks(true, "notFlagged"))
        .mount(&rig.server)
        .await;

    rig.graph
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap();

    let requests = rig.requests().await;
    assert_eq!(
        rig.check().await,
        [
            "GET /v1.0/me/messages/m1?$select=isRead,flag",
            "PATCH /v1.0/me/messages/m1",
        ]
    );
    assert_eq!(header(&requests[1], "content-type"), "application/json");
    assert_eq!(
        requests[1].body_json::<Value>().unwrap(),
        json!({"isRead": true})
    );
}

#[tokio::test]
async fn flagging_and_unflagging_write_the_flag() {
    for (now, wanted, body) in [
        (
            marks(true, "notFlagged"),
            Flags {
                flagged: true,
                ..READ
            },
            json!({"flag": {"flagStatus": "flagged"}}),
        ),
        (
            marks(true, "flagged"),
            Flags::default(),
            json!({"isRead": false, "flag": {"flagStatus": "notFlagged"}}),
        ),
        // Answered and draft are not Graph's to set: only the rest goes.
        (
            marks(false, "notFlagged"),
            Flags {
                seen: true,
                flagged: true,
                answered: true,
                draft: true,
            },
            json!({"isRead": true, "flag": {"flagStatus": "flagged"}}),
        ),
    ] {
        let rig = Rig::start().await;
        rig.on("GET", "/me/messages/m1")
            .respond_with(now)
            .mount(&rig.server)
            .await;
        rig.on("PATCH", "/me/messages/m1")
            .respond_with(ok(json!({"id": "m1"})))
            .mount(&rig.server)
            .await;

        rig.graph
            .apply(&Operation::SetFlags {
                id: id("m1"),
                flags: wanted,
            })
            .await
            .unwrap();

        let requests = rig.requests().await;
        assert_eq!(rig.check().await.len(), 2);
        assert_eq!(requests[1].body_json::<Value>().unwrap(), body);
    }
}

#[tokio::test]
async fn flags_that_are_already_right_are_not_written() {
    for (now, wanted) in [
        (marks(true, "notFlagged"), READ),
        // Marked complete in Outlook: not flagged here, and left as it is.
        (marks(true, "complete"), READ),
        (
            marks(true, "notFlagged"),
            Flags {
                answered: true,
                ..READ
            },
        ),
    ] {
        let rig = Rig::start().await;
        rig.on("GET", "/me/messages/m1")
            .respond_with(now)
            .mount(&rig.server)
            .await;

        rig.graph
            .apply(&Operation::SetFlags {
                id: id("m1"),
                flags: wanted,
            })
            .await
            .unwrap();
        assert_eq!(
            rig.check().await,
            ["GET /v1.0/me/messages/m1?$select=isRead,flag"]
        );
    }
}

#[tokio::test]
async fn flags_of_a_message_that_is_gone_are_done_with() {
    // Gone before it is looked at.
    let rig = Rig::start().await;
    rig.on("GET", "/me/messages/m1")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;
    rig.graph
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap();
    assert_eq!(rig.check().await.len(), 1);

    // Gone between the look and the change.
    let rig = Rig::start().await;
    rig.on("GET", "/me/messages/m1")
        .respond_with(marks(false, "notFlagged"))
        .mount(&rig.server)
        .await;
    rig.on("PATCH", "/me/messages/m1")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;
    rig.graph
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap();
    assert_eq!(rig.check().await.len(), 2);
}

#[tokio::test]
async fn a_refused_flag_change_is_an_error() {
    let rig = Rig::start().await;
    rig.on("GET", "/me/messages/m1")
        .respond_with(marks(false, "notFlagged"))
        .mount(&rig.server)
        .await;
    rig.on("PATCH", "/me/messages/m1")
        .respond_with(refusal(
            400,
            "ErrorInvalidRequest",
            "The request is invalid.",
        ))
        .mount(&rig.server)
        .await;

    let err = rig
        .graph
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("ErrorInvalidRequest"), "{err}");
    rig.check().await;
}

#[tokio::test]
async fn a_moved_message_keeps_its_id() {
    let rig = Rig::start().await;
    let mut moved = message(
        "m1",
        "Minutes, 30 October",
        ("Ana Ruiz", "ana.ruiz@example.com"),
        "2026-10-30T16:40:00Z",
        true,
    );
    moved["parentFolderId"] = json!("folder-archive");
    rig.on("POST", "/me/messages/m1/move")
        .respond_with(ResponseTemplate::new(201).set_body_json(moved))
        .mount(&rig.server)
        .await;

    let new_id = rig
        .graph
        .move_message(&id("m1"), &folder("folder-archive"))
        .await
        .unwrap();
    assert_eq!(new_id, Some(id("m1")));

    let requests = rig.requests().await;
    assert_eq!(rig.check().await, ["POST /v1.0/me/messages/m1/move"]);
    assert_eq!(header(&requests[0], "content-type"), "application/json");
    assert_eq!(
        requests[0].body_json::<Value>().unwrap(),
        json!({"destinationId": "folder-archive"})
    );
}

#[tokio::test]
async fn a_move_through_apply_is_the_same_move() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/messages/m1/move")
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "m1"})))
        .mount(&rig.server)
        .await;

    rig.graph
        .apply(&Operation::Move {
            id: id("m1"),
            to: folder("folder-trash"),
        })
        .await
        .unwrap();
    let requests = rig.requests().await;
    assert_eq!(rig.check().await, ["POST /v1.0/me/messages/m1/move"]);
    assert_eq!(
        requests[0].body_json::<Value>().unwrap(),
        json!({"destinationId": "folder-trash"})
    );
}

#[tokio::test]
async fn moving_a_message_that_is_gone_is_done_already() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/messages/m1/move")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/messages/m1")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;

    // A replay after a crash must not fail because the first try worked
    // and the message has since been dealt with.
    let new_id = rig
        .graph
        .move_message(&id("m1"), &folder("folder-archive"))
        .await
        .unwrap();
    assert_eq!(new_id, None);
    assert_eq!(
        rig.check().await,
        [
            "POST /v1.0/me/messages/m1/move",
            "GET /v1.0/me/messages/m1?$select=id",
        ]
    );
}

#[tokio::test]
async fn moving_to_a_folder_that_is_gone_is_an_error() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/messages/m1/move")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;
    // The message is still there, so it was the folder that was not found.
    rig.on("GET", "/me/messages/m1")
        .respond_with(ok(json!({"id": "m1"})))
        .mount(&rig.server)
        .await;

    let err = rig
        .graph
        .move_message(&id("m1"), &folder("folder-long-gone"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert_eq!(rig.check().await.len(), 2);
}

#[tokio::test]
async fn a_move_whose_answer_cannot_be_read_still_happened() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/messages/m1/move")
        .respond_with(ResponseTemplate::new(201).set_body_raw("{\"subject\":", "application/json"))
        .mount(&rig.server)
        .await;

    // Moved, under an id the next sync will tell.
    let new_id = rig
        .graph
        .move_message(&id("m1"), &folder("folder-archive"))
        .await
        .unwrap();
    assert_eq!(new_id, None);
    rig.check().await;
}

#[tokio::test]
async fn a_copy_names_where_it_goes() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/messages/m1/copy")
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "m1-copy"})))
        .mount(&rig.server)
        .await;

    rig.graph
        .apply(&Operation::Copy {
            id: id("m1"),
            to: folder("folder-receipts"),
        })
        .await
        .unwrap();

    let requests = rig.requests().await;
    assert_eq!(rig.check().await, ["POST /v1.0/me/messages/m1/copy"]);
    assert_eq!(
        requests[0].body_json::<Value>().unwrap(),
        json!({"destinationId": "folder-receipts"})
    );
}

#[tokio::test]
async fn copying_a_message_that_is_gone_is_an_error() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/messages/m1/copy")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;

    // Nothing says an earlier try made the copy.
    let err = rig
        .graph
        .apply(&Operation::Copy {
            id: id("m1"),
            to: folder("folder-receipts"),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    rig.check().await;
}

#[tokio::test]
async fn a_delete_is_a_delete_and_one_already_done_is_fine() {
    let rig = Rig::start().await;
    rig.on("DELETE", "/me/messages/m1")
        .respond_with(ResponseTemplate::new(204))
        .up_to_n_times(1)
        .mount(&rig.server)
        .await;
    rig.on("DELETE", "/me/messages/m1")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;

    let delete = Operation::Delete { id: id("m1") };
    rig.graph.apply(&delete).await.unwrap();
    // The replay after a crash.
    rig.graph.apply(&delete).await.unwrap();

    let requests = rig.requests().await;
    assert!(requests.iter().all(|request| request.body.is_empty()));
    assert_eq!(
        rig.check().await,
        ["DELETE /v1.0/me/messages/m1", "DELETE /v1.0/me/messages/m1"]
    );
}

#[tokio::test]
async fn a_delete_the_account_may_not_make_is_an_error() {
    let rig = Rig::start().await;
    rig.on("DELETE", "/me/messages/m1")
        .respond_with(refusal(
            403,
            "ErrorAccessDenied",
            "Access is denied. Check credentials and try again.",
        ))
        .mount(&rig.server)
        .await;

    let err = rig
        .graph
        .apply(&Operation::Delete { id: id("m1") })
        .await
        .unwrap_err();
    // Undone locally and shown, not waited on: see `Reply::error`.
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    rig.check().await;
}

const DRAFT: &[u8] = b"From: Robin Okafor <robin@example.com>\r\n\
    To: Ana Ruiz <ana.ruiz@example.com>\r\n\
    Subject: Re: Harbour survey\r\n\
    Message-ID: <draft.1@example.com>\r\n\
    \r\n\
    Not finished yet.\r\n";

#[tokio::test]
async fn a_draft_is_saved_into_its_folder_as_the_message_it_is() {
    let rig = Rig::start().await;
    let mut created = message(
        "draft-1",
        "Re: Harbour survey",
        ("Robin Okafor", "robin@example.com"),
        "2026-11-04T08:00:00Z",
        true,
    );
    created["isDraft"] = json!(true);
    rig.on("POST", "/me/mailFolders/folder-drafts/messages")
        .respond_with(ResponseTemplate::new(201).set_body_json(created))
        .mount(&rig.server)
        .await;

    let saved = rig
        .graph
        .append(
            &folder("folder-drafts"),
            DRAFT,
            Flags {
                seen: true,
                draft: true,
                ..Flags::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(saved, Some(id("draft-1")));

    let requests = rig.requests().await;
    assert_eq!(
        rig.check().await,
        ["POST /v1.0/me/mailFolders/folder-drafts/messages"]
    );
    // The whole message as base64 text, which is how Graph takes MIME.
    assert_eq!(header(&requests[0], "content-type"), "text/plain");
    assert_eq!(BASE64.decode(&requests[0].body).unwrap(), DRAFT);
}

#[tokio::test]
async fn a_saved_draft_gets_the_marks_it_was_saved_with() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/mailFolders/folder-drafts/messages")
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "draft-2",
            "isRead": false,
            "isDraft": true,
            "flag": {"flagStatus": "notFlagged"}
        })))
        .mount(&rig.server)
        .await;
    // The draft is saved even if its marks cannot be set: failing then
    // would have it saved a second time on the next try.
    rig.on("PATCH", "/me/messages/draft-2")
        .respond_with(refusal(
            500,
            "ErrorInternalServerError",
            "An internal server error occurred.",
        ))
        .mount(&rig.server)
        .await;

    let saved = rig
        .graph
        .append(
            &folder("folder-drafts"),
            DRAFT,
            Flags {
                seen: true,
                flagged: true,
                draft: true,
                answered: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(saved, Some(id("draft-2")));

    let requests = rig.requests().await;
    assert_eq!(
        rig.check().await,
        [
            "POST /v1.0/me/mailFolders/folder-drafts/messages",
            "PATCH /v1.0/me/messages/draft-2",
        ]
    );
    assert_eq!(
        requests[1].body_json::<Value>().unwrap(),
        json!({"isRead": true, "flag": {"flagStatus": "flagged"}})
    );
}

#[tokio::test]
async fn a_copy_of_sent_mail_is_not_saved_as_a_draft() {
    let rig = Rig::start().await;

    let err = rig
        .graph
        .append(&folder("folder-sent"), DRAFT, READ)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
    // Nothing was asked of the service.
    assert!(rig.check().await.is_empty());
}

#[tokio::test]
async fn a_draft_the_service_will_not_take_is_an_error() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/mailFolders/folder-gone/messages")
        .respond_with(not_found())
        .mount(&rig.server)
        .await;

    let err = rig
        .graph
        .append(
            &folder("folder-gone"),
            DRAFT,
            Flags {
                draft: true,
                ..Flags::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    rig.check().await;
}

#[tokio::test]
async fn there_is_nothing_to_watch_and_the_backend_says_what_it_can_do() {
    let rig = Rig::start().await;
    let (tx, mut rx) = mpsc::channel(1);

    // Returns at once: the sync engine polls.
    rig.graph.watch(tx).await.unwrap();
    assert!(rx.recv().await.is_none());

    let can = rig.graph.capabilities();
    assert!(can.folders && can.flags);
    assert!(!can.push && !can.server_search && !can.threads);
    assert!(rig.check().await.is_empty());
}
