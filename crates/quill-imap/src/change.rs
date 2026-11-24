//! Changing what is on the server: moving, copying, deleting and saving
//! messages.
//!
//! These run from the operation queue, which replays after a crash, so each
//! one is written to be safe to run twice:
//!
//! - A message is acted on only under the UIDVALIDITY its id was made under.
//!   If the folder has been renumbered, or is gone, or no longer holds the
//!   UID, the message is not there to act on. For a move or a delete that
//!   means an earlier attempt already did the work, and the answer is `Ok`.
//! - A message is removed from a folder by `UID EXPUNGE`, which removes that
//!   one message. Plain `EXPUNGE` removes everything marked `\Deleted`,
//!   whoever marked it, so it is the last resort of a server without
//!   UIDPLUS and is used only when nothing else in the folder is marked.
//! - After removing a message the folder is asked whether it is really
//!   gone. A server may accept `\Deleted` and quietly not set it (shared
//!   folders do this to people without the right), and a move that reported
//!   success there would be a copy.
//!
//! What cannot be made safe is said where it applies: a copy made twice is
//! two copies, and without `MOVE` a move is a copy followed by a removal,
//! with a moment in between.
//!
//! Every command here goes through [`Command`] and not through async-imap's
//! own methods. Those do not look at how a command ended, so a search the
//! server refused reads as a search that found nothing. Here "nothing
//! found" is what decides that a message is gone, or that an expunge would
//! take nobody else's mail, and it has to be the server's answer and not the
//! absence of one.
//!
//! Nothing about a selected folder is remembered between calls. Every
//! operation selects its folder again and takes the UIDVALIDITY from that
//! answer, so there is no cached state for a move or an expunge to leave
//! stale.

use std::collections::HashSet;

use async_imap::Session;
use async_imap::imap_proto::{Capability, MailboxDatum, NameAttribute, Response};
use quill_core::types::{Flags, FolderId, RemoteId};
use quill_core::{Error, Result};

use crate::backend::MessageRef;
use crate::command::{Command, Outcome, Reply, Wire, uid_search};
use crate::names;

/// The optional parts of IMAP that decide how a message is moved or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Extensions {
    /// `MOVE` (RFC 6851): one command moves a message, with no moment at
    /// which it is in both folders or in neither.
    pub moves: bool,
    /// `UIDPLUS` (RFC 4315): `UID EXPUNGE` removes the messages named and no
    /// others, and COPY and APPEND say what UID the new message was given.
    pub uidplus: bool,
    /// `IDLE` (RFC 2177): the server says when a folder changes, to a
    /// connection that waits to be told.
    pub idle: bool,
    /// `X-GM-EXT-1`: the server is Gmail's, where all mail is kept in one
    /// place and a folder is a label on it, so that a message taken out
    /// of a folder, or a folder deleted, leaves the mail in All Mail.
    /// Nothing weaker is taken for it: an `\All` folder can be a view
    /// over folders that hold the only copies.
    pub gmail: bool,
}

impl Extensions {
    /// Asks the server what it can do. Asked after sign-in, since what a
    /// server offers can change once it knows who is asking.
    ///
    /// Only what it advertises is used: a server that happens to accept
    /// `UID MOVE` without saying so is not relied on. A server that will
    /// not answer the question is taken to have none of them, which is the
    /// cautious reading.
    pub async fn ask<T: Wire>(session: &mut Session<T>) -> Result<Self> {
        let mut found = Self {
            moves: false,
            uidplus: false,
            idle: false,
            gmail: false,
        };
        Command::new("CAPABILITY")
            .run_with(session, |response| {
                if let Response::Capabilities(capabilities) = response {
                    // Capability names are not case-sensitive.
                    let has = |name: &str| {
                        capabilities.iter().any(|capability| match capability {
                            Capability::Atom(atom) => atom.eq_ignore_ascii_case(name),
                            _ => false,
                        })
                    };
                    found.moves |= has("MOVE");
                    found.uidplus |= has("UIDPLUS");
                    found.idle |= has("IDLE");
                    found.gmail |= has("X-GM-EXT-1");
                }
            })
            .await?;
        Ok(found)
    }
}

