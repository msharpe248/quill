//! Folder sync with `messages/delta`: pages, later rounds, starting again.

use quill_core::types::{Address, Flags, FolderId, RemoteId, SyncCursor, SyncDelta};
use quill_core::{Backend, Error};
use serde_json::{Value, json};
use wiremock::matchers::{query_param, query_param_is_missing};
use wiremock::{MockServer, ResponseTemplate};

use crate::common::{IMMUTABLE_IDS, Rig, SUMMARY, header, message, ok, refusal};

const DELTA: &str = "/me/mailFolders/folder-inbox/messages/delta";

fn inbox() -> FolderId {
    FolderId("folder-inbox".to_owned())
}

/// What a first sync asks, as it goes over the wire.
fn fresh() -> String {
    format!("GET /v1.0{DELTA}?$select={SUMMARY}&$orderby=receivedDateTime%20desc")
}

fn ids(delta: &SyncDelta) -> Vec<&str> {
    delta.added.iter().map(|m| m.id.0.as_str()).collect()
}

/// Mounts the answer to a first sync: a request with no token of either
/// kind in it.
async fn first_page(rig: &Rig, answer: ResponseTemplate) {
    rig.on("GET", DELTA)
        .and(query_param_is_missing("$skiptoken"))
        .and(query_param_is_missing("$deltatoken"))
        .respond_with(answer)
        .mount(&rig.server)
        .await;
}

fn ana() -> (&'static str, &'static str) {
    ("Ana Ruiz", "ana.ruiz@example.com")
}

