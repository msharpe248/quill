//! What EWS's XML says about a message or a folder, and what Quill makes
//! of it.
//!
//! Only an id is insisted on. Everything else about a message may be
//! absent (a draft has no sender yet, a note to self no recipients), and is
//! read as empty. Text is held to a length and cleaned of control
//! characters here: it came from a stranger's mail by way of the server.

use quill_core::types::{Address, Flags, FolderRole, Headers, RemoteId, RemoteMessage};

use crate::request::LAST_VERB;
use crate::xml::{Element, Ns};

/// The folders EWS knows by a fixed name whatever they are called in the
/// user's language, and what each is for.
pub(crate) const WELL_KNOWN: [(&str, FolderRole); 5] = [
    ("inbox", FolderRole::Inbox),
    ("drafts", FolderRole::Drafts),
    ("sentitems", FolderRole::Sent),
    ("junkemail", FolderRole::Junk),
    ("deleteditems", FolderRole::Trash),
];

/// The folder Outlook's Archive button files into. Newer servers know it
/// by this name; the rest have never heard of it, and say so in ways of
/// their own. It is asked for apart from the others for that reason.
pub(crate) const ARCHIVE: &str = "archive";

/// The longest an id is taken to be. Exchange's are a hundred and fifty
/// characters or so.
const ID_LIMIT: usize = 2048;

/// The longest a subject, a name or an address is kept.
const TEXT_LIMIT: usize = 998;

/// The most of a `References` header that is looked at.
const HEADER_LIMIT: usize = 64 * 1024;

/// How many recipients of one kind are kept for a list row.
const RECIPIENTS: usize = 500;

/// How many earlier messages one message is taken to follow. The store
/// has a bound of its own; this one keeps a hostile header from being
/// carried that far.
const REFERENCES: usize = 100;

/// The kinds of item that are mail: they have a sender and a read mark,
/// and Exchange can hand them over as the bytes of a message. A contact or
/// a task that has strayed into a mail folder is not, and is passed over.
const MAIL: [&str; 6] = [
    "Message",
    "MeetingMessage",
    "MeetingRequest",
    "MeetingResponse",
    "MeetingCancellation",
    "PostItem",
];

/// An id out of an `Id` attribute, if it is one that can be used.
pub(crate) fn id_of(element: Option<&Element<'_>>) -> Option<String> {
    let id = element?.attribute("Id")?;
    (!id.is_empty() && id.len() <= ID_LIMIT && !id.chars().any(char::is_control))
        .then(|| id.to_owned())
}

/// A message as a folder sync or a `GetItem` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub id: String,
    subject: String,
    from: Option<Address>,
    to: Vec<Address>,
    cc: Vec<Address>,
    received: Option<i64>,
    sent: Option<i64>,
    message_id: Option<String>,
    conversation: Option<String>,
    references: Vec<String>,
    read: bool,
    flag: Option<String>,
    draft: bool,
    answered: bool,
    has_attachments: bool,
    size: u64,
}

