//! Folders given another name, and folders deleted.
//!
//! A server's folder is renamed and deleted on the server first; what is
//! here is then made to agree, keeping every message's row, so that
//! what was downloaded, snoozed or threaded stays so. A folder made on
//! this Mac is nobody else's, and is renamed here alone.

use quill_core::types::{
    FolderId, FolderRole, LocalAccountId, LocalFolderId, LocalMessageId, RemoteFolder, RemoteId,
};
use rusqlite::{OptionalExtension, params};

use crate::local::{LOCAL_FOLDER, name_problem};
use crate::{Result, Store};

/// Where a folder is and what it is known by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPlace {
    pub account: LocalAccountId,
    /// What its server knows it by, or, for one that is only here, the
    /// store.
    pub remote: String,
    pub name: String,
    pub role: FolderRole,
    /// How many messages are in it here.
    pub messages: usize,
}

impl Store {
    /// A folder's account, names and role, and how much mail is in it.
    pub fn folder_place(&self, folder: LocalFolderId) -> Result<Option<FolderPlace>> {
        Ok(self
            .conn
            .query_row(
                "SELECT f.account_id, f.remote_id, f.name, f.role,
                        (SELECT count(*) FROM messages m WHERE m.folder_id = f.id)
                 FROM folders f WHERE f.id = ?1",
                [folder.0],
                |row| {
                    Ok(FolderPlace {
                        account: LocalAccountId(row.get(0)?),
                        remote: row.get(1)?,
                        name: row.get(2)?,
                        role: FolderRole::from_stored(&row.get::<_, String>(3)?),
                        messages: usize::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                    })
                },
            )
            .optional()?)
    }

