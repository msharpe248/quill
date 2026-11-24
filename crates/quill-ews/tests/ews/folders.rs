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

/// A small mailbox: the standard folders, `Projects` with `Harbour`
/// inside it, and `Receipts`.
async fn small_mailbox(rig: &Rig) {
    standard(rig).await;
    archive(rig, answer("GetFolder", &[no_such_folder()])).await;
    rig.answers(
        "FindFolder",
        page(
            &[
                mail("folder-inbox", "folder-top", "Inbox"),
                mail("folder-trash", "folder-top", "Deleted Items"),
                mail("folder-projects", "folder-top", "Projects"),
                mail("folder-harbour", "folder-projects", "Harbour"),
                mail("folder-receipts", "folder-top", "Receipts"),
            ],
            5,
            true,
        ),
    )
    .await;
}

/// `CreateFolder`'s answer: the new folder's id.
fn created(id: &str) -> ResponseTemplate {
    answer(
        "CreateFolder",
        &[success(
            "CreateFolder",
            &format!(
                "<m:Folders><t:Folder><t:FolderId Id=\"{id}\" ChangeKey=\"AQAAAA==\"/>\
                 </t:Folder></m:Folders>"
            ),
        )],
    )
}

/// What asking for a new folder looks like on the wire.
fn creating(parent: &str, name: &str) -> String {
    format!(
        "<m:CreateFolder><m:ParentFolderId>{parent}</m:ParentFolderId><m:Folders><t:Folder>\
         <t:FolderClass>IPF.Note</t:FolderClass><t:DisplayName>{name}</t:DisplayName>\
         </t:Folder></m:Folders></m:CreateFolder>"
    )
}

/// What the test's requests asked, leaving out the looking up.
async fn changes(rig: &Rig) -> Vec<String> {
    rig.check()
        .await
        .into_iter()
        .filter(|asked| {
            ["<m:CreateFolder", "<m:UpdateFolder", "<m:DeleteFolder"]
                .iter()
                .any(|start| asked.starts_with(start))
        })
        .collect()
}

#[tokio::test]
async fn a_folder_is_made_at_the_top_or_inside_another_which_is_made_too() {
    let rig = Rig::start().await;
    small_mailbox(&rig).await;
    rig.on("CreateFolder")
        .and(body_string_contains("<t:DisplayName>Notes</t:DisplayName>"))
        .respond_with(created("folder-notes"))
        .mount(&rig.server)
        .await;
    rig.on("CreateFolder")
        .and(body_string_contains("<t:DisplayName>2026</t:DisplayName>"))
        .respond_with(created("folder-year"))
        .mount(&rig.server)
        .await;
    rig.on("CreateFolder")
        .and(body_string_contains(
            "<t:DisplayName>Travel</t:DisplayName>",
        ))
        .respond_with(created("folder-travel"))
        .mount(&rig.server)
        .await;
    rig.on("CreateFolder")
        .and(body_string_contains(
            "<t:DisplayName>Lisbon</t:DisplayName>",
        ))
        .respond_with(created("folder-lisbon"))
        .mount(&rig.server)
        .await;

    // At the top: nothing is looked up first.
    let notes = rig.ews.create_folder("Notes").await.unwrap();
    assert_eq!(notes, remote("folder-notes", "Notes", FolderRole::Other));
    assert_eq!(rig.requests().await.len(), 1);
    // Inside one that is there, whatever capitals it was typed in.
    let year = rig.ews.create_folder("projects / 2026").await.unwrap();
    assert_eq!(
        year,
        remote("folder-year", "Projects/2026", FolderRole::Other)
    );
    // Inside one that is not: that one is made first.
    let lisbon = rig.ews.create_folder("Travel/Lisbon").await.unwrap();
    assert_eq!(
        lisbon,
        remote("folder-lisbon", "Travel/Lisbon", FolderRole::Other)
    );
    // A name with nothing on one side of its slash asks for nothing.
    assert!(rig.ews.create_folder("Travel/").await.is_err());

    let top = "<t:DistinguishedFolderId Id=\"msgfolderroot\"/>";
    assert_eq!(
        changes(&rig).await,
        [
            creating(top, "Notes"),
            creating("<t:FolderId Id=\"folder-projects\"/>", "2026"),
            creating(top, "Travel"),
            creating("<t:FolderId Id=\"folder-travel\"/>", "Lisbon"),
        ]
    );
}

