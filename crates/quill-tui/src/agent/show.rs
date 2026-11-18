//! How mail is written into an answer: as data, and as a line to read.

use quill_core::types::{Address, LocalAccountId, LocalFolderId};
use quill_store::{FolderRow, MessageRow, Store};
use serde_json::{Value, json};

use crate::text;

/// The most messages an answer describes one by one. Their ids are all
/// given, however many there are.
pub(crate) const DESCRIBED: usize = 50;

/// The accounts and folders a row is written with.
pub(crate) struct Names {
    pub accounts: Vec<(LocalAccountId, String)>,
    pub folders: Vec<FolderRow>,
}

impl Names {
    pub fn of(store: &Store) -> quill_store::Result<Self> {
        Ok(Self {
            accounts: store
                .accounts()?
                .into_iter()
                .map(|account| (account.id, account.name))
                .collect(),
            folders: store.folders()?,
        })
    }

    pub fn account(&self, id: LocalAccountId) -> &str {
        self.accounts
            .iter()
            .find(|(account, _)| *account == id)
            .map_or("", |(_, name)| name.as_str())
    }

    pub fn folder(&self, id: LocalFolderId) -> Option<&FolderRow> {
        self.folders.iter().find(|folder| folder.id == id)
    }

    /// A folder as it is named to a person: `Work/Archive`, and said to be
    /// on this Mac when it is.
    pub fn place(&self, id: LocalFolderId) -> String {
        match self.folder(id) {
            Some(folder) => {
                let name = format!("{}/{}", self.account(folder.account), folder.name);
                if folder.local {
                    format!("{name} (on this Mac)")
                } else {
                    name
                }
            }
            None => "a folder that has gone".to_owned(),
        }
    }
}

/// A moment as RFC 3339, in UTC.
pub(crate) fn when(seconds: i64) -> String {
    jiff::Timestamp::from_second(seconds)
        .map(|at| at.to_string())
        .unwrap_or_default()
}

pub(crate) fn people(list: &[Address]) -> Vec<Value> {
    list.iter()
        .map(|person| json!({ "name": person.name, "address": person.email }))
        .collect()
}

/// A message as data. What it says is in fields of its own.
pub(crate) fn row_json(row: &MessageRow, names: &Names) -> Value {
    let folder = names.folder(row.folder);
    json!({
        "id": row.id.0,
        "account": names.account(row.account),
        "folder": folder.map_or("", |folder| folder.name.as_str()),
        "on_this_mac": folder.is_some_and(|folder| folder.local),
        "thread": row.thread,
        "date": when(row.headers.date),
        "from": people(&row.headers.from),
        "to": people(&row.headers.to),
        "cc": people(&row.headers.cc),
        "subject": row.headers.subject,
        "message_id": row.headers.message_id,
        "unread": !row.flags.seen,
        "starred": row.flags.flagged,
        "answered": row.flags.answered,
        "attachments": row.has_attachments,
        "snoozed_until": row.snoozed_until.map(when),
    })
}

/// One line for a message: its id, its marks, when, who and what, safe to
/// print.
pub(crate) fn row_line(row: &MessageRow) -> String {
    let marks = format!(
        "{}{}{}",
        if row.flags.seen { ' ' } else { '*' },
        if row.flags.flagged { '!' } else { ' ' },
        if row.has_attachments { '@' } else { ' ' },
    );
    let who = row
        .headers
        .from
        .first()
        .map_or_else(String::new, |from| from.display_name().to_owned());
    let date = when(row.headers.date);
    format!(
        "{:>8}  {marks}  {}  {}  {}",
        row.id.0,
        date.get(..16).unwrap_or(&date).replace('T', " "),
        text::fit(&text::sanitize_line(&who), 22),
        text::sanitize_line(&row.headers.subject),
    )
}

/// Messages as data: the first [`DESCRIBED`] of them in full, and the id
/// of every one.
pub(crate) fn described(rows: &[MessageRow], names: &Names) -> (Vec<Value>, Vec<i64>) {
    (
        rows.iter()
            .take(DESCRIBED)
            .map(|row| row_json(row, names))
            .collect(),
        rows.iter().map(|row| row.id.0).collect(),
    )
}
