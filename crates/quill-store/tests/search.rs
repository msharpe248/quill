//! Search, through the store's public face: a query as typed, parsed and
//! run against mail in two accounts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use quill_core::search::parse;
use quill_core::types::{
    Address, FlagChange, Flags, FolderId, FolderRole, Headers, LocalFolderId, RemoteFolder,
    RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use quill_store::Store;

const DAY: i64 = 86_400;
/// 2026-11-01T00:00:00Z.
const NOVEMBER: i64 = 1_793_491_200;

struct Mail {
    id: &'static str,
    from: (&'static str, &'static str),
    to: (&'static str, &'static str),
    subject: &'static str,
    /// Days after the first of November.
    day: i64,
    flags: Flags,
    attachments: bool,
    /// The body, once downloaded.
    body: Option<&'static str>,
}

const ANA: (&str, &str) = ("Ana Ruiz", "ana.ruiz@example.com");
const SAM: (&str, &str) = ("Sam Lee", "sam.lee@example.com");
const ZOE: (&str, &str) = ("Zoë Müller", "zoe@example.com");
const ROBIN: (&str, &str) = ("Robin Vale", "robin@example.com");

const SEEN: Flags = Flags {
    seen: true,
    flagged: false,
    answered: false,
    draft: false,
};

fn mail(
    id: &'static str,
    from: (&'static str, &'static str),
    subject: &'static str,
    day: i64,
    body: Option<&'static str>,
) -> Mail {
    Mail {
        id,
        from,
        to: ROBIN,
        subject,
        day,
        flags: SEEN,
        attachments: false,
        body,
    }
}

fn store() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let mut fill = |account: &str, folder: &str, role: FolderRole, mail: Vec<Mail>| {
        let account = store
            .upsert_account(account, "robin@example.com", "imap")
            .unwrap();
        let folder_id: LocalFolderId = store
            .upsert_folder(
                account,
                &RemoteFolder {
                    id: FolderId(folder.to_owned()),
                    name: folder.to_owned(),
                    role,
                },
            )
            .unwrap();
        let added = mail
            .iter()
            .map(|mail| RemoteMessage {
                id: RemoteId(mail.id.to_owned()),
                headers: Headers {
                    message_id: Some(format!("<{}@example.com>", mail.id)),
                    subject: mail.subject.to_owned(),
                    from: vec![Address::new(mail.from.0, mail.from.1)],
                    to: vec![Address::new(mail.to.0, mail.to.1)],
                    cc: Vec::new(),
                    date: NOVEMBER + mail.day * DAY + 3600,
                },
                flags: mail.flags,
                size: 100,
                has_attachments: mail.attachments,
                thread: Vec::new(),
            })
            .collect();
        store
            .apply_delta(
                folder_id,
                &SyncDelta {
                    reset: false,
                    added,
                    changed: Vec::new(),
                    removed: Vec::new(),
                    has_more: false,
                    next_cursor: SyncCursor("c".to_owned()),
                },
            )
            .unwrap();
        for mail in &mail {
            if let Some(body) = mail.body {
                let id = store
                    .local_message_id(folder_id, &RemoteId(mail.id.to_owned()))
                    .unwrap()
                    .unwrap();
                store.put_body(id, body).unwrap();
            }
        }
    };
    fill(
        "Work",
        "INBOX",
        FolderRole::Inbox,
        vec![
            Mail {
                flags: Flags::default(),
                attachments: true,
                ..mail(
                    "roadmap",
                    ANA,
                    "Q4 roadmap",
                    13,
                    Some(
                        "Here is the draft roadmap. The search milestone assumes the index format is settled.",
                    ),
                )
            },
            mail(
                "offsite",
                SAM,
                "Offsite agenda",
                10,
                Some("Lunch by the water, then breakouts."),
            ),
            Mail {
                flags: Flags {
                    flagged: true,
                    answered: true,
                    ..SEEN
                },
                ..mail(
                    "invoice",
                    ZOE,
                    "Rechnung für Oktober",
                    5,
                    Some("Die Rechnung ist im Anhang. Viele Grüße"),
                )
            },
            // Its body has not been downloaded.
            mail("unread-body", ANA, "Holiday rota", 2, None),
        ],
    );
    fill(
        "Work",
        "Sent Items",
        FolderRole::Sent,
        vec![Mail {
            to: ANA,
            ..mail(
                "reply",
                ROBIN,
                "Re: Q4 roadmap",
                14,
                Some("Thursday works. The roadmap looks right to me."),
            )
        }],
    );
    fill(
        "Personal",
        "INBOX",
        FolderRole::Inbox,
        vec![mail(
            "run",
            SAM,
            "Trail run Saturday",
            12,
            Some(
                "Meet at the car park at eight. Bring water; the roadmap of the route is attached.",
            ),
        )],
    );
    store
}

