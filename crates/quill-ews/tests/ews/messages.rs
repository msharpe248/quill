//! Messages by id: their raw bytes, their flags, moving, copying,
//! deleting, and saving one into a folder.

use quill_core::types::{Flags, FolderId, Operation, RemoteId};
use quill_core::{Backend, Error};
use tokio::sync::mpsc;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;

use crate::common::{
    Rig, answer, failure, gone, got, item_ids, large_message, mime, posted_message, success,
};

fn id(text: &str) -> RemoteId {
    RemoteId(text.to_owned())
}

fn folder(text: &str) -> FolderId {
    FolderId(text.to_owned())
}

/// What asking for whole messages looks like on the wire.
fn downloading(ids: &[&str]) -> String {
    format!(
        "<m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape>\
         <t:IncludeMimeContent>true</t:IncludeMimeContent></m:ItemShape>{}</m:GetItem>",
        item_ids(ids)
    )
}

/// What asking for a message's marks looks like on the wire.
fn reading_marks(id: &str) -> String {
    format!(
        "<m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>\
         <t:FieldURI FieldURI=\"item:Flag\"/><t:FieldURI FieldURI=\"message:IsRead\"/>\
         </t:AdditionalProperties></m:ItemShape>{}</m:GetItem>",
        item_ids(&[id])
    )
}

/// What changing a message's marks looks like on the wire.
fn writing_marks(id: &str, updates: &str) -> String {
    format!(
        "<m:UpdateItem MessageDisposition=\"SaveOnly\" ConflictResolution=\"AlwaysOverwrite\" \
         SuppressReadReceipts=\"true\"><m:ItemChanges><t:ItemChange><t:ItemId Id=\"{id}\"/>\
         <t:Updates>{updates}</t:Updates></t:ItemChange></m:ItemChanges></m:UpdateItem>"
    )
}

fn read_mark(read: bool) -> String {
    format!(
        "<t:SetItemField><t:FieldURI FieldURI=\"message:IsRead\"/>\
         <t:Message><t:IsRead>{read}</t:IsRead></t:Message></t:SetItemField>"
    )
}

fn flag_mark(status: &str) -> String {
    format!(
        "<t:SetItemField><t:FieldURI FieldURI=\"item:Flag\"/><t:Message><t:Flag>\
         <t:FlagStatus>{status}</t:FlagStatus></t:Flag></t:Message></t:SetItemField>"
    )
}

/// What moving or copying a message looks like on the wire.
fn filing(operation: &str, id: &str, to: &str) -> String {
    // A move asks what the message is called where it went.
    let new_ids = if operation == "MoveItem" {
        "<m:ReturnNewItemIds>true</m:ReturnNewItemIds>"
    } else {
        ""
    };
    format!(
        "<m:{operation}><m:ToFolderId><t:FolderId Id=\"{to}\"/></m:ToFolderId>{}{new_ids}\
         </m:{operation}>",
        item_ids(&[id])
    )
}

/// What deleting messages looks like on the wire.
fn deleting(ids: &[&str]) -> String {
    format!(
        "<m:DeleteItem DeleteType=\"HardDelete\" SendMeetingCancellations=\"SendToNone\" \
         AffectedTaskOccurrences=\"AllOccurrences\" SuppressReadReceipts=\"true\">{}\
         </m:DeleteItem>",
        item_ids(ids)
    )
}

/// What Exchange says a message's read mark and flag are.
fn marks(read: bool, flag: &str) -> ResponseTemplate {
    answer(
        "GetItem",
        &[got(&format!(
            "<t:Message><t:ItemId Id=\"m1\" ChangeKey=\"CQAAABYAAAB1\"/>\
             <t:Flag><t:FlagStatus>{flag}</t:FlagStatus></t:Flag>\
             <t:IsRead>{read}</t:IsRead></t:Message>"
        ))],
    )
}

/// The answer to an operation that makes or changes one message.
fn item(operation: &str, id: &str) -> ResponseTemplate {
    answer(
        operation,
        &[success(
            operation,
            &format!(
                "<m:Items><t:Message><t:ItemId Id=\"{id}\" ChangeKey=\"CQAAABYAAAB2\"/>\
                 </t:Message></m:Items>"
            ),
        )],
    )
}

