//! What Graph's JSON says, and what Quill makes of it.
//!
//! Only `id` is insisted on. Everything else about a message may be absent
//! or null (a draft has no sender yet, a note to self no recipients), and
//! is read as empty. A value of the wrong kind is an error: it means the
//! answer is not what was asked for.

use quill_core::types::{Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage};
use serde::Deserialize;
use serde::de::IgnoredAny;

/// The properties of a message a folder sync asks for: enough for a row in
/// the list, and no body.
pub(crate) const SUMMARY: &str = "id,subject,from,toRecipients,ccRecipients,receivedDateTime,\
                                  sentDateTime,internetMessageId,isRead,flag,hasAttachments,isDraft";

/// The folders Graph knows by a fixed name whatever they are called in the
/// user's language, and what each is for.
pub(crate) const WELL_KNOWN: [(&str, FolderRole); 6] = [
    ("inbox", FolderRole::Inbox),
    ("drafts", FolderRole::Drafts),
    ("sentitems", FolderRole::Sent),
    ("archive", FolderRole::Archive),
    ("junkemail", FolderRole::Junk),
    ("deleteditems", FolderRole::Trash),
];

/// One page of a list. A list that does not fit comes with a link to the
/// rest; the last page of a delta comes with the link to start from next
/// time.
#[derive(Debug, Deserialize)]
pub(crate) struct Page<T> {
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    pub next: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    pub delta: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Folder {
    pub id: String,
    pub display_name: Option<String>,
    pub child_folder_count: Option<u64>,
}

/// Anything, read for its id alone.
#[derive(Debug, Deserialize)]
pub(crate) struct Identified {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Message {
    pub id: String,
    /// Present on an entry of a delta that says the message has left the
    /// folder, deleted or moved away.
    #[serde(rename = "@removed")]
    pub removed: Option<IgnoredAny>,
    subject: Option<String>,
    from: Option<Recipient>,
    to_recipients: Option<Vec<Recipient>>,
    cc_recipients: Option<Vec<Recipient>>,
    received_date_time: Option<String>,
    sent_date_time: Option<String>,
    internet_message_id: Option<String>,
    pub is_read: Option<bool>,
    flag: Option<FollowUp>,
    has_attachments: Option<bool>,
    is_draft: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recipient {
    email_address: Option<EmailAddress>,
}

#[derive(Debug, Deserialize)]
struct EmailAddress {
    name: Option<String>,
    address: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FollowUp {
    flag_status: Option<String>,
}

impl Message {
    /// Whether everything asked for is here. A delta is allowed to describe
    /// a changed message by its changed properties alone, and such an entry
    /// must not be mistaken for a message with no subject and no sender.
    /// These two are never null on a whole message.
    pub fn whole(&self) -> bool {
        self.is_read.is_some() && self.is_draft.is_some()
    }

    /// Flagged for follow-up. A flag marked complete is not one any more.
    pub fn flagged(&self) -> bool {
        self.flag
            .as_ref()
            .and_then(|flag| flag.flag_status.as_deref())
            .is_some_and(|status| status.eq_ignore_ascii_case("flagged"))
    }

    pub fn flags(&self) -> Flags {
        Flags {
            seen: self.is_read.unwrap_or(false),
            flagged: self.flagged(),
            // Graph has no property for it: see the crate's notes.
            answered: false,
            draft: self.is_draft.unwrap_or(false),
        }
    }

    pub fn into_remote(self) -> RemoteMessage {
        let flags = self.flags();
        // When it arrived is the server's own word, which a sender cannot
        // forge. A draft has only the other.
        let date = [&self.received_date_time, &self.sent_date_time]
            .into_iter()
            .flatten()
            .find_map(|text| seconds(text))
            .unwrap_or(0);
        let addresses = |recipients: Option<Vec<Recipient>>| -> Vec<Address> {
            recipients
                .unwrap_or_default()
                .into_iter()
                .filter_map(Recipient::address)
                .collect()
        };
        RemoteMessage {
            id: RemoteId(self.id),
            headers: Headers {
                message_id: self.internet_message_id.as_deref().and_then(message_id),
                subject: self.subject.as_deref().map(clean).unwrap_or_default(),
                from: self.from.and_then(Recipient::address).into_iter().collect(),
                to: addresses(self.to_recipients),
                cc: addresses(self.cc_recipients),
                date,
            },
            flags,
            // The message resource does not say how large it is.
            size: 0,
            has_attachments: self.has_attachments.unwrap_or(false),
        }
    }
}

impl Recipient {
    fn address(self) -> Option<Address> {
        let address = self.email_address?;
        let email = address.address.as_deref().map(clean).unwrap_or_default();
        let name = address.name.as_deref().map(clean).unwrap_or_default();
        if email.is_empty() && name.is_empty() {
            return None;
        }
        Some(Address {
            // Graph repeats the address as the name when it has no name.
            name: (!name.is_empty() && !name.eq_ignore_ascii_case(&email)).then_some(name),
            email,
        })
    }
}

/// Seconds since the Unix epoch for a time such as `2026-11-03T09:15:00Z`.
fn seconds(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_second)
}

/// A `Message-ID` as the rest of Quill holds it: with its angle brackets.
fn message_id(text: &str) -> Option<String> {
    let cleaned = clean(text);
    let id = cleaned.trim_matches(|c: char| c == '<' || c == '>' || c.is_whitespace());
    (!id.is_empty()).then(|| format!("<{id}>"))
}

/// Header text for a list row. It came from a stranger's mail by way of the
/// service, and a control character in it could drive the terminal.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn message(value: serde_json::Value) -> Message {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_whole_message_becomes_a_list_row() {
        let remote = message(json!({
            "@odata.etag": "W/\"CQAAABYAAAB1\"",
            "id": "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A",
            "subject": "Harbour survey: second draft",
            "from": {"emailAddress": {"name": "Ana Ruiz", "address": "ana.ruiz@example.com"}},
            "toRecipients": [
                {"emailAddress": {"name": "Robin Okafor", "address": "robin@example.com"}},
                {"emailAddress": {"name": "omar.haddad@example.com", "address": "omar.haddad@example.com"}}
            ],
            "ccRecipients": [
                {"emailAddress": {"name": "Haddad, Omar", "address": "omar.haddad@example.com"}}
            ],
            "receivedDateTime": "2026-11-03T09:15:04Z",
            "sentDateTime": "2026-11-03T09:15:00Z",
            "internetMessageId": "<survey.2@example.com>",
            "isRead": true,
            "flag": {"flagStatus": "flagged"},
            "hasAttachments": true,
            "isDraft": false
        }))
        .into_remote();

