//! Push: being told of new mail instead of asking (IMAP IDLE, RFC 2177).
//!
//! A connection that has sent `IDLE` is told what happens in its selected
//! folder as it happens, and may send nothing else until it has said `DONE`
//! and been answered. So a watched folder has a connection to itself, apart
//! from the one the backend runs commands on, and a sync never waits for it.
//!
//! All that is passed on is that the folder changed. What changed is for a
//! sync to find out on the other connection, the way it finds out
//! everything else: the reports here are message numbers in this
//! connection's own view of the folder, and mean nothing outside it.
//!
//! **What counts as a change.** `EXISTS` and `RECENT` with a count other
//! than the last one heard, `EXPUNGE`, `VANISHED`, and any `FETCH`: nothing
//! is ever fetched on this connection, so whatever the server volunteers
//! about a message is news of it. A count repeated is not a change (some
//! servers recite them whenever an idle starts or stops), nor is
//! `* OK Still here` or anything else the server says to pass the time.
//!
//! **When it is reported.** A quarter of a second after the first sign of
//! it ([`SETTLE`]), by which time the rest of what the server had to say
//! about it has arrived too: new mail is two lines, and emptying a folder
//! is a line for every message. If the receiver has no room, the report
//! waits for room, and whatever else is heard meanwhile is part of it. So
//! there is one event for a burst, and at most one waiting at any time,
//! while a change is never dropped: a change that went unreported would be
//! mail that does not show up until something else happens.
//!
//! The first event is sent as soon as the folder is open, before anything
//! has changed. Nothing here can vouch for the time before that: mail that
//! came while the connection was being made, or was down, was never
//! reported to anyone. One sync then covers it, and from that moment the
//! server reports everything to this connection. It is also why a watch
//! that fails loses nothing: the one started after it begins the same way.
//!
//! **How a dead connection is found.** Not by its silence, which is what
//! idling is: the usual limit on silence is lifted for exactly as long as
//! the connection idles (`wait::Bounded::expect_silence`). Instead the idle
//! is ended and begun again every few minutes ([`REFRESH`]), and the
//! `DONE` that ends it is an ordinary exchange under the ordinary limit.
//! A server that does not answer it is gone.

use std::time::Duration;

use async_imap::Session;
use async_imap::imap_proto::{MailboxDatum, RequestId, Response, Status};
use quill_core::types::{FolderId, FolderRole, RemoteEvent};
use quill_core::{Error, Result};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{Permit, Sender};
use tokio::time::Instant;

use crate::backend::list_folders;
use crate::change::Extensions;
use crate::command::{self, Command, Outcome, Wire};
use crate::wait::Bounded;

/// How long a connection idles before it stops and starts again.
///
/// Servers allow about half an hour, and that is not what decides this.
/// Ending the idle is the one moment at which a connection that died
/// without a word is found out: a laptop that slept, a router that forgot
/// the connection. Until then no mail is pushed and nobody knows it. Five
/// minutes keeps that short, costs four short lines each time, and is about
/// as long as the less patient home routers keep a silent connection open.
pub(crate) const REFRESH: Duration = Duration::from_secs(5 * 60);

/// How long after the first sign of a change the folder is reported
/// changed. Long enough for a server to finish its sentence, and short
/// enough that nobody waiting for mail can tell.
const SETTLE: Duration = Duration::from_millis(250);

/// Watches the account's inbox on a signed-in connection that is used for
/// nothing else, until the receiver of `tx` is dropped.
///
/// `Ok` straight away, with nothing sent to `tx`, when there is nothing to
/// wait for: the server has no `IDLE`, or lists no inbox. The caller polls
/// then. Whenever it returns `Ok` the server has been told goodbye, where
/// it was still there to be told.
pub(crate) async fn watch_inbox<S: Wire>(
    session: &mut Session<Bounded<S>>,
    extensions: Extensions,
    tx: &Sender<RemoteEvent>,
    refresh: Duration,
) -> Result<()> {
    // The inbox goes by the name `list_folders` gives it, found the way
    // `list_folders` finds it. That is the name the engine knows the folder
    // by, and an event under any other would be about a folder it has
    // never heard of.
    let inbox = if extensions.idle {
        list_folders(session)
            .await?
            .into_iter()
            .find(|folder| folder.role == FolderRole::Inbox)
    } else {
        None
    };
    if let Some(inbox) = inbox {
        match watch_folder(session, &inbox.id, tx, refresh).await {
            Ok(()) => {}
            // Nobody is left to hear how it ended.
            Err(_) if tx.is_closed() => return Ok(()),
            Err(err) => return Err(err),
        }
    }
    // A courtesy. The connection is closed whatever comes of it.
    let _ = Command::new("LOGOUT").run(session).await;
    Ok(())
}

