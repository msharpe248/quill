//! A [`Backend`] over an invented mailbox, for `quill tui --demo` and tests.
//!
//! Everyone here is made up and lives at example.com. Message dates are
//! relative to the `now` the backend is built with, so the demo always looks
//! recent and tests stay deterministic.

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc::Sender;

use crate::types::{
    Address, Capabilities, Flags, FolderId, FolderRole, Headers, Operation, RemoteEvent,
    RemoteFolder, RemoteId, RemoteMessage, SyncCursor, SyncDelta,
};
use crate::{Backend, Error, Result};

/// The two accounts of the demo mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoAccount {
    Work,
    Personal,
}

impl DemoAccount {
    pub const ALL: [DemoAccount; 2] = [DemoAccount::Work, DemoAccount::Personal];

    pub fn name(self) -> &'static str {
        match self {
            Self::Work => "Work",
            Self::Personal => "Personal",
        }
    }

    /// The mailbox owner.
    pub fn owner(self) -> Address {
        match self {
            Self::Work => Address::new("Robin Vale", "robin@example.com"),
            Self::Personal => Address::new("Robin Vale", "robin.vale@example.com"),
        }
    }

    fn seeds(self) -> &'static [Seed] {
        match self {
            Self::Work => WORK,
            Self::Personal => PERSONAL,
        }
    }
}

const FOLDERS: [(&str, &str, FolderRole); 5] = [
    ("inbox", "Inbox", FolderRole::Inbox),
    ("drafts", "Drafts", FolderRole::Drafts),
    ("sent", "Sent", FolderRole::Sent),
    ("archive", "Archive", FolderRole::Archive),
    ("trash", "Trash", FolderRole::Trash),
];

/// The cursor a full sync hands back. Syncing from it reports no changes.
const SYNCED: &str = "fixture-v1";

/// One fixture message. In Sent and Drafts, `who` is the recipient; everywhere
/// else it is the sender.
struct Seed {
    folder: &'static str,
    who: (&'static str, &'static str),
    subject: &'static str,
    minutes_ago: i64,
    unread: bool,
    starred: bool,
    attachments: bool,
    body: &'static str,
}

const HOUR: i64 = 60;
const DAY: i64 = 24 * HOUR;

