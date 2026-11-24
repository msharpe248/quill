//! Listing folders: every level, with what each folder is for.

use quill_core::types::{FolderId, FolderRole, RemoteFolder};
use quill_core::{Backend, Error};
use serde_json::{Value, json};
use wiremock::MockServer;
use wiremock::matchers::{query_param, query_param_is_missing};

use crate::common::{Rig, ok, refusal};

const LISTING: &str = "$select=id,displayName,childFolderCount&$top=100";

fn folder(id: &str, name: &str, children: u32) -> Value {
    json!({
        "id": id,
        "displayName": name,
        "childFolderCount": children
    })
}

fn listed(id: &str, name: &str, role: FolderRole) -> RemoteFolder {
    RemoteFolder {
        id: FolderId(id.to_owned()),
        name: name.to_owned(),
        role,
    }
}

/// Answers for the folders Graph knows by a fixed name. `archive` is left
/// out by the tests whose mailbox has none.
async fn well_known(rig: &Rig, names: &[(&str, &str)]) {
    for (name, id) in names {
        rig.on("GET", &format!("/me/mailFolders/{name}"))
            .respond_with(ok(json!({
                "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users('robin%40example.com')/mailFolders(id)/$entity",
                "id": id
            })))
            .mount(&rig.server)
            .await;
    }
}

const STANDARD: [(&str, &str); 6] = [
    ("inbox", "folder-inbox"),
    ("drafts", "folder-drafts"),
    ("sentitems", "folder-sent"),
    ("archive", "folder-archive"),
    ("junkemail", "folder-junk"),
    ("deleteditems", "folder-trash"),
];

#[tokio::test]
async fn every_folder_is_listed_with_its_children_and_what_it_is_for() {
    let rig = Rig::start().await;
    well_known(&rig, &STANDARD).await;
    // The top level comes in two pages.
    rig.on("GET", "/me/mailFolders")
        .and(query_param_is_missing("$skip"))
        .respond_with(ok(json!({
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users('robin%40example.com')/mailFolders(id,displayName,childFolderCount)",
            "value": [
                folder("folder-inbox", "Posteingang", 2),
                folder("folder-drafts", "Entwürfe", 0),
                folder("folder-sent", "Gesendete Elemente", 0),
            ],
            "@odata.nextLink": rig.link(&format!("/me/mailFolders?{LISTING}&$skip=3")),
        })))
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/mailFolders")
        .and(query_param("$skip", "3"))
        .respond_with(ok(json!({
            "value": [
                folder("folder-trash", "Gelöschte Elemente", 0),
                folder("folder-junk", "Junk-E-Mail", 0),
                folder("folder-archive", "Archiv", 0),
                folder("folder-receipts", "Receipts 2026", 0),
            ]
        })))
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/mailFolders/folder-inbox/childFolders")
        .respond_with(ok(json!({
            "value": [
                folder("folder-projects", "Projects", 1),
                folder("folder-travel", "Travel", 0),
            ]
        })))
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/mailFolders/folder-projects/childFolders")
        .respond_with(ok(json!({
            "value": [folder("folder-harbour", "Harbour", 0)]
        })))
        .mount(&rig.server)
        .await;

    let folders = rig.graph.list_folders().await.unwrap();

    // The roles come from the fixed names, not from what the folders are
    // called: this mailbox is in German.
    assert_eq!(
        folders,
        [
            listed("folder-inbox", "Posteingang", FolderRole::Inbox),
            listed("folder-projects", "Posteingang/Projects", FolderRole::Other),
            listed(
                "folder-harbour",
                "Posteingang/Projects/Harbour",
                FolderRole::Other
            ),
            listed("folder-travel", "Posteingang/Travel", FolderRole::Other),
            listed("folder-drafts", "Entwürfe", FolderRole::Drafts),
            listed("folder-sent", "Gesendete Elemente", FolderRole::Sent),
            listed("folder-trash", "Gelöschte Elemente", FolderRole::Trash),
            listed("folder-junk", "Junk-E-Mail", FolderRole::Junk),
            listed("folder-archive", "Archiv", FolderRole::Archive),
            listed("folder-receipts", "Receipts 2026", FolderRole::Other),
        ]
    );
    assert_eq!(
        rig.check().await,
        [
            "GET /v1.0/me/mailFolders/inbox?$select=id".to_owned(),
            "GET /v1.0/me/mailFolders/drafts?$select=id".to_owned(),
            "GET /v1.0/me/mailFolders/sentitems?$select=id".to_owned(),
            "GET /v1.0/me/mailFolders/archive?$select=id".to_owned(),
            "GET /v1.0/me/mailFolders/junkemail?$select=id".to_owned(),
            "GET /v1.0/me/mailFolders/deleteditems?$select=id".to_owned(),
            // A level is read to its end before the folders inside it.
            format!("GET /v1.0/me/mailFolders?{LISTING}"),
            format!("GET /v1.0/me/mailFolders?{LISTING}&$skip=3"),
            format!("GET /v1.0/me/mailFolders/folder-inbox/childFolders?{LISTING}"),
            format!("GET /v1.0/me/mailFolders/folder-projects/childFolders?{LISTING}"),
        ]
    );
}

