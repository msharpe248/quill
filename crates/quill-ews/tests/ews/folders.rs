//! Listing folders: names, nesting, pages, and which folder is the inbox.

use quill_core::types::{FolderId, FolderRole, RemoteFolder};
use quill_core::{Backend, Error};
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;

use crate::common::{Rig, answer, failure, fault, soap, success};

/// The folders with a fixed name that are asked for together, in the
/// order they are asked for, and the ids the stand-in gives them.
const STANDARD: [(&str, &str); 5] = [
    ("inbox", "folder-inbox"),
    ("drafts", "folder-drafts"),
    ("sentitems", "folder-sent"),
    ("junkemail", "folder-junk"),
    ("deleteditems", "folder-trash"),
];

/// What asking for folders by fixed name looks like on the wire.
fn by_name(names: &[&str]) -> String {
    let ids: String = names
        .iter()
        .map(|name| format!("<t:DistinguishedFolderId Id=\"{name}\"/>"))
        .collect();
    format!(
        "<m:GetFolder><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape></m:FolderShape>\
         <m:FolderIds>{ids}</m:FolderIds></m:GetFolder>"
    )
}

/// What asking for a page of the folder list looks like on the wire.
fn listing(offset: usize) -> String {
    format!(
        "<m:FindFolder Traversal=\"Deep\"><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape>\
         <t:AdditionalProperties><t:FieldURI FieldURI=\"folder:DisplayName\"/>\
         <t:FieldURI FieldURI=\"folder:ParentFolderId\"/>\
         <t:FieldURI FieldURI=\"folder:FolderClass\"/></t:AdditionalProperties></m:FolderShape>\
         <m:IndexedPageFolderView MaxEntriesReturned=\"256\" Offset=\"{offset}\" \
         BasePoint=\"Beginning\"/><m:ParentFolderIds>\
         <t:DistinguishedFolderId Id=\"msgfolderroot\"/></m:ParentFolderIds></m:FindFolder>"
    )
}

/// `GetFolder`'s answer about one folder.
fn found(id: &str) -> String {
    success(
        "GetFolder",
        &format!(
            "<m:Folders><t:Folder><t:FolderId Id=\"{id}\" ChangeKey=\"AQAAABYAAAB1\"/>\
             </t:Folder></m:Folders>"
        ),
    )
}

fn no_such_folder() -> String {
    failure(
        "GetFolder",
        "ErrorFolderNotFound",
        "The specified folder could not be found in the store.",
    )
}

/// Mounts the answer about the five folders asked for together.
async fn standard(rig: &Rig) {
    let messages: Vec<String> = STANDARD.iter().map(|(_, id)| found(id)).collect();
    rig.on("GetFolder")
        .and(body_string_contains("Id=\"inbox\""))
        .respond_with(answer("GetFolder", &messages))
        .mount(&rig.server)
        .await;
}

/// Mounts the answer about the archive folder.
async fn archive(rig: &Rig, answer: ResponseTemplate) {
    rig.on("GetFolder")
        .and(body_string_contains("Id=\"archive\""))
        .respond_with(answer)
        .mount(&rig.server)
        .await;
}

/// A folder in a list, of the kind `element` and the class `class`.
fn folder(element: &str, id: &str, parent: &str, class: &str, name: &str) -> String {
    format!(
        "<t:{element}><t:FolderId Id=\"{id}\" ChangeKey=\"AQAAABYAAAB1\"/>\
         <t:ParentFolderId Id=\"{parent}\" ChangeKey=\"AQAAAA==\"/>\
         <t:FolderClass>{class}</t:FolderClass><t:DisplayName>{name}</t:DisplayName>\
         </t:{element}>"
    )
}

fn mail(id: &str, parent: &str, name: &str) -> String {
    folder("Folder", id, parent, "IPF.Note", name)
}

/// One page of `FindFolder`'s answer.
fn page(folders: &[String], next: usize, last: bool) -> ResponseTemplate {
    answer(
        "FindFolder",
        &[success(
            "FindFolder",
            &format!(
                "<m:RootFolder IndexedPagingOffset=\"{next}\" TotalItemsInView=\"7\" \
                 IncludesLastItemInRange=\"{last}\"><t:Folders>{}</t:Folders></m:RootFolder>",
                folders.concat()
            ),
        )],
    )
}

fn remote(id: &str, name: &str, role: FolderRole) -> RemoteFolder {
    RemoteFolder {
        id: FolderId(id.to_owned()),
        name: name.to_owned(),
        role,
    }
}