/// Watches one folder until the receiver of `tx` is dropped, sending
/// `FolderChanged(folder)` as the module describes. `Ok` leaves the
/// connection signed in and not idling. `Err` is a connection that failed,
/// or a server that would not open the folder or would not idle; the
/// connection is of no further use then.
///
/// The connection must be signed in and belong to this call alone. One
/// folder takes one connection, since a connection has one selected folder.
pub(crate) async fn watch_folder<S: Wire>(
    session: &mut Session<Bounded<S>>,
    folder: &FolderId,
    tx: &Sender<RemoteEvent>,
    refresh: Duration,
) -> Result<()> {
    let mut watched = examine(session, folder).await?;
    // From here the server reports every change to this connection. What
    // happened before, nobody reported, so the first event is due now.
    watched.report_at = Some(Instant::now());
    loop {
        let tag = begin(session, &mut watched).await?;
        // Idling from the go-ahead, and not a moment before or after: the
        // limit on silence has to be back before `DONE` is sent.
        session.get_mut().expect_silence(true);
        let woke = idle(session, &tag, &mut watched, folder, tx, refresh).await;
        session.get_mut().expect_silence(false);
        let woke = woke?;
        end(session, &tag, &mut watched).await?;
        if woke == Woke::Gone {
            return Ok(());
        }
    }
}

/// A watched folder as this connection knows it.
#[derive(Debug, Default)]
struct Watched {
    /// How many messages the server last said the folder holds.
    exists: Option<u32>,
    /// How many of them it last said are recent.
    recent: Option<u32>,
    /// When to say the folder changed, if there is a change not yet passed
    /// on.
    report_at: Option<Instant>,
}

/// What a line from the server means to a connection that is idling, about
/// to, or just finished.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    /// `+`: the server is idling from here.
    GoAhead,
    /// The tagged end of the `IDLE` command, with the server's words.
    Ended { ok: bool, words: String },
    /// `BYE`: the server is hanging up.
    Bye(String),
    /// Anything else. If it was news of the folder, `Watched` has noted it.
    Other,
}

impl Watched {
    /// Reads one line sent while the `IDLE` command tagged `tag` is in
    /// progress, noting whether the folder changed.
    fn hear(&mut self, tag: &RequestId, response: &Response<'_>) -> Line {
        let words = |text: &Option<std::borrow::Cow<'_, str>>| {
            text.as_deref().unwrap_or_default().to_owned()
        };
        match response {
            Response::Continue { .. } => Line::GoAhead,
            Response::Done {
                tag: theirs,
                status,
                outcome,
            } if theirs == tag => Line::Ended {
                ok: matches!(status, Status::Ok),
                words: words(&outcome.information),
            },
            Response::Data {
                status: Status::Bye,
                outcome,
            } => Line::Bye(words(&outcome.information)),
            other => {
                if self.changed_by(other) && self.report_at.is_none() {
                    self.report_at = Some(Instant::now() + SETTLE);
                }
                Line::Other
            }
        }
    }

    /// Whether what the server said means the folder is not as it was.
    fn changed_by(&mut self, response: &Response<'_>) -> bool {
        match response {
            // A count is news only when it is a different count. One fewer
            // message and one more leaves the count alone, and is still
            // caught: the one that went has an `EXPUNGE` of its own.
            Response::MailboxData(MailboxDatum::Exists(count)) => {
                self.exists.replace(*count) != Some(*count)
            }
            Response::MailboxData(MailboxDatum::Recent(count)) => {
                self.recent.replace(*count) != Some(*count)
            }
            Response::Expunge(_) => {
                // The count is not repeated after an expunge. It is one
                // less, and the server knows that we know.
                self.exists = self.exists.map(|count| count.saturating_sub(1));
                true
            }
            Response::Vanished { .. } => {
                // Named by UID, and a UID may name no message, so how many
                // are left is not known until the server next says.
                self.exists = None;
                true
            }
            Response::Fetch(..) => true,
            _ => false,
        }
    }
}

/// Opens `folder` for watching and notes how the server describes it now.
///
/// `EXAMINE`, not `SELECT`: a watch only looks. Selecting would take the
/// `\Recent` mark off new mail on some servers, for a connection that never
/// reads any.
async fn examine<T: Wire>(session: &mut Session<T>, folder: &FolderId) -> Result<Watched> {
    let mut watched = Watched::default();
    let reply = Command::new("EXAMINE")
        .mailbox(&folder.0)?
        .run_with(session, |response| match response {
            Response::MailboxData(MailboxDatum::Exists(count)) => watched.exists = Some(*count),
            Response::MailboxData(MailboxDatum::Recent(count)) => watched.recent = Some(*count),
            _ => {}
        })
        .await?;
    match reply.outcome {
        Outcome::Ok => Ok(watched),
        Outcome::No => Err(Error::NotFound(format!(
            "{}: {}",
            folder.0,
            reply.words("the server would not open it")
        ))),
        Outcome::Bad => Err(Error::Backend(format!(
            "{}: {}",
            folder.0,
            reply.words("the server did not understand")
        ))),
    }
}