        assert_eq!(
            remote.id,
            RemoteId("AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A".to_owned())
        );
        assert_eq!(remote.headers.subject, "Harbour survey: second draft");
        assert_eq!(
            remote.headers.from,
            [Address::new("Ana Ruiz", "ana.ruiz@example.com")]
        );
        assert_eq!(
            remote.headers.to,
            [
                Address::new("Robin Okafor", "robin@example.com"),
                Address::new("", "omar.haddad@example.com"),
            ]
        );
        assert_eq!(
            remote.headers.cc,
            [Address::new("Haddad, Omar", "omar.haddad@example.com")]
        );
        assert_eq!(remote.headers.date, 1_793_697_304);
        assert_eq!(
            remote.headers.message_id.as_deref(),
            Some("<survey.2@example.com>")
        );
        assert_eq!(
            remote.flags,
            Flags {
                seen: true,
                flagged: true,
                answered: false,
                draft: false,
            }
        );
        assert!(remote.has_attachments);
        assert_eq!(remote.size, 0);
    }

    #[test]
    fn a_draft_with_nothing_filled_in_is_still_a_message() {
        let draft = message(json!({
            "id": "draft-1",
            "subject": null,
            "from": null,
            "toRecipients": [],
            "sentDateTime": "2026-11-03T10:00:00Z",
            "internetMessageId": null,
            "isRead": true,
            "flag": {"flagStatus": "notFlagged"},
            "hasAttachments": false,
            "isDraft": true
        }));
        assert!(draft.whole());
        let remote = draft.into_remote();
        assert_eq!(remote.headers.subject, "");
        assert!(remote.headers.from.is_empty());
        assert!(remote.headers.to.is_empty() && remote.headers.cc.is_empty());
        assert_eq!(remote.headers.message_id, None);
        // No arrival time, so the time it was written.
        assert_eq!(remote.headers.date, 1_793_700_000);
        assert!(remote.flags.draft && remote.flags.seen && !remote.flags.flagged);
    }

    #[test]
    fn a_completed_flag_is_not_flagged() {
        for (status, flagged) in [
            ("flagged", true),
            ("complete", false),
            ("notFlagged", false),
        ] {
            let message = message(json!({"id": "m", "flag": {"flagStatus": status}}));
            assert_eq!(message.flagged(), flagged, "{status}");
        }
        assert!(!message(json!({"id": "m"})).flagged());
    }

    #[test]
    fn an_entry_with_only_what_changed_is_not_a_whole_message() {
        assert!(!message(json!({"id": "m", "isRead": true})).whole());
        assert!(message(json!({"id": "m", "isRead": true, "isDraft": false})).whole());
    }

    #[test]
    fn control_characters_never_reach_a_list_row() {
        let remote = message(json!({
            "id": "m",
            "subject": "Invoice\u{1b}]0;gotcha\u{7}\r\nBcc: x",
            "from": {"emailAddress": {"name": "Ana\u{1b}[2J", "address": "ana.ruiz@example.com"}},
            "internetMessageId": " <a\u{0}b@example.com>\r\n",
        }))
        .into_remote();
        assert_eq!(remote.headers.subject, "Invoice ]0;gotcha   Bcc: x");
        assert_eq!(remote.headers.from[0].name.as_deref(), Some("Ana [2J"));
        assert_eq!(
            remote.headers.message_id.as_deref(),
            Some("<a b@example.com>")
        );
    }

    #[test]
    fn a_message_id_always_has_its_brackets() {
        assert_eq!(
            message_id("abc@example.com").as_deref(),
            Some("<abc@example.com>")
        );
        assert_eq!(
            message_id("<abc@example.com>").as_deref(),
            Some("<abc@example.com>")
        );
        assert_eq!(message_id("  "), None);
        assert_eq!(message_id("<>"), None);
    }

    #[test]
    fn a_time_that_cannot_be_read_is_passed_over() {
        let remote = message(json!({
            "id": "m",
            "receivedDateTime": "yesterday",
            "sentDateTime": "2026-11-03T09:15:00+01:00"
        }))
        .into_remote();
        assert_eq!(remote.headers.date, 1_793_693_700);
        assert_eq!(message(json!({"id": "m"})).into_remote().headers.date, 0);
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_an_error_and_a_missing_id_too() {
        assert!(serde_json::from_value::<Message>(json!({"subject": "no id"})).is_err());
        assert!(serde_json::from_value::<Message>(json!({"id": "m", "isRead": "yes"})).is_err());
        assert!(serde_json::from_value::<Message>(json!({"id": 7})).is_err());
        assert!(serde_json::from_value::<Page<Message>>(json!({"values": []})).is_err());
    }
}