const WORK: &[Seed] = &[
    Seed {
        folder: "inbox",
        who: ("Ana Ruiz", "ana.ruiz@example.com"),
        subject: "Q4 roadmap",
        minutes_ago: 16,
        unread: true,
        starred: false,
        attachments: true,
        body: "Hi Robin,\n\nHere is the draft roadmap for Q4. The short version: we ship the sync \
               rewrite in October, spend November on search, and keep December for whatever the \
               first two months break.\n\nThree things I would like your eyes on before Thursday:\n\n\
               1. The search milestone assumes the index format is settled. Is it?\n\
               2. I have pencilled in two weeks for the offline queue. Tell me if that is fantasy.\n\
               3. The on-call rota over the holidays is still empty.\n\n\
               The PDF has the detail, including the dependency chart you asked for. Comments in \
               the doc or by reply, whichever is easier.\n\nThanks,\nAna\n\n-- \nAna Ruiz\n\
               Product, Example Corp\n",
    },
    Seed {
        folder: "inbox",
        who: ("Build Bot", "ci@example.com"),
        subject: "[quill] PR #42 passed",
        minutes_ago: 39,
        unread: false,
        starred: false,
        attachments: false,
        body: "All 214 checks passed for PR #42 (store: forward-only migrations).\n\n\
               fmt      ok\nclippy   ok\ntest     ok   (212 passed, 2 ignored)\ndeny     ok\n\n\
               This message was sent by a robot. Replies go nowhere.\n",
    },
    Seed {
        folder: "inbox",
        who: ("Sam Lee", "sam.lee@example.com"),
        subject: "Photos from Sat",
        minutes_ago: DAY + 2 * HOUR,
        unread: true,
        starred: false,
        attachments: true,
        body: "Robin,\n\nA few shots from the team hike attached. The one of you falling into the \
               stream is, I think, the best photograph I have ever taken.\n\nSam\n",
    },
    Seed {
        folder: "inbox",
        who: ("Priya Nair", "priya.nair@example.com"),
        subject: "Re: Offsite agenda",
        minutes_ago: DAY + 5 * HOUR,
        unread: true,
        starred: false,
        attachments: false,
        body: "Works for me. I would move the retro to the morning while people still have \
               opinions, and push the planning session after lunch.\n\n\
               > Draft agenda attached. Shout if anything is missing.\n\
               > We have the room from nine until four.\n\nPriya\n",
    },
    Seed {
        folder: "inbox",
        who: ("Tomás Ortega", "tomas.ortega@example.com"),
        subject: "Invoice 2026-114",
        minutes_ago: 2 * DAY + 3 * HOUR,
        unread: true,
        starred: true,
        attachments: true,
        body: "Hello Robin,\n\nPlease find attached invoice 2026-114 for October. Payment terms \
               are thirty days as usual.\n\nBest regards,\nTomás\n",
    },
    Seed {
        folder: "inbox",
        who: ("陈静 Chen Jing", "chen.jing@example.com"),
        subject: "Design review notes 设计评审",
        minutes_ago: 3 * DAY + HOUR,
        unread: true,
        starred: false,
        attachments: false,
        body: "Notes from today's review:\n\n\
               - The three-pane layout holds up down to about a hundred columns.\n\
               - Below that the reader should replace the list. Nobody wanted a tiny reader.\n\
               - The help overlay needs to say which pane it is describing.\n\n\
               谢谢大家 🎉\n\nJing\n",
    },
    Seed {
        folder: "inbox",
        who: ("IT Helpdesk", "helpdesk@example.com"),
        subject: "Scheduled maintenance Sunday",
        minutes_ago: 4 * DAY + 6 * HOUR,
        unread: false,
        starred: false,
        attachments: false,
        body: "The mail servers will be unavailable on Sunday between 02:00 and 04:00 UTC while \
               we apply updates. Mail sent to you during the window will be delivered afterwards.\n",
    },
    Seed {
        folder: "inbox",
        who: ("Lena Fischer", "lena.fischer@example.com"),
        subject: "Lunch on Thursday?",
        minutes_ago: 5 * DAY + 2 * HOUR,
        unread: true,
        starred: false,
        attachments: false,
        body: "Are you around on Thursday? There is a new noodle place by the station and I \
               refuse to go alone.\n\nLena\n",
    },
    Seed {
        folder: "inbox",
        who: ("Omar Haddad", "omar.haddad@example.com"),
        subject: "Re: Contract redlines",
        minutes_ago: 8 * DAY + 4 * HOUR,
        unread: true,
        starred: true,
        attachments: true,
        body: "Robin,\n\nLegal accepted everything except clause 7.2. Their counter-proposal is \
               attached with changes tracked. I think we can live with it.\n\nOmar\n",
    },
    Seed {
        folder: "inbox",
        who: ("The Weekly Parser", "newsletter@example.com"),
        subject: "Issue 88: Parsing email is still hard",
        minutes_ago: 12 * DAY,
        unread: true,
        starred: false,
        attachments: false,
        body: "This week: why nobody agrees on what a quoted-printable soft line break is, a \
               tour of the worst Content-Type headers seen in the wild, and a reader asks whether \
               RFC 5322 comments can really be nested. (They can.)\n",
    },
    Seed {
        folder: "inbox",
        who: ("Ana Ruiz", "ana.ruiz@example.com"),
        subject: "Welcome to the team",
        minutes_ago: 400 * DAY,
        unread: false,
        starred: false,
        attachments: false,
        body: "Robin,\n\nWelcome aboard! Your laptop is at the front desk and your first week is \
               mostly meeting people.\n\nAna\n",
    },
    Seed {
        folder: "drafts",
        who: ("Ana Ruiz", "ana.ruiz@example.com"),
        subject: "Re: Q4 roadmap",
        minutes_ago: 5,
        unread: false,
        starred: false,
        attachments: false,
        body: "Ana,\n\nTwo weeks for the offline queue is fantasy. Four, maybe.\n",
    },
    Seed {
        folder: "sent",
        who: ("Priya Nair", "priya.nair@example.com"),
        subject: "Offsite agenda",
        minutes_ago: DAY + 7 * HOUR,
        unread: false,
        starred: false,
        attachments: true,
        body: "Draft agenda attached. Shout if anything is missing.\n\
               We have the room from nine until four.\n\nRobin\n",
    },
    Seed {
        folder: "sent",
        who: ("Omar Haddad", "omar.haddad@example.com"),
        subject: "Contract redlines",
        minutes_ago: 9 * DAY,
        unread: false,
        starred: false,
        attachments: true,
        body: "Omar,\n\nOur redlines are attached. Clause 7.2 is the one that matters.\n\nRobin\n",
    },
    Seed {
        folder: "archive",
        who: ("Lena Fischer", "lena.fischer@example.com"),
        subject: "Conference travel",
        minutes_ago: 40 * DAY,
        unread: false,
        starred: false,
        attachments: false,
        body: "Flights are booked. We land at ten and the first talk is at two.\n\nLena\n",
    },
    Seed {
        folder: "archive",
        who: ("Build Bot", "ci@example.com"),
        subject: "[quill] PR #17 failed",
        minutes_ago: 55 * DAY,
        unread: false,
        starred: false,
        attachments: false,
        body: "1 of 198 checks failed for PR #17.\n\nclippy   FAILED\n",
    },
];

