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