#[tokio::test]
async fn the_standard_folders_are_asked_for_once_and_a_missing_one_again() {
    let rig = Rig::start().await;
    // No archive folder yet: Graph makes it when it is first used.
    well_known(&rig, &STANDARD[..3]).await;
    well_known(&rig, &STANDARD[4..]).await;
    rig.on("GET", "/me/mailFolders/archive")
        .respond_with(refusal(
            404,
            "ErrorFolderNotFound",
            "The specified folder could not be found in the store.",
        ))
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/mailFolders")
        .respond_with(ok(json!({
            "value": [
                folder("folder-inbox", "Inbox", 0),
                folder("folder-trash", "Deleted Items", 0),
            ]
        })))
        .mount(&rig.server)
        .await;

    let first = rig.graph.list_folders().await.unwrap();
    assert_eq!(
        first,
        [
            listed("folder-inbox", "Inbox", FolderRole::Inbox),
            listed("folder-trash", "Deleted Items", FolderRole::Trash),
        ]
    );
    assert_eq!(rig.requests().await.len(), 7);

    let second = rig.graph.list_folders().await.unwrap();
    assert_eq!(second, first);
    let requests = rig.check().await;
    assert_eq!(
        requests[7..],
        [
            "GET /v1.0/me/mailFolders/archive?$select=id".to_owned(),
            format!("GET /v1.0/me/mailFolders?{LISTING}"),
        ]
    );
}

#[tokio::test]
async fn a_folder_that_vanished_while_listing_and_one_listed_twice_do_no_harm() {
    let rig = Rig::start().await;
    well_known(&rig, &STANDARD).await;
    rig.on("GET", "/me/mailFolders")
        .respond_with(ok(json!({
            "value": [
                folder("folder-inbox", "Inbox", 1),
                folder("folder-inbox", "Inbox", 1),
                // No name: shown by its id sooner than not at all.
                {"id": "folder-odd", "childFolderCount": 0},
            ]
        })))
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/mailFolders/folder-inbox/childFolders")
        .respond_with(refusal(
            404,
            "ErrorItemNotFound",
            "The specified object was not found in the store.",
        ))
        .mount(&rig.server)
        .await;

    assert_eq!(
        rig.graph.list_folders().await.unwrap(),
        [
            listed("folder-inbox", "Inbox", FolderRole::Inbox),
            listed("folder-odd", "folder-odd", FolderRole::Other),
        ]
    );
    assert_eq!(rig.check().await.len(), 8);
}

#[tokio::test]
async fn a_link_to_another_server_is_not_followed() {
    let rig = Rig::start().await;
    let elsewhere = MockServer::start().await;
    well_known(&rig, &STANDARD).await;
    rig.on("GET", "/me/mailFolders")
        .respond_with(ok(json!({
            "value": [folder("folder-inbox", "Inbox", 0)],
            "@odata.nextLink": format!("{}/v1.0/me/mailFolders?$skip=1", elsewhere.uri()),
        })))
        .mount(&rig.server)
        .await;

    let err = rig.graph.list_folders().await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("another server"), "{err}");
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
    rig.check().await;
}

#[tokio::test]
async fn a_folder_list_that_is_not_what_was_asked_for_is_a_backend_error() {
    for body in [
        "this is not JSON",
        "{\"value\": \"not a list\"}",
        "{\"folders\": []}",
        "{\"value\": [{\"displayName\": \"No id\"}]}",
        "{\"value\": [{\"id\": \"f\", \"childFolderCount\": \"two\"}]}",
        "",
    ] {
        let rig = Rig::start().await;
        well_known(&rig, &STANDARD).await;
        rig.on("GET", "/me/mailFolders")
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_raw(body, "application/json"),
            )
            .mount(&rig.server)
            .await;

        let err = rig.graph.list_folders().await.unwrap_err();
        assert!(matches!(err, Error::Backend(_)), "{body:?}: {err:?}");
        rig.check().await;
    }
}