/// Sends `IDLE` and waits for the server's go-ahead. Only then is it
/// idling: until then it may still refuse, and is held to the usual limit
/// on silence. It may also get in what it has been saving up about the
/// folder first, which is news like any other.
async fn begin<T: Wire>(session: &mut Session<T>, watched: &mut Watched) -> Result<RequestId> {
    let tag = session.run_command("IDLE").await.map_err(command::broken)?;
    loop {
        match command::next(session, |response| watched.hear(&tag, response)).await? {
            Line::GoAhead => return Ok(tag),
            Line::Ended { words, .. } => {
                return Err(Error::Backend(said("the server would not idle", &words)));
            }
            Line::Bye(words) => return Err(hung_up(&words)),
            Line::Other => {}
        }
    }
}

/// Why an idle was interrupted from our side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Woke {
    /// It has idled long enough to be worth checking the server is there.
    Refresh,
    /// The receiver was dropped: nobody wants to know any more.
    Gone,
}

/// Waits while the server idles, for however long that is, sending the
/// events that fall due. Returns when it is time to say `DONE`, which the
/// caller does: nothing is sent to the server from here.
async fn idle<T: Wire>(
    session: &mut Session<T>,
    tag: &RequestId,
    watched: &mut Watched,
    folder: &FolderId,
    tx: &Sender<RemoteEvent>,
    refresh: Duration,
) -> Result<Woke> {
    let refresh = tokio::time::sleep(refresh);
    tokio::pin!(refresh);
    loop {
        let report_at = watched.report_at;
        // Whichever is ready is taken, with no order of preference. Lines
        // that came together are all read long before a report falls due,
        // so a burst is still one event, and a server that never stops
        // talking cannot keep the report, or the refresh, from happening.
        tokio::select! {
            line = command::next(session, |response| watched.hear(tag, response)) => match line? {
                // Without a `DONE` from us. Whatever it means, the server
                // is not idling, and starting again at once could go round
                // for ever. The caller waits before trying again.
                Line::Ended { words, .. } => {
                    return Err(Error::Backend(said("the server stopped idling", &words)));
                }
                Line::Bye(words) => return Err(hung_up(&words)),
                Line::GoAhead | Line::Other => {}
            },
            room = room(tx, report_at) => match room {
                Ok(room) => {
                    room.send(RemoteEvent::FolderChanged(folder.clone()));
                    watched.report_at = None;
                }
                Err(_) => return Ok(Woke::Gone),
            },
            () = tx.closed() => return Ok(Woke::Gone),
            () = &mut refresh => return Ok(Woke::Refresh),
        }
    }
}

/// Waits until there is a change to report, it is time to report it, and
/// the receiver has room. Never returns while there is nothing to report.
/// `Err` when the receiver has been dropped.
async fn room(
    tx: &Sender<RemoteEvent>,
    report_at: Option<Instant>,
) -> std::result::Result<Permit<'_, RemoteEvent>, SendError<()>> {
    match report_at {
        Some(at) => {
            tokio::time::sleep_until(at).await;
            tx.reserve().await
        }
        None => std::future::pending().await,
    }
}

/// Sends `DONE` and waits for the server to end the `IDLE` command. This is
/// where a server that has gone is found: it has the usual patience to
/// answer. What it reports on the way is news, as ever.
async fn end<T: Wire>(
    session: &mut Session<T>,
    tag: &RequestId,
    watched: &mut Watched,
) -> Result<()> {
    session
        .run_command_untagged("DONE")
        .await
        .map_err(command::broken)?;
    loop {
        match command::next(session, |response| watched.hear(tag, response)).await? {
            Line::Ended { ok: true, .. } => return Ok(()),
            Line::Ended { words, .. } => {
                return Err(Error::Backend(said(
                    "the server would not stop idling",
                    &words,
                )));
            }
            Line::Bye(words) => return Err(hung_up(&words)),
            Line::GoAhead | Line::Other => {}
        }
    }
}

/// `what`, with the server's own words when it gave any.
fn said(what: &str, words: &str) -> String {
    if words.is_empty() {
        what.to_owned()
    } else {
        format!("{what}: {words}")
    }
}

/// The server said `BYE`. A connection error, so the caller tries again on
/// a new one like after any other loss.
fn hung_up(words: &str) -> Error {
    Error::Connection(said("the server closed the connection", words))
}

#[cfg(test)]
mod tests {
    use std::time::Instant as Clock;