const PERSONAL: &[Seed] = &[
    Seed {
        folder: "inbox",
        who: ("Peter Vale", "peter.vale@example.com"),
        subject: "Sunday dinner",
        minutes_ago: 55,
        unread: true,
        starred: false,
        attachments: false,
        body: "Your mother wants to know whether you are coming on Sunday and whether you are \
               bringing anyone. I want to know whether you still have my drill.\n\nDad\n",
    },
    Seed {
        folder: "inbox",
        who: ("Maya Okafor", "maya.okafor@example.com"),
        subject: "Trail run Saturday",
        minutes_ago: 5 * HOUR,
        unread: true,
        starred: false,
        attachments: false,
        body: "Seven o'clock at the north car park. It is twelve kilometres and only the first \
               three are uphill, which is what I said last time too.\n\nMaya\n",
    },
    Seed {
        folder: "inbox",
        who: ("Riverside Library", "holds@example.com"),
        subject: "Your hold is ready",
        minutes_ago: DAY + 3 * HOUR,
        unread: true,
        starred: false,
        attachments: false,
        body: "The item you requested is ready for collection at the Riverside branch. We will \
               keep it for seven days.\n",
    },
    Seed {
        folder: "inbox",
        who: ("Jo Park", "jo.park@example.com"),
        subject: "Re: Book club pick",
        minutes_ago: 2 * DAY + 5 * HOUR,
        unread: false,
        starred: false,
        attachments: false,
        body: "Fine, but if it is another nine-hundred-page novel about a sad professor I am \
               leaving the club.\n\n> How about the one everybody is talking about?\n\nJo\n",
    },
    Seed {
        folder: "inbox",
        who: ("Hillcrest Dental", "reception@example.com"),
        subject: "Appointment reminder",
        minutes_ago: 6 * DAY,
        unread: true,
        starred: false,
        attachments: false,
        body: "This is a reminder of your check-up next Tuesday at 08:40. Please let us know at \
               least a day ahead if you cannot make it.\n",
    },
    Seed {
        folder: "inbox",
        who: ("Maya Okafor", "maya.okafor@example.com"),
        subject: "Recipe you asked for",
        minutes_ago: 20 * DAY,
        unread: false,
        starred: true,
        attachments: true,
        body: "The jollof recipe is attached. Do not skip the step where you let the base fry \
               down. Everything depends on it.\n\nMaya\n",
    },
    Seed {
        folder: "sent",
        who: ("Jo Park", "jo.park@example.com"),
        subject: "Book club pick",
        minutes_ago: 2 * DAY + 8 * HOUR,
        unread: false,
        starred: false,
        attachments: false,
        body: "How about the one everybody is talking about?\n\nRobin\n",
    },
    Seed {
        folder: "archive",
        who: ("Riverside Library", "holds@example.com"),
        subject: "Your loan is due soon",
        minutes_ago: 30 * DAY,
        unread: false,
        starred: false,
        attachments: false,
        body: "One item on your account is due back in three days.\n",
    },
];

/// A read-only backend serving one [`DemoAccount`].
pub struct FixtureBackend {
    account: DemoAccount,
    now: i64,
}

impl FixtureBackend {
    /// `now` is seconds since the Unix epoch; message dates count back from it.
    pub fn demo(account: DemoAccount, now: i64) -> Self {
        Self { account, now }
    }

    pub fn account(&self) -> DemoAccount {
        self.account
    }

    fn remote_id(seed: &Seed, index: usize) -> RemoteId {
        RemoteId(format!("{}-{}", seed.folder, index + 1))
    }

    fn find(&self, id: &RemoteId) -> Result<(usize, &'static Seed)> {
        self.account
            .seeds()
            .iter()
            .enumerate()
            .find(|(index, seed)| Self::remote_id(seed, *index) == *id)
            .ok_or_else(|| Error::NotFound(format!("message {}", id.0)))
    }

    fn has_folder(folder: &FolderId) -> bool {
        FOLDERS.iter().any(|(id, _, _)| *id == folder.0)
    }