#[tokio::test]
async fn a_standard_folder_that_cannot_be_asked_about_fails_the_listing() {
    let rig = Rig::start().await;
    rig.on("GET", "/me/mailFolders/inbox")
        .respond_with(refusal(
            403,
            "ErrorAccessDenied",
            "Access is denied. Check credentials and try again.",
        ))
        .mount(&rig.server)
        .await;

    // Half a list of roles is not used: a folder would lose its place in
    // the sidebar for no reason anyone could see.
    let err = rig.graph.list_folders().await.unwrap_err();
    assert!(matches!(err, Error::Backend(_)), "{err:?}");
    assert!(err.to_string().contains("ErrorAccessDenied"), "{err}");
    assert_eq!(rig.check().await.len(), 1);
}

#[tokio::test]
async fn a_folder_is_made_at_the_top_of_the_mailbox_under_the_name_given() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/mailFolders")
        .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
            "id": "folder-receipts",
            "displayName": "Receipts",
            "childFolderCount": 0
        })))
        .mount(&rig.server)
        .await;

    let made = rig.graph.create_folder("Receipts").await.unwrap();
    assert_eq!(
        made,
        listed("folder-receipts", "Receipts", FolderRole::Other)
    );
    let requests = rig.requests().await;
    assert_eq!(rig.check().await, ["POST /v1.0/me/mailFolders"]);
    let asked: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(asked, json!({ "displayName": "Receipts" }));
}

#[tokio::test]
async fn a_folder_the_service_will_not_make_is_refused_in_its_words() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/mailFolders")
        .respond_with(refusal(
            409,
            "ErrorFolderExists",
            "A folder with the specified name already exists.",
        ))
        .mount(&rig.server)
        .await;

    let err = rig.graph.create_folder("Receipts").await.unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    assert_eq!(rig.check().await.len(), 1);
}

/// A small mailbox: the standard folders, and `Projects` with `Harbour`
/// inside it, and `Receipts`.
async fn small_mailbox(rig: &Rig) {
    well_known(rig, &STANDARD).await;
    rig.on("GET", "/me/mailFolders")
        .respond_with(ok(json!({
            "value": [
                folder("folder-inbox", "Inbox", 0),
                folder("folder-trash", "Deleted Items", 0),
                folder("folder-projects", "Projects", 1),
                folder("folder-receipts", "Receipts", 0),
            ]
        })))
        .mount(&rig.server)
        .await;
    rig.on("GET", "/me/mailFolders/folder-projects/childFolders")
        .respond_with(ok(json!({
            "value": [folder("folder-harbour", "Harbour", 0)]
        })))
        .mount(&rig.server)
        .await;
}

fn made(id: &str, name: &str) -> wiremock::ResponseTemplate {
    wiremock::ResponseTemplate::new(201).set_body_json(folder(id, name, 0))
}

