//! Saving the message on show as a file of its own: its text, its page,
//! or the message itself as it was sent.
//!
//! Which is chosen from a list, with the keyboard. The file is made by
//! the store's worker; where it goes is the host's to ask where it has a
//! panel to ask with, and the Downloads folder where it has none.

use quill_core::types::LocalMessageId;
use quill_store::DraftId;

use super::rules::{PickFor, PickLine, PickList};
use super::{Effects, Env, State};
use crate::command::Context;
use crate::data::DataRequest;
use crate::files::SaveAs;

/// The kinds of file a message can be saved as, in the order they are
/// listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveKind {
    Text,
    Page,
    Source,
}

impl SaveKind {
    pub const ALL: [SaveKind; 3] = [Self::Text, Self::Page, Self::Source];

    fn name(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Page => "Web page",
            Self::Source => "Message file",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Text => ".txt   what it says, as plain text",
            Self::Page => ".html  as it looks, with the pictures it carries",
            Self::Source => ".eml   the whole message and its attachments, for any mail program",
        }
    }
}

impl State {
    /// The message that can be copied or saved as a file: the one in the
    /// reader, once its text is here, when it is the one the user is on
    /// and is mail, not something still being written.
    pub(super) fn message_to_save(&self, context: Context) -> Option<LocalMessageId> {
        self.page_message(context)
            .filter(|id| DraftId::from_message_id(*id).is_none())
    }

    /// Whether the list on show is of the kinds of file a message can be
    /// saved as.
    pub fn saving_as(&self) -> bool {
        self.hooks.as_ref().is_some_and(|picker| {
            matches!(
                picker.list,
                Some(PickList {
                    what: PickFor::SaveAs(_),
                    ..
                })
            )
        })
    }

    /// Lists the kinds of file the message can be saved as, to choose
    /// one.
    pub(super) fn ask_save_as(&mut self, _env: &Env) {
        let Some(message) = self.message_to_save(self.context()) else {
            return;
        };
        self.open_pick_list(PickList {
            title: " Save this message as ".to_owned(),
            none: "It can be saved as text, a web page or a message file.".to_owned(),
            lines: SaveKind::ALL
                .iter()
                .map(|kind| PickLine {
                    name: kind.name().to_owned(),
                    detail: kind.detail().to_owned(),
                })
                .collect(),
            what: PickFor::SaveAs(message),
        });
    }

    /// Has the message written out as the kind of file chosen. It is
    /// still the message in the reader, or nothing is saved: the list was
    /// over it all the while.
    pub(super) fn save_as(
        &mut self,
        env: &Env,
        message: LocalMessageId,
        kind: SaveKind,
        out: &mut Effects,
    ) {
        let Some(row) = self.reader.row.as_ref().filter(|row| row.id == message) else {
            return;
        };
        let subject = row.headers.subject.clone();
        let what = match kind {
            SaveKind::Text => match self.message_text_for_file(env) {
                Some(text) => SaveAs::Text(text),
                None => {
                    self.flash = Some((
                        "This message's text is not here yet to save.".to_owned(),
                        false,
                    ));
                    return;
                }
            },
            SaveKind::Page => SaveAs::Page {
                remote: self.remote_content(env),
                fields: self.message_fields(env),
            },
            SaveKind::Source => SaveAs::Source,
        };
        out.data.push(DataRequest::SaveMessage {
            message,
            subject,
            what,
            ask: env.file_panel,
        });
    }
}
