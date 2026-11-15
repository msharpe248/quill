-- Accounts, folders, message headers, extracted body text and sync cursors.
-- Threads, attachments, the operations queue, contacts and the FTS5 index
-- arrive with the milestones that use them.

CREATE TABLE accounts (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE,
    address TEXT NOT NULL,
    kind    TEXT NOT NULL
) STRICT;

CREATE TABLE folders (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    remote_id  TEXT NOT NULL,
    name       TEXT NOT NULL,
    role       TEXT NOT NULL,
    UNIQUE (account_id, remote_id)
) STRICT;

CREATE TABLE messages (
    id              INTEGER PRIMARY KEY,
    folder_id       INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    remote_id       TEXT NOT NULL,
    message_id      TEXT,
    subject         TEXT NOT NULL,
    -- JSON arrays of {"name": ..., "email": ...}.
    from_addrs      TEXT NOT NULL,
    to_addrs        TEXT NOT NULL,
    cc_addrs        TEXT NOT NULL,
    -- Seconds since the Unix epoch, UTC.
    date            INTEGER NOT NULL,
    size            INTEGER NOT NULL,
    seen            INTEGER NOT NULL,
    flagged         INTEGER NOT NULL,
    answered        INTEGER NOT NULL,
    draft           INTEGER NOT NULL,
    has_attachments INTEGER NOT NULL,
    UNIQUE (folder_id, remote_id)
) STRICT;

CREATE INDEX messages_by_folder_date ON messages (folder_id, date DESC, id DESC);

CREATE TABLE bodies (
    message_id INTEGER PRIMARY KEY REFERENCES messages (id) ON DELETE CASCADE,
    text       TEXT NOT NULL
) STRICT;

CREATE TABLE sync_state (
    folder_id INTEGER PRIMARY KEY REFERENCES folders (id) ON DELETE CASCADE,
    cursor    TEXT NOT NULL
) STRICT;