/// The requests that changed something, with what each one said.
async fn changes(rig: &Rig) -> Vec<(String, Value)> {
    rig.requests()
        .await
        .iter()
        .filter(|request| request.method.as_str() != "GET")
        .map(|request| {
            (
                format!("{} {}", request.method, request.url.path()),
                serde_json::from_slice(&request.body).unwrap_or(Value::Null),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_folder_is_made_inside_another_and_the_one_it_is_inside_too() {
    let rig = Rig::start().await;
    small_mailbox(&rig).await;
    rig.on("POST", "/me/mailFolders/folder-projects/childFolders")
        .respond_with(made("folder-year", "2026"))
        .mount(&rig.server)
        .await;
    rig.on("POST", "/me/mailFolders")
        .respond_with(made("folder-travel", "Travel"))
        .mount(&rig.server)
        .await;
    rig.on("POST", "/me/mailFolders/folder-travel/childFolders")
        .respond_with(made("folder-lisbon", "Lisbon"))
        .mount(&rig.server)
        .await;

    // Inside one that is there, whatever capitals it was typed in.
    let year = rig.graph.create_folder("projects / 2026").await.unwrap();
    assert_eq!(
        year,
        listed("folder-year", "Projects/2026", FolderRole::Other)
    );
    // Inside one that is not: that one is made first.
    let lisbon = rig.graph.create_folder("Travel/Lisbon").await.unwrap();
    assert_eq!(
        lisbon,
        listed("folder-lisbon", "Travel/Lisbon", FolderRole::Other)
    );
    assert_eq!(
        changes(&rig).await,
        [
            (
                "POST /v1.0/me/mailFolders/folder-projects/childFolders".to_owned(),
                json!({ "displayName": "2026" })
            ),
            (
                "POST /v1.0/me/mailFolders".to_owned(),
                json!({ "displayName": "Travel" })
            ),
            (
                "POST /v1.0/me/mailFolders/folder-travel/childFolders".to_owned(),
                json!({ "displayName": "Lisbon" })
            ),
        ]
    );

    // A name with nothing on one side of its slash asks the service for
    // nothing.
    assert!(rig.graph.create_folder("Travel/").await.is_err());
    assert_eq!(changes(&rig).await.len(), 3);
    rig.check().await;
}

#[tokio::test]
async fn a_folder_is_renamed_where_it_is_and_the_folders_inside_it_are_named_after_it() {
    let rig = Rig::start().await;
    small_mailbox(&rig).await;
    rig.on("PATCH", "/me/mailFolders/folder-projects")
        .respond_with(ok(folder("folder-projects", "Work", 1)))
        .mount(&rig.server)
        .await;
    let projects = FolderId("folder-projects".to_owned());

    let renamed = rig.graph.rename_folder(&projects, "Work").await.unwrap();
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
    // The service knows them by what it knew them by.
    assert!(renamed.iter().all(|renamed| renamed.from == renamed.to.id));
    assert_eq!(
        changes(&rig).await,
        [(
            "PATCH /v1.0/me/mailFolders/folder-projects".to_owned(),
            json!({ "displayName": "Work" })
        )]
    );

    // The name it has changes nothing; one that is taken, or that would
    // put it inside another folder, is refused; and none of them asks the
    // service to change anything.
    assert_eq!(
        rig.graph
            .rename_folder(&projects, "Projects")
            .await
            .unwrap(),
        []
    );
    let taken = rig.graph.rename_folder(&projects, "receipts").await;
    assert!(taken.unwrap_err().to_string().contains("already a folder"));
    let harbour = FolderId("folder-harbour".to_owned());
    let moved = rig.graph.rename_folder(&harbour, "Receipts/Harbour").await;
    assert!(matches!(moved, Err(Error::Unsupported(_))), "{moved:?}");
    let gone = rig
        .graph
        .rename_folder(&FolderId("folder-nowhere".to_owned()), "Somewhere")
        .await;
    assert!(matches!(gone, Err(Error::NotFound(_))), "{gone:?}");
    assert_eq!(changes(&rig).await.len(), 1);
    rig.check().await;
}

#[tokio::test]
async fn a_folder_is_deleted_by_moving_it_to_deleted_items() {
    let rig = Rig::start().await;
    rig.on("POST", "/me/mailFolders/folder-receipts/move")
        .respond_with(made("folder-receipts-moved", "Receipts"))
        .mount(&rig.server)
        .await;
    rig.on("POST", "/me/mailFolders/folder-gone/move")
        .respond_with(refusal(
            404,
            "ErrorItemNotFound",
            "The specified object was not found in the store.",
        ))
        .mount(&rig.server)
        .await;
    rig.on("POST", "/me/mailFolders/folder-kept/move")
        .respond_with(refusal(
            409,
            "ErrorFolderExists",
            "A folder with the specified name already exists.",
        ))
        .mount(&rig.server)
        .await;
    let delete = |id: &str| {
        let id = FolderId(id.to_owned());
        let graph = &rig.graph;
        async move { graph.delete_folder(&id, None).await }
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
    // What the service will not do is said in its words.
    let refused = delete("folder-kept").await.unwrap_err();
    assert!(refused.to_string().contains("already exists"), "{refused}");
    // Never a delete: only ever a move into Deleted Items.
    let asked = changes(&rig).await;
    assert_eq!(asked.len(), 3);
    for (line, body) in asked {
        assert!(
            line.starts_with("POST ") && line.ends_with("/move"),
            "{line}"
        );
        assert_eq!(body, json!({ "destinationId": "deleteditems" }));
    }
    rig.check().await;
}