/// Moves a message and says what it is called in `to`, when the server
/// tells (the `COPYUID` response code, which UIDPLUS servers send for MOVE
/// and COPY alike).
///
/// With `MOVE` this is one command and cannot be half done. Without it the
/// message is copied, marked `\Deleted` and expunged, by UID when the server
/// has UIDPLUS. If that is interrupted between the copy and the expunge the
/// message is in both folders, and a replay copies it again: IMAP offers no
/// way to tell that an earlier copy was made.
///
/// `Ok(None)` when the message is no longer in its folder: an earlier
/// attempt moved it.
pub(crate) async fn move_message<T: Wire>(
    session: &mut Session<T>,
    extensions: Extensions,
    message: &MessageRef,
    to: &FolderId,
) -> Result<Option<RemoteId>> {
    if !present(session, message).await? {
        return Ok(None);
    }
    if to.0 == message.folder {
        // Already where it is wanted. A server would give it a new UID for
        // nothing, or refuse.
        return Ok(Some(message.id()));
    }
    let uid = message.uid.to_string();

    if extensions.moves {
        let reply = Command::new("UID MOVE")
            .arg(&uid)
            .mailbox(&to.0)?
            .run(session)
            .await?;
        let reply = delivered(session, reply, to).await?;
        return Ok(arrival(&reply, message.uid, to));
    }

    if !extensions.uidplus {
        // Before the copy, so that a refusal leaves nothing behind for a
        // replay to duplicate.
        nothing_else_marked(session, message).await?;
    }
    let reply = Command::new("UID COPY")
        .arg(&uid)
        .mailbox(&to.0)?
        .run(session)
        .await?;
    let reply = delivered(session, reply, to).await?;
    remove(session, extensions, message)
        .await
        .map_err(|err| match err {
            Error::Backend(reason) => Error::Backend(format!(
                "the message was copied to {} but is still in {}: {reason}",
                to.0, message.folder
            )),
            other => other,
        })?;
    Ok(arrival(&reply, message.uid, to))
}

/// Copies a message into `to`.
///
/// A copy cannot be made idempotent: the server keeps no record that links
/// a copy to its original, so a replay after a crash that followed the
/// server's OK makes a second copy. Unlike a move, a message that is gone
/// is an error here, since nothing says an earlier attempt copied it.
pub(crate) async fn copy<T: Wire>(
    session: &mut Session<T>,
    message: &MessageRef,
    to: &FolderId,
) -> Result<()> {
    if !present(session, message).await? {
        return Err(Error::NotFound(format!(
            "message {} is no longer in {}",
            message.uid, message.folder
        )));
    }
    let reply = Command::new("UID COPY")
        .arg(&message.uid.to_string())
        .mailbox(&to.0)?
        .run(session)
        .await?;
    delivered(session, reply, to).await.map(drop)
}

/// Removes a message for good: not to Trash, which is a move.
///
/// `Ok` when the message is already gone.
pub(crate) async fn delete<T: Wire>(
    session: &mut Session<T>,
    extensions: Extensions,
    message: &MessageRef,
) -> Result<()> {
    if !present(session, message).await? {
        return Ok(());
    }
    if !extensions.uidplus {
        nothing_else_marked(session, message).await?;
    }
    remove(session, extensions, message).await
}

/// Saves a complete message into `folder` with `flags`, dated now by the
/// server, and says what it is called there when the server tells
/// (`APPENDUID`, from UIDPLUS).
///
/// The bytes are sent as they are. Saving is not idempotent: saving the same
/// message twice stores it twice, and the caller is the one who knows
/// whether it has been saved before.
pub(crate) async fn append<T: Wire>(
    session: &mut Session<T>,
    folder: &FolderId,
    raw: &[u8],
    flags: Flags,
) -> Result<Option<RemoteId>> {
    if raw.is_empty() {
        // To a server an empty literal means "never mind".
        return Err(Error::Backend(
            "an empty message cannot be saved".to_owned(),
        ));
    }
    let mut command = Command::new("APPEND").mailbox(&folder.0)?;
    let flags = flag_list(flags);
    if !flags.is_empty() {
        command = command.arg(&format!("({})", flags.join(" ")));
    }
    let reply = command.literal(raw).run(session).await?;
    let reply = delivered(session, reply, folder).await?;
    Ok(reply.appended.map(|(validity, uid)| {
        MessageRef {
            uid,
            validity,
            folder: folder.0.clone(),
        }
        .id()
    }))
}