const READ: Flags = Flags {
    seen: true,
    flagged: false,
    answered: false,
    draft: false,
};

const MESSAGE: &[u8] = b"From: Ana Ruiz <ana.ruiz@example.com>\r\n\
    To: Robin Okafor <robin@example.com>\r\n\
    Subject: Minutes, 30 October\r\n\
    Message-ID: <minutes.30@example.com>\r\n\
    \r\n\
    Attached are the minutes.\r\n";

#[tokio::test]
async fn a_large_message_is_downloaded_byte_for_byte() {
    let rig = Rig::start().await;
    let raw = large_message();
    assert!(raw.len() > 2 * 1024 * 1024);
    rig.answers("GetItem", answer("GetItem", &[mime("m1", &raw)]))
        .await;

    let fetched = rig.ews.fetch_raw(&id("m1")).await.unwrap();
    assert!(fetched == raw, "the message changed on the way");
    assert_eq!(rig.check().await, [downloading(&["m1"])]);
}

#[tokio::test]
async fn an_id_with_awkward_characters_stays_one_id() {
    let rig = Rig::start().await;
    rig.answers(
        "GetItem",
        answer("GetItem", &[mime("x", b"Subject: x\r\n\r\n")]),
    )
    .await;

    let awkward = "AAMk/b+c=\"/><t:ItemId Id=\"other\"/>&amp;<";
    let fetched = rig.ews.fetch_raw(&id(awkward)).await.unwrap();
    assert_eq!(&fetched[..], b"Subject: x\r\n\r\n");
    assert_eq!(
        rig.check().await,
        ["<m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape>\
             <t:IncludeMimeContent>true</t:IncludeMimeContent></m:ItemShape><m:ItemIds>\
             <t:ItemId Id=\"AAMk/b+c=&quot;/&gt;&lt;t:ItemId Id=&quot;other&quot;/&gt;&amp;amp;&lt;\"/>\
             </m:ItemIds></m:GetItem>"]
    );
}

#[tokio::test]
async fn a_message_that_is_gone_cannot_be_downloaded() {
    let rig = Rig::start().await;
    rig.answers("GetItem", answer("GetItem", &[gone("GetItem")]))
        .await;

    // The sync engine takes this to mean the message has left the server.
    let err = rig.ews.fetch_raw(&id("m1")).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert!(err.to_string().contains("ErrorItemNotFound"), "{err}");
    rig.check().await;
}

#[tokio::test]
async fn an_answer_without_the_message_in_it_is_a_backend_error() {
    for inside in [
        // No content, content that is not base64, and no item at all.
        "<m:Items><t:Message><t:ItemId Id=\"m1\"/></t:Message></m:Items>",
        "<m:Items><t:Message><t:MimeContent>not base64!</t:MimeContent></t:Message></m:Items>",
        "<m:Items/>",
        "",
    ] {
        let rig = Rig::start().await;
        rig.answers("GetItem", answer("GetItem", &[success("GetItem", inside)]))
            .await;

        let err = rig.ews.fetch_raw(&id("m1")).await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{inside}: {err:?}");
        rig.check().await;
    }
}

#[tokio::test]
async fn several_messages_are_downloaded_ten_to_a_request_in_the_order_asked() {
    let rig = Rig::start().await;
    let names: Vec<String> = (0..23).map(|i| format!("m{i:02}")).collect();
    let body = |name: &str| format!("Subject: {name}\r\n\r\nThe body of {name}.\r\n").into_bytes();
    for batch in names.chunks(10) {
        let messages: Vec<String> = batch
            .iter()
            // One has left the server since it was listed.
            .map(|name| match name.as_str() {
                "m07" | "m22" => gone("GetItem"),
                name => mime(name, &body(name)),
            })
            .collect();
        rig.on("GetItem")
            .and(body_string_contains(format!(
                "<m:ItemIds><t:ItemId Id=\"{}\"/>",
                batch[0]
            )))
            .respond_with(answer("GetItem", &messages))
            .mount(&rig.server)
            .await;
    }

    let ids: Vec<RemoteId> = names.iter().map(|name| id(name)).collect();
    let fetched = rig.ews.fetch_many(&ids).await.unwrap();

    assert_eq!(fetched.len(), 23);
    for (name, fetched) in names.iter().zip(&fetched) {
        match name.as_str() {
            "m07" | "m22" => assert!(fetched.is_none(), "{name}"),
            name => assert_eq!(fetched.as_deref(), Some(&body(name)[..]), "{name}"),
        }
    }
    let asked = |range: std::ops::Range<usize>| {
        downloading(&names[range].iter().map(String::as_str).collect::<Vec<_>>())
    };
    assert_eq!(
        rig.check().await,
        [asked(0..10), asked(10..20), asked(20..23)]
    );
    assert!(rig.ews.fetch_many(&[]).await.unwrap().is_empty());
}