#[tokio::test]
async fn a_first_sync_arrives_in_pages_and_ends_with_the_link_for_next_time() {
    let rig = Rig::with_page_size(2).await;
    first_page(
        &rig,
        ok(json!({
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#Collection(message)",
            "value": [
                message("m5", "Harbour survey: final", ana(), "2026-11-03T09:15:04Z", false),
                message("m4", "Lunch on Thursday?", ("Omar Haddad", "omar.haddad@example.com"), "2026-11-02T12:01:00Z", true),
            ],
            "@odata.nextLink": rig.link(&format!("{DELTA}?$skiptoken=page-2")),
        })),
    )
    .await;
    rig.on("GET", DELTA)
        .and(query_param("$skiptoken", "page-2"))
        .respond_with(ok(json!({
            "value": [
                message("m3", "Minutes, 30 October", ana(), "2026-10-30T16:40:00Z", true),
                message("m2", "Harbour survey: second draft", ana(), "2026-10-29T08:00:00Z", true),
            ],
            "@odata.nextLink": rig.link(&format!("{DELTA}?$skiptoken=page-3")),
        })))
        .mount(&rig.server)
        .await;
    rig.on("GET", DELTA)
        .and(query_param("$skiptoken", "page-3"))
        .respond_with(ok(json!({
            "value": [
                message("m1", "Welcome to the project", ana(), "2026-10-01T07:30:00Z", true),
            ],
            "@odata.deltaLink": rig.link(&format!("{DELTA}?$deltatoken=round-1")),
        })))
        .mount(&rig.server)
        .await;

    // The way the sync engine calls: again from the cursor while there is
    // more.
    let mut cursor = None;
    let mut pages = Vec::new();
    loop {
        let delta = rig.graph.sync_folder(&inbox(), cursor).await.unwrap();
        cursor = Some(delta.next_cursor.clone());
        let done = !delta.has_more;
        pages.push(delta);
        if done {
            break;
        }
    }

    assert_eq!(pages.len(), 3);
    assert_eq!(ids(&pages[0]), ["m5", "m4"]);
    assert_eq!(ids(&pages[1]), ["m3", "m2"]);
    assert_eq!(ids(&pages[2]), ["m1"]);
    assert_eq!(
        pages.iter().map(|page| page.has_more).collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(
        pages[0].next_cursor,
        SyncCursor(rig.link(&format!("{DELTA}?$skiptoken=page-2")))
    );
    assert_eq!(
        pages[2].next_cursor,
        SyncCursor(rig.link(&format!("{DELTA}?$deltatoken=round-1")))
    );
    for page in &pages {
        assert!(!page.reset);
        assert!(page.changed.is_empty() && page.removed.is_empty());
    }

    // Enough for a row in the list, without a body.
    let newest = &pages[0].added[0];
    assert_eq!(newest.headers.subject, "Harbour survey: final");
    assert_eq!(
        newest.headers.from,
        [Address::new("Ana Ruiz", "ana.ruiz@example.com")]
    );
    assert_eq!(
        newest.headers.to,
        [Address::new("Robin Okafor", "robin@example.com")]
    );
    assert_eq!(newest.headers.date, 1_793_697_304);
    assert_eq!(
        newest.headers.message_id.as_deref(),
        Some("<m5@example.com>")
    );
    assert_eq!(newest.flags, Flags::default());
    assert!(pages[0].added[1].flags.seen);

    for request in rig.requests().await {
        assert_eq!(
            header(&request, "prefer"),
            format!("{IMMUTABLE_IDS}, odata.maxpagesize=2")
        );
    }
    assert_eq!(
        rig.check().await,
        [
            fresh(),
            format!("GET /v1.0{DELTA}?$skiptoken=page-2"),
            format!("GET /v1.0{DELTA}?$skiptoken=page-3"),
        ]
    );
}

#[tokio::test]
async fn a_later_round_brings_new_mail_changes_and_removals() {
    let rig = Rig::start().await;
    let mut reread = message(
        "m2",
        "Harbour survey: second draft",
        ana(),
        "2026-10-29T08:00:00Z",
        true,
    );
    reread["flag"] = json!({"flagStatus": "flagged"});
    let mut draft = message(
        "m7",
        "Re: Lunch on Thursday?",
        ("Robin Okafor", "robin@example.com"),
        "2026-11-04T08:00:00Z",
        true,
    );
    draft["isDraft"] = json!(true);
    draft["hasAttachments"] = json!(true);
    rig.on("GET", DELTA)
        .and(query_param("$deltatoken", "round-1"))
        .respond_with(ok(json!({
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#Collection(message)",
            "value": [
                message("m6", "Invoice 2026-114", ("Lena Fischer", "lena.fischer@example.com"), "2026-11-04T07:45:00Z", false),
                // Seen before: read and flagged since.
                reread,
                {
                    "@odata.type": "#microsoft.graph.message",
                    "id": "m3",
                    "@removed": {"reason": "deleted"}
                },
                draft,
                {"id": "m1", "@removed": {"reason": "deleted"}},
            ],
            "@odata.deltaLink": rig.link(&format!("{DELTA}?$deltatoken=round-2")),
        })))
        .mount(&rig.server)
        .await;

    let cursor = SyncCursor(rig.link(&format!("{DELTA}?$deltatoken=round-1")));
    let delta = rig.graph.sync_folder(&inbox(), Some(cursor)).await.unwrap();

    assert!(!delta.reset && !delta.has_more);
    // New and changed alike are in `added`, as they are now: the store
    // updates the ones it already holds.
    assert_eq!(ids(&delta), ["m6", "m2", "m7"]);
    assert!(delta.changed.is_empty());
    assert_eq!(
        delta.added[1].flags,
        Flags {
            seen: true,
            flagged: true,
            answered: false,
            draft: false,
        }
    );
    assert!(delta.added[2].flags.draft && delta.added[2].has_attachments);
    assert_eq!(
        delta.removed,
        [RemoteId("m3".to_owned()), RemoteId("m1".to_owned())]
    );
    assert_eq!(
        delta.next_cursor,
        SyncCursor(rig.link(&format!("{DELTA}?$deltatoken=round-2")))
    );
    assert_eq!(
        rig.check().await,
        [format!("GET /v1.0{DELTA}?$deltatoken=round-1")]
    );
}

#[tokio::test]
async fn nothing_new_is_an_empty_delta_with_a_new_link() {
    let rig = Rig::start().await;
    rig.on("GET", DELTA)
        .and(query_param("$deltatoken", "round-2"))
        .respond_with(ok(json!({
            "value": [],
            "@odata.deltaLink": rig.link(&format!("{DELTA}?$deltatoken=round-3")),
        })))
        .mount(&rig.server)
        .await;

    let cursor = SyncCursor(rig.link(&format!("{DELTA}?$deltatoken=round-2")));
    let delta = rig.graph.sync_folder(&inbox(), Some(cursor)).await.unwrap();
    assert_eq!(
        delta,
        SyncDelta {
            reset: false,
            added: Vec::new(),
            changed: Vec::new(),
            removed: Vec::new(),
            has_more: false,
            next_cursor: SyncCursor(rig.link(&format!("{DELTA}?$deltatoken=round-3"))),
        }
    );
    rig.check().await;
}

#[tokio::test]
async fn an_expired_link_starts_the_folder_again_and_says_so() {
    for expired in [
        refusal(
            410,
            "SyncStateNotFound",
            "The sync state generation is not found; counts: 14 13 0",
        ),
        // The status alone is enough.
        ResponseTemplate::new(410),
        // And so are the words, under another status.
        refusal(400, "syncStateNotFound", "Sync state not found."),
    ] {
        let rig = Rig::start().await;
        rig.on("GET", DELTA)
            .and(query_param("$deltatoken", "too-old"))
            .respond_with(expired)
            .mount(&rig.server)
            .await;
        first_page(
            &rig,
            ok(json!({
                "value": [
                    message("m5", "Harbour survey: final", ana(), "2026-11-03T09:15:04Z", true),
                ],
                "@odata.nextLink": rig.link(&format!("{DELTA}?$skiptoken=again-2")),
            })),
        )
        .await;

        let cursor = SyncCursor(rig.link(&format!("{DELTA}?$deltatoken=too-old")));
        let delta = rig.graph.sync_folder(&inbox(), Some(cursor)).await.unwrap();

        assert!(delta.reset);
        assert!(delta.has_more);
        assert_eq!(ids(&delta), ["m5"]);
        assert_eq!(
            delta.next_cursor,
            SyncCursor(rig.link(&format!("{DELTA}?$skiptoken=again-2")))
        );
        assert_eq!(
            rig.check().await,
            [format!("GET /v1.0{DELTA}?$deltatoken=too-old"), fresh()]
        );
    }
}

#[tokio::test]
async fn only_the_page_that_follows_a_reset_says_reset() {
    let rig = Rig::start().await;
    rig.on("GET", DELTA)
        .and(query_param("$skiptoken", "again-2"))
        .respond_with(ok(json!({
            "value": [],
            "@odata.deltaLink": rig.link(&format!("{DELTA}?$deltatoken=round-9")),
        })))
        .mount(&rig.server)
        .await;

    let cursor = SyncCursor(rig.link(&format!("{DELTA}?$skiptoken=again-2")));
    let delta = rig.graph.sync_folder(&inbox(), Some(cursor)).await.unwrap();
    assert!(!delta.reset && !delta.has_more);
    rig.check().await;
}

#[tokio::test]
async fn a_cursor_that_leads_elsewhere_is_never_asked_and_the_folder_starts_again() {
    let elsewhere = MockServer::start().await;
    for cursor in [
        format!("{}/v1.0{DELTA}?$deltatoken=round-1", elsewhere.uri()),
        "1;4;1:12".to_owned(),
        String::new(),
    ] {
        let rig = Rig::start().await;
        first_page(
            &rig,
            ok(json!({
                "value": [],
                "@odata.deltaLink": rig.link(&format!("{DELTA}?$deltatoken=round-1")),
            })),
        )
        .await;

        let delta = rig
            .graph
            .sync_folder(&inbox(), Some(SyncCursor(cursor)))
            .await
            .unwrap();
        // What the store holds was synced from somewhere else, or by
        // something else: it cannot be built on.
        assert!(delta.reset);
        assert_eq!(rig.check().await, [fresh()]);
    }
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_entry_with_only_what_changed_is_completed_before_it_is_reported() {
    let rig = Rig::start().await;
    rig.on("GET", DELTA)
        .and(query_param("$deltatoken", "round-1"))
        .respond_with(ok(json!({
            "value": [
                {"id": "m2", "isRead": true},
                {"id": "m9", "isRead": false},
            ],
            "@odata.deltaLink": rig.link(&format!("{DELTA}?$deltatoken=round-2")),
        })))
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/messages/m2")
        .respond_with(ok(message(
            "m2",
            "Harbour survey: second draft",
            ana(),
            "2026-10-29T08:00:00Z",
            true,
        )))
        .mount(&rig.server)
        .await;
    // Gone again by the time it is asked for.
    rig.on("GET", "/me/messages/m9")
        .respond_with(refusal(
            404,
            "ErrorItemNotFound",
            "The specified object was not found in the store.",
        ))
        .mount(&rig.server)
        .await;

    let cursor = SyncCursor(rig.link(&format!("{DELTA}?$deltatoken=round-1")));
    let delta = rig.graph.sync_folder(&inbox(), Some(cursor)).await.unwrap();

    // Reported whole, so the row keeps its subject and sender.
    assert_eq!(ids(&delta), ["m2"]);
    assert_eq!(
        delta.added[0].headers.subject,
        "Harbour survey: second draft"
    );
    assert!(delta.added[0].flags.seen);
    assert!(delta.removed.is_empty());
    assert_eq!(
        rig.check().await,
        [
            format!("GET /v1.0{DELTA}?$deltatoken=round-1"),
            format!("GET /v1.0/me/messages/m2?$select={SUMMARY}"),
            format!("GET /v1.0/me/messages/m9?$select={SUMMARY}"),
        ]
    );
}

#[tokio::test]
async fn an_answer_that_is_not_a_delta_is_a_backend_error() {
    let link = "\"@odata.deltaLink\": \"http://127.0.0.1:9/v1.0/x\"";
    let bodies: Vec<String> = vec![
        "<html><body>Service unavailable</body></html>".to_owned(),
        "{\"value\": [".to_owned(),
        String::new(),
        "[]".to_owned(),
        // No list of messages.
        format!("{{{link}}}"),
        // A message with no id.
        format!("{{\"value\": [{{\"subject\": \"Minutes\"}}], {link}}}"),
        // Values of the wrong kind.
        format!("{{\"value\": [{{\"id\": \"m1\", \"isRead\": \"yes\"}}], {link}}}"),
        format!("{{\"value\": [{{\"id\": \"m1\", \"from\": \"ana.ruiz@example.com\"}}], {link}}}"),
        format!("{{\"value\": [\"m1\"], {link}}}"),
        // Nowhere to carry on from.
        "{\"value\": []}".to_owned(),
    ];
    for body in bodies {
        let rig = Rig::start().await;
        first_page(
            &rig,
            ResponseTemplate::new(200).set_body_raw(body.clone(), "application/json"),
        )
        .await;

        let err = rig.graph.sync_folder(&inbox(), None).await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{body:?}: {err:?}");
        assert_eq!(rig.check().await, [fresh()]);
    }
}

#[tokio::test]
async fn a_link_to_another_server_is_not_kept_as_the_cursor() {
    let elsewhere = MockServer::start().await;
    let rig = Rig::start().await;
    first_page(
        &rig,
        ok(json!({
            "value": [message("m1", "Welcome to the project", ana(), "2026-10-01T07:30:00Z", true)],
            "@odata.nextLink": format!("{}/v1.0{DELTA}?$skiptoken=page-2", elsewhere.uri()),
        })),
    )
    .await;

    let err = rig.graph.sync_folder(&inbox(), None).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
    rig.check().await;
}

#[tokio::test]
async fn messages_with_parts_missing_are_still_messages() {
    let rig = Rig::start().await;
    let sparse: Value = json!({
        "id": "m8",
        "subject": null,
        "from": null,
        "toRecipients": [{"emailAddress": {"name": "", "address": ""}}, {}],
        "receivedDateTime": "2026-11-04T10:00:00Z",
        "isRead": false,
        "isDraft": true
    });
    first_page(
        &rig,
        ok(json!({
            "value": [sparse],
            "@odata.deltaLink": rig.link(&format!("{DELTA}?$deltatoken=round-1")),
        })),
    )
    .await;

    let delta = rig.graph.sync_folder(&inbox(), None).await.unwrap();
    let only = &delta.added[0];
    assert_eq!(only.headers.subject, "");
    assert!(only.headers.from.is_empty() && only.headers.to.is_empty());
    assert_eq!(only.headers.message_id, None);
    assert!(only.flags.draft && !only.flags.seen && !only.flags.flagged);
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_folder_that_is_gone_is_not_found() {
    let rig = Rig::start().await;
    first_page(
        &rig,
        refusal(
            404,
            "ErrorItemNotFound",
            "The specified object was not found in the store.",
        ),
    )
    .await;

    let err = rig.graph.sync_folder(&inbox(), None).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert!(
        err.to_string()
            .contains("The specified object was not found in the store. (ErrorItemNotFound)"),
        "{err}"
    );
    rig.check().await;
}
