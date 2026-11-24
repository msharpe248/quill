//! Folders the user makes, renames and deletes, and mail put back where
//! it came from.
//!
//! A folder can be made in two places, and which is asked with its name
//! ([`FolderPrompt`]): on the account's server, where every mail program
//! sees it, or on this Mac, where mail moved into it is kept whole and
//! taken off the server. The two are never shown alike: a folder on this
//! Mac is listed under a heading that says so, and is named as such
//! wherever a folder is named.
//!
//! A folder is inside another by its name: `Projects/2026` is `2026`
//! inside `Projects`, on a server and on this Mac alike, and is made,
//! renamed and shown so.
//!
//! The same question renames a folder, and asks before a server's folder
//! is deleted ([`FolderAsk`]). Deleting one deletes no mail: what the
//! store knows of the folder is read first ([`FolderLeaving`]), the
//! question says where its mail goes, and a folder whose mail has
//! nowhere to go, or that has folders inside it, is not asked about at
//! all.
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

/// What the question about a folder is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderAsk {
    /// A new folder: its name, and where it is to be.
    New,
    /// Another name for a folder there is.
    Rename { folder: LocalFolderId, was: String },
    /// Before a server's folder is deleted: `becomes` says what happens
    /// to it and to its mail.
    Delete {
        folder: LocalFolderId,
        name: String,
        becomes: String,
    },
}

/// What the store knows of a folder that is asked to be deleted from its
/// server: enough to say what would become of its mail, or why it cannot
/// be deleted yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderLeaving {
    pub folder: LocalFolderId,
    pub account: LocalAccountId,
    pub name: String,
    /// How many messages are in it here.
    pub messages: usize,
    /// How many folders are inside it.
    pub inside: usize,
    /// Changes to its mail are still on their way to the server.
    pub busy: bool,
    pub mail: LeavingMail,
}

/// Where the mail in a folder goes when the folder is deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeavingMail {
    /// To the account's trash folder, which is called this.
    Trash(String),
    /// Nowhere: the server keeps all mail in one place, and a folder is
    /// a label on it (Gmail).
    AllMail,
    /// Into the mailbox's Deleted Items with its folder (Microsoft 365,
    /// Exchange).
    WithFolder,
    /// There is no trash folder for it to go to.
    Nowhere,
}

impl FolderLeaving {
    /// Why the folder cannot be deleted as things stand, in words for
    /// the user.
    pub fn refusal(&self) -> Option<String> {
        let name = text::sanitize_line(&self.name);
        if self.inside > 0 {
            return Some(format!(
                "{name} has folders inside it. Delete or move those first."
            ));
        }
        if self.busy {
            return Some(format!(
                "Changes to the mail in {name} are still on their way to the server. Try \
                 again in a moment."
            ));
        }
        if self.messages > 0 && self.mail == LeavingMail::Nowhere {
            return Some(format!(
                "{name} has mail in it, and this account has no trash folder for the mail \
                 to go to. Move or delete the mail first."
            ));
        }
        None
    }

    /// What deleting the folder does, and what becomes of its mail.
    pub fn becomes(&self) -> String {
        let mail = match self.messages {
            1 => "The 1 message".to_owned(),
            many => format!("The {many} messages"),
        };
        let (verb, has) = if self.messages == 1 {
            ("is", "stays")
        } else {
            ("are", "stay")
        };
        match (&self.mail, self.messages) {
            (LeavingMail::WithFolder, 0) => {
                "The folder is moved into Deleted Items on the account's server, where it \
                 can be had back."
                    .to_owned()
            }
            (LeavingMail::WithFolder, _) => format!(
                "The folder is moved into Deleted Items on the account's server, with what \
                 is in it. {mail} in it {verb} not deleted, and can be had back from there."
            ),
            (_, 0) => "The folder is deleted on the account's server, for every mail program. \
                 There is no mail in it."
                .to_owned(),
            (LeavingMail::AllMail, _) => format!(
                "The folder is deleted on the account's server, for every mail program. \
                 {mail} in it {has} in All Mail."
            ),
            (LeavingMail::Trash(trash), _) => format!(
                "The folder is deleted on the account's server, for every mail program. \
                 {mail} in it {verb} moved to {} first: no mail is deleted with the folder.",
                text::sanitize_line(trash)
            ),
            // Not asked about: see `refusal`.
            (LeavingMail::Nowhere, _) => String::new(),
        }
    }
}

/// The box asking what a new folder is to be called and where it is to
/// be, what another name a folder is to have, or whether a server's
/// folder is to be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPrompt {
    /// What is asked.
    pub ask: FolderAsk,
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

/// What is said of a `/` in a folder's name, wherever a name is typed.
pub const INSIDE_ANOTHER: &str = "A / in the name puts the folder inside another: Projects/2026.";

impl FolderPrompt {
    pub fn title(&self) -> String {
        match &self.ask {
            FolderAsk::New => format!("New folder in {}", self.account_name),
            FolderAsk::Rename { was, .. } => format!("Rename {}", text::sanitize_line(was)),
            FolderAsk::Delete { name, .. } => format!("Delete {}?", text::sanitize_line(name)),
        }
    }

    /// Whether a name is typed: not where the question is only whether
    /// to delete.
    pub fn typed(&self) -> bool {
        !matches!(self.ask, FolderAsk::Delete { .. })
    }