/// Runs `text` as the user would type it and returns the subjects found,
/// newest first.
fn find(store: &Store, text: &str) -> Vec<String> {
    // Days are UTC in these tests.
    let day = |year: i32, month: u32, day: u32| {
        (year == 2026 && (1..=12).contains(&month) && (1..=31).contains(&day))
            .then(|| NOVEMBER + (i64::from(month) - 11) * 30 * DAY + (i64::from(day) - 1) * DAY)
    };
    store
        .search(&parse(text, day), 50)
        .unwrap()
        .into_iter()
        .map(|row| row.headers.subject)
        .collect()
}

#[test]
fn words_are_found_in_subject_people_and_body_newest_first() {
    let store = store();
    // In two subjects and in a body of a third, across both accounts.
    assert_eq!(
        find(&store, "roadmap "),
        ["Re: Q4 roadmap", "Q4 roadmap", "Trail run Saturday"]
    );
    // In the body only.
    assert_eq!(find(&store, "breakouts "), ["Offsite agenda"]);
    // In a name, and in an address.
    assert_eq!(find(&store, "ruiz ").len(), 3);
    assert_eq!(
        find(&store, "sam.lee@example.com "),
        ["Trail run Saturday", "Offsite agenda"]
    );
    // Every word must be there.
    assert_eq!(find(&store, "roadmap thursday "), ["Re: Q4 roadmap"]);
    assert!(find(&store, "roadmap zebra ").is_empty());
}

#[test]
fn case_and_accents_do_not_matter() {
    let store = store();
    assert_eq!(
        find(&store, "ROADMAP q4 "),
        ["Re: Q4 roadmap", "Q4 roadmap"]
    );
    assert_eq!(find(&store, "zoe "), ["Rechnung für Oktober"]);
    assert_eq!(find(&store, "Müller "), ["Rechnung für Oktober"]);
    assert_eq!(find(&store, "fur oktober "), ["Rechnung für Oktober"]);
    // The sharp s is a letter of its own, not an accented one.
    assert_eq!(find(&store, "GRÜSSE "), Vec::<String>::new());
    assert_eq!(find(&store, "gruße "), ["Rechnung für Oktober"]);
}

#[test]
fn the_word_being_typed_matches_what_starts_with_it() {
    let store = store();
    assert_eq!(find(&store, "bre"), ["Offsite agenda"]);
    assert_eq!(find(&store, "road"), find(&store, "roadmap "));
    // Finished with a space, it is a whole word, and "road" is not one.
    assert!(find(&store, "road ").is_empty());
    // A letter at a time, nothing fails.
    let whole =
        "from:ana has:attachment after:2026-11-10 is:unread in:work/inbox \"draft roadmap\"";
    for end in 1..=whole.len() {
        let _ = find(&store, &whole[..end]);
    }
    assert_eq!(find(&store, whole), ["Q4 roadmap"]);
}

#[test]
fn phrases_are_words_together_in_order() {
    let store = store();
    assert_eq!(find(&store, "\"draft roadmap\""), ["Q4 roadmap"]);
    assert!(find(&store, "\"roadmap draft\"").is_empty());
    assert_eq!(find(&store, "\"car park\" water "), ["Trail run Saturday"]);
}

#[test]
fn filters_name_where_a_word_must_be() {
    let store = store();
    assert_eq!(find(&store, "from:ana "), ["Q4 roadmap", "Holiday rota"]);
    // Ana is in the recipients of the reply, not its sender.
    assert_eq!(find(&store, "to:ana "), ["Re: Q4 roadmap"]);
    assert_eq!(
        find(&store, "from:\"Sam Lee\""),
        ["Trail run Saturday", "Offsite agenda"]
    );
    // "roadmap" is in the trail run's body, not its subject.
    assert_eq!(
        find(&store, "subject:roadmap "),
        ["Re: Q4 roadmap", "Q4 roadmap"]
    );
    assert_eq!(
        find(&store, "subject:roadmap from:robin "),
        ["Re: Q4 roadmap"]
    );
}