#[tokio::test]
async fn messages_too_large_to_come_together_are_asked_for_one_by_one() {
    let rig = Rig::start().await;
    let raw = large_message();
    rig.on("GetItem")
        .and(body_string_contains(
            "<t:ItemId Id=\"m1\"/><t:ItemId Id=\"m2\"/>",
        ))
        .respond_with(answer("GetItem", &[mime("m1", &raw), mime("m2", &raw)]))
        .mount(&rig.server)
        .await;
    rig.on("GetItem")
        .and(body_string_contains(
            "<m:ItemIds><t:ItemId Id=\"m1\"/></m:ItemIds>",
        ))
        .respond_with(answer("GetItem", &[mime("m1", &raw)]))
        .mount(&rig.server)
        .await;
    rig.on("GetItem")
        .and(body_string_contains(
            "<m:ItemIds><t:ItemId Id=\"m2\"/></m:ItemIds>",
        ))
        .respond_with(answer("GetItem", &[gone("GetItem")]))
        .mount(&rig.server)
        .await;
    // Room for one such message as base64, and not for two.
    let cramped = rig.backend().with_answer_limit(4 * 1024 * 1024);

    let fetched = cramped.fetch_many(&[id("m1"), id("m2")]).await.unwrap();
    assert!(fetched[0].as_deref() == Some(&raw[..]));
    assert!(fetched[1].is_none());
    assert_eq!(
        rig.check().await,
        [
            downloading(&["m1", "m2"]),
            downloading(&["m1"]),
            downloading(&["m2"]),
        ]
    );
}

#[tokio::test]
async fn a_message_larger_than_is_read_is_an_error_and_not_a_full_memory() {
    let rig = Rig::start().await;
    rig.answers(
        "GetItem",
        answer("GetItem", &[mime("m1", &large_message())]),
    )
    .await;
    let cramped = rig.backend().with_answer_limit(64 * 1024);

    let err = cramped.fetch_raw(&id("m1")).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(
        err.to_string().contains("larger than the 65536 bytes"),
        "{err}"
    );
    let err = cramped.fetch_many(&[id("m1")]).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert_eq!(rig.check().await.len(), 2);
}

#[tokio::test]
async fn marking_read_writes_the_read_mark_and_nothing_else() {
    let rig = Rig::start().await;
    rig.answers("GetItem", marks(false, "NotFlagged")).await;
    rig.answers("UpdateItem", item("UpdateItem", "m1")).await;

    rig.ews
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap();

    // No draft mark, no answered mark, and the flag left alone.
    assert_eq!(
        rig.check().await,
        [reading_marks("m1"), writing_marks("m1", &read_mark(true))]
    );
}

#[tokio::test]
async fn flagging_and_marking_unread_write_what_differs() {
    for (now, wanted, updates) in [
        (
            marks(true, "NotFlagged"),
            Flags {
                seen: true,
                flagged: true,
                ..Flags::default()
            },
            flag_mark("Flagged"),
        ),
        (
            marks(true, "Flagged"),
            Flags::default(),
            format!("{}{}", read_mark(false), flag_mark("NotFlagged")),
        ),
        (
            marks(true, "Flagged"),
            Flags {
                flagged: true,
                // Not Quill's to write, and not written.
                answered: true,
                draft: true,
                ..Flags::default()
            },
            read_mark(false),
        ),
    ] {
        let rig = Rig::start().await;
        rig.answers("GetItem", now).await;
        rig.answers("UpdateItem", item("UpdateItem", "m1")).await;

        rig.ews
            .apply(&Operation::SetFlags {
                id: id("m1"),
                flags: wanted,
            })
            .await
            .unwrap();

        assert_eq!(
            rig.check().await,
            [reading_marks("m1"), writing_marks("m1", &updates)]
        );
    }
}

