//! What is asked of EWS: one SOAP envelope per operation.
//!
//! Elements are written in the order the EWS schema lists them, which the
//! server insists on. Every id and every other value from outside goes
//! through the writer's escaping.

use bytes::Bytes;

use crate::xml::{MESSAGES, SOAP, TYPES, Writer};

/// The schema every request asks to be answered in.
///
/// Exchange 2013's is the oldest that has the follow-up flag as a property
/// of its own (`item:Flag`), and its first service pack's the oldest in
/// which a message can be marked read without a read receipt being sent
/// behind the reader's back. A server older than that (2007, 2010, 2013
/// as first released) refuses the version outright.
pub(crate) const SERVER_VERSION: &str = "Exchange2013_SP1";

/// The property Outlook calls the message flags (`PR_MESSAGE_FLAGS`). Set
/// on a message as it is made, it says whether the message is a draft.
const MESSAGE_FLAGS: &str = "0x0E07";

/// The property that says what was last done with a message
/// (`PR_LAST_VERB_EXECUTED`): replied to, forwarded.
pub(crate) const LAST_VERB: &str = "0x1081";

/// EWS's fixed name for the folder all of a mailbox's own folders are in:
/// the top of what Outlook shows.
const TOP_OF_MAILBOX: &str = "msgfolderroot";

/// What a list row needs of a message, as far as a folder sync gives it.
const LISTED: [&str; 11] = [
    "item:Subject",
    "item:DateTimeReceived",
    "item:DateTimeSent",
    "item:Size",
    "item:HasAttachments",
    "item:IsDraft",
    "item:ConversationId",
    "item:Flag",
    "message:From",
    "message:IsRead",
    "message:InternetMessageId",
];

/// What a folder sync leaves out or cuts short, and only a message asked
/// for by id comes with: who it is to, and what it answers.
const BY_ID_ONLY: [&str; 4] = [
    "item:InReplyTo",
    "message:ToRecipients",
    "message:CcRecipients",
    "message:References",
];

/// One request, ready to post. The body is kept as `Bytes` so that a
/// second try does not copy it.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// The operation's name, for the `SOAPAction` header.
    pub operation: &'static str,
    pub body: Bytes,
}

fn envelope(operation: &'static str, write: impl FnOnce(&mut Writer)) -> Request {
    let mut xml = Writer::new();
    xml.open(
        "soap:Envelope",
        &[
            ("xmlns:soap", SOAP),
            ("xmlns:t", TYPES),
            ("xmlns:m", MESSAGES),
        ],
    );
    xml.open("soap:Header", &[]);
    xml.empty("t:RequestServerVersion", &[("Version", SERVER_VERSION)]);
    xml.close();
    xml.open("soap:Body", &[]);
    write(&mut xml);
    Request {
        operation,
        body: xml.finish().into(),
    }
}

/// `IdOnly` and these properties besides.
fn shape(xml: &mut Writer, name: &'static str, properties: &[&str], last_verb: bool) {
    xml.open(name, &[]);
    xml.text("t:BaseShape", "IdOnly");
    xml.open("t:AdditionalProperties", &[]);
    for property in properties {
        xml.empty("t:FieldURI", &[("FieldURI", property)]);
    }
    if last_verb {
        xml.empty(
            "t:ExtendedFieldURI",
            &[("PropertyTag", LAST_VERB), ("PropertyType", "Integer")],
        );
    }
    xml.close();
    xml.close();
}

fn item_ids(xml: &mut Writer, ids: &[&str]) {
    xml.open("m:ItemIds", &[]);
    for id in ids {
        xml.empty("t:ItemId", &[("Id", id)]);
    }
    xml.close();
}

