-- The outbound queue: every change to mail on a server is written here
-- first, shown locally at once, and replayed against the server in order.
-- It survives restarts.
CREATE TABLE operations (
    id            INTEGER PRIMARY KEY,
    account_id    INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- 'move' | 'delete' | 'flags' | 'append'
    kind          TEXT NOT NULL,
    -- 'pending' until the sync engine takes it, then 'running'. A change can
    -- be undone only while it is pending.
    state         TEXT NOT NULL DEFAULT 'pending',
    -- The local message the operation is about. Gone if the message is.
    message_id    INTEGER REFERENCES messages (id) ON DELETE SET NULL,
    -- move, delete: what the server called the message before, and where it
    -- was. For a message still waiting on an earlier move this is that
    -- move's placeholder, replaced when the earlier move completes.
    remote_id     TEXT,
    source_folder INTEGER REFERENCES folders (id) ON DELETE CASCADE,
    -- move, append: where to.
    target_folder INTEGER REFERENCES folders (id) ON DELETE CASCADE,
    -- append: the raw message to save, by content hash.
    blob_hash     TEXT,
    -- flags, append: the flags to set, and for flags what to go back to if
    -- the server refuses. Four digits: seen, flagged, answered, draft.
    flags         TEXT,
    previous      TEXT,
    -- Not to be sent to the server before this time (seconds since the
    -- epoch): the window in which the change can be undone.
    not_before    INTEGER NOT NULL,
    attempts      INTEGER NOT NULL DEFAULT 0,
    created       INTEGER NOT NULL
) STRICT;

CREATE INDEX operations_by_account ON operations (account_id, id);

-- A message deleted for good is hidden at once and removed when the server
-- confirms. Hidden rows are in no list and no count.
ALTER TABLE messages ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;

-- Mail being written, and mail waiting to go.
CREATE TABLE drafts (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- 'draft' while being written; 'outbox' once sent on its way;
    -- 'sending' while a server has it; 'failed' when a server refused it.
    state      TEXT NOT NULL DEFAULT 'draft',
    -- The fields as written, as JSON.
    content    TEXT NOT NULL,
    -- outbox: the finished message, by content hash, and who it goes to on
    -- the wire, as JSON. Built once, so every attempt sends the same bytes
    -- under the same Message-ID.
    raw_hash   TEXT,
    envelope   TEXT,
    -- outbox: not to be sent before this time. Undo send and send later.
    send_after INTEGER,
    -- Why the last attempt failed, in the server's words.
    error      TEXT,
    attempts   INTEGER NOT NULL DEFAULT 0,
    updated    INTEGER NOT NULL
) STRICT;

-- Everyone mail has come from or gone to, for completing addresses.
CREATE TABLE contacts (
    email     TEXT PRIMARY KEY COLLATE NOCASE,
    name      TEXT NOT NULL,
    -- How many messages the address has appeared on. Mail the user sent
    -- counts for more than mail they received.
    weight    INTEGER NOT NULL,
    -- The date of the newest such message.
    last_seen INTEGER NOT NULL
) STRICT;
