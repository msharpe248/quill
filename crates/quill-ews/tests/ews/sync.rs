//! Folder sync with `SyncFolderItems`: rounds, later rounds, the rows
//! asked for by id, starting again.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::types::{Address, FlagChange, Flags, FolderId, RemoteId, SyncCursor, SyncDelta};
use quill_core::{Backend, Error};
use wiremock::matchers::body_string_contains;
use wiremock::{Request, ResponseTemplate};

use crate::common::{
    LISTED, Rig, WHOLE, answer, failure, gone, got, item_ids, listed, success, whole,
};

fn inbox() -> FolderId {
    FolderId("folder-inbox".to_owned())
}

/// A sync state as Exchange hands one out: base64 of something of its own.
fn state(round: u32) -> String {
    BASE64.encode(format!("state after round {round}"))
}

fn cursor(round: u32) -> SyncCursor {
    SyncCursor(state(round))
}

/// What a sync asks, as it goes over the wire.
fn syncing(state: Option<&str>, most: usize) -> String {
    let state = state
        .map(|state| format!("<m:SyncState>{state}</m:SyncState>"))
        .unwrap_or_default();
    format!(
        "<m:SyncFolderItems><m:ItemShape>{LISTED}</m:ItemShape>\
         <m:SyncFolderId><t:FolderId Id=\"folder-inbox\"/></m:SyncFolderId>{state}\
         <m:MaxChangesReturned>{most}</m:MaxChangesReturned>\
         <m:SyncScope>NormalItems</m:SyncScope></m:SyncFolderItems>"
    )
}

/// What asking for rows by id looks like on the wire.
fn rows(ids: &[&str]) -> String {
    format!(
        "<m:GetItem><m:ItemShape>{WHOLE}</m:ItemShape>{}</m:GetItem>",
        item_ids(ids)
    )
}

/// One round of `SyncFolderItems`' answer.
fn round(next: u32, last: bool, changes: &[String]) -> ResponseTemplate {
    answer(
        "SyncFolderItems",
        &[success(
            "SyncFolderItems",
            &format!(
                "<m:SyncState>{}</m:SyncState>\
                 <m:IncludesLastItemInRange>{last}</m:IncludesLastItemInRange>\
                 <m:Changes>{}</m:Changes>",
                state(next),
                changes.concat()
            ),
        )],
    )
}

fn create(item: &str) -> String {
    format!("<t:Create>{item}</t:Create>")
}

fn update(item: &str) -> String {
    format!("<t:Update>{item}</t:Update>")
}

fn delete(id: &str) -> String {
    format!("<t:Delete><t:ItemId Id=\"{id}\" ChangeKey=\"CQAAAA==\"/></t:Delete>")
}

fn read_change(id: &str, read: bool) -> String {
    format!(
        "<t:ReadFlagChange><t:ItemId Id=\"{id}\" ChangeKey=\"CQAAAA==\"/>\
         <t:IsRead>{read}</t:IsRead></t:ReadFlagChange>"
    )
}

/// Mounts the answer to a sync from the start: one that gives no state.
async fn from_the_start(rig: &Rig, answer: ResponseTemplate) {
    rig.on("SyncFolderItems")
        .and(|request: &Request| !String::from_utf8_lossy(&request.body).contains("<m:SyncState>"))
        .respond_with(answer)
        .mount(&rig.server)
        .await;
}

/// Mounts the answer to a sync carrying on from `round`.
async fn after(rig: &Rig, round: u32, answer: ResponseTemplate) {
    rig.on("SyncFolderItems")
        .and(body_string_contains(format!(
            "<m:SyncState>{}</m:SyncState>",
            state(round)
        )))
        .respond_with(answer)
        .mount(&rig.server)
        .await;
}

/// Mounts the rows of the messages a `GetItem` naming `first` asks for.
async fn described(rig: &Rig, first: &str, rows: &[String]) {
    rig.on("GetItem")
        .and(body_string_contains(format!(
            "<m:ItemIds><t:ItemId Id=\"{first}\"/>"
        )))
        .respond_with(answer("GetItem", rows))
        .mount(&rig.server)
        .await;
}

fn ana() -> (&'static str, &'static str) {
    ("Ana Ruiz", "ana.ruiz@example.com")
}

fn omar() -> (&'static str, &'static str) {
    ("Omar Haddad", "omar.haddad@example.com")
}

fn ids(delta: &SyncDelta) -> Vec<&str> {
    delta.added.iter().map(|m| m.id.0.as_str()).collect()
}

