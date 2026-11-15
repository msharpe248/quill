//! Writing a message: the header fields, the body, and what the keys do
//! while one is being written.
//!
//! Compose takes the reader's place, so the message list stays in view.
//! Like the rest of the UI it does no work itself: saving, completing
//! addresses, attaching files and sending are asked of the store worker and
//! answered with events.

use quill_core::types::{Address, LocalAccountId};
use quill_store::{Contact, DraftAttachment, DraftContent, DraftId};

use crate::editor::{BuiltinEditor, Editor, EditorView, Outcome};
use crate::input::LineInput;
use crate::keymap::{Key, KeyChord, Mods};

/// The parts of a message being written, in the order `tab` visits them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    From,
    To,
    Cc,
    Bcc,
    Subject,
    Body,
}

impl Field {
    const ORDER: [Field; 6] = [
        Field::From,
        Field::To,
        Field::Cc,
        Field::Bcc,
        Field::Subject,
        Field::Body,
    ];

    fn step(self, forward: bool) -> Self {
        let at = Self::ORDER
            .iter()
            .position(|field| *field == self)
            .unwrap_or(0);
        let count = Self::ORDER.len();
        Self::ORDER[if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        }]
    }

    /// The fields that hold addresses, where the completion popup appears.
    pub fn takes_addresses(self) -> bool {
        matches!(self, Field::To | Field::Cc | Field::Bcc)
    }
}

/// What a message starts out as: empty, a reply, a forward, or a draft
/// taken up again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefill {
    /// The saved draft this continues, if it is one.
    pub draft: Option<DraftId>,
    pub account: LocalAccountId,
    pub content: DraftContent,
    /// Where in the body the cursor starts, as a byte offset.
    pub cursor: usize,
    pub focus: Field,
    /// What the pane is titled: "New message", "Reply", "Forward".
    pub title: &'static str,
    /// Something to say as it opens: why it was not sent last time.
    pub note: Option<String>,
}

/// The people on offer to complete an address with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// What had been typed when these were looked up.
    pub typed: String,
    pub people: Vec<Contact>,
    pub selected: usize,
}

/// What a key did in compose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Did {
    /// Not compose's key: a command may have it.
    Nothing,
    Handled,
    /// Handled, and this text goes on the system clipboard.
    Copy(String),
    /// Handled, and people matching this text should be looked up.
    Lookup(String),
    /// Handled: attach the file at this path.
    Attach(String),
}

pub struct Compose {
    pub draft: Option<DraftId>,
    pub account: LocalAccountId,
    pub title: &'static str,
    pub to: LineInput,
    pub cc: LineInput,
    pub bcc: LineInput,
    pub subject: LineInput,
    pub body: BuiltinEditor,
    /// The body as last laid out for the pane, for the view to draw.
    pub body_view: EditorView,
    pub field: Field,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub replied_to: Option<i64>,
    pub attachments: Vec<DraftAttachment>,
    pub completion: Option<Completion>,
    /// A line being typed to name a file to attach.
    pub attaching: Option<LineInput>,
    /// Things to look at before the message goes ("there is no subject").
    /// Asking to send again sends it regardless.
    pub doubts: Vec<String>,
    /// Why the message could not be sent or saved.
    pub error: Option<String>,
    /// Sending has been asked for and not yet answered.
    pub sending: bool,
    /// The header fields have changed since the last save. The body keeps
    /// its own account of that.
    touched: bool,
}

impl Compose {
    pub fn new(prefill: Prefill) -> Self {
        let content = prefill.content;
        let mut body = BuiltinEditor::new(&content.body);
        body.tab_inserts = true;
        body.set_cursor_offset(prefill.cursor);
        Self {
            draft: prefill.draft,
            account: prefill.account,
            title: prefill.title,
            to: LineInput::new(content.to),
            cc: LineInput::new(content.cc),
            bcc: LineInput::new(content.bcc),
            subject: LineInput::new(content.subject),
            body,
            body_view: EditorView::default(),
            field: prefill.focus,
            in_reply_to: content.in_reply_to,
            references: content.references,
            replied_to: content.replied_to,
            attachments: content.attachments,
            completion: None,
            attaching: None,
            doubts: Vec::new(),
            error: prefill.note,
            sending: false,
            touched: false,
        }
    }