impl Item {
    /// Reads a `<t:Message>` or one of its kin. `None` for anything that
    /// is not mail, or has no id to call it by.
    pub fn read(element: &Element<'_>) -> Option<Self> {
        if !MAIL.iter().any(|kind| element.is(Ns::Types, kind)) {
            return None;
        }
        let id = id_of(element.child(Ns::Types, "ItemId"))?;
        let text = |name: &str| element.child_text(Ns::Types, name);
        let truth = |name: &str| text(name).is_some_and(is_true);
        let mailboxes = |name: &str| -> Vec<Address> {
            element
                .child(Ns::Types, name)
                .map(|list| {
                    list.children()
                        .iter()
                        .filter(|mailbox| mailbox.is(Ns::Types, "Mailbox"))
                        .filter_map(address)
                        .take(RECIPIENTS)
                        .collect()
                })
                .unwrap_or_default()
        };

        let mut references = text("References").map(message_ids).unwrap_or_default();
        if let Some(parent) = text("InReplyTo")
            .map(message_ids)
            .and_then(|ids| ids.into_iter().next())
            && references.last() != Some(&parent)
        {
            references.push(parent);
        }
        Some(Self {
            id,
            subject: text("Subject").map(clean).unwrap_or_default(),
            from: element
                .child(Ns::Types, "From")
                .and_then(|from| from.child(Ns::Types, "Mailbox"))
                .and_then(address),
            to: mailboxes("ToRecipients"),
            cc: mailboxes("CcRecipients"),
            received: text("DateTimeReceived").and_then(seconds),
            sent: text("DateTimeSent").and_then(seconds),
            message_id: text("InternetMessageId")
                .map(message_ids)
                .and_then(|ids| ids.into_iter().next()),
            conversation: element
                .child(Ns::Types, "ConversationId")
                .and_then(|conversation| conversation.attribute("Id"))
                .map(|id| {
                    clean(id)
                        .chars()
                        .filter(|c| !c.is_whitespace() && *c != '<' && *c != '>')
                        .collect::<String>()
                })
                .filter(|id| !id.is_empty()),
            references,
            read: truth("IsRead"),
            flag: element
                .child(Ns::Types, "Flag")
                .and_then(|flag| flag.child_text(Ns::Types, "FlagStatus"))
                .map(str::to_owned),
            draft: truth("IsDraft"),
            answered: last_verb(element).is_some_and(|verb| verb == 102 || verb == 103),
            has_attachments: truth("HasAttachments"),
            size: text("Size").and_then(|size| size.parse().ok()).unwrap_or(0),
        })
    }

    /// Flagged for follow-up. A flag marked complete is not one any more.
    pub fn flagged(&self) -> bool {
        self.flag
            .as_deref()
            .is_some_and(|status| status.eq_ignore_ascii_case("Flagged"))
    }

    pub fn is_read(&self) -> bool {
        self.read
    }

    pub fn flags(&self) -> Flags {
        Flags {
            seen: self.read,
            flagged: self.flagged(),
            answered: self.answered,
            draft: self.draft,
        }
    }

    pub fn into_remote(self) -> RemoteMessage {
        let flags = self.flags();
        // What a message follows, in its own words where it says. Where it
        // does not, Exchange still knows which conversation it belongs to,
        // and that, written as an id no message has, ties its messages
        // together just as well.
        let thread = if self.references.is_empty() {
            self.conversation
                .map(|id| format!("<{id}@conversation.ews>"))
                .into_iter()
                .collect()
        } else {
            self.references
        };
        RemoteMessage {
            id: RemoteId(self.id),
            headers: Headers {
                message_id: self.message_id,
                subject: self.subject,
                from: self.from.into_iter().collect(),
                to: self.to,
                cc: self.cc,
                // When it arrived is the server's own word, which a sender
                // cannot forge. A draft has only the other.
                date: self.received.or(self.sent).unwrap_or(0),
            },
            flags,
            size: self.size,
            has_attachments: self.has_attachments,
            thread,
        }
    }
}

/// What was last done with the message, by Outlook's numbering: 102 is a
/// reply, 103 a reply to all, 104 forwarding.
fn last_verb(item: &Element<'_>) -> Option<u32> {
    item.children()
        .iter()
        .filter(|child| child.is(Ns::Types, "ExtendedProperty"))
        .find(|property| {
            property
                .child(Ns::Types, "ExtendedFieldURI")
                .and_then(|uri| uri.attribute("PropertyTag"))
                .is_some_and(|tag| same_tag(tag, LAST_VERB))
        })?
        .child_text(Ns::Types, "Value")?
        .parse()
        .ok()
}

/// Whether two property tags are the same number: `0x1081`, `0X1081`,
/// `4225`.
fn same_tag(a: &str, b: &str) -> bool {
    fn number(tag: &str) -> Option<u32> {
        let tag = tag.trim();
        match tag.strip_prefix("0x").or_else(|| tag.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => tag.parse().ok(),
        }
    }
    number(a).is_some() && number(a) == number(b)
}