    /// The folder of `account` that its server knows as `remote`.
    pub fn folder_known_as(
        &self,
        account: LocalAccountId,
        remote: &str,
    ) -> Result<Option<LocalFolderId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM folders WHERE account_id = ?1 AND remote_id = ?2",
                params![account.0, remote],
                |row| row.get(0),
            )
            .optional()?
            .map(LocalFolderId))
    }

    /// How many folders are inside this one, going by their names: a
    /// folder inside another is named after it, with a `/` between. A
    /// server's folders are inside a server's, and this Mac's inside this
    /// Mac's.
    pub fn folders_inside(&self, folder: LocalFolderId) -> Result<usize> {
        let inside: i64 = self.conn.query_row(
            "SELECT count(*) FROM folders f JOIN folders o ON o.account_id = f.account_id
             WHERE f.id = ?1 AND o.id != f.id
               AND (o.remote_id LIKE 'kept:%') = (f.remote_id LIKE 'kept:%')
               AND substr(o.name, 1, length(f.name) + 1) = f.name || '/'",
            [folder.0],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(inside).unwrap_or(0))
    }

    /// The account's folder for `role`, as its server knows it.
    pub fn folder_for(
        &self,
        account: LocalAccountId,
        role: FolderRole,
    ) -> Result<Option<FolderId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT remote_id FROM folders
                 WHERE account_id = ?1 AND role = ?2 AND remote_id NOT LIKE 'kept:%'
                 ORDER BY id LIMIT 1",
                params![account.0, role.as_str()],
                |row| row.get(0),
            )
            .optional()?
            .map(FolderId))
    }

    /// Whether changes to mail in the folder, or mail on its way into it,
    /// are still waiting to reach the server.
    pub fn folder_busy(&self, folder: LocalFolderId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM operations o
                            WHERE o.source_folder = ?1 OR o.target_folder = ?1
                               OR o.message_id IN (SELECT id FROM messages WHERE folder_id = ?1))",
            [folder.0],
            |row| row.get(0),
        )?)
    }

    /// What each message in a folder is known by on its server.
    pub fn folder_messages(
        &self,
        folder: LocalFolderId,
    ) -> Result<Vec<(LocalMessageId, RemoteId)>> {
        let mut statement = self
            .conn
            .prepare("SELECT id, remote_id FROM messages WHERE folder_id = ?1")?;
        let rows = statement.query_map([folder.0], |row| {
            Ok((LocalMessageId(row.get(0)?), RemoteId(row.get(1)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records that a server's folder is now known as `to`, and its
    /// messages by the names given: all of it or none. A row a sync made
    /// meanwhile for the folder under its new name gives way to this one,
    /// which has the mail.
    pub fn folder_renamed(
        &mut self,
        folder: LocalFolderId,
        to: &RemoteFolder,
        messages: &[(LocalMessageId, RemoteId)],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM folders
             WHERE remote_id = ?2 AND id != ?1
               AND account_id = (SELECT account_id FROM folders WHERE id = ?1)",
            params![folder.0, to.id.0],
        )?;
        tx.execute(
            "UPDATE folders SET remote_id = ?2, name = ?3, role = ?4 WHERE id = ?1",
            params![folder.0, to.id.0, to.name, to.role.as_str()],
        )?;
        {
            let mut rename =
                tx.prepare("UPDATE messages SET remote_id = ?2 WHERE id = ?1 AND folder_id = ?3")?;
            for (message, remote) in messages {
                rename.execute(params![message.0, remote.0, folder.0])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Gives a folder made on this Mac another name. Returns the name it
    /// had, or why it keeps it, in words for the user. The folders inside
    /// it, which are so by their names, are renamed with it.
    pub fn rename_local_folder(
        &mut self,
        folder: LocalFolderId,
        name: &str,
    ) -> Result<std::result::Result<String, String>> {
        let name = name.trim();
        if let Some(problem) = name_problem(name) {
            return Ok(Err(problem));
        }
        let tx = self.conn.transaction()?;
        let found: Option<(i64, String)> = tx
            .query_row(
                "SELECT account_id, name FROM folders
                 WHERE id = ?1 AND remote_id LIKE 'kept:folder:%'",
                [folder.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((account, was)) = found else {
            return Ok(Err("That folder is not one made on this Mac.".to_owned()));
        };
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM folders
                 WHERE account_id = ?1 AND remote_id LIKE 'kept:folder:%'
                   AND name = ?2 COLLATE NOCASE AND id != ?3)",
            params![account, name, folder.0],
            |row| row.get(0),
        )?;
        if taken {
            return Ok(Err(format!(
                "There is already a folder called {name} on this Mac."
            )));
        }
        // The folders inside it first, while its own name still says
        // which they are.
        tx.execute(
            "UPDATE folders
             SET name = ?3 || substr(name, length(?2) + 1),
                 remote_id = ?4 || ?3 || substr(name, length(?2) + 1)
             WHERE account_id = ?1 AND remote_id LIKE 'kept:folder:%'
               AND substr(name, 1, length(?2) + 1) = ?2 || '/'",
            params![account, was, name, LOCAL_FOLDER],
        )?;
        tx.execute(
            "UPDATE folders SET name = ?2, remote_id = ?3 WHERE id = ?1",
            params![folder.0, name, format!("{LOCAL_FOLDER}{name}")],
        )?;
        tx.commit()?;
        Ok(Ok(was))
    }

    /// Takes a server's folder out of the store, with the copies of its
    /// mail kept here: for a folder that has been deleted on its server.
    /// A folder made on this Mac is not taken out this way.
    pub fn forget_folder(&mut self, folder: LocalFolderId) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM folders WHERE id = ?1 AND remote_id NOT LIKE 'kept:%'",
            [folder.0],
        )? > 0)
    }
}

#[cfg(test)]
mod tests {
    use quill_core::types::{Flags, Headers, RemoteMessage, SyncCursor, SyncDelta};

    use super::*;
    use crate::Mailbox;

    fn folder(id: &str, name: &str) -> RemoteFolder {
        RemoteFolder {
            id: FolderId(id.to_owned()),
            name: name.to_owned(),
            role: FolderRole::Other,
        }
    }

    fn with_mail(store: &mut Store, folder: LocalFolderId, ids: &[&str]) {
        let delta = SyncDelta {
            reset: false,
            added: ids
                .iter()
                .map(|id| RemoteMessage {
                    id: RemoteId((*id).to_owned()),
                    headers: Headers {
                        message_id: None,
                        subject: (*id).to_owned(),
                        from: Vec::new(),
                        to: Vec::new(),
                        cc: Vec::new(),
                        date: 1,
                    },
                    flags: Flags::default(),
                    size: 1,
                    has_attachments: false,
                    thread: Vec::new(),
                })
                .collect(),
            changed: Vec::new(),
            removed: Vec::new(),
            has_more: false,
            next_cursor: SyncCursor("v1".to_owned()),
        };
        store.apply_delta(folder, &delta).unwrap();
    }

    fn names(store: &Store) -> Vec<String> {
        store
            .folders()
            .unwrap()
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    #[test]
    fn a_renamed_folder_keeps_its_mail_and_what_was_downloaded() {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let projects = store
            .upsert_folder(account, &folder("Projects", "Projects"))
            .unwrap();
        with_mail(&mut store, projects, &["1;7;Projects", "2;7;Projects"]);
        let first = store
            .local_message_id(projects, &RemoteId("1;7;Projects".to_owned()))
            .unwrap()
            .unwrap();
        store.put_body(first, "the text").unwrap();
        let place = store.folder_place(projects).unwrap().unwrap();
        assert_eq!((place.name.as_str(), place.messages), ("Projects", 2));
        assert_eq!(
            store.folder_known_as(account, "Projects").unwrap(),
            Some(projects)
        );

        // A sync got there first, and made a row for the new name.
        let early = store
            .upsert_folder(account, &folder("Work", "Work"))
            .unwrap();
        let messages: Vec<(LocalMessageId, RemoteId)> = store
            .folder_messages(projects)
            .unwrap()
            .into_iter()
            .map(|(id, remote)| (id, RemoteId(remote.0.replace("Projects", "Work"))))
            .collect();
        store
            .folder_renamed(projects, &folder("Work", "Work"), &messages)
            .unwrap();

        assert_eq!(names(&store), ["Work"]);
        assert_eq!(
            store.folder_known_as(account, "Work").unwrap(),
            Some(projects)
        );
        assert_ne!(early, projects);
        assert_eq!(
            store
                .local_message_id(projects, &RemoteId("1;7;Work".to_owned()))
                .unwrap(),
            Some(first)
        );
        assert_eq!(store.body(first).unwrap().as_deref(), Some("the text"));
        assert_eq!(store.messages(Mailbox::Folder(projects)).unwrap().len(), 2);
    }

    #[test]
    fn folders_are_inside_one_another_by_their_names() {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let projects = store
            .upsert_folder(account, &folder("Projects", "Projects"))
            .unwrap();
        let year = store
            .upsert_folder(account, &folder("Projects.2026", "Projects/2026"))
            .unwrap();
        // Named alike, and not inside it.
        store
            .upsert_folder(account, &folder("Projects2", "Projects2"))
            .unwrap();
        // On this Mac: not inside a server's folder, whatever it is called.
        let here = store
            .create_local_folder(account, "Projects/Kept")
            .unwrap()
            .unwrap();
        assert_eq!(store.folders_inside(projects).unwrap(), 1);
        assert_eq!(store.folders_inside(year).unwrap(), 0);
        assert_eq!(store.folders_inside(here).unwrap(), 0);

        // A server's folder is forgotten with its mail; this Mac's is not
        // forgotten this way at all.
        with_mail(&mut store, year, &["1"]);
        assert!(store.forget_folder(year).unwrap());
        assert!(!store.forget_folder(here).unwrap());
        assert_eq!(names(&store).len(), 3);
        assert_eq!(store.folder_place(year).unwrap(), None);
    }

    #[test]
    fn a_folder_on_this_mac_is_renamed_with_the_folders_inside_it() {
        let mut store = Store::open_in_memory().unwrap();
        let account = store
            .upsert_account("Work", "robin@example.com", "imap")
            .unwrap();
        let kept = store.create_local_folder(account, "Kept").unwrap().unwrap();
        let inside = store
            .create_local_folder(account, "Kept/Tax")
            .unwrap()
            .unwrap();
        let other = store
            .create_local_folder(account, "Other")
            .unwrap()
            .unwrap();
        let server = store
            .upsert_folder(account, &folder("Kept/Server", "Kept/Server"))
            .unwrap();

        assert_eq!(
            store.rename_local_folder(kept, " Papers ").unwrap(),
            Ok("Kept".to_owned())
        );
        let place = |store: &Store, id| store.folder_place(id).unwrap().unwrap();
        assert_eq!(place(&store, kept).name, "Papers");
        assert_eq!(place(&store, kept).remote, "kept:folder:Papers");
        assert_eq!(place(&store, inside).name, "Papers/Tax");
        assert_eq!(place(&store, inside).remote, "kept:folder:Papers/Tax");
        // A server's folder is not this Mac's to rename, by name or at all.
        assert_eq!(place(&store, server).name, "Kept/Server");
        assert!(store.rename_local_folder(server, "Mine").unwrap().is_err());

        // A name that is taken, or is none, is refused with the reason.
        let taken = store.rename_local_folder(other, "papers").unwrap();
        assert!(taken.unwrap_err().contains("already a folder"));
        assert!(store.rename_local_folder(other, "  ").unwrap().is_err());
        assert!(store.rename_local_folder(other, "a//b").unwrap().is_err());
        // Its own name in other capitals is a rename like any other.
        assert_eq!(
            store.rename_local_folder(other, "OTHER").unwrap(),
            Ok("Other".to_owned())
        );
    }
}