#[test]
fn states_dates_and_places_narrow_without_any_word() {
    let store = store();
    assert_eq!(find(&store, "is:unread "), ["Q4 roadmap"]);
    assert_eq!(find(&store, "has:attachment "), ["Q4 roadmap"]);
    assert_eq!(find(&store, "is:starred "), ["Rechnung für Oktober"]);
    assert_eq!(find(&store, "is:answered "), ["Rechnung für Oktober"]);
    assert_eq!(find(&store, "is:read is:unstarred ").len(), 4);
    // On or after the 12th; before the 6th.
    assert_eq!(
        find(&store, "after:2026-11-12 "),
        ["Re: Q4 roadmap", "Q4 roadmap", "Trail run Saturday"]
    );
    assert_eq!(find(&store, "before:2026-11-06 "), ["Holiday rota"]);
    assert_eq!(
        find(&store, "after:2026-11-06 before:2026-11-12 "),
        ["Offsite agenda", "Rechnung für Oktober"]
    );
    // An account, a folder by its name or by what it is for, or both.
    assert_eq!(find(&store, "in:personal "), ["Trail run Saturday"]);
    assert_eq!(find(&store, "in:sent "), ["Re: Q4 roadmap"]);
    assert_eq!(find(&store, "in:\"sent items\""), ["Re: Q4 roadmap"]);
    // And by the whole name one of Google's own folders has on Google's
    // servers, which it was shown by once: a search kept since then
    // still finds it.
    assert_eq!(
        find(&store, "in:\"[Gmail]/sent items\""),
        ["Re: Q4 roadmap"]
    );
    assert_eq!(
        find(&store, "in:\"work/[Gmail]/Sent Items\""),
        ["Re: Q4 roadmap"]
    );
    assert!(find(&store, "in:\"[Gmail]/nowhere\"").is_empty());
    assert!(find(&store, "in:\"work/[Gmail]/nowhere\"").is_empty());
    assert_eq!(find(&store, "in:work/inbox roadmap "), ["Q4 roadmap"]);
    assert_eq!(
        find(&store, "in:inbox roadmap "),
        ["Q4 roadmap", "Trail run Saturday"]
    );
    assert!(find(&store, "in:nowhere ").is_empty());
}

#[test]
fn a_body_is_searchable_once_it_is_downloaded_and_mail_that_goes_is_gone() {
    let mut store = store();
    assert_eq!(find(&store, "pencilled "), Vec::<String>::new());
    // Found by its subject before its body is here.
    assert_eq!(find(&store, "rota "), ["Holiday rota"]);
    let inbox = store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.name == "INBOX")
        .unwrap()
        .id;
    let id = store
        .local_message_id(inbox, &RemoteId("unread-body".to_owned()))
        .unwrap()
        .unwrap();
    store
        .put_body(id, "I have pencilled you in for the 27th.")
        .unwrap();
    assert_eq!(find(&store, "pencilled "), ["Holiday rota"]);
    assert_eq!(find(&store, "pencilled rota "), ["Holiday rota"]);
    // The body is replaced: the old words no longer find it.
    store.put_body(id, "Changed my mind.").unwrap();
    assert!(find(&store, "pencilled ").is_empty());
    assert_eq!(find(&store, "\"changed my mind\""), ["Holiday rota"]);

    // A flag changed on the server is seen by the next search.
    let delta = |changed: Vec<FlagChange>, removed: Vec<RemoteId>| SyncDelta {
        reset: false,
        added: Vec::new(),
        changed,
        removed,
        has_more: false,
        next_cursor: SyncCursor("d".to_owned()),
    };
    store
        .apply_delta(
            inbox,
            &delta(
                vec![FlagChange {
                    id: RemoteId("offsite".to_owned()),
                    flags: Flags::default(),
                }],
                Vec::new(),
            ),
        )
        .unwrap();
    assert_eq!(find(&store, "is:unread "), ["Q4 roadmap", "Offsite agenda"]);
    // Removed from the server, it is found no more.
    store
        .apply_delta(
            inbox,
            &delta(Vec::new(), vec![RemoteId("roadmap".to_owned())]),
        )
        .unwrap();
    assert_eq!(
        find(&store, "roadmap "),
        ["Re: Q4 roadmap", "Trail run Saturday"]
    );
}

