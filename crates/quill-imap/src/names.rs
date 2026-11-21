//! Mailbox names: decoding them for display and working out what a folder
//! is for.

use std::borrow::Cow;

use async_imap::types::NameAttribute;
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use quill_core::types::FolderRole;

/// Takes the escapes out of a name the server sent in quotes.
///
/// The parser hands a quoted string over as it was written, with `\"` for
/// `"` and `\\` for `\`, and does not say whether a name came in quotes at
/// all. Sent back like that, the name would be escaped a second time and
/// would name no folder. In quotes a backslash is always followed by one of
/// those two characters, so a name where every backslash is, is read as
/// quoted. A backslash followed by anything else can only have come in a
/// literal, which has no escapes, and that name is kept as it is.
pub(crate) fn unescape(name: &str) -> Cow<'_, str> {
    if !name.contains('\\') {
        return Cow::Borrowed(name);
    }
    let mut out = String::with_capacity(name.len());
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(escaped @ ('\\' | '"')) => out.push(escaped),
            _ => return Cow::Borrowed(name),
        }
    }
    Cow::Owned(out)
}

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

/// Writes a mailbox name in IMAP's modified UTF-7, as a server expects a
/// name it is asked to make: printable ASCII as it is, `&` as `&-`, and
/// every run of anything else shifted.
pub(crate) fn encode_utf7(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut shifted: Vec<u16> = Vec::new();
    let flush = |shifted: &mut Vec<u16>, out: &mut String| {
        if shifted.is_empty() {
            return;
        }
        let bytes: Vec<u8> = shifted.iter().flat_map(|unit| unit.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&STANDARD_NO_PAD.encode(bytes).replace('/', ","));
        out.push('-');
        shifted.clear();
    };
    for letter in name.chars() {
        if matches!(letter, ' '..='~') {
            flush(&mut shifted, &mut out);
            if letter == '&' {
                out.push_str("&-");
            } else {
                out.push(letter);
            }
        } else {
            let mut units = [0u16; 2];
            shifted.extend_from_slice(letter.encode_utf16(&mut units));
        }
    }
    flush(&mut shifted, &mut out);
    out
}