/// One `<t:Mailbox>` as an address.
///
/// Mail from inside the organisation can name its sender by a directory
/// path instead of an address (routing type `EX`). That is no address
/// anyone could write to, so only the name is kept of it.
fn address(mailbox: &Element<'_>) -> Option<Address> {
    let text = |name: &str| mailbox.child_text(Ns::Types, name).map(clean);
    let routable = mailbox
        .child_text(Ns::Types, "RoutingType")
        .is_none_or(|routing| routing.eq_ignore_ascii_case("SMTP"));
    let email = text("EmailAddress")
        .filter(|_| routable)
        .unwrap_or_default();
    let name = text("Name").unwrap_or_default();
    if email.is_empty() && name.is_empty() {
        return None;
    }
    Some(Address {
        // Exchange repeats the address as the name when it has no name.
        name: (!name.is_empty() && !name.eq_ignore_ascii_case(&email)).then_some(name),
        email,
    })
}

/// A folder as `FindFolder` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Folder {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    /// Whether mail is what it holds: not a calendar, not contacts, not a
    /// saved search.
    pub mail: bool,
}

impl Folder {
    /// Reads a `<t:Folder>` or one of its kin. `None` for one with no id.
    pub fn read(element: &Element<'_>) -> Option<Self> {
        let id = id_of(element.child(Ns::Types, "FolderId"))?;
        let class = element.child_text(Ns::Types, "FolderClass").unwrap_or("");
        // A folder made by a program that did not say what it is for holds
        // mail, as far as Outlook is concerned.
        let class_is_mail = class.is_empty()
            || class.eq_ignore_ascii_case("IPF.Note")
            || class
                .get(..9)
                .is_some_and(|start| start.eq_ignore_ascii_case("IPF.Note."));
        let name = element
            .child_text(Ns::Types, "DisplayName")
            .map(clean)
            .unwrap_or_default();
        Some(Self {
            parent: id_of(element.child(Ns::Types, "ParentFolderId")),
            name: if name.is_empty() { id.clone() } else { name },
            mail: element.is(Ns::Types, "Folder") && class_is_mail,
            id,
        })
    }
}

/// `true` as XML Schema writes it.
pub(crate) fn is_true(text: &str) -> bool {
    text.eq_ignore_ascii_case("true") || text == "1"
}

/// Seconds since the Unix epoch for a time such as `2026-11-03T09:15:00Z`.
fn seconds(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_second)
}

/// The `Message-ID`s in a header's text, as the rest of Quill holds them:
/// each with its angle brackets. A lone id written without them is taken
/// as it is.
fn message_ids(text: &str) -> Vec<String> {
    let cleaned = clean_to(text, HEADER_LIMIT);
    let mut ids = Vec::new();
    let mut rest = cleaned.as_str();
    while let Some(start) = rest.find('<') {
        let Some(length) = rest[start..].find('>') else {
            break;
        };
        let id = rest[start + 1..start + length].trim();
        if !id.is_empty() && !id.contains('<') && id.len() <= TEXT_LIMIT {
            ids.push(format!("<{id}>"));
            if ids.len() == REFERENCES {
                return ids;
            }
        }
        rest = &rest[start + length + 1..];
    }
    if ids.is_empty() {
        let id = cleaned.trim_matches(|c: char| c == '<' || c == '>' || c.is_whitespace());
        if !id.is_empty() && !id.contains(char::is_whitespace) && id.len() <= TEXT_LIMIT {
            ids.push(format!("<{id}>"));
        }
    }
    ids
}

/// Header text for a list row. It came from a stranger's mail by way of the
/// server, and a control character in it could drive the terminal.
fn clean(text: &str) -> String {
    clean_to(text, TEXT_LIMIT)
}