    use tokio::sync::mpsc::{self, Receiver};
    use tokio::task::JoinHandle;
    use tokio::time::timeout;

    use super::*;
    use crate::scripted::{scripted, scripted_within, steps};

    const PUSH: Extensions = Extensions {
        moves: false,
        uidplus: false,
        idle: true,
    };
    const LISTED: (&str, &str) = (
        "LIST \"\" *",
        "* LIST (\\HasNoChildren) \"/\" INBOX\r\n* LIST (\\HasNoChildren) \"/\" Sent\r\n\
         {tag} OK Listed\r\n",
    );
    const EXAMINED: (&str, &str) = (
        "EXAMINE \"INBOX\"",
        "* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n* 3 EXISTS\r\n\
         * 0 RECENT\r\n* OK [UIDVALIDITY 7] UIDs valid\r\n{tag} OK [READ-ONLY] Examined\r\n",
    );
    /// Up to the first `IDLE`, which each test answers its own way.
    const OPENED: [(&str, &str); 2] = [LISTED, EXAMINED];
    const IDLING: (&str, &str) = ("IDLE", "+ idling\r\n");
    const DONE: (&str, &str) = ("DONE", "{tag} OK Idle completed\r\n");
    const LOGOUT: (&str, &str) = ("LOGOUT", "* BYE Logging out\r\n{tag} OK Logged out\r\n");
    /// The receiver went away: the idle is ended and the server told goodbye.
    const LEFT: [(&str, &str); 2] = [DONE, LOGOUT];

    /// Long enough that a test never sees it happen.
    const NEVER: Duration = Duration::from_secs(600);
    /// For a test's own waiting: generous, since it is only reached when
    /// something is wrong.
    const SOON: Duration = Duration::from_secs(5);

    fn inbox() -> RemoteEvent {
        RemoteEvent::FolderChanged(FolderId("INBOX".to_owned()))
    }

    struct Watching {
        events: Receiver<RemoteEvent>,
        watch: JoinHandle<Result<()>>,
        server: JoinHandle<()>,
    }

    /// Starts a watch of the inbox on a server that follows `script`.
    async fn watching(script: &[(&str, &str)], patience: Duration, refresh: Duration) -> Watching {
        let (mut session, server) = scripted_within(script, patience).await;
        let (tx, events) = mpsc::channel(8);
        let watch = tokio::spawn(async move {
            // The session is dropped when the watch ends, which is what
            // lets the server see that nothing more was sent.
            watch_inbox(&mut session, PUSH, &tx, refresh).await
        });
        Watching {
            events,
            watch,
            server,
        }
    }

    impl Watching {
        async fn next(&mut self) -> Option<RemoteEvent> {
            timeout(SOON, self.events.recv())
                .await
                .expect("an event, or the end of the watch")
        }

        /// Checks nothing is reported for a while: several times as long
        /// as a report takes to fall due.
        async fn quiet(&mut self) {
            let waited = timeout(SETTLE * 3, self.events.recv()).await;
            assert!(waited.is_err(), "{waited:?}");
        }

        /// How the watch ended, once the server has seen its whole script
        /// played and nothing after it.
        async fn ended(self) -> Result<()> {
            let ended = timeout(SOON, self.watch).await.expect("the watch to end");
            self.server.await.unwrap();
            ended.unwrap()
        }

