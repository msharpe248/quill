//! A POP3 account from `config.toml` to the store, against a real POP3
//! server: the milestone's acceptance, that repeated polls never duplicate
//! or lose a message under each policy for the server's copies.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use quill_auth::{MemorySecrets, SecretStore};
use quill_core::config::Config;
use quill_core::types::FolderRole;
use quill_store::{BlobStore, Mailbox, Store};
use quill_sync::{SyncEvent, SyncHandle, SyncOptions, SyncRequest};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::accounts;
use crate::pop3_test_server::{Dovecot, PASSWORD, numbered};

struct Rig {
    server: Dovecot,
    user: &'static str,
    store: Store,
    events: UnboundedReceiver<SyncEvent>,
    handle: SyncHandle,
    _dir: tempfile::TempDir,
}

impl Rig {
    /// A server with five messages waiting, and Quill started on an
    /// account that reads them with `policy` for the server's copies.
    async fn start(user: &'static str, policy: &str) -> Option<Self> {
        let server = Dovecot::start().await?;
        server.fill(user, 1, 5);
        let config = Config::parse(&format!(
            "[[account]]\nname = \"Old Mail\"\ntype = \"pop3\"\naddress = \"{user}\"\n{policy}\npop3 = {{ host = \"localhost\", port = {}, tls = \"none\" }}\n",
            server.pop3_port
        ))
        .unwrap();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::default());
        secrets
            .set(&format!("pop3:{user}@localhost"), PASSWORD)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quill.db");
        let blobs = dir.path().join("blobs");
        let mut store = Store::open(&path)
            .unwrap()
            .with_blobs(BlobStore::new(blobs.clone()));
        let prepared = accounts::prepare(&config, &mut store, &secrets, true).unwrap();
        assert_eq!(prepared.accounts.len(), 1, "{:?}", prepared.notices);
        let (events_tx, events) = tokio::sync::mpsc::unbounded_channel();
        let handle = quill_sync::spawn(
            Store::open(&path).unwrap(),
            Some(blobs),
            prepared.accounts,
            SyncOptions {
                interval: Duration::from_secs(3600),
                ..SyncOptions::default()
            },
            events_tx,
        );
        let mut rig = Self {
            server,
            user,
            store,
            events,
            handle,
            _dir: dir,
        };
        rig.pass().await;
        Some(rig)
    }

    /// Waits out the pass in progress. A problem fails the test.
    async fn pass(&mut self) {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(30), self.events.recv())
                .await
                .expect("the engine went quiet")
                .expect("the engine stopped");
            match event {
                SyncEvent::Syncing { active: false, .. } => return,
                SyncEvent::Problem { message, .. } => panic!("{message}"),
                _ => {}
            }
        }
    }

    async fn poll(&mut self) {
        self.handle.send(SyncRequest::SyncNow);
        self.pass().await;
    }

    /// The subjects of everything the account has here, in every folder.
    fn here(&self) -> Vec<String> {
        let mut subjects = Vec::new();
        for folder in self.store.folders().unwrap() {
            subjects.extend(
                self.store
                    .messages(Mailbox::Folder(folder.id))
                    .unwrap()
                    .into_iter()
                    .map(|row| row.headers.subject),
            );
        }
        subjects.sort();
        subjects
    }

    async fn on_server(&self) -> usize {
        self.server.uidls(self.user).await.len()
    }
}

fn subjects(numbers: std::ops::RangeInclusive<usize>) -> Vec<String> {
    let mut subjects: Vec<String> = numbers
        .map(|n| {
            numbered(n)
                .lines()
                .find_map(|line| line.strip_prefix("Subject: "))
                .unwrap()
                .to_owned()
        })
        .collect();
    subjects.sort();
    subjects
}

#[tokio::test(flavor = "multi_thread")]
async fn left_on_the_server_polls_bring_each_message_once_and_whole() {
    let Some(mut rig) = Rig::start("keeper@example.com", "").await else {
        return;
    };
    assert_eq!(rig.here(), subjects(1..=5));
    // Sent, Archive and Trash exist here though the server has only mail.
    let roles: Vec<FolderRole> = rig
        .store
        .folders()
        .unwrap()
        .iter()
        .map(|f| f.role)
        .collect();
    for role in [
        FolderRole::Inbox,
        FolderRole::Sent,
        FolderRole::Archive,
        FolderRole::Trash,
    ] {
        assert!(roles.contains(&role), "{role:?}");
    }
    // Every message was downloaded whole.
    for row in rig.store.messages(Mailbox::Unified).unwrap() {
        let raw = rig.store.raw(row.id).unwrap().expect("downloaded");
        assert!(String::from_utf8_lossy(&raw).contains(&row.headers.subject));
    }

    for round in 0..3 {
        rig.server.fill(rig.user, 6 + round, 1);
        rig.poll().await;
        rig.poll().await;
    }
    assert_eq!(rig.here(), subjects(1..=8));
    assert_eq!(rig.on_server().await, 8);

    // Another program deletes two from the server. They stay here.
    let ids = rig.server.uidls(rig.user).await;
    rig.server
        .delete_elsewhere(rig.user, &[ids[0].as_str(), ids[3].as_str()])
        .await;
    rig.poll().await;
    rig.poll().await;
    assert_eq!(rig.here(), subjects(1..=8));
    assert_eq!(rig.on_server().await, 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_once_downloaded_the_server_empties_and_nothing_is_lost() {
    let Some(mut rig) = Rig::start("tidy@example.com", "delete_from_server = \"downloaded\"").await
    else {
        return;
    };
    assert_eq!(rig.here(), subjects(1..=5));
    assert_eq!(rig.on_server().await, 0);
    for round in 0..3 {
        rig.server.fill(rig.user, 6 + round * 2, 2);
        rig.poll().await;
        rig.poll().await;
    }
    assert_eq!(rig.here(), subjects(1..=11));
    assert_eq!(rig.on_server().await, 0);
    for row in rig.store.messages(Mailbox::Unified).unwrap() {
        assert!(rig.store.raw(row.id).unwrap().is_some());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_after_some_days_new_mail_stays_on_the_server_meanwhile() {
    let Some(mut rig) = Rig::start("patient@example.com", "delete_from_server = 7").await else {
        return;
    };
    for round in 0..2 {
        rig.server.fill(rig.user, 6 + round, 1);
        rig.poll().await;
        rig.poll().await;
    }
    // Nothing is a week old yet.
    assert_eq!(rig.here(), subjects(1..=7));
    assert_eq!(rig.on_server().await, 7);
}