/// As [`clean`], keeping at most `limit` characters.
fn clean_to(text: &str, limit: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse;

    /// `inside` as the children of an element of EWS's types.
    fn types(name: &str, inside: &str) -> String {
        format!(
            "<t:{name} xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\">{inside}</t:{name}>"
        )
    }

    fn item(inside: &str) -> Option<Item> {
        Item::read(&parse(&types("Message", inside)).unwrap())
    }

    const WHOLE: &str = r#"
        <t:ItemId Id="AAMkAGI2NGVhZTVlLTI1OGMtNDI4My1iZmE5LTA5OGJiZGEzMTc0YQBGAAAAAAA=" ChangeKey="CQAAABYAAAB1"/>
        <t:Subject>Harbour survey: second draft</t:Subject>
        <t:DateTimeReceived>2026-11-03T09:15:04Z</t:DateTimeReceived>
        <t:Size>48213</t:Size>
        <t:InReplyTo>&lt;survey.1@example.com&gt;</t:InReplyTo>
        <t:DateTimeSent>2026-11-03T09:15:00Z</t:DateTimeSent>
        <t:HasAttachments>true</t:HasAttachments>
        <t:ExtendedProperty>
          <t:ExtendedFieldURI PropertyTag="0x1081" PropertyType="Integer"/>
          <t:Value>103</t:Value>
        </t:ExtendedProperty>
        <t:IsDraft>false</t:IsDraft>
        <t:ConversationId Id="AAQkAGI2TOPIC="/>
        <t:Flag><t:FlagStatus>Flagged</t:FlagStatus></t:Flag>
        <t:ToRecipients>
          <t:Mailbox><t:Name>Robin Okafor</t:Name><t:EmailAddress>robin@example.com</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType><t:MailboxType>Mailbox</t:MailboxType></t:Mailbox>
          <t:Mailbox><t:Name>omar.haddad@example.com</t:Name><t:EmailAddress>omar.haddad@example.com</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType></t:Mailbox>
        </t:ToRecipients>
        <t:CcRecipients>
          <t:Mailbox><t:Name>Haddad, Omar</t:Name><t:EmailAddress>omar.haddad@example.com</t:EmailAddress></t:Mailbox>
        </t:CcRecipients>
        <t:From><t:Mailbox><t:Name>Ana Ruiz</t:Name><t:EmailAddress>ana.ruiz@example.com</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType></t:Mailbox></t:From>
        <t:InternetMessageId>&lt;survey.2@example.com&gt;</t:InternetMessageId>
        <t:IsRead>true</t:IsRead>
        <t:References>&lt;survey.0@example.com&gt; &lt;survey.1@example.com&gt;</t:References>"#;

    #[test]
    fn a_whole_message_becomes_a_list_row() {
        let remote = item(WHOLE).unwrap().into_remote();
        assert_eq!(
            remote.id,
            RemoteId("AAMkAGI2NGVhZTVlLTI1OGMtNDI4My1iZmE5LTA5OGJiZGEzMTc0YQBGAAAAAAA=".to_owned())
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
                answered: true,
                draft: false,
            }
        );
        assert!(remote.has_attachments);
        assert_eq!(remote.size, 48213);
        // Its own words about what it follows, the parent last and once.
        assert_eq!(
            remote.thread,
            ["<survey.0@example.com>", "<survey.1@example.com>"]
        );
    }

    #[test]
    fn a_draft_with_nothing_filled_in_is_still_a_message() {
        let draft = item(
            r#"<t:ItemId Id="draft-1"/>
               <t:DateTimeSent>2026-11-03T10:00:00Z</t:DateTimeSent>
               <t:IsDraft>true</t:IsDraft>
               <t:IsRead>true</t:IsRead>"#,
        )
        .unwrap();
        let remote = draft.into_remote();
        assert_eq!(remote.headers.subject, "");
        assert!(remote.headers.from.is_empty());
        assert!(remote.headers.to.is_empty() && remote.headers.cc.is_empty());
        assert_eq!(remote.headers.message_id, None);
        // No arrival time, so the time it was written.
        assert_eq!(remote.headers.date, 1_793_700_000);
        assert!(remote.flags.draft && remote.flags.seen && !remote.flags.flagged);
        assert!(remote.thread.is_empty());
        assert_eq!(remote.size, 0);
    }

    #[test]
    fn a_message_that_does_not_say_what_it_follows_is_tied_to_its_conversation() {
        let remote = item(r#"<t:ItemId Id="m1"/><t:ConversationId Id="AAQk AGI2/TOPIC+1="/>"#)
            .unwrap()
            .into_remote();
        assert_eq!(remote.thread, ["<AAQkAGI2/TOPIC+1=@conversation.ews>"]);
        // Only the parent, when that is all the message names.
        let remote = item(
            r#"<t:ItemId Id="m1"/><t:InReplyTo>&lt;a@example.com&gt;</t:InReplyTo>
               <t:ConversationId Id="AAQkTOPIC="/>"#,
        )
        .unwrap()
        .into_remote();
        assert_eq!(remote.thread, ["<a@example.com>"]);
        // A parent the references do not end with comes after them.
        let remote = item(
            r#"<t:ItemId Id="m1"/><t:InReplyTo>&lt;c@example.com&gt;</t:InReplyTo>
               <t:References>&lt;a@example.com&gt;&lt;b@example.com&gt;</t:References>"#,
        )
        .unwrap()
        .into_remote();
        assert_eq!(
            remote.thread,
            ["<a@example.com>", "<b@example.com>", "<c@example.com>"]
        );
    }

    #[test]
    fn a_completed_flag_is_not_flagged() {
        for (status, flagged) in [
            ("Flagged", true),
            ("Complete", false),
            ("NotFlagged", false),
        ] {
            let item = item(&format!(
                "<t:ItemId Id=\"m\"/><t:Flag><t:FlagStatus>{status}</t:FlagStatus></t:Flag>"
            ))
            .unwrap();
            assert_eq!(item.flagged(), flagged, "{status}");
        }
        assert!(!item("<t:ItemId Id=\"m\"/>").unwrap().flagged());
    }

    #[test]
    fn only_a_reply_makes_a_message_answered() {
        for (verb, answered) in [("102", true), ("103", true), ("104", false), ("x", false)] {
            let item = item(&format!(
                "<t:ItemId Id=\"m\"/><t:ExtendedProperty>\
                 <t:ExtendedFieldURI PropertyTag=\"0x1081\" PropertyType=\"Integer\"/>\
                 <t:Value>{verb}</t:Value></t:ExtendedProperty>"
            ))
            .unwrap();
            assert_eq!(item.flags().answered, answered, "{verb}");
        }
        // Some other property is not this one.
        let other = item(
            "<t:ItemId Id=\"m\"/><t:ExtendedProperty>\
             <t:ExtendedFieldURI PropertyTag=\"0x0e07\" PropertyType=\"Integer\"/>\
             <t:Value>102</t:Value></t:ExtendedProperty>",
        )
        .unwrap();
        assert!(!other.flags().answered);
        assert!(same_tag("0x1081", "0X1081") && same_tag("4225", "0x1081"));
        assert!(!same_tag("", "") && !same_tag("0x1081", "0x1082"));
    }

    #[test]
    fn what_is_not_mail_or_has_no_id_is_passed_over() {
        for kind in [
            "Contact",
            "Task",
            "CalendarItem",
            "DistributionList",
            "Item",
        ] {
            let element = types(kind, "<t:ItemId Id=\"c1\"/>");
            assert_eq!(Item::read(&parse(&element).unwrap()), None, "{kind}");
        }
        for kind in ["MeetingRequest", "MeetingResponse", "PostItem"] {
            let element = types(kind, "<t:ItemId Id=\"c1\"/>");
            assert!(Item::read(&parse(&element).unwrap()).is_some(), "{kind}");
        }
        assert_eq!(item("<t:Subject>No id</t:Subject>"), None);
        assert_eq!(item("<t:ItemId/>"), None);
        assert_eq!(item("<t:ItemId Id=\"\"/>"), None);
        assert_eq!(
            item(&format!("<t:ItemId Id=\"{}\"/>", "A".repeat(ID_LIMIT + 1))),
            None
        );
        // The right name in the wrong namespace is not a message.
        assert_eq!(
            Item::read(&parse("<Message><ItemId Id=\"m\"/></Message>").unwrap()),
            None
        );
    }

    #[test]
    fn control_characters_never_reach_a_list_row_and_text_is_bounded() {
        let remote = item(&format!(
            "<t:ItemId Id=\"m\"/>\
             <t:Subject>Invoice&#x1b;]0;gotcha&#7;&#13;&#10;Bcc: x {}</t:Subject>\
             <t:From><t:Mailbox><t:Name>Ana&#x1b;[2J</t:Name>\
             <t:EmailAddress>ana.ruiz@example.com</t:EmailAddress></t:Mailbox></t:From>\
             <t:InternetMessageId> &lt;a&#9;b@example.com&gt;&#13;&#10;</t:InternetMessageId>",
            "x".repeat(5000)
        ))
        .unwrap()
        .into_remote();
        assert!(
            remote
                .headers
                .subject
                .starts_with("Invoice ]0;gotcha   Bcc: x xxx")
        );
        assert_eq!(remote.headers.subject.chars().count(), TEXT_LIMIT);
        assert_eq!(remote.headers.from[0].name.as_deref(), Some("Ana [2J"));
        assert_eq!(
            remote.headers.message_id.as_deref(),
            Some("<a b@example.com>")
        );
        // An id with a control character in it is no id.
        assert_eq!(item("<t:ItemId Id=\"m&#x1b;[2J\"/>"), None);
    }

    #[test]
    fn recipients_and_references_are_bounded() {
        let many: String = (0..RECIPIENTS + 50)
            .map(|i| {
                format!("<t:Mailbox><t:EmailAddress>p{i}@example.com</t:EmailAddress></t:Mailbox>")
            })
            .collect();
        let references: String = (0..REFERENCES + 50)
            .map(|i| format!("&lt;r{i}@example.com&gt; "))
            .collect();
        let remote = item(&format!(
            "<t:ItemId Id=\"m\"/><t:ToRecipients>{many}</t:ToRecipients>\
             <t:References>{references}</t:References>"
        ))
        .unwrap()
        .into_remote();
        assert_eq!(remote.headers.to.len(), RECIPIENTS);
        assert_eq!(remote.thread.len(), REFERENCES);
        // One id as long as a header is not an id.
        let long = format!("<{}@example.com>", "a".repeat(TEXT_LIMIT));
        assert!(message_ids(&long).is_empty());
        assert!(message_ids(long.trim_matches(['<', '>'])).is_empty());
    }

    #[test]
    fn a_sender_known_only_by_a_directory_path_keeps_only_a_name() {
        let mailbox = |inside: &str| address(&parse(&types("Mailbox", inside)).unwrap());
        assert_eq!(
            mailbox(
                "<t:Name>Lena Fischer</t:Name>\
                 <t:EmailAddress>/O=EXAMPLE/OU=EXCHANGE ADMINISTRATIVE GROUP/CN=RECIPIENTS/CN=LENA</t:EmailAddress>\
                 <t:RoutingType>EX</t:RoutingType>"
            ),
            Some(Address {
                name: Some("Lena Fischer".to_owned()),
                email: String::new(),
            })
        );
        // A folder sync gives a sender's name and nothing else.
        assert_eq!(
            mailbox("<t:Name>Ana Ruiz</t:Name>"),
            Some(Address {
                name: Some("Ana Ruiz".to_owned()),
                email: String::new(),
            })
        );
        assert_eq!(mailbox(""), None);
        assert_eq!(
            mailbox(
                "<t:EmailAddress>/O=EXAMPLE/CN=X</t:EmailAddress><t:RoutingType>EX</t:RoutingType>"
            ),
            None
        );
    }

    #[test]
    fn message_ids_always_have_their_brackets() {
        assert_eq!(message_ids("abc@example.com"), ["<abc@example.com>"]);
        assert_eq!(message_ids("<abc@example.com>"), ["<abc@example.com>"]);
        assert_eq!(
            message_ids("<a@example.com>\r\n\t<b@example.com>,<c@example.com> trailing"),
            ["<a@example.com>", "<b@example.com>", "<c@example.com>"]
        );
        assert!(message_ids("  ").is_empty());
        assert!(message_ids("<>").is_empty());
        assert!(message_ids("two words").is_empty());
        assert_eq!(
            message_ids("<a@example.com> <unfinished"),
            ["<a@example.com>"]
        );
    }

    #[test]
    fn a_time_that_cannot_be_read_is_passed_over() {
        let remote = item(
            "<t:ItemId Id=\"m\"/><t:DateTimeReceived>yesterday</t:DateTimeReceived>\
             <t:DateTimeSent>2026-11-03T09:15:00+01:00</t:DateTimeSent>",
        )
        .unwrap()
        .into_remote();
        assert_eq!(remote.headers.date, 1_793_693_700);
        assert_eq!(
            item("<t:ItemId Id=\"m\"/>")
                .unwrap()
                .into_remote()
                .headers
                .date,
            0
        );
    }

    #[test]
    fn a_folder_holds_mail_unless_it_says_otherwise() {
        let folder = |kind: &str, inside: &str| Folder::read(&parse(&types(kind, inside)).unwrap());
        let inbox = folder(
            "Folder",
            "<t:FolderId Id=\"folder-inbox\" ChangeKey=\"AQAAAA==\"/>\
             <t:ParentFolderId Id=\"folder-top\" ChangeKey=\"AQAAAA==\"/>\
             <t:FolderClass>IPF.Note</t:FolderClass><t:DisplayName>Inbox</t:DisplayName>",
        )
        .unwrap();
        assert_eq!(
            inbox,
            Folder {
                id: "folder-inbox".to_owned(),
                parent: Some("folder-top".to_owned()),
                name: "Inbox".to_owned(),
                mail: true,
            }
        );
        for (kind, class, mail) in [
            ("Folder", "", true),
            ("Folder", "ipf.note", true),
            ("Folder", "IPF.Note.OutlookHomepage", true),
            ("Folder", "IPF.Notebook", false),
            ("Folder", "IPF.Configuration", false),
            ("Folder", "IPF.StickyNote", false),
            ("CalendarFolder", "IPF.Appointment", false),
            ("ContactsFolder", "IPF.Contact", false),
            ("TasksFolder", "IPF.Task", false),
            ("SearchFolder", "IPF.Note", false),
        ] {
            let read = folder(
                kind,
                &format!("<t:FolderId Id=\"f\"/><t:FolderClass>{class}</t:FolderClass>"),
            )
            .unwrap();
            assert_eq!(read.mail, mail, "{kind} {class}");
            // Without a name it goes by its id.
            assert_eq!(read.name, "f");
            assert_eq!(read.parent, None);
        }
        assert_eq!(
            folder("Folder", "<t:DisplayName>No id</t:DisplayName>"),
            None
        );
        let named = folder(
            "Folder",
            "<t:FolderId Id=\"f\"/><t:DisplayName>Pro&#x1b;[2Jjects</t:DisplayName>",
        )
        .unwrap();
        assert_eq!(named.name, "Pro [2Jjects");
    }

    #[test]
    fn truth_is_read_as_xml_schema_writes_it() {
        for yes in ["true", "True", "1"] {
            assert!(is_true(yes), "{yes}");
        }
        for no in ["false", "0", "", "yes"] {
            assert!(!is_true(no), "{no}");
        }
    }
}