#[test]
fn nothing_typed_is_read_as_anything_but_words() {
    let store = store();
    // Full-text operators, SQL, quotes and wildcards are all just text.
    for hostile in [
        "roadmap OR zebra ",
        "roadmap NOT agenda ",
        "NEAR(roadmap agenda) ",
        "subject : roadmap ",
        "road* ",
        "\"unterminated",
        "'; DROP TABLE messages; -- ",
        "a\"b\"c ",
        "^roadmap ",
        "{subject}:x ",
        "!!! ",
        "- ",
        "from:'\" ",
        "in:\"work\"/inbox ",
        "%_ ",
    ] {
        let _ = find(&store, hostile);
    }
    // "OR" is a word like any other: all three must be there, so nothing.
    assert!(find(&store, "roadmap OR zebra ").is_empty());
    // And the mail is all still there.
    assert_eq!(find(&store, "roadmap ").len(), 3);
    // Punctuation alone finds nothing; an empty box finds nothing.
    assert!(find(&store, "!!! ").is_empty());
    assert!(find(&store, "").is_empty());
    assert!(find(&store, "   ").is_empty());
}

#[test]
fn a_dash_leaves_mail_out() {
    let store = store();
    assert_eq!(
        find(&store, "roadmap -from:ana "),
        ["Re: Q4 roadmap", "Trail run Saturday"]
    );
    assert_eq!(find(&store, "roadmap -thursday -water "), ["Q4 roadmap"]);
    assert_eq!(
        find(&store, "roadmap -in:work/sent "),
        ["Q4 roadmap", "Trail run Saturday"]
    );
    assert_eq!(
        find(&store, "roadmap -account:work "),
        ["Trail run Saturday"]
    );
    assert_eq!(find(&store, "from:ana -has:attachment "), ["Holiday rota"]);
    assert_eq!(find(&store, "from:ana -is:read "), ["Q4 roadmap"]);
    // Nothing but what to leave out: all the rest of the mail.
    assert_eq!(
        find(&store, "-from:ana -from:sam -from:robin "),
        ["Rechnung für Oktober"]
    );
    // Dashes alone are punctuation, and find nothing.
    assert!(find(&store, "-- ").is_empty());
}

#[test]
fn accounts_ages_sizes_conversations_and_message_ids_are_filters() {
    let store = store();
    assert_eq!(find(&store, "account:personal "), ["Trail run Saturday"]);
    assert_eq!(find(&store, "roadmap account:work ").len(), 2);
    // Unlike `in:`, an account is never taken for a folder.
    assert!(find(&store, "account:inbox ").is_empty());
    // Every message is 100 bytes, and all are newer than a hundred years.
    assert_eq!(find(&store, "larger:99 ").len(), 6);
    assert!(find(&store, "larger:100 ").is_empty());
    assert_eq!(find(&store, "smaller:1k from:sam ").len(), 2);
    assert_eq!(find(&store, "newer:36500d ").len(), 6);
    assert!(find(&store, "older:36500d ").is_empty());
    assert_eq!(
        find(&store, "msgid:<run@example.com> "),
        ["Trail run Saturday"]
    );
    assert_eq!(
        find(&store, "msgid:run@example.com "),
        ["Trail run Saturday"]
    );
    assert!(find(&store, "msgid:nobody@example.com ").is_empty());
    // A message in no conversation is a conversation of one.
    let run = store
        .search(&parse("msgid:run@example.com ", |_, _, _| None), 1)
        .unwrap()[0]
        .id
        .0;
    assert_eq!(
        find(&store, &format!("thread:{run} ")),
        ["Trail run Saturday"]
    );
    assert!(find(&store, "thread:999999 ").is_empty());
}

#[test]
fn the_limit_keeps_the_newest() {
    let store = store();
    let day = |_: i32, _: u32, _: u32| None;
    let two = store.search(&parse("roadmap ", day), 2).unwrap();
    assert_eq!(
        two.iter()
            .map(|row| row.headers.subject.as_str())
            .collect::<Vec<_>>(),
        ["Re: Q4 roadmap", "Q4 roadmap"]
    );
    assert!(store.search(&parse("roadmap ", day), 0).unwrap().is_empty());
}

