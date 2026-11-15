//! The demo mailbox behind `quill tui --demo`.
//!
//! It travels the real path: the fixture [`Backend`] is synced into a store
//! and the UI reads the store, exactly as it will for a real account.

use quill_core::Backend;
use quill_core::fixture::{DemoAccount, FixtureBackend};
use quill_store::{Store, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error(transparent)]
    Backend(#[from] quill_core::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Fills `store` with the demo accounts, with message dates counted back from
/// `now` (seconds since the Unix epoch). Running it twice changes nothing.
pub async fn seed(store: &mut Store, now: i64) -> Result<(), DemoError> {
    for account in DemoAccount::ALL {
        let backend = FixtureBackend::demo(account, now);
        let account_id = store.upsert_account(account.name(), &account.owner().email, "demo")?;
        for folder in backend.list_folders().await? {
            let folder_id = store.upsert_folder(account_id, &folder)?;
            let cursor = store.sync_cursor(folder_id)?;
            let delta = backend.sync_folder(&folder.id, cursor).await?;
            store.apply_delta(folder_id, &delta)?;
            for message in &delta.added {
                let raw = backend.fetch_raw(&message.id).await?;
                if let Some(local) = store.local_message_id(folder_id, &message.id)? {
                    store.put_body(local, &plain_body(&raw, message.headers.date))?;
                }
            }
        }
    }
    Ok(())
}

/// The message's text, read the way real mail is.
fn plain_body(raw: &[u8], received: i64) -> String {
    quill_mime::parse(raw, received).map_or_else(String::new, |parsed| parsed.body)
}

#[cfg(test)]
mod tests {
    use quill_store::Mailbox;

    use super::*;

    const NOW: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z

    #[tokio::test]
    async fn seeding_fills_both_accounts_with_bodies() {
        let mut store = Store::open_in_memory().unwrap();
        seed(&mut store, NOW).await.unwrap();
        let accounts = store.accounts().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].name, "Work");

        let unified = store.messages(Mailbox::Unified).unwrap();
        assert_eq!(unified.len(), 17);
        assert_eq!(unified.iter().filter(|row| !row.flags.seen).count(), 12);
        assert_eq!(unified[0].headers.subject, "Q4 roadmap");
        for row in &unified {
            let body = store.body(row.id).unwrap().unwrap();
            assert!(
                !body.is_empty() && !body.contains('\r'),
                "{}",
                row.headers.subject
            );
        }
        let body = store.body(unified[0].id).unwrap().unwrap();
        assert!(body.starts_with("Hi Robin,\n\nHere is the draft roadmap"));
    }

    #[tokio::test]
    async fn seeding_twice_changes_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        seed(&mut store, NOW).await.unwrap();
        let before = store.messages(Mailbox::Unified).unwrap();
        let folders = store.folders().unwrap();
        seed(&mut store, NOW).await.unwrap();
        assert_eq!(store.messages(Mailbox::Unified).unwrap(), before);
        assert_eq!(store.folders().unwrap(), folders);
    }

    #[test]
    fn bodies_go_through_the_mime_parser() {
        assert_eq!(
            plain_body(b"A: 1\r\nB: 2\r\n\r\none\r\n\r\ntwo\r\n", NOW),
            "one\n\ntwo\n"
        );
        // Decoded, which the split at a blank line this replaces never did.
        assert_eq!(
            plain_body(
                b"Content-Transfer-Encoding: quoted-printable\r\n\r\nTom=C3=A1s\r\n",
                NOW
            ),
            "Tom\u{e1}s\n"
        );
        assert_eq!(plain_body(b"", NOW), "");
    }
}