#[tokio::test]
async fn marks_that_are_already_right_are_not_written() {
    for (now, wanted) in [
        (marks(true, "NotFlagged"), READ),
        // A flag marked complete in Outlook is not flagged, and stays
        // complete.
        (marks(true, "Complete"), READ),
        (
            marks(false, "Flagged"),
            Flags {
                flagged: true,
                ..Flags::default()
            },
        ),
    ] {
        let rig = Rig::start().await;
        rig.answers("GetItem", now).await;

        rig.ews
            .apply(&Operation::SetFlags {
                id: id("m1"),
                flags: wanted,
            })
            .await
            .unwrap();
        assert_eq!(rig.check().await, [reading_marks("m1")]);
    }
}

#[tokio::test]
async fn marking_a_message_that_is_gone_is_not_an_error() {
    // Gone before it was looked at, and gone between the look and the
    // change.
    let rig = Rig::start().await;
    rig.answers("GetItem", answer("GetItem", &[gone("GetItem")]))
        .await;
    rig.ews
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap();
    assert_eq!(rig.check().await.len(), 1);

    let rig = Rig::start().await;
    rig.answers("GetItem", marks(false, "NotFlagged")).await;
    rig.answers("UpdateItem", answer("UpdateItem", &[gone("UpdateItem")]))
        .await;
    rig.ews
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap();
    assert_eq!(rig.check().await.len(), 2);
}

#[tokio::test]
async fn marks_that_may_not_be_changed_are_an_error() {
    let rig = Rig::start().await;
    rig.answers("GetItem", marks(false, "NotFlagged")).await;
    rig.answers(
        "UpdateItem",
        answer(
            "UpdateItem",
            &[failure(
                "UpdateItem",
                "ErrorAccessDenied",
                "Access is denied. Check credentials and try again.",
            )],
        ),
    )
    .await;

    let err = rig
        .ews
        .apply(&Operation::SetFlags {
            id: id("m1"),
            flags: READ,
        })
        .await
        .unwrap_err();
    // Not a failed sign-in: the rest of the account's queue goes on.
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("ErrorAccessDenied"), "{err}");
    rig.check().await;
}

#[tokio::test]
async fn a_moved_message_has_a_new_id_which_the_answer_says() {
    let rig = Rig::start().await;
    rig.answers("MoveItem", item("MoveItem", "m1-in-archive"))
        .await;

    let moved = rig
        .ews
        .move_message(&id("m1"), &folder("folder-archive"))
        .await
        .unwrap();
    assert_eq!(moved, Some(id("m1-in-archive")));
    // And through `apply`, which has nowhere to put the id.
    rig.ews
        .apply(&Operation::Move {
            id: id("m1"),
            to: folder("folder-archive"),
        })
        .await
        .unwrap();
    let asked = filing("MoveItem", "m1", "folder-archive");
    assert_eq!(rig.check().await, [asked.clone(), asked]);
}