    fn headers(&self, seed: &Seed, index: usize) -> Headers {
        let owner = self.account.owner();
        let other = Address::new(seed.who.0, seed.who.1);
        let outgoing = matches!(seed.folder, "sent" | "drafts");
        let (from, to) = if outgoing {
            (owner, other)
        } else {
            (other, owner)
        };
        Headers {
            message_id: Some(format!(
                "<{}.{}@example.com>",
                self.account.name().to_lowercase(),
                Self::remote_id(seed, index).0
            )),
            subject: seed.subject.to_owned(),
            from: vec![from],
            to: vec![to],
            cc: Vec::new(),
            date: self.now - seed.minutes_ago * 60,
        }
    }

    fn raw(&self, seed: &Seed, index: usize) -> Result<String> {
        let headers = self.headers(seed, index);
        let date = jiff::Timestamp::from_second(headers.date)
            .map_err(|err| Error::Backend(err.to_string()))?
            .to_zoned(jiff::tz::TimeZone::UTC);
        let date =
            jiff::fmt::rfc2822::to_string(&date).map_err(|err| Error::Backend(err.to_string()))?;
        let mut raw = String::new();
        for (name, value) in [
            ("From", join(&headers.from)),
            ("To", join(&headers.to)),
            ("Subject", headers.subject.clone()),
            ("Date", date),
            ("Message-ID", headers.message_id.clone().unwrap_or_default()),
            ("MIME-Version", "1.0".to_owned()),
            ("Content-Type", "text/plain; charset=utf-8".to_owned()),
            ("Content-Transfer-Encoding", "8bit".to_owned()),
        ] {
            raw.push_str(name);
            raw.push_str(": ");
            raw.push_str(&value);
            raw.push_str("\r\n");
        }
        raw.push_str("\r\n");
        raw.push_str(&seed.body.replace('\n', "\r\n"));
        Ok(raw)
    }
}