/// What a folder the user asks for is to be called on the server: the name
/// in the server's writing, inside the place it keeps the user's folders.
/// That place is the inbox on a server whose every other folder is listed
/// inside it (see [`folders`]), and the top of the mailbox otherwise.
pub(crate) fn new_folder(listed: &[Listed<'_>], name: &str) -> String {
    let others: Vec<&Listed<'_>> = listed
        .iter()
        .filter(|folder| !folder.raw.eq_ignore_ascii_case("INBOX"))
        .collect();
    let inside = listed
        .iter()
        .find_map(|folder| folder.delimiter)
        .map(|delimiter| format!("INBOX{delimiter}"))
        .filter(|prefix| {
            !others.is_empty()
                && others.iter().all(|folder| {
                    folder
                        .raw
                        .get(..prefix.len())
                        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
                })
        });
    format!("{}{}", inside.unwrap_or_default(), encode_utf7(name))
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

    // Google's own folders are listed inside `[Gmail]`, which is where
    // Google keeps them and not something the user named: it is left out
    // of the names shown, as the inbox's prefix is. Not where that would
    // give two folders one name.
    let short: Vec<Option<String>> = shown
        .iter()
        .map(|(name, _, _)| {
            let short = quill_core::providers::google_folder(name)?;
            let taken = shown
                .iter()
                .filter(|(other, _, _)| other != name)
                .any(|(other, _, _)| {
                    quill_core::providers::google_folder(other)
                        .unwrap_or(other)
                        .to_lowercase()
                        == short.to_lowercase()
                });
            (!taken).then(|| short.to_owned())
        })
        .collect();
    let shown = shown
        .into_iter()
        .zip(short)
        .map(|((name, declared, guessed), short)| (short.unwrap_or(name), declared, guessed));

    // What the server declares always stands. A guess from a name stands only
    // where the server declared no folder for that role, and only for the
    // first such name: one "Junk" beside a declared junk folder, or a "Spam"
    // beside a "Junk", is an ordinary folder.
    let shown: Vec<(String, Option<FolderRole>, FolderRole)> = shown.collect();
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
    fn a_quoted_name_loses_its_escapes_and_any_other_is_kept() {
        for (written, name) in [
            ("INBOX", "INBOX"),
            ("Entw&APw-rfe", "Entw&APw-rfe"),
            (r#"Say \"when\""#, r#"Say "when""#),
            (r"Back\\slash", r"Back\slash"),
            (r#"\\\""#, r#"\""#),
            // A delimiter, as some servers have it.
            (r"\\", r"\"),
            // Not how a quoted string is written, so it came as a literal.
            (r"Back\slash", r"Back\slash"),
            (r"ends in one\", r"ends in one\"),
        ] {
            assert_eq!(unescape(written), name, "{written}");
        }
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
    fn a_name_is_written_the_way_a_server_reads_one() {
        for (typed, written) in [
            ("Receipts", "Receipts"),
            ("Entwürfe", "Entw&APw-rfe"),
            ("台北", "&U,BTFw-"),
            ("😀", "&2D3eAA-"),
            ("Tom & Jerry", "Tom &- Jerry"),
            ("~peter/mail/台北/日本語", "~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
        ] {
            assert_eq!(encode_utf7(typed), written);
            // And reads back as it was typed.
            assert_eq!(decode_utf7(written), typed);
        }
    }

    #[test]
    fn a_new_folder_goes_where_the_server_keeps_the_users_folders() {
        let listed =
            |names: &'static [&'static str], delimiter: &'static str| -> Vec<Listed<'static>> {
                names
                    .iter()
                    .map(|raw| Listed {
                        raw,
                        delimiter: Some(delimiter),
                        attributes: &[],
                    })
                    .collect()
            };
        // Beside the inbox, on most servers.
        let beside = listed(&["INBOX", "Sent", "Trash"], "/");
        assert_eq!(new_folder(&beside, "Receipts"), "Receipts");
        // Inside it, where every other folder is.
        let inside = listed(&["INBOX", "INBOX.Sent", "INBOX.Trash"], ".");
        assert_eq!(new_folder(&inside, "Receipts"), "INBOX.Receipts");
        assert_eq!(new_folder(&inside, "Reçus"), "INBOX.Re&AOc-us");
        // A mailbox with nothing but an inbox says nothing about where:
        // the top is as good a guess as any, and the server may refuse it.
        let alone = listed(&["INBOX"], ".");
        assert_eq!(new_folder(&alone, "Receipts"), "Receipts");
        // One folder outside the inbox is enough to say the top is used.
        let mixed = listed(&["INBOX", "INBOX.Sent", "Archive"], ".");
        assert_eq!(new_folder(&mixed, "Receipts"), "Receipts");
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
    fn googles_own_folders_are_named_without_the_place_google_keeps_them() {
        let all = [NameAttribute::All];
        let drafts = [NameAttribute::Drafts];
        let sent = [NameAttribute::Sent];
        let junk = [NameAttribute::Junk];
        let trash = [NameAttribute::Trash];
        let noselect = [NameAttribute::NoSelect];
        let listed = [
            listed("INBOX", "/", &[]),
            listed("Notes", "/", &[]),
            listed("Receipts/2026", "/", &[]),
            listed("[Gmail]", "/", &noselect),
            listed("[Gmail]/All Mail", "/", &all),
            listed("[Gmail]/Drafts", "/", &drafts),
            listed("[Gmail]/Important", "/", &[]),
            listed("[Gmail]/Sent Mail", "/", &sent),
            listed("[Gmail]/Spam", "/", &junk),
            listed("[Gmail]/Starred", "/", &[]),
            listed("[Gmail]/Trash", "/", &trash),
        ];
        assert_eq!(
            folders(&listed),
            [
                ("Inbox".to_owned(), FolderRole::Inbox),
                ("Notes".to_owned(), FolderRole::Other),
                ("Receipts/2026".to_owned(), FolderRole::Other),
                // The place itself holds no mail, and is not one of the
                // folders in it.
                ("[Gmail]".to_owned(), FolderRole::Other),
                ("All Mail".to_owned(), FolderRole::Archive),
                ("Drafts".to_owned(), FolderRole::Drafts),
                ("Important".to_owned(), FolderRole::Other),
                ("Sent Mail".to_owned(), FolderRole::Sent),
                ("Spam".to_owned(), FolderRole::Junk),
                ("Starred".to_owned(), FolderRole::Other),
                ("Trash".to_owned(), FolderRole::Trash),
            ]
        );

        // As Google names the place in some countries, and in a name that
        // is not ASCII.
        assert_eq!(
            plain(&["INBOX", "[Google Mail]/Bin", "[Gmail]/Entw&APw-rfe"], "/")
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["Inbox", "Bin", "Entwürfe"]
        );

        // A folder of the user's with the same name keeps its own, and
        // Google's keeps the whole of its: two folders are never shown
        // alike.
        assert_eq!(
            plain(
                &["INBOX", "Important", "[Gmail]/Important", "[Gmail]/Starred"],
                "/"
            )
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
            ["Inbox", "Important", "[Gmail]/Important", "Starred"]
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
