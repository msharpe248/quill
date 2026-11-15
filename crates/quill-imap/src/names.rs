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

/// What a folder is for: from the server's SPECIAL-USE attributes when it
/// gives them, otherwise from the names servers commonly use.
pub(crate) fn role(name: &str, attributes: &[NameAttribute<'_>]) -> FolderRole {
    if name.eq_ignore_ascii_case("INBOX") {
        return FolderRole::Inbox;
    }
    for attribute in attributes {
        let role = match attribute {
            NameAttribute::Sent => FolderRole::Sent,
            NameAttribute::Drafts => FolderRole::Drafts,
            NameAttribute::Trash => FolderRole::Trash,
            NameAttribute::Archive | NameAttribute::All => FolderRole::Archive,
            NameAttribute::Junk => FolderRole::Junk,
            _ => continue,
        };
        return role;
    }
    // Only a top-level folder is guessed at: "Projects/Sent" is not Sent.
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

    #[test]
    fn special_use_attributes_decide_the_role() {
        assert_eq!(role("Gesendet", &[NameAttribute::Sent]), FolderRole::Sent);
        assert_eq!(
            role("Papierkorb", &[NameAttribute::Trash]),
            FolderRole::Trash
        );
        assert_eq!(
            role("[Gmail]/All Mail", &[NameAttribute::All]),
            FolderRole::Archive
        );
        assert_eq!(
            role(
                "Whatever",
                &[NameAttribute::NoInferiors, NameAttribute::Junk]
            ),
            FolderRole::Junk
        );
        // INBOX is the inbox whatever else the server says about it.
        assert_eq!(role("inbox", &[NameAttribute::Sent]), FolderRole::Inbox);
    }

    #[test]
    fn common_names_are_the_fallback() {
        assert_eq!(role("Sent Items", &[]), FolderRole::Sent);
        assert_eq!(role("Deleted Items", &[]), FolderRole::Trash);
        assert_eq!(role("Spam", &[]), FolderRole::Junk);
        assert_eq!(role("Drafts", &[]), FolderRole::Drafts);
        assert_eq!(role("Projects/Sent", &[]), FolderRole::Other);
        assert_eq!(role("Receipts", &[]), FolderRole::Other);
        // An attribute beats a name.
        assert_eq!(role("Sent", &[NameAttribute::Trash]), FolderRole::Trash);
    }
}