fn join(addresses: &[Address]) -> String {
    addresses
        .iter()
        .map(Address::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[async_trait]
impl Backend for FixtureBackend {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            folders: true,
            flags: true,
            push: false,
            server_search: false,
            threads: false,
        }
    }

    async fn list_folders(&self) -> Result<Vec<RemoteFolder>> {
        Ok(FOLDERS
            .iter()
            .map(|(id, name, role)| RemoteFolder {
                id: FolderId((*id).to_owned()),
                name: (*name).to_owned(),
                role: *role,
            })
            .collect())
    }

    async fn sync_folder(
        &self,
        folder: &FolderId,
        cursor: Option<SyncCursor>,
    ) -> Result<SyncDelta> {
        if !Self::has_folder(folder) {
            return Err(Error::NotFound(format!("folder {}", folder.0)));
        }
        let mut delta = SyncDelta {
            added: Vec::new(),
            changed: Vec::new(),
            removed: Vec::new(),
            reset: false,
            has_more: false,
            next_cursor: SyncCursor(SYNCED.to_owned()),
        };
        if cursor.is_some_and(|cursor| cursor.0 == SYNCED) {
            return Ok(delta);
        }
        for (index, seed) in self.account.seeds().iter().enumerate() {
            if seed.folder != folder.0 {
                continue;
            }
            delta.added.push(RemoteMessage {
                id: Self::remote_id(seed, index),
                headers: self.headers(seed, index),
                flags: Flags {
                    seen: !seed.unread,
                    flagged: seed.starred,
                    answered: false,
                    draft: seed.folder == "drafts",
                },
                size: self.raw(seed, index)?.len() as u64,
                has_attachments: seed.attachments,
            });
        }
        Ok(delta)
    }

    async fn fetch_raw(&self, id: &RemoteId) -> Result<Bytes> {
        let (index, seed) = self.find(id)?;
        Ok(Bytes::from(self.raw(seed, index)?))
    }

    /// The fixture mailbox is read-only: an operation on a message that exists
    /// succeeds and changes nothing.
    async fn apply(&self, op: &Operation) -> Result<()> {
        let (id, target) = match op {
            Operation::SetFlags { id, .. } | Operation::Delete { id } => (id, None),
            Operation::Move { id, to } | Operation::Copy { id, to } => (id, Some(to)),
        };
        self.find(id)?;
        match target {
            Some(folder) if !Self::has_folder(folder) => {
                Err(Error::NotFound(format!("folder {}", folder.0)))
            }
            _ => Ok(()),
        }
    }

    /// Nothing ever changes remotely, so there is nothing to watch.
    async fn watch(&self, _tx: Sender<RemoteEvent>) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_648_600; // 2026-11-14T09:30:00Z

    fn folder(id: &str) -> FolderId {
        FolderId(id.to_owned())
    }

    async fn inbox(account: DemoAccount) -> SyncDelta {
        FixtureBackend::demo(account, NOW)
            .sync_folder(&folder("inbox"), None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn lists_the_five_folders_with_roles() {
        let folders = FixtureBackend::demo(DemoAccount::Work, NOW)
            .list_folders()
            .await
            .unwrap();
        assert_eq!(folders.len(), 5);
        assert_eq!(folders[0].role, FolderRole::Inbox);
        assert!(folders.iter().any(|f| f.role == FolderRole::Sent));
    }

    #[tokio::test]
    async fn unread_counts_match_the_spec_sketch() {
        let unread = |delta: &SyncDelta| delta.added.iter().filter(|m| !m.flags.seen).count();
        assert_eq!(unread(&inbox(DemoAccount::Work).await), 8);
        assert_eq!(unread(&inbox(DemoAccount::Personal).await), 4);
    }

    #[tokio::test]
    async fn syncing_from_the_returned_cursor_reports_nothing() {
        let backend = FixtureBackend::demo(DemoAccount::Work, NOW);
        let first = backend.sync_folder(&folder("inbox"), None).await.unwrap();
        assert!(!first.added.is_empty());
        let second = backend
            .sync_folder(&folder("inbox"), Some(first.next_cursor))
            .await
            .unwrap();
        assert!(second.added.is_empty() && second.changed.is_empty() && second.removed.is_empty());
    }

    #[tokio::test]
    async fn remote_ids_are_unique_within_an_account() {
        for account in DemoAccount::ALL {
            let backend = FixtureBackend::demo(account, NOW);
            let mut ids = Vec::new();
            for f in backend.list_folders().await.unwrap() {
                let delta = backend.sync_folder(&f.id, None).await.unwrap();
                ids.extend(delta.added.into_iter().map(|m| m.id.0));
            }
            let total = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), total);
        }
    }

    #[tokio::test]
    async fn raw_messages_are_crlf_rfc5322_with_matching_headers() {
        let backend = FixtureBackend::demo(DemoAccount::Work, NOW);
        let delta = inbox(DemoAccount::Work).await;
        let first = &delta.added[0];
        let raw = backend.fetch_raw(&first.id).await.unwrap();
        let text = std::str::from_utf8(&raw).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(head.contains("Subject: Q4 roadmap\r\n"));
        assert!(head.contains("From: Ana Ruiz <ana.ruiz@example.com>\r\n"));
        assert!(head.contains("Date: Sat, 14 Nov 2026 09:14:00 +0000"));
        assert!(body.starts_with("Hi Robin,\r\n"));
        assert!(!body.replace("\r\n", "").contains('\n'));
        assert_eq!(first.size, raw.len() as u64);
    }

    #[tokio::test]
    async fn sent_mail_is_from_the_owner() {
        let backend = FixtureBackend::demo(DemoAccount::Work, NOW);
        let sent = backend.sync_folder(&folder("sent"), None).await.unwrap();
        assert!(!sent.added.is_empty());
        for message in sent.added {
            assert_eq!(message.headers.from, vec![DemoAccount::Work.owner()]);
        }
    }

    #[tokio::test]
    async fn every_address_is_at_example_com() {
        for account in DemoAccount::ALL {
            let backend = FixtureBackend::demo(account, NOW);
            for f in backend.list_folders().await.unwrap() {
                for message in backend.sync_folder(&f.id, None).await.unwrap().added {
                    let headers = message.headers;
                    for address in headers.from.iter().chain(&headers.to).chain(&headers.cc) {
                        assert!(address.email.ends_with("@example.com"), "{address}");
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn unknown_folders_and_messages_are_not_found() {
        let backend = FixtureBackend::demo(DemoAccount::Work, NOW);
        assert!(matches!(
            backend.sync_folder(&folder("nope"), None).await,
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            backend.fetch_raw(&RemoteId("inbox-999".to_owned())).await,
            Err(Error::NotFound(_))
        ));
        let op = Operation::Move {
            id: RemoteId("inbox-1".to_owned()),
            to: folder("nope"),
        };
        assert!(matches!(backend.apply(&op).await, Err(Error::NotFound(_))));
        let op = Operation::Move {
            id: RemoteId("inbox-1".to_owned()),
            to: folder("archive"),
        };
        assert!(backend.apply(&op).await.is_ok());
    }
}