    /// What the key that answers does, in a word or two.
    pub fn answer(&self) -> &'static str {
        match self.ask {
            FolderAsk::New => "Create",
            FolderAsk::Rename { .. } => "Rename",
            FolderAsk::Delete { .. } => "Delete",
        }
    }

    /// Where the folder will be, in words.
    pub fn place(&self) -> &'static str {
        if self.here {
            ON_THIS_MAC
        } else {
            ON_THE_SERVER
        }
    }

    /// What the place chosen means for the mail put there, or what the
    /// change asked about does.
    pub fn means(&self) -> &str {
        match &self.ask {
            FolderAsk::New => {}
            FolderAsk::Rename { .. } if self.here => {
                return "The folder is on this Mac, and is renamed here, with the folders \
                        inside it.";
            }
            FolderAsk::Rename { .. } => {
                return "The folder is renamed on the account's server, with the folders \
                        inside it, for every mail program.";
            }
            FolderAsk::Delete { becomes, .. } => return becomes,
        }
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
        if !self.typed() {
            return false;
        }
        if chord.key == Key::Tab && chord.mods == Mods::default() {
            if self.either && self.ask == FolderAsk::New {
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
        let either = env.accounts && quill_store::keeps_server_folders(&kind);
        self.new_folder = Some(FolderPrompt {
            ask: FolderAsk::New,
            account,
            account_name: text::sanitize_line(&name),
            input: LineInput::default(),
            here: !either,
            either,
            problem: None,
        });
    }

    /// The folder the accounts pane is on, when it is one the user may
    /// rename or delete: one they made, on a server that can be asked or
    /// on this Mac. Not a folder a server keeps for a purpose, such as
    /// its Sent or its Trash.
    fn chosen_own_folder(
        &self,
        env: &Env,
    ) -> Option<(LocalFolderId, &str, bool, &quill_store::AccountRow)> {
        let SidebarEntry::Folder {
            id,
            account,
            name,
            role: FolderRole::Other,
            local,
            ..
        } = self.sidebar.entries.get(self.sidebar.selected)?
        else {
            return None;
        };
        let account = self.accounts.get(*account)?;
        let asks = env.accounts && account.has_server_folders();
        (*local || asks).then_some((*id, name.as_str(), *local, account))
    }

    /// Whether the folder the accounts pane is on can be renamed, or
    /// asked to be deleted.
    pub fn can_change_folder(&self, env: &Env) -> bool {
        self.compose.is_none() && self.new_folder.is_none() && self.chosen_own_folder(env).is_some()
    }

    /// Opens the question for another name, with the one it has to change.
    pub(super) fn ask_rename_folder(&mut self, env: &Env) {
        let Some((folder, name, local, account)) = self.chosen_own_folder(env) else {
            return;
        };
        let mut input = LineInput::default();
        input.set(name);
        self.new_folder = Some(FolderPrompt {
            ask: FolderAsk::Rename {
                folder,
                was: name.to_owned(),
            },
            account: account.id,
            account_name: text::sanitize_line(&account.name),
            input,
            here: local,
            either: false,
            problem: None,
        });
    }

    /// Deletes the folder the accounts pane is on. One on this Mac goes
    /// if it is empty. A server's is asked about first, once the store
    /// has said what is in it: see [`State::on_folder_leaving`].
    pub(super) fn delete_folder(&mut self, env: &Env, out: &mut Effects) {
        let Some((folder, _, local, _)) = self.chosen_own_folder(env) else {
            return;
        };
        out.data.push(if local {
            DataRequest::RemoveLocalFolder(folder)
        } else {
            DataRequest::FolderLeaving(folder)
        });
    }

    /// The store has said what a folder holds: the question whether to
    /// delete it is asked, with what would become of its mail, or the
    /// reason it cannot be deleted yet is said and nothing is asked.
    pub(super) fn on_folder_leaving(&mut self, leaving: &FolderLeaving) {
        if self.compose.is_some() || self.new_folder.is_some() {
            return;
        }
        if let Some(refusal) = leaving.refusal() {
            self.flash = Some((refusal, false));
            return;
        }
        let account_name = self
            .accounts
            .iter()
            .find(|account| account.id == leaving.account)
            .map(|account| text::sanitize_line(&account.name))
            .unwrap_or_default();
        self.new_folder = Some(FolderPrompt {
            ask: FolderAsk::Delete {
                folder: leaving.folder,
                name: leaving.name.clone(),
                becomes: leaving.becomes(),
            },
            account: leaving.account,
            account_name,
            input: LineInput::default(),
            here: false,
            either: false,
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
        if prompt.typed() {
            if name.is_empty() {
                prompt.problem = Some("Give the folder a name.".to_owned());
                return;
            }
            if name.split('/').any(|part| part.trim().is_empty()) {
                prompt.problem =
                    Some("A folder's name needs something on each side of a /.".to_owned());
                return;
            }
        }
        let Some(prompt) = self.new_folder.take() else {
            return;
        };
        match prompt.ask {
            FolderAsk::New if prompt.here => out.data.push(DataRequest::NewLocalFolder {
                account: prompt.account,
                name,
            }),
            FolderAsk::New => {
                self.flash = Some((format!("Asking the server for {name}\u{2026}"), false));
                out.sync.push(SyncRequest::CreateFolder {
                    account: prompt.account,
                    name,
                });
            }
            // The name it has: there is nothing to do.
            FolderAsk::Rename { was, .. } if was == name => {}
            FolderAsk::Rename { folder, .. } if prompt.here => {
                out.data
                    .push(DataRequest::RenameLocalFolder { folder, name });
            }
            FolderAsk::Rename { folder, was } => {
                self.flash = Some((
                    format!(
                        "Asking the server to rename {} to {name}\u{2026}",
                        text::sanitize_line(&was)
                    ),
                    false,
                ));
                out.sync.push(SyncRequest::RenameFolder { folder, name });
            }
            FolderAsk::Delete { folder, name, .. } => {
                self.flash = Some((
                    format!(
                        "Asking the server to delete {}\u{2026}",
                        text::sanitize_line(&name)
                    ),
                    false,
                ));
                out.sync.push(SyncRequest::DeleteFolder(folder));
            }
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