    /// The message as written so far.
    pub fn content(&self) -> DraftContent {
        DraftContent {
            to: self.to.text().to_owned(),
            cc: self.cc.text().to_owned(),
            bcc: self.bcc.text().to_owned(),
            subject: self.subject.text().to_owned(),
            body: self.body.text(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            attachments: self.attachments.clone(),
            replied_to: self.replied_to,
        }
    }

    /// Whether anything has been written at all. A message with nothing in
    /// it is not worth keeping as a draft.
    pub fn is_blank(&self) -> bool {
        self.to.is_empty()
            && self.cc.is_empty()
            && self.bcc.is_empty()
            && self.subject.is_empty()
            && self.attachments.is_empty()
            && self.body.text().trim().is_empty()
    }

    /// Whether there is anything a save would write that the last did not.
    pub fn is_modified(&self) -> bool {
        self.touched || self.body.is_modified()
    }

    pub fn mark_saved(&mut self, draft: DraftId) {
        self.draft = Some(draft);
        self.touched = false;
        self.body.mark_saved();
    }

    fn input_mut(&mut self) -> Option<&mut LineInput> {
        match self.field {
            Field::To => Some(&mut self.to),
            Field::Cc => Some(&mut self.cc),
            Field::Bcc => Some(&mut self.bcc),
            Field::Subject => Some(&mut self.subject),
            Field::From | Field::Body => None,
        }
    }

    /// The header field with the cursor, if the cursor is in one.
    pub fn input(&self) -> Option<&LineInput> {
        match self.field {
            Field::To => Some(&self.to),
            Field::Cc => Some(&self.cc),
            Field::Bcc => Some(&self.bcc),
            Field::Subject => Some(&self.subject),
            Field::From | Field::Body => None,
        }
    }

    /// `tab` and `shift-tab`.
    pub fn move_field(&mut self, forward: bool) {
        self.completion = None;
        self.field = self.field.step(forward);
        // Bcc is out of the way until it is asked for or has something
        // in it.
        if self.field == Field::Bcc && self.bcc.is_empty() {
            self.field = self.field.step(forward);
        }
    }

    /// Brings up the Bcc field, with the cursor in it.
    pub fn show_bcc(&mut self) {
        self.completion = None;
        self.attaching = None;
        self.field = Field::Bcc;
    }

    /// The address being typed in the field with the cursor: what follows
    /// the last comma.
    fn address_in_progress(&self) -> Option<&str> {
        if !self.field.takes_addresses() {
            return None;
        }
        let text = self.input()?.text();
        Some(text.rsplit(',').next().unwrap_or(text).trim_start())
    }

    /// Takes the people found for what was typed. An answer for something
    /// typed over since is dropped.
    pub fn offer(&mut self, typed: String, people: Vec<Contact>) {
        if self.address_in_progress() != Some(typed.as_str()) || people.is_empty() {
            self.completion = None;
            return;
        }
        self.completion = Some(Completion {
            typed,
            people,
            selected: 0,
        });
    }

    /// Puts the chosen person in place of what was typed, ready for the
    /// next address.
    fn accept_completion(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(person) = completion.people.get(completion.selected) else {
            return;
        };
        let written = written_address(&person.name, &person.email);
        let Some(input) = self.input_mut() else {
            return;
        };
        let text = input.text();
        let kept = match text.rfind(',') {
            Some(comma) => format!("{} ", &text[..=comma]),
            None => String::new(),
        };
        input.set(format!("{kept}{written}, "));
        self.touched = true;
    }

    /// Pasted text goes where the cursor is. In an address field what is
    /// now being typed is looked up, as if it had been typed.
    pub fn paste(&mut self, text: &str) -> Did {
        self.error = None;
        self.doubts.clear();
        if let Some(path) = &mut self.attaching {
            path.insert(text);
        } else if self.field == Field::Body {
            self.body.insert(text);
        } else if let Some(input) = self.input_mut() {
            // A header field is one line.
            input.insert(&text.replace(['\r', '\n'], " "));
            self.touched = true;
            match self.address_in_progress() {
                Some(typed) if !typed.is_empty() => return Did::Lookup(typed.to_owned()),
                _ => self.completion = None,
            }
        }
        Did::Handled
    }

    /// Adds a file to the message. It is part of what has been written: a
    /// message with a file on it is worth keeping, and worth checking again.
    pub fn attach(&mut self, attachment: DraftAttachment) {
        self.attachments.push(attachment);
        self.touched = true;
        self.doubts.clear();
        self.error = None;
    }

    /// Starts or stops asking for a file to attach.
    pub fn ask_for_attachment(&mut self) {
        self.attaching = match self.attaching {
            Some(_) => None,
            None => Some(LineInput::default()),
        };
    }

    /// Takes a key. `accounts` is how many accounts there are to send
    /// from; the From field moves between them.
    pub fn key(&mut self, chord: KeyChord, accounts: &[LocalAccountId]) -> Did {
        if self.sending {
            // Keys wait while the message is being handed over.
            return match chord.key {
                Key::Char(_) | Key::Enter | Key::Backspace | Key::Delete => Did::Handled,
                _ => Did::Nothing,
            };
        }
        let plain = chord.mods == Mods::default();

        // A file name being typed takes the keys first.
        if let Some(path) = &mut self.attaching {
            return match chord.key {
                Key::Enter if plain => {
                    let path = path.text().trim().to_owned();
                    self.attaching = None;
                    if path.is_empty() {
                        Did::Handled
                    } else {
                        Did::Attach(path)
                    }
                }
                Key::Esc => {
                    self.attaching = None;
                    Did::Handled
                }
                _ if path.key(chord) => Did::Handled,
                _ => Did::Nothing,
            };
        }

        self.error = None;
        // The people on offer: arrows choose, enter or tab takes, esc
        // declines. Any other key goes on to the field as usual.
        if let Some(completion) = &mut self.completion {
            match chord.key {
                Key::Down if plain => {
                    completion.selected = (completion.selected + 1) % completion.people.len();
                    return Did::Handled;
                }
                Key::Up if plain => {
                    let count = completion.people.len();
                    completion.selected = (completion.selected + count - 1) % count;
                    return Did::Handled;
                }
                Key::Enter | Key::Tab if plain => {
                    self.accept_completion();
                    return Did::Handled;
                }
                Key::Esc => {
                    self.completion = None;
                    return Did::Handled;
                }
                _ => {}
            }
        }

        match self.field {
            Field::From => match chord.key {
                // Which account the message goes from.
                Key::Left | Key::Right | Key::Char(' ') if plain && accounts.len() > 1 => {
                    let at = accounts
                        .iter()
                        .position(|account| *account == self.account)
                        .unwrap_or(0);
                    let next = if chord.key == Key::Left {
                        (at + accounts.len() - 1) % accounts.len()
                    } else {
                        (at + 1) % accounts.len()
                    };
                    self.account = accounts[next];
                    self.touched = true;
                    self.doubts.clear();
                    Did::Handled
                }
                Key::Enter | Key::Down if plain => {
                    self.move_field(true);
                    Did::Handled
                }
                // Letters are not typed into a choice.
                Key::Char(_) if plain => Did::Handled,
                _ => Did::Nothing,
            },
            Field::Body => {
                // The first line of the body leads back up to the subject.
                match self.body.key(chord) {
                    Outcome::Ignored => Did::Nothing,
                    Outcome::Handled => {
                        self.doubts.clear();
                        Did::Handled
                    }
                    Outcome::Copy(text) => Did::Copy(text),
                }
            }
            Field::To | Field::Cc | Field::Bcc | Field::Subject => {
                // In a one-line field, enter and the down arrow go on to
                // the next field and the up arrow goes back.
                match chord.key {
                    Key::Enter | Key::Down if plain => {
                        self.move_field(true);
                        return Did::Handled;
                    }
                    Key::Up if plain => {
                        self.move_field(false);
                        return Did::Handled;
                    }
                    _ => {}
                }
                let before = self.input().map(|input| input.text().to_owned());
                let handled = self.input_mut().is_some_and(|input| input.key(chord));
                if !handled {
                    return Did::Nothing;
                }
                let changed = self.input().map(|input| input.text().to_owned()) != before;
                if changed {
                    self.touched = true;
                    self.doubts.clear();
                }
                match self.address_in_progress() {
                    Some(typed) if changed && !typed.is_empty() => Did::Lookup(typed.to_owned()),
                    Some(_) if changed => {
                        self.completion = None;
                        Did::Handled
                    }
                    _ => Did::Handled,
                }
            }
        }
    }
}

/// An address as it is written in a header field: `Ana Ruiz
/// <ana.ruiz@example.com>`, or the bare address when there is no name. A
/// name that holds a comma is quoted, or it would read as two people.
pub fn written_address(name: &str, email: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        email.to_owned()
    } else if name.contains([',', '<', '>', '"', ';', ':', '@']) {
        format!("\"{}\" <{email}>", name.replace('"', ""))
    } else {
        format!("{name} <{email}>")
    }
}

