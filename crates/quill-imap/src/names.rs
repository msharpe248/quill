//! Mailbox names: decoding them for display and working out what a folder
//! is for.

use async_imap::types::NameAttribute;
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use quill_core::types::FolderRole;

/// Decodes a mailbox name from IMAP's modified UTF-7 (RFC 3501 section
/// 5.1.3). A run that does not decode is kept as it was written, so a name
/// is never lost, only shown raw.
pub(crate) fn decode_utf7(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            // An unterminated shift: not valid, so leave it alone.
            out.push_str(&rest[start..]);
            return out;
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            match decode_run(encoded) {
                Some(text) => out.push_str(&text),
                None => out.push_str(&rest[start..=start + 1 + end]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// One shifted run: base64 with `,` for `/`, holding UTF-16BE.
fn decode_run(encoded: &str) -> Option<String> {
    let bytes = STANDARD_NO_PAD.decode(encoded.replace(',', "/")).ok()?;
    let (pairs, odd) = bytes.as_chunks::<2>();
    if !odd.is_empty() {
        return None;
    }
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
    String::from_utf16(&units).ok()
}

/// A mailbox as the server lists it.
pub(crate) struct Listed<'a> {
    /// The name as the server writes it: what every later command uses.
    pub raw: &'a str,
    /// What separates the levels of the server's folder tree, if it has one.
    pub delimiter: Option<&'a str>,
    pub attributes: &'a [NameAttribute<'a>],
}

/// Turns the server's list into folders to show: a readable name and a role
/// for each. The order follows `listed`.
///
/// Some servers (Courier, and Dovecot set up the same way) keep every folder
/// inside the inbox, so the list reads `INBOX`, `INBOX.Sent`, `INBOX.Trash`.
/// That prefix is where the server keeps mail, not something the user named,
/// so it is left out of the names shown.
pub(crate) fn folders(listed: &[Listed<'_>]) -> Vec<(String, FolderRole)> {
    let prefix = inbox_prefix(listed);
    let shown: Vec<(String, Option<FolderRole>, FolderRole)> = listed
        .iter()
        .map(|folder| {
            if folder.raw.eq_ignore_ascii_case("INBOX") {
                return (
                    "Inbox".to_owned(),
                    Some(FolderRole::Inbox),
                    FolderRole::Other,
                );
            }
            let path = prefix
                .as_deref()
                .and_then(|prefix| strip_prefix_ignoring_case(folder.raw, prefix))
                .unwrap_or(folder.raw);
            let top_level = folder
                .delimiter
                .is_none_or(|delimiter| delimiter.is_empty() || !path.contains(delimiter));
            let name = match folder.delimiter {
                Some(delimiter) if !delimiter.is_empty() && delimiter != "/" => {
                    decode_utf7(path).replace(delimiter, "/")
                }
                _ => decode_utf7(path),
            };
            // Only a top-level folder is guessed at: "Projects/Sent" is not Sent.
            let guessed = if top_level {
                role_from_name(path)
            } else {
                FolderRole::Other
            };
            (name, declared_role(folder.attributes), guessed)
        })
        .collect();

    // What the server declares always stands. A guess from a name stands only
    // where the server declared no folder for that role, and only for the
    // first such name: one "Junk" beside a declared junk folder, or a "Spam"
    // beside a "Junk", is an ordinary folder.
    let mut taken: Vec<FolderRole> = shown
        .iter()
        .filter_map(|(_, declared, _)| *declared)
        .collect();
    shown
        .into_iter()
        .map(|(name, declared, guessed)| {
            let role = match declared {
                Some(role) => role,
                None if guessed != FolderRole::Other && !taken.contains(&guessed) => {
                    taken.push(guessed);
                    guessed
                }
                None => FolderRole::Other,
            };
            (name, role)
        })
        .collect()
}

/// `INBOX` and its delimiter, when every folder but the inbox lives under it.
fn inbox_prefix(listed: &[Listed<'_>]) -> Option<String> {
    let inbox = listed
        .iter()
        .find(|folder| folder.raw.eq_ignore_ascii_case("INBOX"))?;
    let delimiter = inbox.delimiter.filter(|delimiter| !delimiter.is_empty())?;
    let prefix = format!("{}{delimiter}", inbox.raw);
    let mut others = listed
        .iter()
        .filter(|folder| !folder.raw.eq_ignore_ascii_case("INBOX"))
        .peekable();
    others.peek()?;
    others
        .all(|folder| {
            strip_prefix_ignoring_case(folder.raw, &prefix).is_some_and(|rest| !rest.is_empty())
        })
        .then_some(prefix)
}

fn strip_prefix_ignoring_case<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let head = name.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &name[prefix.len()..])
}

/// The role the server's SPECIAL-USE attributes give a folder, if any.
fn declared_role(attributes: &[NameAttribute<'_>]) -> Option<FolderRole> {
    attributes.iter().find_map(|attribute| match attribute {
        NameAttribute::Sent => Some(FolderRole::Sent),
        NameAttribute::Drafts => Some(FolderRole::Drafts),
        NameAttribute::Trash => Some(FolderRole::Trash),
        NameAttribute::Archive | NameAttribute::All => Some(FolderRole::Archive),
        NameAttribute::Junk => Some(FolderRole::Junk),
        _ => None,
    })
}

/// The role a folder's name suggests, from the names servers commonly use.
fn role_from_name(name: &str) -> FolderRole {
    match name.to_ascii_lowercase().as_str() {
        "sent" | "sent items" | "sent messages" | "sent mail" => FolderRole::Sent,
        "drafts" | "draft" => FolderRole::Drafts,
        "trash" | "deleted items" | "deleted messages" | "bin" => FolderRole::Trash,
        "archive" | "archives" => FolderRole::Archive,
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => FolderRole::Junk,
        _ => FolderRole::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_pass_through() {
        assert_eq!(decode_utf7("INBOX"), "INBOX");
        assert_eq!(decode_utf7("Work/Projects 2026"), "Work/Projects 2026");
        assert_eq!(decode_utf7(""), "");
    }

    #[test]
    fn shifted_runs_are_decoded() {
        // Examples from RFC 3501 and common servers.
        assert_eq!(decode_utf7("Entw&APw-rfe"), "Entwürfe");
        assert_eq!(decode_utf7("&U,BTFw-"), "台北");
        assert_eq!(
            decode_utf7("~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
            "~peter/mail/台北/日本語"
        );
        assert_eq!(decode_utf7("&2D3eAA-"), "😀");
        assert_eq!(decode_utf7("Tom &- Jerry"), "Tom & Jerry");
        assert_eq!(decode_utf7("&-"), "&");
    }

    #[test]
    fn what_does_not_decode_is_kept_as_written() {
        assert_eq!(decode_utf7("R&D"), "R&D");
        assert_eq!(decode_utf7("a&!!!-b"), "a&!!!-b");
        assert_eq!(decode_utf7("&AA-"), "&AA-");
        // A lone surrogate is not text.
        assert_eq!(decode_utf7("&2D0-x"), "&2D0-x");
    }

    fn listed<'a>(
        raw: &'a str,
        delimiter: &'a str,
        attributes: &'a [NameAttribute<'a>],
    ) -> Listed<'a> {
        Listed {
            raw,
            delimiter: Some(delimiter),
            attributes,
        }
    }

    fn plain<'a>(names: &[&'a str], delimiter: &'a str) -> Vec<(String, FolderRole)> {
        let listed: Vec<_> = names
            .iter()
            .map(|name| listed(name, delimiter, &[]))
            .collect();
        folders(&listed)
    }

    fn one(name: &str, attributes: &[NameAttribute<'_>]) -> FolderRole {
        folders(&[listed(name, "/", attributes)])[0].1
    }

    #[test]
    fn special_use_attributes_decide_the_role() {
        assert_eq!(one("Gesendet", &[NameAttribute::Sent]), FolderRole::Sent);
        assert_eq!(
            one("Papierkorb", &[NameAttribute::Trash]),
            FolderRole::Trash
        );
        assert_eq!(
            one("[Gmail]/All Mail", &[NameAttribute::All]),
            FolderRole::Archive
        );
        assert_eq!(
            one(
                "Whatever",
                &[NameAttribute::NoInferiors, NameAttribute::Junk]
            ),
            FolderRole::Junk
        );
        // INBOX is the inbox whatever else the server says about it.
        assert_eq!(one("inbox", &[NameAttribute::Sent]), FolderRole::Inbox);
        // An attribute beats a name.
        assert_eq!(one("Sent", &[NameAttribute::Trash]), FolderRole::Trash);
    }

    #[test]
    fn common_names_are_the_fallback() {
        assert_eq!(one("Sent Items", &[]), FolderRole::Sent);
        assert_eq!(one("Deleted Items", &[]), FolderRole::Trash);
        assert_eq!(one("Spam", &[]), FolderRole::Junk);
        assert_eq!(one("Drafts", &[]), FolderRole::Drafts);
        assert_eq!(one("Projects/Sent", &[]), FolderRole::Other);
        assert_eq!(one("Receipts", &[]), FolderRole::Other);
    }

    #[test]
    fn the_inbox_is_called_inbox() {
        assert_eq!(
            plain(&["INBOX", "Sent"], "/"),
            [
                ("Inbox".to_owned(), FolderRole::Inbox),
                ("Sent".to_owned(), FolderRole::Sent)
            ]
        );
    }

    #[test]
    fn a_server_that_keeps_everything_inside_the_inbox_loses_the_prefix() {
        let sent = [NameAttribute::Sent];
        let junk = [NameAttribute::Junk];
        let listed = [
            listed("INBOX", ".", &[]),
            listed("INBOX.Archive", ".", &[]),
            listed("INBOX.Junk", ".", &[]),
            listed("INBOX.Sent", ".", &sent),
            listed("INBOX.spam", ".", &junk),
            listed("INBOX.Projects.2026", ".", &[]),
            listed("INBOX.Projects.Sent", ".", &[]),
            listed("INBOX.Entw&APw-rfe", ".", &[]),
        ];
        assert_eq!(
            folders(&listed),
            [
                ("Inbox".to_owned(), FolderRole::Inbox),
                ("Archive".to_owned(), FolderRole::Archive),
                // The server says which folder is for junk, so a folder that
                // is merely called Junk is an ordinary one.
                ("Junk".to_owned(), FolderRole::Other),
                ("Sent".to_owned(), FolderRole::Sent),
                ("spam".to_owned(), FolderRole::Junk),
                ("Projects/2026".to_owned(), FolderRole::Other),
                ("Projects/Sent".to_owned(), FolderRole::Other),
                ("Entwürfe".to_owned(), FolderRole::Other),
            ]
        );
    }

    #[test]
    fn folders_inside_the_inbox_keep_their_path_when_others_live_beside_it() {
        assert_eq!(
            plain(&["INBOX", "INBOX/Receipts", "Sent"], "/"),
            [
                ("Inbox".to_owned(), FolderRole::Inbox),
                ("INBOX/Receipts".to_owned(), FolderRole::Other),
                ("Sent".to_owned(), FolderRole::Sent),
            ]
        );
    }

    #[test]
    fn only_the_first_guess_at_a_role_stands() {
        assert_eq!(
            plain(&["INBOX", "Junk", "Spam"], "/"),
            [
                ("Inbox".to_owned(), FolderRole::Inbox),
                ("Junk".to_owned(), FolderRole::Junk),
                ("Spam".to_owned(), FolderRole::Other),
            ]
        );
    }
}