/// Both ways of describing one message: as a sync lists it, and whole.
fn both(
    id: &str,
    subject: &str,
    from: (&str, &str),
    received: &str,
    read: bool,
) -> (String, String) {
    (
        listed(id, subject, from, received, read),
        got(&whole(id, subject, from, received, read)),
    )
}

#[tokio::test]
async fn a_first_sync_arrives_in_rounds_and_ends_with_the_state_for_next_time() {
    let rig = Rig::with_page_size(2).await;
    let m1 = both(
        "m1",
        "Welcome to the project",
        ana(),
        "2026-10-01T07:30:00Z",
        true,
    );
    let m2 = both(
        "m2",
        "Harbour survey: second draft",
        ana(),
        "2026-10-29T08:00:00Z",
        true,
    );
    let m3 = both(
        "m3",
        "Minutes, 30 October",
        ana(),
        "2026-10-30T16:40:00Z",
        true,
    );
    let m4 = both(
        "m4",
        "Lunch on Thursday?",
        omar(),
        "2026-11-02T12:01:00Z",
        true,
    );
    let m5 = both(
        "m5",
        "Harbour survey: final",
        ana(),
        "2026-11-03T09:15:04Z",
        false,
    );
    from_the_start(&rig, round(1, false, &[create(&m1.0), create(&m2.0)])).await;
    after(&rig, 1, round(2, false, &[create(&m3.0), create(&m4.0)])).await;
    after(&rig, 2, round(3, true, &[create(&m5.0)])).await;
    described(&rig, "m1", &[m1.1, m2.1]).await;
    described(&rig, "m3", &[m3.1, m4.1]).await;
    described(&rig, "m5", &[m5.1]).await;

    // The way the sync engine calls: again from the cursor while there is
    // more.
    let mut cursor = None;
    let mut rounds = Vec::new();
    loop {
        let delta = rig.ews.sync_folder(&inbox(), cursor).await.unwrap();
        cursor = Some(delta.next_cursor.clone());
        let done = !delta.has_more;
        rounds.push(delta);
        if done {
            break;
        }
    }

    assert_eq!(rounds.len(), 3);
    assert_eq!(ids(&rounds[0]), ["m1", "m2"]);
    assert_eq!(ids(&rounds[1]), ["m3", "m4"]);
    assert_eq!(ids(&rounds[2]), ["m5"]);
    assert_eq!(
        rounds
            .iter()
            .map(|round| round.has_more)
            .collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(rounds[0].next_cursor, SyncCursor(state(1)));
    assert_eq!(rounds[2].next_cursor, SyncCursor(state(3)));
    for round in &rounds {
        assert!(!round.reset);
        assert!(round.changed.is_empty() && round.removed.is_empty());
    }

    // Enough for a row in the list, without a body.
    let newest = &rounds[2].added[0];
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
    assert_eq!(newest.size, 4821);
    assert_eq!(newest.thread, ["<topic-m5@conversation.ews>"]);
    assert!(rounds[1].added[1].flags.seen);

    assert_eq!(
        rig.check().await,
        [
            syncing(None, 2),
            rows(&["m1", "m2"]),
            syncing(Some(&state(1)), 2),
            rows(&["m3", "m4"]),
            syncing(Some(&state(2)), 2),
            rows(&["m5"]),
        ]
    );
}

#[tokio::test]
async fn a_later_round_brings_new_mail_changes_read_marks_and_removals() {
    let rig = Rig::start().await;
    let lena = ("Lena Fischer", "lena.fischer@example.com");
    let m6 = both(
        "m6",
        "Invoice 2026-114",
        lena,
        "2026-11-04T07:45:00Z",
        false,
    );
    // Seen before: flagged and answered since.
    let m2_now = whole(
        "m2",
        "Harbour survey: second draft",
        ana(),
        "2026-10-29T08:00:00Z",
        true,
    )
    .replace("NotFlagged", "Flagged")
    .replace(
        "<t:IsDraft>",
        "<t:ExtendedProperty>\
         <t:ExtendedFieldURI PropertyTag=\"0x1081\" PropertyType=\"Integer\"/>\
         <t:Value>102</t:Value></t:ExtendedProperty><t:IsDraft>",
    );
    let m2_listed = listed(
        "m2",
        "Harbour survey: second draft",
        ana(),
        "2026-10-29T08:00:00Z",
        true,
    );
    // A draft of Robin's own, with something attached, answering m4.
    let m7_now = whole(
        "m7",
        "Re: Lunch on Thursday?",
        ("Robin Okafor", "robin@example.com"),
        "2026-11-04T08:00:00Z",
        true,
    )
    .replace("<t:IsDraft>false", "<t:IsDraft>true")
    .replace("<t:HasAttachments>false", "<t:HasAttachments>true")
    .replace(
        "<t:IsRead>",
        "<t:References>&lt;m0@example.com&gt; &lt;m4@example.com&gt;</t:References><t:IsRead>",
    )
    .replace(
        "<t:DateTimeSent>",
        "<t:InReplyTo>&lt;m4@example.com&gt;</t:InReplyTo><t:DateTimeSent>",
    );
    let m7_listed = listed(
        "m7",
        "Re: Lunch on Thursday?",
        ("Robin Okafor", "robin@example.com"),
        "2026-11-04T08:00:00Z",
        true,
    );
    // Read elsewhere since; flagged long ago, which a read mark's change
    // does not say.
    let m4_now = whole(
        "m4",
        "Lunch on Thursday?",
        omar(),
        "2026-11-02T12:01:00Z",
        true,
    )
    .replace("NotFlagged", "Flagged");
    after(
        &rig,
        3,
        round(
            4,
            true,
            &[
                create(&m6.0),
                update(&m2_listed),
                delete("m3"),
                read_change("m4", true),
                create(&m7_listed),
                delete("m1"),
            ],
        ),
    )
    .await;
    described(
        &rig,
        "m6",
        &[m6.1, got(&m2_now), got(&m4_now), got(&m7_now)],
    )
    .await;

    let delta = rig
        .ews
        .sync_folder(&inbox(), Some(cursor(3)))
        .await
        .unwrap();

    assert!(!delta.reset && !delta.has_more);
    // New and changed alike are in `added`, as they are now: the store
    // updates the ones it already holds.
    assert_eq!(ids(&delta), ["m6", "m2", "m7"]);
    assert_eq!(
        delta.added[1].flags,
        Flags {
            seen: true,
            flagged: true,
            answered: true,
            draft: false,
        }
    );
    assert!(delta.added[2].flags.draft && delta.added[2].has_attachments);
    // What a message says it follows, in its own words.
    assert_eq!(
        delta.added[2].thread,
        ["<m0@example.com>", "<m4@example.com>"]
    );
    // A read mark's change brings the message's other marks with it.
    assert_eq!(
        delta.changed,
        [FlagChange {
            id: RemoteId("m4".to_owned()),
            flags: Flags {
                seen: true,
                flagged: true,
                answered: false,
                draft: false,
            },
        }]
    );
    assert_eq!(
        delta.removed,
        [RemoteId("m3".to_owned()), RemoteId("m1".to_owned())]
    );
    assert_eq!(delta.next_cursor, cursor(4));
    assert_eq!(
        rig.check().await,
        [
            syncing(Some(&state(3)), 50),
            rows(&["m6", "m2", "m4", "m7"]),
        ]
    );
}

#[tokio::test]
async fn nothing_new_is_an_empty_delta_with_a_new_state_and_nothing_more_is_asked() {
    let rig = Rig::start().await;
    after(&rig, 4, round(5, true, &[])).await;

    let delta = rig
        .ews
        .sync_folder(&inbox(), Some(cursor(4)))
        .await
        .unwrap();
    assert_eq!(
        delta,
        SyncDelta {
            reset: false,
            added: Vec::new(),
            changed: Vec::new(),
            removed: Vec::new(),
            has_more: false,
            next_cursor: cursor(5),
        }
    );
    assert_eq!(rig.check().await, [syncing(Some(&state(4)), 50)]);
}

#[tokio::test]
async fn rows_are_asked_for_a_hundred_at_a_time() {
    let rig = Rig::with_page_size(512).await;
    let names: Vec<String> = (0..230).map(|i| format!("m{i:03}")).collect();
    let messages: Vec<(String, String)> = names
        .iter()
        .map(|id| both(id, "Daily report", ana(), "2026-11-03T06:00:00Z", true))
        .collect();
    let changes: Vec<String> = messages.iter().map(|(listed, _)| create(listed)).collect();
    from_the_start(&rig, round(1, true, &changes)).await;
    for (asked, batch) in names.chunks(100).zip(messages.chunks(100)) {
        let rows: Vec<String> = batch.iter().map(|(_, whole)| whole.clone()).collect();
        described(&rig, &asked[0], &rows).await;
    }

    let delta = rig.ews.sync_folder(&inbox(), None).await.unwrap();
    assert_eq!(
        ids(&delta),
        names.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(
        delta
            .added
            .iter()
            .all(|message| message.headers.to.len() == 1)
    );

    let requests = rig.check().await;
    let asked = |range: std::ops::Range<usize>| {
        rows(&names[range].iter().map(String::as_str).collect::<Vec<_>>())
    };
    assert_eq!(
        requests,
        [
            syncing(None, 512),
            asked(0..100),
            asked(100..200),
            asked(200..230),
        ]
    );
}

#[tokio::test]
async fn a_message_that_cannot_be_asked_for_by_id_is_listed_as_the_sync_gave_it() {
    let rig = Rig::start().await;
    let m1 = both(
        "m1",
        "Welcome to the project",
        ana(),
        "2026-10-01T07:30:00Z",
        true,
    );
    let m2 = listed("m2", "Meeting moved", omar(), "2026-10-02T07:30:00Z", false);
    let m3 = listed(
        "m3",
        "Gone in a moment",
        ana(),
        "2026-10-03T07:30:00Z",
        false,
    );
    from_the_start(
        &rig,
        round(
            1,
            true,
            &[
                create(&m1.0),
                create(&m2),
                create(&m3),
                read_change("m4", true),
                read_change("m5", true),
                // Not mail: left out, and not asked about.
                create("<t:Contact><t:ItemId Id=\"c1\"/></t:Contact>"),
            ],
        ),
    )
    .await;
    described(
        &rig,
        "m1",
        &[
            m1.1,
            // Exchange will not describe this one the way it was asked.
            failure(
                "GetItem",
                "ErrorInvalidPropertyRequest",
                "Property is not valid for this object type.",
            ),
            // Deleted between the sync and the question. A later round
            // says so.
            gone("GetItem"),
            gone("GetItem"),
            failure(
                "GetItem",
                "ErrorInvalidPropertyRequest",
                "Property is not valid for this object type.",
            ),
        ],
    )
    .await;

    let delta = rig.ews.sync_folder(&inbox(), None).await.unwrap();
    assert_eq!(ids(&delta), ["m1", "m2"]);
    // The sender by name alone, and nobody it is to: all a sync says.
    assert_eq!(
        delta.added[1].headers.from,
        [Address {
            name: Some("Omar Haddad".to_owned()),
            email: String::new(),
        }]
    );
    assert!(delta.added[1].headers.to.is_empty());
    assert_eq!(delta.added[1].headers.subject, "Meeting moved");
    // A read mark alone, with nothing known of the other marks, is not
    // passed on as if they were known.
    assert!(delta.changed.is_empty());
    assert_eq!(
        rig.check().await,
        [syncing(None, 50), rows(&["m1", "m2", "m3", "m4", "m5"])]
    );
}

#[tokio::test]
async fn rows_that_cannot_be_had_just_now_fail_the_round() {
    for (trouble, expected) in [
        (
            answer(
                "GetItem",
                &[failure(
                    "GetItem",
                    "ErrorMailboxStoreUnavailable",
                    "The mailbox database is temporarily unavailable.",
                )],
            ),
            "temporary",
        ),
        // Fewer answers than questions.
        (answer("GetItem", &[]), "backend"),
    ] {
        let rig = Rig::start().await;
        let m1 = listed(
            "m1",
            "Welcome to the project",
            ana(),
            "2026-10-01T07:30:00Z",
            true,
        );
        from_the_start(&rig, round(1, true, &[create(&m1)])).await;
        rig.answers("GetItem", trouble).await;

        // Better the round again later than rows with nobody in them.
        let err = rig.ews.sync_folder(&inbox(), None).await.unwrap_err();
        match expected {
            "temporary" => assert!(matches!(err, Error::Temporary(_)), "{err:?}"),
            _ => assert!(matches!(err, Error::Backend(_)), "{err:?}"),
        }
        assert_eq!(rig.check().await.len(), 2);
    }
}

#[tokio::test]
async fn a_state_the_server_no_longer_knows_starts_the_folder_again_and_says_so() {
    let rig = Rig::start().await;
    after(
        &rig,
        9,
        answer(
            "SyncFolderItems",
            &[failure(
                "SyncFolderItems",
                "ErrorInvalidSyncStateData",
                "The sync state data is corrupt or otherwise invalid.",
            )],
        ),
    )
    .await;
    let m5 = both(
        "m5",
        "Harbour survey: final",
        ana(),
        "2026-11-03T09:15:04Z",
        true,
    );
    from_the_start(&rig, round(1, false, &[create(&m5.0)])).await;
    described(&rig, "m5", &[m5.1]).await;

    let delta = rig
        .ews
        .sync_folder(&inbox(), Some(cursor(9)))
        .await
        .unwrap();

    assert!(delta.reset);
    assert!(delta.has_more);
    assert_eq!(ids(&delta), ["m5"]);
    assert_eq!(delta.next_cursor, cursor(1));
    assert_eq!(
        rig.check().await,
        [
            syncing(Some(&state(9)), 50),
            syncing(None, 50),
            rows(&["m5"]),
        ]
    );
}

#[tokio::test]
async fn a_cursor_that_is_no_sync_state_is_not_sent_and_the_folder_starts_again() {
    for stale in [
        // Left behind by another kind of account, or by nobody knows what.
        "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=abc"
            .to_owned(),
        "</m:SyncState><m:SyncState>".to_owned(),
        String::new(),
        "A".repeat(9 * 1024 * 1024),
    ] {
        let rig = Rig::start().await;
        from_the_start(&rig, round(1, true, &[])).await;

        let delta = rig
            .ews
            .sync_folder(&inbox(), Some(SyncCursor(stale)))
            .await
            .unwrap();

        assert!(delta.reset && !delta.has_more);
        assert_eq!(delta.next_cursor, cursor(1));
        assert_eq!(rig.check().await, [syncing(None, 50)]);
    }
}

#[tokio::test]
async fn a_folder_that_is_gone_is_not_found() {
    for code in ["ErrorSyncFolderNotFound", "ErrorFolderNotFound"] {
        let rig = Rig::start().await;
        rig.answers(
            "SyncFolderItems",
            answer(
                "SyncFolderItems",
                &[failure(
                    "SyncFolderItems",
                    code,
                    "The folder to synchronize could not be found.",
                )],
            ),
        )
        .await;

        let err = rig
            .ews
            .sync_folder(&inbox(), Some(cursor(3)))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
        assert!(err.to_string().contains(code), "{err}");
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn a_round_that_does_not_say_where_to_carry_on_is_an_error() {
    for inside in [
        "<m:IncludesLastItemInRange>true</m:IncludesLastItemInRange><m:Changes/>".to_owned(),
        "<m:SyncState>&lt;not a state&gt;</m:SyncState><m:Changes/>".to_owned(),
        // One that would be thrown away next time, and the whole folder
        // read again, every time.
        format!("<m:SyncState>{}</m:SyncState>", "A".repeat(9 * 1024 * 1024)),
    ] {
        let rig = Rig::start().await;
        rig.answers(
            "SyncFolderItems",
            answer("SyncFolderItems", &[success("SyncFolderItems", &inside)]),
        )
        .await;

        let err = rig.ews.sync_folder(&inbox(), None).await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{err:?}");
        assert!(err.to_string().contains("where to carry on"), "{err}");
        assert_eq!(rig.check().await.len(), 1);
    }
}

#[tokio::test]
async fn a_server_that_says_there_is_more_and_gives_nothing_is_not_asked_for_ever() {
    let rig = Rig::start().await;
    after(&rig, 3, round(3, false, &[])).await;

    let err = rig
        .ews
        .sync_folder(&inbox(), Some(cursor(3)))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_hostile_listing_does_not_reach_the_list_as_it_came() {
    let rig = Rig::start().await;
    let hostile = "<t:Message><t:ItemId Id=\"m1\"/>\
         <t:Subject>Invoice&#x1b;]0;gotcha&#7; due</t:Subject>\
         <t:From><t:Mailbox><t:Name>Ana&#x1b;[2J</t:Name>\
         <t:EmailAddress>ana.ruiz@example.com</t:EmailAddress></t:Mailbox></t:From>\
         <t:InternetMessageId>&lt;a&#0;b@example.com&gt;</t:InternetMessageId>\
         </t:Message>";
    from_the_start(&rig, round(1, true, &[create(hostile)])).await;
    rig.answers("GetItem", answer("GetItem", &[got(hostile)]))
        .await;

    // A character reference to nothing is not XML at all.
    let err = rig.ews.sync_folder(&inbox(), None).await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert_eq!(rig.check().await.len(), 1);

    let rig = Rig::start().await;
    let hostile = hostile.replace("&#0;", "&#9;");
    from_the_start(&rig, round(1, true, &[create(&hostile)])).await;
    rig.answers("GetItem", answer("GetItem", &[got(&hostile)]))
        .await;

    let delta = rig.ews.sync_folder(&inbox(), None).await.unwrap();
    assert_eq!(delta.added[0].headers.subject, "Invoice ]0;gotcha  due");
    assert_eq!(
        delta.added[0].headers.from[0].name.as_deref(),
        Some("Ana [2J")
    );
    assert_eq!(
        delta.added[0].headers.message_id.as_deref(),
        Some("<a b@example.com>")
    );
    rig.check().await;
}