#[tokio::test]
async fn mail_folders_are_listed_with_their_paths_and_what_they_are_for() {
    let rig = Rig::start().await;
    standard(&rig).await;
    archive(&rig, answer("GetFolder", &[found("folder-archive")])).await;
    // In pages, and in no order a reader could rely on.
    rig.on("FindFolder")
        .and(body_string_contains("Offset=\"0\""))
        .respond_with(page(
            &[
                mail("folder-inbox", "folder-top", "Posteingang"),
                folder(
                    "CalendarFolder",
                    "folder-calendar",
                    "folder-top",
                    "IPF.Appointment",
                    "Kalender",
                ),
                mail("folder-2026", "folder-projects", "2026"),
            ],
            3,
            false,
        ))
        .mount(&rig.server)
        .await;
    rig.on("FindFolder")
        .and(body_string_contains("Offset=\"3\""))
        .respond_with(page(
            &[
                mail("folder-projects", "folder-inbox", "Projekte"),
                mail("folder-sent", "folder-top", "Gesendete Elemente"),
                folder(
                    "ContactsFolder",
                    "folder-contacts",
                    "folder-top",
                    "IPF.Contact",
                    "Kontakte",
                ),
                folder(
                    "SearchFolder",
                    "folder-unread",
                    "folder-top",
                    "IPF.Note",
                    "Ungelesen",
                ),
            ],
            7,
            false,
        ))
        .mount(&rig.server)
        .await;
    rig.on("FindFolder")
        .and(body_string_contains("Offset=\"7\""))
        .respond_with(page(
            &[
                mail("folder-drafts", "folder-top", "Entw&#xFC;rfe"),
                mail("folder-trash", "folder-top", "Gel&#xF6;schte Elemente"),
                mail("folder-junk", "folder-top", "Junk-E-Mail"),
                mail("folder-archive", "folder-top", "Archiv"),
                folder(
                    "Folder",
                    "folder-settings",
                    "folder-top",
                    "IPF.Configuration",
                    "Quick Step Settings",
                ),
                // A folder some program made without saying what for.
                "<t:Folder><t:FolderId Id=\"folder-plain\"/>\
                 <t:ParentFolderId Id=\"folder-top\"/>\
                 <t:DisplayName>Fish &amp; Chips</t:DisplayName></t:Folder>"
                    .to_owned(),
            ],
            13,
            true,
        ))
        .mount(&rig.server)
        .await;

    let folders = rig.ews.list_folders().await.unwrap();
    // Known for what they are by id, whatever the mailbox's language.
    assert_eq!(
        folders,
        [
            remote("folder-inbox", "Posteingang", FolderRole::Inbox),
            remote("folder-projects", "Posteingang/Projekte", FolderRole::Other),
            remote(
                "folder-2026",
                "Posteingang/Projekte/2026",
                FolderRole::Other
            ),
            remote("folder-sent", "Gesendete Elemente", FolderRole::Sent),
            remote("folder-drafts", "Entwürfe", FolderRole::Drafts),
            remote("folder-trash", "Gelöschte Elemente", FolderRole::Trash),
            remote("folder-junk", "Junk-E-Mail", FolderRole::Junk),
            remote("folder-archive", "Archiv", FolderRole::Archive),
            remote("folder-plain", "Fish & Chips", FolderRole::Other),
        ]
    );
    assert_eq!(
        rig.check().await,
        [
            by_name(&["inbox", "drafts", "sentitems", "junkemail", "deleteditems"]),
            by_name(&["archive"]),
            listing(0),
            listing(3),
            listing(7),
        ]
    );
}

#[tokio::test]
async fn the_standard_folders_are_asked_for_once_and_a_missing_one_again() {
    let rig = Rig::start().await;
    // No junk folder, and no archive folder yet.
    rig.on("GetFolder")
        .and(body_string_contains("Id=\"inbox\""))
        .respond_with(answer(
            "GetFolder",
            &[
                found("folder-inbox"),
                found("folder-drafts"),
                found("folder-sent"),
                no_such_folder(),
                found("folder-trash"),
            ],
        ))
        .mount(&rig.server)
        .await;
    rig.on("GetFolder")
        .and(body_string_contains(
            "<m:FolderIds><t:DistinguishedFolderId Id=\"junkemail\"/></m:FolderIds>",
        ))
        .respond_with(answer("GetFolder", &[found("folder-junk")]))
        .mount(&rig.server)
        .await;
    archive(&rig, answer("GetFolder", &[no_such_folder()])).await;
    rig.answers(
        "FindFolder",
        page(
            &[
                mail("folder-inbox", "folder-top", "Inbox"),
                mail("folder-junk", "folder-top", "Junk Email"),
                mail("folder-trash", "folder-top", "Deleted Items"),
            ],
            3,
            true,
        ),
    )
    .await;

    let first = rig.ews.list_folders().await.unwrap();
    assert_eq!(
        first,
        [
            remote("folder-inbox", "Inbox", FolderRole::Inbox),
            remote("folder-junk", "Junk Email", FolderRole::Other),
            remote("folder-trash", "Deleted Items", FolderRole::Trash),
        ]
    );
    assert_eq!(rig.requests().await.len(), 3);

    // Now it is there, and is found; the archive folder is still looked
    // for, and nothing that was found is asked about again.
    let second = rig.ews.list_folders().await.unwrap();
    assert_eq!(second[1].role, FolderRole::Junk);
    let third = rig.ews.list_folders().await.unwrap();
    assert_eq!(third, second);
    let requests = rig.check().await;
    assert_eq!(
        requests[3..],
        [
            by_name(&["junkemail"]),
            by_name(&["archive"]),
            listing(0),
            by_name(&["archive"]),
            listing(0),
        ]
    );
}