#[tokio::test]
async fn a_folder_is_renamed_where_it_is_with_the_mark_of_its_last_change() {
    let rig = Rig::start().await;
    small_mailbox(&rig).await;
    rig.on("GetFolder")
        .and(body_string_contains("<t:FolderId Id=\"folder-projects\"/>"))
        .respond_with(answer("GetFolder", &[found("folder-projects")]))
        .mount(&rig.server)
        .await;
    rig.answers(
        "UpdateFolder",
        answer(
            "UpdateFolder",
            &[success(
                "UpdateFolder",
                "<m:Folders><t:Folder><t:FolderId Id=\"folder-projects\" ChangeKey=\"AQAAAB==\"/>\
                 </t:Folder></m:Folders>",
            )],
        ),
    )
    .await;
    let projects = FolderId("folder-projects".to_owned());

    let renamed = rig.ews.rename_folder(&projects, "Work").await.unwrap();
    let pairs: Vec<(&str, &str)> = renamed
        .iter()
        .map(|renamed| (renamed.from.0.as_str(), renamed.to.name.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("folder-projects", "Work"),
            ("folder-harbour", "Work/Harbour")
        ]
    );

    // The name it has changes nothing; one that is taken, or that would
    // put it inside another folder, is refused; and none of them asks
    // for a change.
    assert_eq!(
        rig.ews.rename_folder(&projects, "Projects").await.unwrap(),
        []
    );
    let taken = rig.ews.rename_folder(&projects, "receipts").await;
    assert!(taken.unwrap_err().to_string().contains("already a folder"));
    let harbour = FolderId("folder-harbour".to_owned());
    let moved = rig.ews.rename_folder(&harbour, "Receipts/Harbour").await;
    assert!(matches!(moved, Err(Error::Unsupported(_))), "{moved:?}");

    assert_eq!(
        changes(&rig).await,
        ["<m:UpdateFolder><m:FolderChanges><t:FolderChange>\
          <t:FolderId Id=\"folder-projects\" ChangeKey=\"AQAAABYAAAB1\"/><t:Updates>\
          <t:SetFolderField><t:FieldURI FieldURI=\"folder:DisplayName\"/><t:Folder>\
          <t:DisplayName>Work</t:DisplayName></t:Folder></t:SetFolderField></t:Updates>\
          </t:FolderChange></m:FolderChanges></m:UpdateFolder>"]
    );
}

#[tokio::test]
async fn a_folder_is_deleted_into_deleted_items_and_never_for_good() {
    let rig = Rig::start().await;
    rig.on("DeleteFolder")
        .and(body_string_contains("folder-receipts"))
        .respond_with(answer("DeleteFolder", &[success("DeleteFolder", "")]))
        .mount(&rig.server)
        .await;
    rig.on("DeleteFolder")
        .and(body_string_contains("folder-gone"))
        .respond_with(answer(
            "DeleteFolder",
            &[failure(
                "DeleteFolder",
                "ErrorFolderNotFound",
                "The specified folder could not be found in the store.",
            )],
        ))
        .mount(&rig.server)
        .await;
    rig.on("DeleteFolder")
        .and(body_string_contains("folder-kept"))
        .respond_with(answer(
            "DeleteFolder",
            &[failure(
                "DeleteFolder",
                "ErrorDeleteDistinguishedFolder",
                "Distinguished folders cannot be deleted.",
            )],
        ))
        .mount(&rig.server)
        .await;
    let delete = |id: &str| {
        let id = FolderId(id.to_owned());
        let ews = &rig.ews;
        async move { ews.delete_folder(&id, None).await }
    };

    assert_eq!(
        delete("folder-receipts").await.unwrap(),
        quill_core::types::FolderMail::Trashed
    );
    // Gone already is what was wanted.
    assert_eq!(
        delete("folder-gone").await.unwrap(),
        quill_core::types::FolderMail::None
    );
    // What the server will not do is said in its words.
    let refused = delete("folder-kept").await.unwrap_err();
    assert!(
        refused.to_string().contains("cannot be deleted"),
        "{refused}"
    );

    let asked = changes(&rig).await;
    assert_eq!(asked.len(), 3);
    assert_eq!(
        asked[0],
        "<m:DeleteFolder DeleteType=\"MoveToDeletedItems\"><m:FolderIds>\
         <t:FolderId Id=\"folder-receipts\"/></m:FolderIds></m:DeleteFolder>"
    );
    assert!(
        asked
            .iter()
            .all(|asked| asked.contains("MoveToDeletedItems"))
    );
}