/// Reads a header field's text as addresses: bare (`ana@example.com`) or
/// named (`Ana Ruiz <ana@example.com>`), separated by commas. A comma
/// inside a quoted name does not separate. Says which part is not an
/// address.
pub fn parse_recipients(text: &str) -> Result<Vec<Address>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' | ';' if !quoted => parts.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    parts.push(current);

    let mut people = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, email) = match part.rsplit_once('<') {
            Some((name, rest)) => {
                let email = rest
                    .strip_suffix('>')
                    .ok_or_else(|| format!("\"{part}\" is missing its closing >"))?;
                (name.trim().trim_matches('"').trim(), email.trim())
            }
            None => ("", part),
        };
        if !is_address(email) {
            return Err(format!("\"{email}\" is not an email address"));
        }
        people.push(Address {
            name: (!name.is_empty()).then(|| name.to_owned()),
            email: email.to_owned(),
        });
    }
    Ok(people)
}

/// Whether `text` has the shape of an address: something, an @, and a
/// domain with a dot in it, with no spaces or brackets. Whether anyone is
/// there is the server's to say.
fn is_address(text: &str) -> bool {
    let Some((user, domain)) = text.rsplit_once('@') else {
        return false;
    };
    !user.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !text.contains(|c: char| c.is_whitespace() || c.is_control() || "<>,;\"".contains(c))
        && !user.contains('@')
}