        /// Stops listening, and checks the watch then ends well.
        async fn leave(self) {
            let Self {
                events,
                watch,
                server,
            } = self;
            drop(events);
            let ended = timeout(SOON, watch).await.expect("the watch to end");
            server.await.unwrap();
            ended.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn a_server_without_idle_is_not_idled_at() {
        // Asked, like at any sign-in, and it does not offer it.
        let script = [
            (
                "CAPABILITY",
                "* CAPABILITY IMAP4rev1 UIDPLUS MOVE\r\n{tag} OK Done\r\n",
            ),
            LOGOUT,
        ];
        let (mut session, server) = scripted(&script).await;
        let extensions = Extensions::ask(&mut session).await.unwrap();
        assert!(!extensions.idle);
        let (tx, mut events) = mpsc::channel(8);
        let started = Clock::now();
        watch_inbox(&mut session, extensions, &tx, NEVER)
            .await
            .unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        // Nothing was reported, and the receiver is still there: that is
        // how the caller knows to poll.
        assert!(events.try_recv().is_err());
        assert!(!tx.is_closed());
        drop(session);
        // The script is the whole of what was sent: no `IDLE`, no `EXAMINE`.
        server.await.unwrap();
    }

    #[tokio::test]
    async fn idle_is_recognised_however_the_server_spells_it() {
        for (advertised, idle) in [
            ("* CAPABILITY IMAP4rev1 IDLE UIDPLUS\r\n", true),
            ("* CAPABILITY IMAP4rev1 idle\r\n", true),
            ("* CAPABILITY IMAP4rev1 IDLENESS X-IDLE\r\n", false),
            ("", false),
        ] {
            let answer = format!("{advertised}{{tag}} OK Done\r\n");
            let script = [("CAPABILITY", answer.as_str())];
            let (mut session, server) = scripted(&script).await;
            let extensions = Extensions::ask(&mut session).await.unwrap();
            assert_eq!(extensions.idle, idle, "{advertised}");
            drop(session);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn the_first_event_says_the_watch_is_in_place_and_nothing_has_to_change_for_it() {
        let script = steps(&[&OPENED, &[IDLING], &LEFT]);
        let mut watching = watching(&script, SOON, NEVER).await;
        assert_eq!(watching.next().await, Some(inbox()));
        watching.quiet().await;
        watching.leave().await;
    }

    #[tokio::test]
    async fn a_change_that_comes_with_the_first_event_is_part_of_it_or_one_more() {
        // The first event falls due as this arrives. Whichever is seen to
        // first, the change is reported, and not more than once.
        let script = steps(&[&OPENED, &[("IDLE", "+ idling\r\n* 4 EXISTS\r\n")], &LEFT]);
        let mut watching = watching(&script, SOON, NEVER).await;
        assert_eq!(watching.next().await, Some(inbox()));
        let mut more = 0;
        while timeout(SETTLE * 3, watching.events.recv()).await.is_ok() {
            more += 1;
        }
        assert!(more <= 1, "{more}");
        watching.leave().await;
    }

    #[tokio::test]
    async fn each_kind_of_change_is_reported_as_one_event() {
        let cases = [
            "* 4 EXISTS\r\n",
            // New mail as servers really say it.
            "* 4 EXISTS\r\n* 1 RECENT\r\n",
            "* 1 RECENT\r\n",
            "* 2 EXPUNGE\r\n",
            "* 2 FETCH (FLAGS (\\Seen))\r\n",
            "* 2 FETCH (UID 9 FLAGS (\\Seen \\Flagged) MODSEQ (12))\r\n",
            "* VANISHED 41:43\r\n",
            // One message gone and one arrived: the count is as it was.
            "* 1 EXPUNGE\r\n* 3 EXISTS\r\n",
        ]
        .map(|news| {
            tokio::spawn(async move {
                // On the second idle, when the first event has long gone
                // and cannot be taken for this one. After it the server
                // answers nothing more, which is what ends the test.
                let idling = format!("+ idling\r\n{news}");
                let script = steps(&[
                    &OPENED,
                    &[IDLING, DONE, ("IDLE", idling.as_str()), ("DONE", "")],
                ]);
                let refresh = Duration::from_secs(1);
                let mut watching = watching(&script, Duration::from_millis(80), refresh).await;
                assert_eq!(watching.next().await, Some(inbox()), "{news}");
                assert_eq!(watching.next().await, Some(inbox()), "{news}");
                assert_eq!(watching.next().await, None, "{news}");
                assert!(watching.ended().await.is_err(), "{news}");
            })
        });
        for case in cases {
            case.await.unwrap();
        }
    }

    #[tokio::test]
    async fn what_a_server_says_to_pass_the_time_is_not_a_change() {
        // The counts are the ones it gave when the folder was opened.
        let idling = "+ idling\r\n* OK Still here\r\n* 3 EXISTS\r\n* 0 RECENT\r\n\
                      * FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft $Forwarded)\r\n\
                      * NO Disk is nearly full\r\n* BAD Something was not understood\r\n\
                      * OK [ALERT] Maintenance on Sunday\r\n* CAPABILITY IMAP4rev1 IDLE\r\n\
                      + still idling\r\nA9999 OK Somebody else's command\r\n";
        let script = steps(&[&OPENED, &[("IDLE", idling)], &LEFT]);
        let mut watching = watching(&script, SOON, NEVER).await;
        assert_eq!(watching.next().await, Some(inbox()));
        watching.quiet().await;
        watching.leave().await;
    }

    #[tokio::test]
    async fn the_inbox_is_reported_under_the_name_list_folders_gives_it() {
        // However the server spells it, `list_folders` calls the inbox
        // `INBOX` (the parser reads every spelling of that one name alike),
        // and the event has to name the same folder.
        let listed = (
            "LIST \"\" *",
            "* LIST (\\HasNoChildren) \".\" Sent\r\n* LIST (\\HasNoChildren) \".\" Inbox\r\n\
             {tag} OK Listed\r\n",
        );
        let (mut session, server) = scripted(&[listed]).await;
        let folders = list_folders(&mut session).await.unwrap();
        let named = folders
            .into_iter()
            .find(|folder| folder.role == FolderRole::Inbox)
            .unwrap()
            .id;
        drop(session);
        server.await.unwrap();

        let script = steps(&[&[listed, EXAMINED, IDLING], &LEFT]);
        let mut watching = watching(&script, SOON, NEVER).await;
        assert_eq!(
            watching.next().await,
            Some(RemoteEvent::FolderChanged(named))
        );
        watching.leave().await;
    }

    #[tokio::test]
    async fn an_account_that_lists_no_inbox_has_nothing_to_watch() {
        let script = [
            (
                "LIST \"\" *",
                "* LIST (\\HasNoChildren) \"/\" Sent\r\n{tag} OK Listed\r\n",
            ),
            LOGOUT,
        ];
        let mut watching = watching(&script, SOON, NEVER).await;
        assert_eq!(watching.next().await, None);
        watching.ended().await.unwrap();
    }

    #[tokio::test]
    async fn what_the_server_says_before_its_go_ahead_is_news_too() {
        // On the second idle, so that it is not taken for the first event.
        let script = steps(&[
            &OPENED,
            &[
                IDLING,
                DONE,
                (
                    "IDLE",
                    "* 4 EXISTS\r\n* 1 RECENT\r\n* OK Still here\r\n+ idling\r\n",
                ),
                // And then it is gone, which ends the test.
                ("DONE", ""),
            ],
        ]);
        let refresh = Duration::from_secs(1);
        let mut watching = watching(&script, Duration::from_millis(80), refresh).await;
        assert_eq!(watching.next().await, Some(inbox()));
        assert_eq!(watching.next().await, Some(inbox()));
        assert_eq!(watching.next().await, None);
        assert!(matches!(watching.ended().await, Err(Error::Connection(_))));
    }

    #[tokio::test]
    async fn what_the_server_says_while_the_idle_is_being_ended_is_news_too() {
        let script = steps(&[
            &OPENED,
            &[
                IDLING,
                (
                    "DONE",
                    "* 2 FETCH (FLAGS (\\Seen))\r\n{tag} OK Idle completed\r\n",
                ),
                IDLING,
                ("DONE", ""),
            ],
        ]);
        let refresh = Duration::from_secs(1);
        let mut watching = watching(&script, Duration::from_millis(80), refresh).await;
        assert_eq!(watching.next().await, Some(inbox()));
        assert_eq!(watching.next().await, Some(inbox()));
        assert_eq!(watching.next().await, None);
        assert!(watching.ended().await.is_err());
    }

    #[tokio::test]
    async fn a_burst_of_reports_is_one_event() {
        let mut burst = "+ idling\r\n".to_owned();
        for count in 4..54 {
            burst.push_str(&format!("* {count} EXISTS\r\n"));
        }
        // On the second idle, so the count is exact: the first event has
        // long gone and cannot take any of the burst with it.
        let script = steps(&[
            &OPENED,
            &[IDLING, DONE, ("IDLE", burst.as_str()), ("DONE", "")],
        ]);
        let refresh = Duration::from_secs(1);
        let mut watching = watching(&script, Duration::from_millis(80), refresh).await;
        assert_eq!(watching.next().await, Some(inbox()));
        assert_eq!(watching.next().await, Some(inbox()));
        assert_eq!(watching.next().await, None);
        assert!(watching.ended().await.is_err());
    }

    #[tokio::test]
    async fn a_receiver_with_no_room_is_waited_for_and_gets_one_event_for_all_it_missed() {
        let idling = "+ idling\r\n* 4 EXISTS\r\n";
        let script = steps(&[&OPENED, &[("IDLE", idling)], &LEFT]);
        let (mut session, server) = scripted(&script).await;
        let (tx, mut events) = mpsc::channel(1);
        // Somebody else's event, still unread: the channel is full.
        tx.send(RemoteEvent::FolderChanged(FolderId("Sent".to_owned())))
            .await
            .unwrap();
        let watch = tokio::spawn(async move { watch_inbox(&mut session, PUSH, &tx, NEVER).await });
        // Long enough for the first event and the change both to fall due
        // with nowhere to go.
        tokio::time::sleep(SETTLE * 3).await;
        assert_eq!(
            events.recv().await,
            Some(RemoteEvent::FolderChanged(FolderId("Sent".to_owned())))
        );
        assert_eq!(timeout(SOON, events.recv()).await.unwrap(), Some(inbox()));
        assert!(timeout(SETTLE * 3, events.recv()).await.is_err());
        drop(events);
        timeout(SOON, watch).await.unwrap().unwrap().unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_server_that_says_bye_while_idling_is_a_lost_connection() {
        for (idling, words) in [
            (
                "+ idling\r\n* BYE Server shutting down\r\n",
                "the server closed the connection: Server shutting down",
            ),
            // Before it had even agreed to idle.
            (
                "* BYE Server shutting down\r\n",
                "the server closed the connection: Server shutting down",
            ),
            // With no reason given.
            ("+ idling\r\n* BYE\r\n", "the server closed the connection"),
        ] {
            let script = steps(&[&OPENED, &[("IDLE", idling)]]);
            let watching = watching(&script, SOON, NEVER).await;
            // Nothing is sent after it: the script ends at the `IDLE`.
            match watching.ended().await {
                Err(Error::Connection(text)) => assert_eq!(text, words),
                other => panic!("{idling}: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_server_that_hangs_up_while_idling_is_a_lost_connection() {
        // The script ends with the server idling, and then it hangs up.
        let (mut session, server) = scripted(&steps(&[&OPENED, &[IDLING]])).await;
        let (tx, mut events) = mpsc::channel(8);
        let watch = tokio::spawn(async move {
            let ended = watch_inbox(&mut session, PUSH, &tx, NEVER).await;
            (ended, session)
        });
        assert_eq!(timeout(SOON, events.recv()).await.unwrap(), Some(inbox()));
        // It is still waiting for us to hang up, as the script's end
        // checks. Closing its side is what a crash looks like from here.
        server.abort();
        let (ended, _session) = timeout(SOON, watch).await.unwrap().unwrap();
        assert!(matches!(ended, Err(Error::Connection(_))), "{ended:?}");
    }

    #[tokio::test]
    async fn a_server_that_never_answers_done_is_given_up_on() {
        let script = steps(&[&OPENED, &[IDLING, ("DONE", "")]]);
        let (patience, refresh) = (Duration::from_millis(80), Duration::from_millis(50));
        let started = Clock::now();
        let mut watching = watching(&script, patience, refresh).await;
        assert_eq!(watching.next().await, Some(inbox()));
        match watching.ended().await {
            Err(Error::Connection(text)) => assert_eq!(text, "timed out waiting for the server"),
            other => panic!("{other:?}"),
        }
        // It idled its time in silence, which was fine, and then waited
        // the usual time for an answer, which was not.
        assert!(started.elapsed() >= refresh + patience);
        assert!(started.elapsed() < SOON);
    }

    #[tokio::test]
    async fn silence_while_idling_is_not_a_lost_connection() {
        // Many times the patience, with not a word from the server.
        let script = steps(&[&OPENED, &[IDLING], &LEFT]);
        let patience = Duration::from_millis(40);
        let mut watching = watching(&script, patience, NEVER).await;
        assert_eq!(watching.next().await, Some(inbox()));
        tokio::time::sleep(patience * 10).await;
        assert!(!watching.watch.is_finished());
        assert!(watching.events.try_recv().is_err());
        watching.leave().await;
    }

    #[tokio::test]
    async fn the_idle_is_ended_and_begun_again_and_nothing_is_reported_for_it() {
        // Three idles, each ended before the next is begun, as the script
        // insists. The server answers the first two endings and not the
        // third, which is what ends the test.
        let script = steps(&[&OPENED, &[IDLING, DONE, IDLING, DONE, IDLING, ("DONE", "")]]);
        let (patience, refresh) = (Duration::from_millis(80), Duration::from_millis(150));
        let started = Clock::now();
        let mut watching = watching(&script, patience, refresh).await;
        assert_eq!(watching.next().await, Some(inbox()));
        assert_eq!(watching.next().await, None);
        assert!(matches!(watching.ended().await, Err(Error::Connection(_))));
        assert!(started.elapsed() >= refresh * 3 + patience);
    }

    #[tokio::test]
    async fn a_server_that_will_not_open_the_inbox_or_will_not_idle_is_an_error() {
        /// What the server is sent and answers, whether the refusal means
        /// the folder is not there, and the words to expect.
        type Refusal<'a> = (&'a [(&'a str, &'a str)], bool, &'a str);
        let refused: [Refusal<'_>; 5] = [
            (
                &[
                    LISTED,
                    ("EXAMINE \"INBOX\"", "{tag} NO Mailbox is locked\r\n"),
                ],
                true,
                "Mailbox is locked",
            ),
            (
                &[
                    LISTED,
                    ("EXAMINE \"INBOX\"", "{tag} BAD Unknown command\r\n"),
                ],
                false,
                "Unknown command",
            ),
            (
                &[LISTED, EXAMINED, ("IDLE", "{tag} NO Not now\r\n")],
                false,
                "would not idle: Not now",
            ),
            (
                &[LISTED, EXAMINED, ("IDLE", "{tag} BAD Unknown command\r\n")],
                false,
                "would not idle: Unknown command",
            ),
            // An idle that the server ends by itself is not one to start
            // again without a pause.
            (
                &[
                    LISTED,
                    EXAMINED,
                    ("IDLE", "+ idling\r\n{tag} OK Idle terminated\r\n"),
                ],
                false,
                "stopped idling: Idle terminated",
            ),
        ];
        for (script, not_found, words) in refused {
            let ended = watching(script, SOON, NEVER).await.ended().await;
            match &ended {
                Err(Error::NotFound(text)) if not_found => assert!(text.contains(words), "{text}"),
                Err(Error::Backend(text)) if !not_found => assert!(text.contains(words), "{text}"),
                _ => panic!("{words}: {ended:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_server_that_will_not_end_the_idle_is_an_error() {
        let script = steps(&[&OPENED, &[IDLING, ("DONE", "{tag} BAD Not idling\r\n")]]);
        let ended = watching(&script, SOON, Duration::from_millis(50))
            .await
            .ended()
            .await;
        match ended {
            Err(Error::Backend(text)) => assert!(text.contains("Not idling"), "{text}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn lines_that_are_not_imap_end_the_watch_and_nothing_worse() {
        for garbage in [
            "* 3 FROBNICATE\r\n",
            "* \r\n",
            "*\r\n",
            "\r\n",
            "\u{0}\u{1}\u{2}\u{7f}\r\n",
            "Grüße aus dem Rechenzentrum\r\n",
            // Numbers no message count can be.
            "* 99999999999999999999 EXISTS\r\n",
            "* -1 EXISTS\r\n",
            "* 4294967296 EXPUNGE\r\n",
            "* VANISHED (EARLIER\r\n",
            // A literal far larger than anything that will be held in
            // memory.
            "* 2 FETCH (BODY[] {99999999999}\r\n",
        ] {
            // While idling, and before the server has agreed to.
            for idling in [format!("+ idling\r\n{garbage}"), garbage.to_owned()] {
                let script = steps(&[&OPENED, &[("IDLE", idling.as_str())]]);
                let ended = watching(&script, Duration::from_millis(300), NEVER)
                    .await
                    .ended()
                    .await;
                assert!(ended.is_err(), "{garbage:?}: {ended:?}");
            }
        }
    }

    #[tokio::test]
    async fn a_line_the_server_never_finishes_is_found_out_when_the_idle_is_next_ended() {
        // To the parser these are the start of something longer, so it
        // waits for the rest, and while idling a wait has no limit. The
        // next `DONE` has one.
        for unfinished in ["* 2 FETCH (FLAGS (\\Seen\r\n", "* 2 FETCH (FLAGS \r\n"] {
            let idling = format!("+ idling\r\n{unfinished}");
            let script = steps(&[&OPENED, &[("IDLE", idling.as_str()), ("DONE", "")]]);
            let (patience, refresh) = (Duration::from_millis(80), Duration::from_millis(50));
            let ended = watching(&script, patience, refresh).await.ended().await;
            assert!(
                matches!(ended, Err(Error::Connection(_))),
                "{unfinished:?}: {ended:?}"
            );

            // Before the server has agreed to idle, the wait for the rest
            // is an ordinary one.
            let script = steps(&[&OPENED, &[("IDLE", unfinished)]]);
            let ended = watching(&script, patience, NEVER).await.ended().await;
            assert!(
                matches!(ended, Err(Error::Connection(_))),
                "{unfinished:?}: {ended:?}"
            );
        }
    }

    #[test]
    fn a_count_is_news_only_when_it_is_a_different_count() {
        let mut watched = Watched {
            exists: Some(3),
            recent: Some(0),
            report_at: None,
        };
        let exists = |count| Response::MailboxData(MailboxDatum::Exists(count));
        let recent = |count| Response::MailboxData(MailboxDatum::Recent(count));
        assert!(!watched.changed_by(&exists(3)));
        assert!(!watched.changed_by(&recent(0)));
        assert!(watched.changed_by(&exists(4)));
        assert!(!watched.changed_by(&exists(4)));
        assert!(watched.changed_by(&recent(1)));
        assert!(!watched.changed_by(&recent(1)));
        // An expunge is always news, and takes one off the count, so the
        // count the server may give after it is the one already known.
        assert!(watched.changed_by(&Response::Expunge(1)));
        assert!(!watched.changed_by(&exists(3)));
        assert!(watched.changed_by(&exists(2)));
        // A folder cannot hold fewer than no messages, whatever is said.
        watched.exists = Some(0);
        assert!(watched.changed_by(&Response::Expunge(1)));
        assert_eq!(watched.exists, Some(0));

        // A server that gave no counts when the folder was opened: the
        // first of each is news.
        let mut unknown = Watched::default();
        assert!(unknown.changed_by(&exists(0)));
        assert!(unknown.changed_by(&recent(0)));
        assert!(!unknown.changed_by(&exists(0)));
    }
}