fn flag_list(flags: Flags) -> Vec<&'static str> {
    [
        (flags.seen, "\\Seen"),
        (flags.flagged, "\\Flagged"),
        (flags.answered, "\\Answered"),
        (flags.draft, "\\Draft"),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name)
    .collect()
}

/// Selects the message's folder and says whether the message is in it.
///
/// `false` covers every way of not being there: the folder is gone, it was
/// renumbered so the UID now names some other message or none, or the UID
/// is no longer in it. A folder that exists and will not open is an error,
/// not an absence.
async fn present<T: Wire>(session: &mut Session<T>, message: &MessageRef) -> Result<bool> {
    let reply = Command::new("SELECT")
        .mailbox(&message.folder)?
        .run(session)
        .await?;
    match reply.outcome {
        Outcome::Ok => {}
        Outcome::No => {
            if exists(session, &message.folder).await? {
                return Err(Error::Backend(format!(
                    "{} could not be opened: {}",
                    message.folder,
                    reply.words("the server refused")
                )));
            }
            return Ok(false);
        }
        Outcome::Bad => {
            return Err(Error::Backend(format!(
                "{} could not be opened: {}",
                message.folder,
                reply.words("the server did not understand")
            )));
        }
    }
    let Some(validity) = reply.uid_validity else {
        return Err(Error::Backend(format!(
            "{} has no UIDVALIDITY",
            message.folder
        )));
    };
    // UID 0 names nothing, and a server is entitled to choke on it.
    if validity != message.validity || message.uid == 0 {
        return Ok(false);
    }
    Ok(uid_search(session, &format!("UID {}", message.uid))
        .await?
        .contains(&message.uid))
}

/// Whether a folder of exactly this name can be opened.
async fn exists<T: Wire>(session: &mut Session<T>, folder: &str) -> Result<bool> {
    let mut found = false;
    // LIST takes a pattern, so a name with `%` or `*` in it may bring back
    // other folders too. Only an exact match counts.
    let reply = Command::new("LIST")
        .arg("\"\"")
        .mailbox(folder)?
        .run_with(session, |response| {
            if let Response::MailboxData(MailboxDatum::List(listed)) = response {
                let name = names::unescape(&listed.name);
                let same = name == folder
                    || (name.eq_ignore_ascii_case("INBOX") && folder.eq_ignore_ascii_case("INBOX"));
                let usable = !listed.name_attributes.iter().any(|attribute| {
                    matches!(attribute, NameAttribute::NoSelect)
                        || matches!(attribute, NameAttribute::Extension(name)
                            if name.eq_ignore_ascii_case("\\NonExistent"))
                });
                found |= same && usable;
            }
        })
        .await?;
    reply.require("the server would not list its folders")?;
    Ok(found)
}

/// Judges the reply to a command that puts a message into `folder`.
///
/// A missing folder is `NotFound`. Servers are required to say `TRYCREATE`
/// for one; for those that do not, the folder is looked up.
async fn delivered<T: Wire>(
    session: &mut Session<T>,
    reply: Reply,
    folder: &FolderId,
) -> Result<Reply> {
    if reply.is_ok() {
        return Ok(reply);
    }
    let words = reply.words("the server refused");
    if reply.try_create || !exists(session, &folder.0).await? {
        return Err(Error::NotFound(format!("{}: {words}", folder.0)));
    }
    Err(Error::Backend(format!("{}: {words}", folder.0)))
}

/// The id of the copy of `uid` in `to`, when the reply says.
fn arrival(reply: &Reply, uid: u32, to: &FolderId) -> Option<RemoteId> {
    let copied = reply.copied.as_ref()?;
    Some(
        MessageRef {
            uid: copied.new_uid(uid)?,
            validity: copied.validity,
            folder: to.0.clone(),
        }
        .id(),
    )
}