/// A guess at a file's type from its name, for the attachment's label.
/// Unknown endings are plain bytes.
pub fn content_type_for(name: &str) -> &'static str {
    let ending = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ending.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "ics" => "text/calendar",
        "eml" => "message/rfc822",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: LocalAccountId = LocalAccountId(1);
    const HOME: LocalAccountId = LocalAccountId(2);

    fn blank() -> Compose {
        Compose::new(Prefill {
            draft: None,
            account: WORK,
            content: DraftContent::default(),
            cursor: 0,
            focus: Field::To,
            title: "New message",
            note: None,
        })
    }

    fn press(compose: &mut Compose, keys: &str) -> Vec<Did> {
        keys.split_whitespace()
            .map(|key| compose.key(key.parse().unwrap(), &[WORK, HOME]))
            .collect()
    }

    fn type_text(compose: &mut Compose, text: &str) -> Did {
        let mut last = Did::Nothing;
        for c in text.chars() {
            last = compose.key(KeyChord::plain(Key::Char(c)), &[WORK, HOME]);
        }
        last
    }

    fn ana() -> Contact {
        Contact {
            name: "Ana Ruiz".to_owned(),
            email: "ana.ruiz@example.com".to_owned(),
        }
    }

    #[test]
    fn typing_goes_to_the_field_with_the_cursor() {
        let mut compose = blank();
        assert!(compose.is_blank() && !compose.is_modified());
        type_text(&mut compose, "sam.lee@example.com");
        // `tab` is the form's key, not a field's. Past Cc; Bcc is empty
        // and so out of the way.
        assert_eq!(press(&mut compose, "tab"), [Did::Nothing]);
        compose.move_field(true);
        compose.move_field(true);
        assert_eq!(compose.field, Field::Subject);
        type_text(&mut compose, "Lunch?");
        compose.move_field(true);
        assert_eq!(compose.field, Field::Body);
        type_text(&mut compose, "Thursday?");
        let content = compose.content();
        assert_eq!(
            (
                content.to.as_str(),
                content.subject.as_str(),
                content.body.as_str()
            ),
            ("sam.lee@example.com", "Lunch?", "Thursday?")
        );
        assert!(!compose.is_blank() && compose.is_modified());

        compose.mark_saved(DraftId(7));
        assert!(!compose.is_modified());
        assert_eq!(compose.draft, Some(DraftId(7)));
        type_text(&mut compose, "!");
        assert!(compose.is_modified());
    }

    #[test]
    fn enter_and_the_arrows_walk_the_header_fields() {
        let mut compose = blank();
        press(&mut compose, "enter");
        assert_eq!(compose.field, Field::Cc);
        press(&mut compose, "down");
        assert_eq!(compose.field, Field::Subject);
        // Asked for, Bcc is between them, and stays while it holds something.
        compose.show_bcc();
        assert_eq!(compose.field, Field::Bcc);
        type_text(&mut compose, "x");
        press(&mut compose, "down up up down down");
        assert_eq!(compose.field, Field::Subject);
        press(&mut compose, "up backspace up down");
        assert_eq!(compose.field, Field::Subject);
        press(&mut compose, "up up up up");
        assert_eq!(compose.field, Field::From);
        press(&mut compose, "shift-tab");
        // shift-tab is the form's, not a field's.
        assert_eq!(compose.field, Field::From);
        compose.move_field(false);
        assert_eq!(compose.field, Field::Body);
        compose.move_field(true);
        assert_eq!(compose.field, Field::From);
    }

    #[test]
    fn the_from_field_moves_between_accounts() {
        let mut compose = blank();
        compose.field = Field::From;
        press(&mut compose, "right");
        assert_eq!(compose.account, HOME);
        press(&mut compose, "right");
        assert_eq!(compose.account, WORK);
        press(&mut compose, "left");
        assert_eq!(compose.account, HOME);
        // Letters are not typed into it, and with one account it stays.
        assert_eq!(type_text(&mut compose, "x"), Did::Handled);
        let only = compose.key("right".parse().unwrap(), &[HOME]);
        assert_eq!((only, compose.account), (Did::Nothing, HOME));
    }

    #[test]
    fn an_address_being_typed_is_looked_up_and_completed() {
        let mut compose = blank();
        assert_eq!(type_text(&mut compose, "an"), Did::Lookup("an".to_owned()));
        compose.offer(
            "an".to_owned(),
            vec![
                ana(),
                Contact {
                    name: String::new(),
                    email: "anders@example.com".to_owned(),
                },
            ],
        );
        assert_eq!(compose.completion.as_ref().unwrap().people.len(), 2);

        // Arrows choose among them and go round.
        press(&mut compose, "down down up");
        assert_eq!(compose.completion.as_ref().unwrap().selected, 1);
        press(&mut compose, "enter");
        assert_eq!(compose.to.text(), "anders@example.com, ");
        assert!(compose.completion.is_none());
        // Enter took the person; it did not leave the field.
        assert_eq!(compose.field, Field::To);

        // A second address is completed after the first, which is kept.
        assert_eq!(type_text(&mut compose, "ru"), Did::Lookup("ru".to_owned()));
        compose.offer("ru".to_owned(), vec![ana()]);
        press(&mut compose, "tab");
        assert_eq!(
            compose.to.text(),
            "anders@example.com, Ana Ruiz <ana.ruiz@example.com>, "
        );
    }

    #[test]
    fn a_stale_or_empty_answer_offers_nobody() {
        let mut compose = blank();
        type_text(&mut compose, "an");
        // The answer for "a" arrives after "an" was typed.
        compose.offer("a".to_owned(), vec![ana()]);
        assert!(compose.completion.is_none());
        compose.offer("an".to_owned(), Vec::new());
        assert!(compose.completion.is_none());

        // Esc declines the offer and leaves the text; the next esc is not
        // compose's.
        compose.offer("an".to_owned(), vec![ana()]);
        assert_eq!(press(&mut compose, "esc esc"), [Did::Handled, Did::Nothing]);
        assert_eq!(compose.to.text(), "an");
        // Nothing is looked up for the subject.
        compose.field = Field::Subject;
        assert_eq!(type_text(&mut compose, "an"), Did::Handled);
        // Moving to another field drops an offer.
        compose.field = Field::To;
        compose.offer("an".to_owned(), vec![ana()]);
        compose.move_field(true);
        assert!(compose.completion.is_none());
    }

    #[test]
    fn a_file_to_attach_is_named_on_a_line_of_its_own() {
        let mut compose = blank();
        compose.ask_for_attachment();
        type_text(&mut compose, "~/roadmap.pdf");
        // The path was not typed into the To field.
        assert_eq!(compose.to.text(), "");
        assert_eq!(
            press(&mut compose, "enter"),
            [Did::Attach("~/roadmap.pdf".to_owned())]
        );
        assert!(compose.attaching.is_none());

        compose.ask_for_attachment();
        assert_eq!(press(&mut compose, "esc"), [Did::Handled]);
        assert!(compose.attaching.is_none());
        compose.ask_for_attachment();
        assert_eq!(press(&mut compose, "enter"), [Did::Handled]);
    }

    #[test]
    fn copying_in_the_body_hands_the_text_over() {
        let mut compose = blank();
        compose.field = Field::Body;
        type_text(&mut compose, "copy me");
        assert_eq!(
            press(&mut compose, "cmd-a cmd-c"),
            [Did::Handled, Did::Copy("copy me".to_owned())]
        );
    }

    #[test]
    fn keys_wait_while_a_message_is_handed_over() {
        let mut compose = blank();
        type_text(&mut compose, "ana.ruiz@example.com");
        compose.sending = true;
        assert_eq!(type_text(&mut compose, "x"), Did::Handled);
        assert_eq!(compose.to.text(), "ana.ruiz@example.com");
    }

    #[test]
    fn typing_clears_a_complaint_and_a_doubt() {
        let mut compose = blank();
        compose.error = Some("no recipients".to_owned());
        compose.doubts = vec!["There is no subject.".to_owned()];
        type_text(&mut compose, "a");
        assert!(compose.error.is_none() && compose.doubts.is_empty());
    }

    #[test]
    fn pasted_text_goes_where_the_cursor_is() {
        let mut compose = blank();
        compose.paste("ana.ruiz@example.com");
        compose.field = Field::Body;
        compose.paste("two\nlines");
        compose.ask_for_attachment();
        compose.paste("/tmp/a.pdf");
        assert_eq!(compose.to.text(), "ana.ruiz@example.com");
        assert_eq!(compose.body.text(), "two\nlines");
        assert_eq!(compose.attaching.as_ref().unwrap().text(), "/tmp/a.pdf");
    }

    #[test]
    fn a_reply_starts_in_the_body_at_its_cursor() {
        let mut compose = Compose::new(Prefill {
            draft: None,
            account: WORK,
            content: DraftContent {
                to: "ana.ruiz@example.com".to_owned(),
                subject: "Re: Q4 roadmap".to_owned(),
                body: "\n\nOn Saturday, Ana wrote:\n> Hi Robin".to_owned(),
                in_reply_to: Some("<1@example.com>".to_owned()),
                references: vec!["<1@example.com>".to_owned()],
                replied_to: Some(5),
                ..DraftContent::default()
            },
            cursor: 0,
            focus: Field::Body,
            title: "Reply",
            note: None,
        });
        assert!(!compose.is_modified() && !compose.is_blank());
        type_text(&mut compose, "Looks good.");
        let content = compose.content();
        assert!(content.body.starts_with("Looks good.\n\nOn Saturday"));
        assert_eq!(content.in_reply_to.as_deref(), Some("<1@example.com>"));
        assert_eq!(content.replied_to, Some(5));
    }

    #[test]
    fn addresses_are_read_from_what_was_typed() {
        let people = parse_recipients(
            "ana.ruiz@example.com, Sam Lee <sam.lee@example.com>; \"Haddad, Omar\" <omar@example.com>,",
        )
        .unwrap();
        assert_eq!(
            people,
            [
                Address {
                    name: None,
                    email: "ana.ruiz@example.com".to_owned()
                },
                Address::new("Sam Lee", "sam.lee@example.com"),
                Address::new("Haddad, Omar", "omar@example.com"),
            ]
        );
        assert_eq!(parse_recipients("  ").unwrap(), []);
        for (text, complaint) in [
            ("ana", "\"ana\" is not an email address"),
            ("ana@localhost", "not an email address"),
            ("ana ruiz@example.com", "not an email address"),
            ("Ana <ana@example.com", "missing its closing >"),
            ("a@b@example.com", "not an email address"),
            ("ana@.com", "not an email address"),
            (
                "ana@example.com\r\nBcc: x@example.com",
                "not an email address",
            ),
        ] {
            let error = parse_recipients(text).unwrap_err();
            assert!(error.contains(complaint), "{text}: {error}");
        }
    }

    #[test]
    fn a_written_address_reads_back_as_the_same_person() {
        for (name, email) in [
            ("Ana Ruiz", "ana.ruiz@example.com"),
            ("", "sam.lee@example.com"),
            ("Haddad, Omar", "omar@example.com"),
            ("陈静", "chen.jing@example.com"),
        ] {
            let written = written_address(name, email);
            let read = parse_recipients(&written).unwrap();
            assert_eq!(read.len(), 1, "{written}");
            assert_eq!(read[0].name.as_deref().unwrap_or_default(), name);
            assert_eq!(read[0].email, email);
        }
    }

    #[test]
    fn file_types_are_guessed_from_names() {
        assert_eq!(content_type_for("Roadmap.PDF"), "application/pdf");
        assert_eq!(content_type_for("photo.jpeg"), "image/jpeg");
        assert_eq!(content_type_for("notes"), "application/octet-stream");
        assert_eq!(
            content_type_for("archive.tar.gz"),
            "application/octet-stream"
        );
    }
}