/// The ids of folders known by a fixed name: `inbox`, `sentitems`.
pub(crate) fn get_folders(names: &[&'static str]) -> Request {
    envelope("GetFolder", |xml| {
        xml.open("m:GetFolder", &[]);
        xml.open("m:FolderShape", &[]);
        xml.text("t:BaseShape", "IdOnly");
        xml.close();
        xml.open("m:FolderIds", &[]);
        for name in names {
            xml.empty("t:DistinguishedFolderId", &[("Id", name)]);
        }
    })
}

/// One page of every folder under the top of the mailbox, however deep,
/// from the `offset`th on.
pub(crate) fn find_folders(offset: usize, page: usize) -> Request {
    envelope("FindFolder", |xml| {
        xml.open("m:FindFolder", &[("Traversal", "Deep")]);
        shape(
            xml,
            "m:FolderShape",
            &[
                "folder:DisplayName",
                "folder:ParentFolderId",
                "folder:FolderClass",
            ],
            false,
        );
        xml.empty(
            "m:IndexedPageFolderView",
            &[
                ("MaxEntriesReturned", &page.to_string()),
                ("Offset", &offset.to_string()),
                ("BasePoint", "Beginning"),
            ],
        );
        xml.open("m:ParentFolderIds", &[]);
        xml.empty("t:DistinguishedFolderId", &[("Id", TOP_OF_MAILBOX)]);
    })
}

/// A folder as it stands, by its id: asked for the mark of its last
/// change, which a change to it has to carry.
pub(crate) fn get_folder(id: &str) -> Request {
    envelope("GetFolder", |xml| {
        xml.open("m:GetFolder", &[]);
        xml.open("m:FolderShape", &[]);
        xml.text("t:BaseShape", "IdOnly");
        xml.close();
        xml.open("m:FolderIds", &[]);
        xml.empty("t:FolderId", &[("Id", id)]);
    })
}

/// A new mail folder called `name`, inside `parent` or at the top of the
/// mailbox.
pub(crate) fn create_folder(parent: Option<&str>, name: &str) -> Request {
    envelope("CreateFolder", |xml| {
        xml.open("m:CreateFolder", &[]);
        xml.open("m:ParentFolderId", &[]);
        match parent {
            Some(parent) => xml.empty("t:FolderId", &[("Id", parent)]),
            None => xml.empty("t:DistinguishedFolderId", &[("Id", TOP_OF_MAILBOX)]),
        };
        xml.close();
        xml.open("m:Folders", &[]);
        xml.open("t:Folder", &[]);
        xml.text("t:FolderClass", "IPF.Note");
        xml.text("t:DisplayName", name);
    })
}

/// Gives a folder another name. `change_key` is the mark of the folder's
/// last change, when the server gave one.
pub(crate) fn rename_folder(id: &str, change_key: Option<&str>, name: &str) -> Request {
    envelope("UpdateFolder", |xml| {
        xml.open("m:UpdateFolder", &[]);
        xml.open("m:FolderChanges", &[]);
        xml.open("t:FolderChange", &[]);
        match change_key {
            Some(key) => xml.empty("t:FolderId", &[("Id", id), ("ChangeKey", key)]),
            None => xml.empty("t:FolderId", &[("Id", id)]),
        };
        xml.open("t:Updates", &[]);
        xml.open("t:SetFolderField", &[]);
        xml.empty("t:FieldURI", &[("FieldURI", "folder:DisplayName")]);
        xml.open("t:Folder", &[]);
        xml.text("t:DisplayName", name);
    })
}

/// Deletes a folder by moving it into Deleted Items with what is in it:
/// never for good.
pub(crate) fn delete_folder(id: &str) -> Request {
    envelope("DeleteFolder", |xml| {
        xml.open("m:DeleteFolder", &[("DeleteType", "MoveToDeletedItems")]);
        xml.open("m:FolderIds", &[]);
        xml.empty("t:FolderId", &[("Id", id)]);
    })
}

/// What changed in a folder since `state`, or all of it from the start.
pub(crate) fn sync_items(folder: &str, state: Option<&str>, most: usize) -> Request {
    envelope("SyncFolderItems", |xml| {
        xml.open("m:SyncFolderItems", &[]);
        shape(xml, "m:ItemShape", &LISTED, true);
        xml.open("m:SyncFolderId", &[]);
        xml.empty("t:FolderId", &[("Id", folder)]);
        xml.close();
        if let Some(state) = state {
            xml.text("m:SyncState", state);
        }
        xml.text("m:MaxChangesReturned", &most.to_string());
        // Not the hidden items programs keep their settings in.
        xml.text("m:SyncScope", "NormalItems");
    })
}

/// Everything a list row needs of each of these messages.
pub(crate) fn get_summaries(ids: &[&str]) -> Request {
    envelope("GetItem", |xml| {
        xml.open("m:GetItem", &[]);
        let mut properties = LISTED.to_vec();
        properties.extend(BY_ID_ONLY);
        shape(xml, "m:ItemShape", &properties, true);
        item_ids(xml, ids);
    })
}

/// A message's read mark and its flag.
pub(crate) fn get_marks(id: &str) -> Request {
    envelope("GetItem", |xml| {
        xml.open("m:GetItem", &[]);
        shape(xml, "m:ItemShape", &["item:Flag", "message:IsRead"], false);
        item_ids(xml, &[id]);
    })
}

/// These messages whole, as the bytes they arrived as.
pub(crate) fn get_mime(ids: &[&str]) -> Request {
    envelope("GetItem", |xml| {
        xml.open("m:GetItem", &[]);
        xml.open("m:ItemShape", &[]);
        xml.text("t:BaseShape", "IdOnly");
        xml.text("t:IncludeMimeContent", "true");
        xml.close();
        item_ids(xml, ids);
    })
}

/// Sets a message's read mark, its flag, or both. `None` leaves one as it
/// is.
///
/// The server's copy is overwritten whatever has happened to it since it
/// was last looked at, and no read receipt is sent on the reader's behalf.
pub(crate) fn update_marks(id: &str, read: Option<bool>, flagged: Option<bool>) -> Request {
    envelope("UpdateItem", |xml| {
        xml.open(
            "m:UpdateItem",
            &[
                ("MessageDisposition", "SaveOnly"),
                ("ConflictResolution", "AlwaysOverwrite"),
                ("SuppressReadReceipts", "true"),
            ],
        );
        xml.open("m:ItemChanges", &[]);
        xml.open("t:ItemChange", &[]);
        xml.empty("t:ItemId", &[("Id", id)]);
        xml.open("t:Updates", &[]);
        if let Some(read) = read {
            xml.open("t:SetItemField", &[]);
            xml.empty("t:FieldURI", &[("FieldURI", "message:IsRead")]);
            xml.open("t:Message", &[]);
            xml.text("t:IsRead", truth(read));
            xml.close();
            xml.close();
        }
        if let Some(flagged) = flagged {
            xml.open("t:SetItemField", &[]);
            xml.empty("t:FieldURI", &[("FieldURI", "item:Flag")]);
            xml.open("t:Message", &[]);
            flag(xml, flagged);
            xml.close();
            xml.close();
        }
    })
}

fn flag(xml: &mut Writer, flagged: bool) {
    xml.open("t:Flag", &[]);
    xml.text(
        "t:FlagStatus",
        if flagged { "Flagged" } else { "NotFlagged" },
    );
    xml.close();
}

fn truth(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn to_folder(xml: &mut Writer, id: &str, to: &str) {
    xml.open("m:ToFolderId", &[]);
    xml.empty("t:FolderId", &[("Id", to)]);
    xml.close();
    item_ids(xml, &[id]);
}

/// Moves a message, and asks what it is called where it went: a message
/// has another id in every folder.
pub(crate) fn move_item(id: &str, to: &str) -> Request {
    envelope("MoveItem", |xml| {
        xml.open("m:MoveItem", &[]);
        to_folder(xml, id, to);
        xml.text("m:ReturnNewItemIds", "true");
    })
}

pub(crate) fn copy_item(id: &str, to: &str) -> Request {
    envelope("CopyItem", |xml| {
        xml.open("m:CopyItem", &[]);
        to_folder(xml, id, to);
    })
}

/// Removes these messages for good: not to Deleted Items, which is a move.
///
/// Nobody is sent anything because of it: no "deleted unread" receipt, and
/// no meeting answer should one of them be an invitation.
pub(crate) fn delete_items(ids: &[&str]) -> Request {
    envelope("DeleteItem", |xml| {
        xml.open(
            "m:DeleteItem",
            &[
                ("DeleteType", "HardDelete"),
                ("SendMeetingCancellations", "SendToNone"),
                ("AffectedTaskOccurrences", "AllOccurrences"),
                ("SuppressReadReceipts", "true"),
            ],
        );
        item_ids(xml, ids);
    })
}

/// How a message saved into a folder is to be marked there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Marks {
    pub read: bool,
    pub flagged: bool,
    pub draft: bool,
}

/// Saves a complete message into a folder without sending it.
///
/// Exchange takes every message made this way for an unsent draft unless
/// its message flags say otherwise, and they can only say so while the
/// message is being made. So for anything but a draft they are set here:
/// `1` for a message that has been read, `0` for one that has not.
pub(crate) fn save_item(folder: &str, raw: &[u8], marks: Marks) -> Request {
    envelope("CreateItem", |xml| {
        xml.open("m:CreateItem", &[("MessageDisposition", "SaveOnly")]);
        xml.open("m:SavedItemFolderId", &[]);
        xml.empty("t:FolderId", &[("Id", folder)]);
        xml.close();
        xml.open("m:Items", &[]);
        xml.open("t:Message", &[]);
        xml.base64("t:MimeContent", &[("CharacterSet", "UTF-8")], raw);
        if !marks.draft {
            xml.open("t:ExtendedProperty", &[]);
            xml.empty(
                "t:ExtendedFieldURI",
                &[("PropertyTag", MESSAGE_FLAGS), ("PropertyType", "Integer")],
            );
            xml.text("t:Value", if marks.read { "1" } else { "0" });
            xml.close();
        }
        if marks.flagged {
            flag(xml, true);
        }
        xml.text("t:IsRead", truth(marks.read));
    })
}

/// Sends a complete message, and has Exchange file its copy in Sent Items.
pub(crate) fn send_item(raw: &[u8]) -> Request {
    envelope("CreateItem", |xml| {
        xml.open("m:CreateItem", &[("MessageDisposition", "SendAndSaveCopy")]);
        xml.open("m:SavedItemFolderId", &[]);
        xml.empty("t:DistinguishedFolderId", &[("Id", "sentitems")]);
        xml.close();
        xml.open("m:Items", &[]);
        xml.open("t:Message", &[]);
        xml.base64("t:MimeContent", &[("CharacterSet", "UTF-8")], raw);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
        <soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\" \
        xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\" \
        xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\">\
        <soap:Header><t:RequestServerVersion Version=\"Exchange2013_SP1\"/></soap:Header>\
        <soap:Body>";
    const TAIL: &str = "</soap:Body></soap:Envelope>";

    /// What the request asks, without the envelope every request has.
    fn asked(request: &Request) -> &str {
        std::str::from_utf8(&request.body)
            .unwrap()
            .strip_prefix(HEAD)
            .unwrap()
            .strip_suffix(TAIL)
            .unwrap()
    }

    #[test]
    fn every_request_is_a_whole_envelope_asking_for_one_schema() {
        let request = get_mime(&["AAMk1"]);
        assert_eq!(request.operation, "GetItem");
        assert_eq!(
            asked(&request),
            "<m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape>\
             <t:IncludeMimeContent>true</t:IncludeMimeContent></m:ItemShape>\
             <m:ItemIds><t:ItemId Id=\"AAMk1\"/></m:ItemIds></m:GetItem>"
        );
        // And it reads back as XML.
        let envelope = crate::xml::parse(std::str::from_utf8(&request.body).unwrap()).unwrap();
        assert_eq!(envelope.children().len(), 2);
    }

    #[test]
    fn an_id_is_only_ever_an_id() {
        let hostile = "x\"/></m:ItemIds></m:DeleteItem><m:DeleteItem DeleteType=\"HardDelete\"><m:ItemIds><t:ItemId Id=\"inbox";
        for request in [
            delete_items(&[hostile]),
            get_mime(&[hostile]),
            get_summaries(&[hostile]),
            get_marks(hostile),
            update_marks(hostile, Some(true), Some(true)),
            move_item(hostile, hostile),
            copy_item(hostile, hostile),
            sync_items(hostile, Some(hostile), 512),
            save_item(
                hostile,
                b"x",
                Marks {
                    read: true,
                    flagged: true,
                    draft: false,
                },
            ),
        ] {
            let text = std::str::from_utf8(&request.body).unwrap();
            let envelope = crate::xml::parse(text).unwrap();
            // One operation in the body, and the id somewhere in it whole.
            let body = &envelope.children()[1];
            assert_eq!(body.children().len(), 1, "{text}");
            let operation = &body.children()[0];
            assert_eq!(operation.name(), request.operation);
            assert!(
                operation
                    .find(&|element| element.attribute("Id") == Some(hostile))
                    .is_some(),
                "{text}"
            );
            assert_eq!(
                text.matches("<m:DeleteItem ").count(),
                usize::from(request.operation == "DeleteItem")
            );
        }
    }

    #[test]
    fn a_folder_sync_asks_for_a_row_and_says_where_it_stopped() {
        assert_eq!(
            asked(&sync_items("folder-inbox", Some("H4sIAAA="), 512)),
            "<m:SyncFolderItems><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape>\
             <t:AdditionalProperties>\
             <t:FieldURI FieldURI=\"item:Subject\"/>\
             <t:FieldURI FieldURI=\"item:DateTimeReceived\"/>\
             <t:FieldURI FieldURI=\"item:DateTimeSent\"/>\
             <t:FieldURI FieldURI=\"item:Size\"/>\
             <t:FieldURI FieldURI=\"item:HasAttachments\"/>\
             <t:FieldURI FieldURI=\"item:IsDraft\"/>\
             <t:FieldURI FieldURI=\"item:ConversationId\"/>\
             <t:FieldURI FieldURI=\"item:Flag\"/>\
             <t:FieldURI FieldURI=\"message:From\"/>\
             <t:FieldURI FieldURI=\"message:IsRead\"/>\
             <t:FieldURI FieldURI=\"message:InternetMessageId\"/>\
             <t:ExtendedFieldURI PropertyTag=\"0x1081\" PropertyType=\"Integer\"/>\
             </t:AdditionalProperties></m:ItemShape>\
             <m:SyncFolderId><t:FolderId Id=\"folder-inbox\"/></m:SyncFolderId>\
             <m:SyncState>H4sIAAA=</m:SyncState>\
             <m:MaxChangesReturned>512</m:MaxChangesReturned>\
             <m:SyncScope>NormalItems</m:SyncScope></m:SyncFolderItems>"
        );
        // From the start there is no state to give.
        assert!(!asked(&sync_items("folder-inbox", None, 10)).contains("SyncState"));
    }

    #[test]
    fn a_saved_message_says_it_is_no_draft_while_it_is_made() {
        let saved = |marks| {
            let request = save_item("folder-sent", b"Subject: x\r\n\r\n", marks);
            asked(&request).to_owned()
        };
        assert_eq!(
            saved(Marks {
                read: true,
                flagged: false,
                draft: false
            }),
            "<m:CreateItem MessageDisposition=\"SaveOnly\">\
             <m:SavedItemFolderId><t:FolderId Id=\"folder-sent\"/></m:SavedItemFolderId>\
             <m:Items><t:Message>\
             <t:MimeContent CharacterSet=\"UTF-8\">U3ViamVjdDogeA0KDQo=</t:MimeContent>\
             <t:ExtendedProperty>\
             <t:ExtendedFieldURI PropertyTag=\"0x0E07\" PropertyType=\"Integer\"/>\
             <t:Value>1</t:Value></t:ExtendedProperty>\
             <t:IsRead>true</t:IsRead>\
             </t:Message></m:Items></m:CreateItem>"
        );
        let unread = saved(Marks {
            read: false,
            flagged: true,
            draft: false,
        });
        assert!(unread.contains("<t:Value>0</t:Value></t:ExtendedProperty>"));
        assert!(unread.contains(
            "<t:Flag><t:FlagStatus>Flagged</t:FlagStatus></t:Flag><t:IsRead>false</t:IsRead>"
        ));
        // A draft is what Exchange makes of a message left to itself.
        let draft = saved(Marks {
            read: true,
            flagged: false,
            draft: true,
        });
        assert!(!draft.contains("ExtendedProperty"));
        assert!(draft.contains("</t:MimeContent><t:IsRead>true</t:IsRead>"));
    }

    #[test]
    fn marks_are_written_one_by_one() {
        assert_eq!(
            asked(&update_marks("AAMk1", Some(false), None)),
            "<m:UpdateItem MessageDisposition=\"SaveOnly\" ConflictResolution=\"AlwaysOverwrite\" \
             SuppressReadReceipts=\"true\"><m:ItemChanges><t:ItemChange>\
             <t:ItemId Id=\"AAMk1\"/><t:Updates>\
             <t:SetItemField><t:FieldURI FieldURI=\"message:IsRead\"/>\
             <t:Message><t:IsRead>false</t:IsRead></t:Message></t:SetItemField>\
             </t:Updates></t:ItemChange></m:ItemChanges></m:UpdateItem>"
        );
        let both = update_marks("AAMk1", Some(true), Some(false));
        assert!(asked(&both).contains(
            "<t:SetItemField><t:FieldURI FieldURI=\"item:Flag\"/><t:Message><t:Flag>\
             <t:FlagStatus>NotFlagged</t:FlagStatus></t:Flag></t:Message></t:SetItemField>"
        ));
    }
}
