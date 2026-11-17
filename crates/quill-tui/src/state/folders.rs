//! Folders the user makes, and mail put back where it came from.
//!
//! A folder can be made in two places, and which is asked with its name
//! ([`FolderPrompt`]): on the account's server, where every mail program
//! sees it, or on this Mac, where mail moved into it is kept whole and
//! taken off the server. The two are never shown alike: a folder on this
//! Mac is listed under a heading that says so, and is named as such
//! wherever a folder is named.
//!
//! Putting mail back is for what is in a trash folder: Quill notes where
//! a message was when it moves it there, which no server does. From a
//! trash folder the messages chosen go back; from anywhere else, the one
//! deleted last.

use quill_core::types::{FolderRole, LocalAccountId, LocalFolderId};
use quill_store::{DraftId, Mailbox};
use quill_sync::SyncRequest;

use super::{Effects, Env, SidebarEntry, State};
use crate::data::DataRequest;
use crate::input::LineInput;
use crate::keymap::{Key, KeyChord, Mods};
use crate::text;

/// The box asking what a new folder is to be called and where it is to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPrompt {
    pub account: LocalAccountId,
    /// The account's name, cleaned, for the question's title.
    pub account_name: String,
    pub input: LineInput,
    /// On this Mac, and not on the account's server.
    pub here: bool,
    /// There is a choice: the account's server can be asked for a folder.
    /// Without one the folder can only be on this Mac.
    pub either: bool,
    /// Why what was typed would not do.
    pub problem: Option<String>,
}

/// The two places a folder can be, as they are named wherever the choice
/// is shown.
pub const ON_THE_SERVER: &str = "On the server";
pub const ON_THIS_MAC: &str = "On this Mac";

impl FolderPrompt {
    pub fn title(&self) -> String {
        format!("New folder in {}", self.account_name)
    }

    /// Where the folder will be, in words.
    pub fn place(&self) -> &'static str {
        if self.here {
            ON_THIS_MAC
        } else {
            ON_THE_SERVER
        }
    }

    /// What the place chosen means for the mail put there.
    pub fn means(&self) -> &'static str {
        match (self.here, self.either) {
            (true, true) => {
                "Mail moved into a folder on this Mac is kept here whole and taken off the \
                 server. It is then on this Mac and nowhere else."
            }
            (true, false) => {
                "This account's server has no folders to make, so the folder is on this Mac, \
                 as its mail is."
            }
            (false, _) => {
                "The folder is made on the account's server, where your other mail programs \
                 see it too."
            }
        }
    }

    /// Takes a key: `tab` goes between the two places, where there are
    /// two, and anything else is typing. Returns whether the key was the
    /// question's.
    pub fn key(&mut self, chord: KeyChord) -> bool {
        if chord.key == Key::Tab && chord.mods == Mods::default() {
            if self.either {
                self.here = !self.here;
            }
            return true;
        }
        if self.input.key(chord) {
            self.problem = None;
            return true;
        }
        false
    }
}

impl State {
    /// The account a new folder would be for: the one the accounts pane
    /// is on, or else the one being looked at, or else the first. The
    /// question says which, by name, before anything is made.
    fn folder_account(&self) -> Option<(LocalAccountId, String, String)> {
        let account = self
            .chosen_account()
            .or_else(|| self.current_account())
            .or_else(|| self.accounts.first())?;
        Some((account.id, account.name.clone(), account.kind.clone()))
    }

    /// Whether a folder can be asked for now.
    pub fn can_make_folder(&self, _env: &Env) -> bool {
        self.compose.is_none() && self.new_folder.is_none() && self.folder_account().is_some()
    }

    /// Opens the question for a new folder.
    pub(super) fn ask_new_folder(&mut self, env: &Env) {
        let Some((account, name, kind)) = self.folder_account() else {
            return;
        };
        // A server that can be asked for one, and an engine to ask it:
        // the demo mailbox has neither.
        let either = env.accounts && matches!(kind.as_str(), "imap" | "graph");
        self.new_folder = Some(FolderPrompt {
            account,
            account_name: text::sanitize_line(&name),
            input: LineInput::default(),
            here: !either,
            either,
            problem: None,
        });
    }

    /// Makes the folder the question describes, or says what is wrong
    /// with its name.
    pub(super) fn answer_new_folder(&mut self, _env: &Env, out: &mut Effects) {
        let Some(prompt) = &mut self.new_folder else {
            return;
        };
        let name = prompt.input.text().trim().to_owned();
        if name.is_empty() {
            prompt.problem = Some("Give the folder a name.".to_owned());
            return;
        }
        let Some(prompt) = self.new_folder.take() else {
            return;
        };
        if prompt.here {
            out.data.push(DataRequest::NewLocalFolder {
                account: prompt.account,
                name,
            });
        } else {
            self.flash = Some((format!("Asking the server for {name}\u{2026}"), false));
            out.sync.push(SyncRequest::CreateFolder {
                account: prompt.account,
                name,
            });
        }
    }

    /// The folder the accounts pane is on, if it is one made on this Mac.
    pub(super) fn chosen_local_folder(&self) -> Option<LocalFolderId> {
        match self.sidebar.entries.get(self.sidebar.selected)? {
            SidebarEntry::Folder {
                id, local: true, ..
            } => Some(*id),
            _ => None,
        }
    }

    /// Whether the list is a folder that is only on this Mac.
    pub fn current_folder_is_local(&self) -> bool {
        let Mailbox::Folder(current) = self.mailbox else {
            return false;
        };
        self.sidebar.entries.iter().any(
            |entry| matches!(entry, SidebarEntry::Folder { id, local: true, .. } if *id == current),
        )
    }

    /// Whether the list is a trash folder.
    fn in_trash(&self) -> bool {
        self.search.is_none()
            && self
                .current_folder()
                .is_some_and(|(_, _, role, _)| role == FolderRole::Trash)
    }

    /// Whether there may be something to put back: in a trash folder, a
    /// message under the cursor; anywhere else, an account to have
    /// deleted from.
    pub fn can_put_back(&self) -> bool {
        if self.compose.is_some() {
            return false;
        }
        if self.in_trash() {
            self.target_row()
                .is_some_and(|row| DraftId::from_message_id(row.id).is_none())
        } else {
            !self.accounts.is_empty()
        }
    }

    /// In a trash folder, puts the messages chosen back where they were.
    /// Anywhere else, the message that was deleted last.
    pub(super) fn put_back(&mut self, out: &mut Effects) {
        if self.in_trash() {
            let ids = self.targets();
            if !ids.is_empty() {
                out.data.push(DataRequest::PutBack(ids));
                self.list.clear_marks();
            }
        } else {
            out.data.push(DataRequest::PutBackLast);
        }
    }
}