/// The UIDs marked `\Deleted` in the selected folder, other than `uid`.
async fn others_marked<T: Wire>(session: &mut Session<T>, uid: u32) -> Result<HashSet<u32>> {
    let mut marked = uid_search(session, "DELETED").await?;
    marked.remove(&uid);
    Ok(marked)
}

/// On a server without UIDPLUS, refuses to go on while other messages in
/// the folder are marked `\Deleted`: the only expunge such a server has
/// would take them too, and they are not ours to remove. Checked before
/// anything is changed, so that a refusal leaves the folder as it was.
async fn nothing_else_marked<T: Wire>(
    session: &mut Session<T>,
    message: &MessageRef,
) -> Result<()> {
    let others = others_marked(session, message.uid).await?;
    if others.is_empty() {
        return Ok(());
    }
    Err(Error::Backend(format!(
        "{} holds {} other message(s) marked for deletion, and this server cannot remove one \
         message without removing them all. Nothing was changed.",
        message.folder,
        others.len()
    )))
}

/// Removes a message from the selected folder, and makes sure it went.
async fn remove<T: Wire>(
    session: &mut Session<T>,
    extensions: Extensions,
    message: &MessageRef,
) -> Result<()> {
    let uid = message.uid.to_string();
    let marked = Command::new("UID STORE")
        .arg(&uid)
        .arg("+FLAGS.SILENT (\\Deleted)")
        .run(session)
        .await?;
    marked.require("the server would not mark the message for deletion")?;

    if extensions.uidplus {
        let expunged = Command::new("UID EXPUNGE").arg(&uid).run(session).await?;
        expunged.require("the server would not expunge the message")?;
    } else {
        // Asked again, now that the next command is the expunge: another
        // mail program may have marked something since the first look.
        let others = others_marked(session, message.uid).await?;
        if !others.is_empty() {
            return Err(Error::Backend(format!(
                "{} other message(s) in {} were marked for deletion meanwhile, and this server \
                 cannot remove one message without removing them all. The message is left \
                 marked for deletion and goes when the folder is next expunged.",
                others.len(),
                message.folder
            )));
        }
        let expunged = Command::new("EXPUNGE").run(session).await?;
        expunged.require("the server would not expunge the folder")?;
    }

    if uid_search(session, &format!("UID {uid}"))
        .await?
        .contains(&message.uid)
    {
        return Err(Error::Backend(format!(
            "the server did not remove message {uid} from {}",
            message.folder
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! What a real server will not do on request: leave out a response
    //! code, refuse halfway, or change its mind between two commands. The
    //! server here says exactly what its script says, and fails the test if
    //! it is sent anything the script did not expect.

    use super::*;
    use crate::scripted::{scripted, steps};

    const BOTH: Extensions = Extensions {
        moves: true,
        uidplus: true,
        idle: false,
        gmail: false,
    };
    const UIDPLUS: Extensions = Extensions {
        moves: false,
        uidplus: true,
        idle: false,
        gmail: false,
    };
    const NEITHER: Extensions = Extensions {
        moves: false,
        uidplus: false,
        idle: false,
        gmail: false,
    };

    /// Selecting the inbox and finding message 5 in it, as every operation
    /// on that message begins.
    const FOUND: [(&str, &str); 2] = [
        (
            "SELECT \"INBOX\"",
            "* 3 EXISTS\r\n* OK [UIDVALIDITY 7] UIDs valid\r\n{tag} OK [READ-WRITE] Selected\r\n",
        ),
        ("UID SEARCH UID 5", "* SEARCH 5\r\n{tag} OK Done\r\n"),
    ];

    fn message() -> MessageRef {
        MessageRef {
            uid: 5,
            validity: 7,
            folder: "INBOX".to_owned(),
        }
    }

    fn archive() -> FolderId {
        FolderId("Archive".to_owned())
    }

    #[tokio::test]
    async fn a_move_reads_the_new_id_from_a_line_of_its_own_or_from_the_last() {
        for answer in [
            "* OK [COPYUID 99 5 41] Moved\r\n* 2 EXPUNGE\r\n{tag} OK Done\r\n",
            "* 2 EXPUNGE\r\n{tag} OK [COPYUID 99 5 41] Done\r\n",
            // Several messages accounted for, ours among them.
            "* OK [COPYUID 99 3:5 39:41] Moved\r\n{tag} OK Done\r\n",
        ] {
            let script = steps(&[&FOUND, &[("UID MOVE 5 \"Archive\"", answer)]]);
            let (mut session, server) = scripted(&script).await;
            let new = move_message(&mut session, BOTH, &message(), &archive())
                .await
                .unwrap();
            assert_eq!(new, Some(RemoteId("41;99;Archive".to_owned())), "{answer}");
            drop(session);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_move_the_server_says_nothing_more_about_is_done_with_no_id() {
        for answer in [
            "* 2 EXPUNGE\r\n{tag} OK Done\r\n",
            // An answer about some other message is not an answer about ours.
            "* OK [COPYUID 99 6 41] Moved\r\n{tag} OK Done\r\n",
        ] {
            let script = steps(&[&FOUND, &[("UID MOVE 5 \"Archive\"", answer)]]);
            let (mut session, server) = scripted(&script).await;
            let new = move_message(&mut session, BOTH, &message(), &archive())
                .await
                .unwrap();
            assert_eq!(new, None, "{answer}");
            drop(session);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_renumbered_folder_is_left_alone() {
        let script = [(
            "SELECT \"INBOX\"",
            "* OK [UIDVALIDITY 8] UIDs valid\r\n{tag} OK Selected\r\n",
        )];
        // The script ends there: one more command of any kind fails the test.
        let (mut session, server) = scripted(&script).await;
        let moved = move_message(&mut session, BOTH, &message(), &archive()).await;
        assert_eq!(moved.unwrap(), None);
        drop(session);
        server.await.unwrap();

        let script = steps(&[&script, &script]);
        let (mut session, server) = scripted(&script).await;
        delete(&mut session, NEITHER, &message()).await.unwrap();
        assert!(matches!(
            copy(&mut session, &message(), &archive()).await,
            Err(Error::NotFound(_))
        ));
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_search_the_server_refuses_is_not_read_as_a_message_that_is_gone() {
        let script = [
            FOUND[0],
            ("UID SEARCH UID 5", "{tag} NO Search is unavailable\r\n"),
        ];
        let (mut session, server) = scripted(&script).await;
        match move_message(&mut session, BOTH, &message(), &archive()).await {
            Err(Error::Backend(text)) => assert!(text.contains("unavailable"), "{text}"),
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_folder_that_exists_and_will_not_open_is_an_error_not_an_absence() {
        let refused = ("SELECT \"INBOX\"", "{tag} NO Mailbox is locked\r\n");
        let script = [
            refused,
            (
                "LIST \"\" \"INBOX\"",
                "* LIST (\\HasNoChildren) \"/\" INBOX\r\n{tag} OK Done\r\n",
            ),
        ];
        let (mut session, server) = scripted(&script).await;
        assert!(matches!(
            move_message(&mut session, BOTH, &message(), &archive()).await,
            Err(Error::Backend(_))
        ));
        drop(session);
        server.await.unwrap();

        // Not listed at all: the folder, and the message with it, is gone.
        let script = [refused, ("LIST \"\" \"INBOX\"", "{tag} OK Done\r\n")];
        let (mut session, server) = scripted(&script).await;
        let moved = move_message(&mut session, BOTH, &message(), &archive()).await;
        assert_eq!(moved.unwrap(), None);
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_refusal_without_trycreate_is_looked_into() {
        let refused = ("UID MOVE 5 \"Archive\"", "{tag} NO Cannot do that\r\n");
        // What LIST answers, and whether that makes the folder missing.
        let cases = [
            // Not listed: that is why the move was refused.
            ("{tag} OK Done\r\n", true),
            // Listed, but only as a place for other folders.
            (
                "* LIST (\\Noselect) \"/\" Archive\r\n{tag} OK Done\r\n",
                true,
            ),
            // The folder is there, so the refusal is about something else.
            (
                "* LIST (\\HasNoChildren) \"/\" Archive\r\n{tag} OK Done\r\n",
                false,
            ),
        ];
        for (listed, missing) in cases {
            let script = steps(&[&FOUND, &[refused, ("LIST \"\" \"Archive\"", listed)]]);
            let (mut session, server) = scripted(&script).await;
            let err = move_message(&mut session, BOTH, &message(), &archive())
                .await
                .unwrap_err();
            match &err {
                Error::NotFound(_) => assert!(missing, "{listed}: {err:?}"),
                Error::Backend(text) => {
                    assert!(
                        !missing && text.contains("Cannot do that"),
                        "{listed}: {err:?}"
                    );
                }
                _ => panic!("{listed}: {err:?}"),
            }
            drop(session);
            server.await.unwrap();
        }

        // A name the server writes with escapes is the same name. One that
        // only matches as a pattern is not.
        let to = FolderId("Say \"100%\"".to_owned());
        let refused = (
            "UID MOVE 5 \"Say \\\"100%\\\"\"",
            "{tag} NO Cannot do that\r\n",
        );
        let similar = "* LIST () \"/\" \"Say \\\"100 or so\\\"\"\r\n";
        let exact = "* LIST () \"/\" \"Say \\\"100%\\\"\"\r\n";
        for (listed, there) in [
            (format!("{similar}{exact}{{tag}} OK Done\r\n"), true),
            (format!("{similar}{{tag}} OK Done\r\n"), false),
        ] {
            let asked = ("LIST \"\" \"Say \\\"100%\\\"\"", listed.as_str());
            let script = steps(&[&FOUND, &[refused, asked]]);
            let (mut session, server) = scripted(&script).await;
            let err = move_message(&mut session, BOTH, &message(), &to)
                .await
                .unwrap_err();
            assert_eq!(matches!(err, Error::Backend(_)), there, "{err:?}");
            assert_eq!(matches!(err, Error::NotFound(_)), !there, "{err:?}");
            drop(session);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_quoted_is_sent_as_a_literal() {
        let to = FolderId("Entwürfe".to_owned());
        let script = steps(&[
            &FOUND,
            &[
                ("UID MOVE 5 {9}", "+ Ready\r\n"),
                ("Entwürfe", "{tag} OK [COPYUID 3 5 1] Done\r\n"),
            ],
        ]);
        let (mut session, server) = scripted(&script).await;
        let new = move_message(&mut session, BOTH, &message(), &to).await;
        assert_eq!(new.unwrap(), Some(RemoteId("1;3;Entwürfe".to_owned())));
        drop(session);
        server.await.unwrap();

        // Two literals in one command: the name, then the message.
        let script = [
            ("APPEND {9}", "+ Ready\r\n"),
            ("Entwürfe (\\Seen \\Draft) {11}", "+ Ready\r\n"),
            ("Subject: hi", "{tag} OK [APPENDUID 3 2] Done\r\n"),
        ];
        let (mut session, server) = scripted(&script).await;
        let flags = Flags {
            seen: true,
            draft: true,
            ..Flags::default()
        };
        // The scripted server reads a line at a time, so the "message" is
        // one line with no ending of its own.
        let saved = append(&mut session, &to, b"Subject: hi", flags).await;
        assert_eq!(saved.unwrap(), Some(RemoteId("2;3;Entwürfe".to_owned())));
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_save_the_server_refuses_never_sends_the_message() {
        let script = [(
            "APPEND \"Sent\" (\\Seen) {11}",
            "{tag} NO [TRYCREATE] No such mailbox\r\n",
        )];
        let (mut session, server) = scripted(&script).await;
        let flags = Flags {
            seen: true,
            ..Flags::default()
        };
        let sent = FolderId("Sent".to_owned());
        assert!(matches!(
            append(&mut session, &sent, b"Subject: hi", flags).await,
            Err(Error::NotFound(_))
        ));
        drop(session);
        // The server fails the test if the message arrives after all.
        server.await.unwrap();

        // Over quota: the folder exists, and the server says so when asked.
        let script = [
            (
                "APPEND \"Sent\" {11}",
                "{tag} NO [OVERQUOTA] Quota exceeded\r\n",
            ),
            (
                "LIST \"\" \"Sent\"",
                "* LIST (\\Sent) \"/\" Sent\r\n{tag} OK Done\r\n",
            ),
        ];
        let (mut session, server) = scripted(&script).await;
        match append(&mut session, &sent, b"Subject: hi", Flags::default()).await {
            Err(Error::Backend(text)) => assert!(text.contains("Quota exceeded"), "{text}"),
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_save_without_appenduid_is_saved_with_no_id() {
        let script = [
            ("APPEND \"Sent\" {11}", "+ Ready\r\n"),
            ("Subject: hi", "* 4 EXISTS\r\n{tag} OK Done\r\n"),
        ];
        let (mut session, server) = scripted(&script).await;
        let sent = FolderId("Sent".to_owned());
        let saved = append(&mut session, &sent, b"Subject: hi", Flags::default()).await;
        assert_eq!(saved.unwrap(), None);
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_message_the_server_would_not_remove_is_not_reported_moved() {
        // A shared folder where we may read and copy but not delete: the
        // server takes the \Deleted mark without setting it, and the
        // expunge has nothing to do.
        let script = steps(&[
            &FOUND,
            &[
                (
                    "UID COPY 5 \"Archive\"",
                    "{tag} OK [COPYUID 99 5 41] Done\r\n",
                ),
                ("UID STORE 5 +FLAGS.SILENT (\\Deleted)", "{tag} OK Done\r\n"),
                ("UID EXPUNGE 5", "{tag} OK Done\r\n"),
                ("UID SEARCH UID 5", "* SEARCH 5\r\n{tag} OK Done\r\n"),
            ],
        ]);
        let (mut session, server) = scripted(&script).await;
        match move_message(&mut session, UIDPLUS, &message(), &archive()).await {
            Err(Error::Backend(text)) => {
                assert!(text.contains("copied to Archive"), "{text}");
                assert!(text.contains("still in INBOX"), "{text}");
            }
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();

        // And a mark the server refuses outright stops before the expunge.
        let script = steps(&[
            &FOUND,
            &[(
                "UID STORE 5 +FLAGS.SILENT (\\Deleted)",
                "{tag} NO Mailbox is read-only\r\n",
            )],
        ]);
        let (mut session, server) = scripted(&script).await;
        match delete(&mut session, UIDPLUS, &message()).await {
            Err(Error::Backend(text)) => assert!(text.contains("read-only"), "{text}"),
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn mail_marked_by_someone_else_at_the_last_moment_stops_a_plain_expunge() {
        let script = steps(&[
            &FOUND,
            &[
                // Nothing else is marked when the delete begins...
                ("UID SEARCH DELETED", "* SEARCH\r\n{tag} OK Done\r\n"),
                ("UID STORE 5 +FLAGS.SILENT (\\Deleted)", "{tag} OK Done\r\n"),
                // ...and by the time ours is, message 2 is as well.
                ("UID SEARCH DELETED", "* SEARCH 2 5\r\n{tag} OK Done\r\n"),
            ],
        ]);
        // The script ends there: an EXPUNGE would fail the test.
        let (mut session, server) = scripted(&script).await;
        match delete(&mut session, NEITHER, &message()).await {
            Err(Error::Backend(text)) => assert!(text.contains("left marked"), "{text}"),
            other => panic!("{other:?}"),
        }
        drop(session);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_search_for_marked_mail_that_fails_does_not_clear_the_way_for_an_expunge() {
        let script = steps(&[
            &FOUND,
            &[("UID SEARCH DELETED", "{tag} BAD Unknown search key\r\n")],
        ]);
        let (mut session, server) = scripted(&script).await;
        assert!(matches!(
            delete(&mut session, NEITHER, &message()).await,
            Err(Error::Backend(_))
        ));
        drop(session);
        server.await.unwrap();
    }

    #[test]
    fn every_flag_quill_tracks_goes_with_a_saved_message() {
        assert_eq!(flag_list(Flags::default()), Vec::<&str>::new());
        assert_eq!(
            flag_list(Flags {
                seen: true,
                flagged: true,
                answered: true,
                draft: true,
            }),
            ["\\Seen", "\\Flagged", "\\Answered", "\\Draft"]
        );
    }
}