#[tokio::test]
async fn moving_a_message_that_is_gone_is_not_an_error_and_a_missing_folder_is() {
    let rig = Rig::start().await;
    rig.answers("MoveItem", answer("MoveItem", &[gone("MoveItem")]))
        .await;
    // A replay after a crash: the first attempt had already worked.
    let moved = rig
        .ews
        .move_message(&id("m1"), &folder("folder-archive"))
        .await
        .unwrap();
    assert_eq!(moved, None);
    rig.check().await;

    for code in ["ErrorFolderNotFound", "ErrorToFolderNotFound"] {
        let rig = Rig::start().await;
        rig.answers(
            "MoveItem",
            answer(
                "MoveItem",
                &[failure(
                    "MoveItem",
                    code,
                    "The specified folder could not be found in the store.",
                )],
            ),
        )
        .await;
        let err = rig
            .ews
            .move_message(&id("m1"), &folder("folder-gone"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
        rig.check().await;
    }
}

#[tokio::test]
async fn a_move_whose_answer_names_no_message_has_still_moved_it() {
    for inside in ["<m:Items/>", ""] {
        let rig = Rig::start().await;
        rig.answers(
            "MoveItem",
            answer("MoveItem", &[success("MoveItem", inside)]),
        )
        .await;
        // The next sync of the folder finds it.
        let moved = rig
            .ews
            .move_message(&id("m1"), &folder("folder-archive"))
            .await
            .unwrap();
        assert_eq!(moved, None);
        rig.check().await;
    }
}

#[tokio::test]
async fn a_message_is_copied_and_one_that_is_gone_cannot_be() {
    let rig = Rig::start().await;
    rig.answers("CopyItem", item("CopyItem", "m1-copy")).await;
    rig.ews
        .apply(&Operation::Copy {
            id: id("m1"),
            to: folder("folder-projects"),
        })
        .await
        .unwrap();
    assert_eq!(
        rig.check().await,
        [filing("CopyItem", "m1", "folder-projects")]
    );

    let rig = Rig::start().await;
    rig.answers("CopyItem", answer("CopyItem", &[gone("CopyItem")]))
        .await;
    // Nothing says an earlier attempt copied it.
    let err = rig
        .ews
        .apply(&Operation::Copy {
            id: id("m1"),
            to: folder("folder-projects"),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    rig.check().await;
}

#[tokio::test]
async fn a_message_is_deleted_for_good_and_one_already_gone_is_no_error() {
    let rig = Rig::start().await;
    rig.answers(
        "DeleteItem",
        answer("DeleteItem", &[success("DeleteItem", "")]),
    )
    .await;
    rig.ews
        .apply(&Operation::Delete { id: id("m1") })
        .await
        .unwrap();
    assert_eq!(rig.check().await, [deleting(&["m1"])]);

    let rig = Rig::start().await;
    rig.answers("DeleteItem", answer("DeleteItem", &[gone("DeleteItem")]))
        .await;
    rig.ews
        .apply(&Operation::Delete { id: id("m1") })
        .await
        .unwrap();
    rig.check().await;
}

#[tokio::test]
async fn many_messages_are_deleted_a_hundred_to_a_request() {
    let rig = Rig::start().await;
    let names: Vec<String> = (0..230).map(|i| format!("m{i:03}")).collect();
    for batch in names.chunks(100) {
        let messages: Vec<String> = batch
            .iter()
            .map(|name| match name.as_str() {
                // Already gone: deleted by another program meanwhile.
                "m042" | "m229" => gone("DeleteItem"),
                _ => success("DeleteItem", ""),
            })
            .collect();
        rig.on("DeleteItem")
            .and(body_string_contains(format!(
                "<m:ItemIds><t:ItemId Id=\"{}\"/>",
                batch[0]
            )))
            .respond_with(answer("DeleteItem", &messages))
            .mount(&rig.server)
            .await;
    }

    let ids: Vec<RemoteId> = names.iter().map(|name| id(name)).collect();
    rig.ews.delete_many(&ids).await.unwrap();
    rig.ews.delete_many(&[]).await.unwrap();

    let asked = |range: std::ops::Range<usize>| {
        deleting(&names[range].iter().map(String::as_str).collect::<Vec<_>>())
    };
    assert_eq!(
        rig.check().await,
        [asked(0..100), asked(100..200), asked(200..230)]
    );
}

#[tokio::test]
async fn a_message_that_may_not_be_deleted_fails_the_deleting() {
    for messages in [
        vec![
            success("DeleteItem", ""),
            failure(
                "DeleteItem",
                "ErrorAccessDenied",
                "Access is denied. Check credentials and try again.",
            ),
        ],
        // One answer to two questions: which was deleted?
        vec![success("DeleteItem", "")],
    ] {
        let rig = Rig::start().await;
        rig.answers("DeleteItem", answer("DeleteItem", &messages))
            .await;
        let err = rig
            .ews
            .delete_many(&[id("m1"), id("m2")])
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{err:?}");
        rig.check().await;
    }
}

#[tokio::test]
async fn a_sent_copy_is_saved_as_a_message_and_not_as_a_draft() {
    let rig = Rig::start().await;
    rig.answers("CreateItem", item("CreateItem", "saved-1"))
        .await;

    let saved = rig
        .ews
        .append(&folder("folder-sent"), MESSAGE, READ)
        .await
        .unwrap();
    assert_eq!(saved, Some(id("saved-1")));

    let requests = rig.requests().await;
    assert!(posted_message(&requests[0]) == MESSAGE);
    let asked = rig.check().await;
    assert!(
        asked[0].starts_with(
            "<m:CreateItem MessageDisposition=\"SaveOnly\"><m:SavedItemFolderId>\
             <t:FolderId Id=\"folder-sent\"/></m:SavedItemFolderId><m:Items><t:Message>\
             <t:MimeContent CharacterSet=\"UTF-8\">"
        ),
        "{}",
        asked[0]
    );
    // The message flags say "read, and not unsent" while it is made:
    // left to itself Exchange would file it as a draft for ever.
    assert!(
        asked[0].ends_with(
            "</t:MimeContent><t:ExtendedProperty>\
             <t:ExtendedFieldURI PropertyTag=\"0x0E07\" PropertyType=\"Integer\"/>\
             <t:Value>1</t:Value></t:ExtendedProperty><t:IsRead>true</t:IsRead>\
             </t:Message></m:Items></m:CreateItem>"
        ),
        "{}",
        asked[0]
    );
}

#[tokio::test]
async fn a_draft_is_saved_as_a_draft_and_marks_are_set_as_asked() {
    let rig = Rig::start().await;
    rig.answers("CreateItem", item("CreateItem", "saved-1"))
        .await;

    rig.ews
        .append(
            &folder("folder-drafts"),
            MESSAGE,
            Flags {
                seen: true,
                draft: true,
                ..Flags::default()
            },
        )
        .await
        .unwrap();
    // An unread, flagged message filed by a rule.
    rig.ews
        .append(
            &folder("folder-projects"),
            MESSAGE,
            Flags {
                flagged: true,
                answered: true,
                ..Flags::default()
            },
        )
        .await
        .unwrap();

    let asked = rig.check().await;
    assert!(
        asked[0].ends_with(
            "</t:MimeContent><t:IsRead>true</t:IsRead></t:Message></m:Items></m:CreateItem>"
        ),
        "{}",
        asked[0]
    );
    assert!(
        asked[1].ends_with(
            "</t:MimeContent><t:ExtendedProperty>\
             <t:ExtendedFieldURI PropertyTag=\"0x0E07\" PropertyType=\"Integer\"/>\
             <t:Value>0</t:Value></t:ExtendedProperty>\
             <t:Flag><t:FlagStatus>Flagged</t:FlagStatus></t:Flag>\
             <t:IsRead>false</t:IsRead></t:Message></m:Items></m:CreateItem>"
        ),
        "{}",
        asked[1]
    );
}

#[tokio::test]
async fn a_message_that_cannot_be_saved_is_an_error_and_an_answer_without_an_id_is_not() {
    let rig = Rig::start().await;
    rig.answers(
        "CreateItem",
        answer(
            "CreateItem",
            &[failure(
                "CreateItem",
                "ErrorQuotaExceeded",
                "The mailbox has exceeded its size limit.",
            )],
        ),
    )
    .await;
    let err = rig
        .ews
        .append(&folder("folder-sent"), MESSAGE, READ)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("ErrorQuotaExceeded"), "{err}");
    rig.check().await;

    let rig = Rig::start().await;
    rig.answers(
        "CreateItem",
        answer("CreateItem", &[success("CreateItem", "<m:Items/>")]),
    )
    .await;
    // Saved. Saying otherwise would have it saved a second time.
    let saved = rig
        .ews
        .append(&folder("folder-sent"), MESSAGE, READ)
        .await
        .unwrap();
    assert_eq!(saved, None);
    rig.check().await;
}

#[tokio::test]
async fn there_is_nothing_to_wait_on_and_the_backend_says_so() {
    let rig = Rig::start().await;
    let (tx, mut rx) = mpsc::channel(1);
    rig.ews.watch(tx).await.unwrap();
    assert!(rx.recv().await.is_none());

    let capabilities = rig.ews.capabilities();
    assert!(capabilities.folders && capabilities.flags);
    assert!(!capabilities.push && !capabilities.server_search && !capabilities.threads);
    assert!(rig.check().await.is_empty());
}