#[test]
fn the_body_and_the_list_a_message_came_through_are_filters_of_their_own() {
    let mut store = store();
    // "breakouts" is in one message's body and in nothing's subject.
    assert_eq!(find(&store, "body:breakouts "), ["Offsite agenda"]);
    assert!(find(&store, "subject:breakouts ").is_empty());
    assert!(find(&store, "breakouts -body:breakouts ").is_empty());
    // "roadmap" is in a body whose subject does not have it, and
    // `body:` looks only there.
    assert!(find(&store, "body:roadmap ").contains(&"Trail run Saturday".to_owned()));
    assert!(!find(&store, "subject:roadmap ").contains(&"Trail run Saturday".to_owned()));
    // In the body and from someone: both must hold.
    assert_eq!(find(&store, "body:breakouts from:sam "), ["Offsite agenda"]);
    assert!(find(&store, "body:breakouts from:ana ").is_empty());

    // No message has said which list it came through: none is found by
    // one, and every one is found by not being from it.
    assert!(find(&store, "list:builds ").is_empty());
    let everything = find(&store, "after:2026-01-01 ").len();
    assert_eq!(
        find(&store, "after:2026-01-01 -list:builds ").len(),
        everything
    );

    // One has, since it was downloaded: part of the id finds it,
    // whatever the case, and nothing else does.
    let offsite = store
        .search(&parse("subject:offsite ", |_, _, _| None), 1)
        .unwrap()[0]
        .id;
    store
        .set_list_id(offsite, Some("Builds <Builds.Example.com>"))
        .unwrap();
    assert_eq!(find(&store, "list:builds.example.com "), ["Offsite agenda"]);
    assert_eq!(find(&store, "list:BUILDS "), ["Offsite agenda"]);
    assert!(find(&store, "list:announce ").is_empty());
    assert_eq!(
        find(&store, "after:2026-01-01 -list:builds ").len(),
        everything - 1
    );
    // What is typed is a value: nothing in it is a pattern.
    assert!(find(&store, "list:% ").is_empty());

    // Looked for and found to have none, it is through no list.
    store.set_list_id(offsite, None).unwrap();
    assert!(find(&store, "list:builds ").is_empty());
}

#[test]
fn the_rules_are_asked_about_inbox_mail_once() {
    let mut store = store();
    let account = store.accounts().unwrap()[0].id;
    let all = store.inbox_mail(account, i64::MIN, true, 100).unwrap();
    assert!(!all.is_empty());
    // Only the inbox, newest first, and only as far back as asked.
    let rows = store.message_rows(&all).unwrap();
    let inbox = store
        .folders()
        .unwrap()
        .into_iter()
        .find(|folder| folder.account == account && folder.role == FolderRole::Inbox)
        .unwrap()
        .id;
    assert!(rows.iter().all(|row| row.folder == inbox));
    let dates: Vec<i64> = all
        .iter()
        .map(|id| rows.iter().find(|row| row.id == *id).unwrap().headers.date)
        .collect();
    assert!(dates.windows(2).all(|pair| pair[0] >= pair[1]));
    let newest = dates[0];
    assert_eq!(
        store.inbox_mail(account, newest, true, 100).unwrap(),
        all[..1]
    );
    assert_eq!(
        store.inbox_mail(account, i64::MIN, true, 1).unwrap().len(),
        1
    );

    // Asked about, they are not unasked any more, and are still mail.
    store.mark_ruled(&all[..1], 500).unwrap();
    assert_eq!(
        store.inbox_mail(account, i64::MIN, true, 100).unwrap(),
        all[1..]
    );
    assert_eq!(
        store.inbox_mail(account, i64::MIN, false, 100).unwrap(),
        all
    );

    // Which of them are downloaded, and which are worth downloading
    // first.
    let with = store.with_body(&all).unwrap();
    let without = store.small_without_body(&all, 1_000).unwrap();
    assert_eq!(with.len() + without.len(), all.len());
    assert!(with.iter().all(|id| !without.contains(id)));
    // Nothing is small enough for a limit of nothing.
    assert!(store.small_without_body(&all, 0).unwrap().is_empty());
}