#[tokio::test]
async fn a_server_that_has_never_heard_of_an_archive_folder_is_asked_once() {
    for refusal in [
        // The name is not in the server's schema at all.
        fault(
            "ErrorSchemaValidation",
            "The request failed schema validation: The 'Id' attribute is invalid - The value \
             'archive' is invalid according to its datatype.",
            None,
        ),
        // Or it is, and the server will not have it asked for this way.
        answer(
            "GetFolder",
            &[failure(
                "GetFolder",
                "ErrorInvalidOperation",
                "The distinguished folder name is unrecognized.",
            )],
        ),
        soap(400, ""),
    ] {
        let rig = Rig::start().await;
        standard(&rig).await;
        archive(&rig, refusal).await;
        rig.answers(
            "FindFolder",
            page(&[mail("folder-inbox", "folder-top", "Inbox")], 1, true),
        )
        .await;

        let folders = rig.ews.list_folders().await.unwrap();
        assert_eq!(
            folders,
            [remote("folder-inbox", "Inbox", FolderRole::Inbox)]
        );
        assert_eq!(rig.ews.list_folders().await.unwrap(), folders);
        assert_eq!(
            rig.check().await,
            [
                by_name(&["inbox", "drafts", "sentitems", "junkemail", "deleteditems"]),
                by_name(&["archive"]),
                listing(0),
                listing(0),
            ]
        );
    }
}

#[tokio::test]
async fn a_folder_list_that_does_not_move_on_is_given_up() {
    let rig = Rig::start().await;
    standard(&rig).await;
    archive(&rig, answer("GetFolder", &[no_such_folder()])).await;
    // "There is more", from the same place, for ever.
    rig.answers(
        "FindFolder",
        page(&[mail("folder-inbox", "folder-top", "Inbox")], 0, false),
    )
    .await;

    let err = rig.ews.list_folders().await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("does not end"), "{err}");
    assert_eq!(rig.check().await.len(), 3);
}

#[tokio::test]
async fn a_folder_list_that_is_not_what_was_asked_for_is_a_backend_error() {
    for body in [
        // Two answers to one question, and none.
        answer(
            "FindFolder",
            &[
                success("FindFolder", "<m:RootFolder/>"),
                success("FindFolder", "<m:RootFolder/>"),
            ],
        ),
        answer("FindFolder", &[]),
        // A success with nothing in it.
        answer("FindFolder", &[success("FindFolder", "")]),
        soap(200, "<m:FindFolderResponse/>"),
        ResponseTemplate::new(200).set_body_raw("this is not XML", "text/xml"),
        ResponseTemplate::new(200),
    ] {
        let rig = Rig::start().await;
        standard(&rig).await;
        archive(&rig, answer("GetFolder", &[no_such_folder()])).await;
        rig.answers("FindFolder", body).await;

        let err = rig.ews.list_folders().await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{err:?}");
        rig.check().await;
    }
}

#[tokio::test]
async fn standard_folders_that_cannot_be_asked_about_fail_the_listing() {
    for refusal in [
        answer(
            "GetFolder",
            &[
                found("folder-inbox"),
                failure(
                    "GetFolder",
                    "ErrorAccessDenied",
                    "Access is denied. Check credentials and try again.",
                ),
                found("folder-sent"),
                found("folder-junk"),
                found("folder-trash"),
            ],
        ),
        // Fewer answers than questions: which folder is which?
        answer("GetFolder", &[found("folder-inbox")]),
    ] {
        let rig = Rig::start().await;
        rig.answers("GetFolder", refusal).await;

        // Half a list of roles is not used: a folder would lose its place
        // in the sidebar for no reason anyone could see.
        let err = rig.ews.list_folders().await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{err:?}");
        assert_eq!(rig.check().await.len(), 1);
    }
}
