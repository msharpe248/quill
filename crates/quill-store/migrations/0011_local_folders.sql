-- Where a message was before it was moved to the trash, so that it can be
-- put back. A server keeps no such record: this is Quill's own, for mail
-- Quill itself moved there.
CREATE TABLE trashed (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    -- The row, for as long as it is the row it was: a move the server
    -- answers with the message's new name keeps it.
    message     INTEGER REFERENCES messages (id) ON DELETE SET NULL,
    -- Its Message-ID header: what it is found by once a sync has brought
    -- it back as a new row.
    message_id  TEXT,
    from_folder INTEGER NOT NULL REFERENCES folders (id) ON DELETE CASCADE,
    at          INTEGER NOT NULL
) STRICT;

CREATE INDEX trashed_by_message ON trashed (message);
CREATE INDEX trashed_by_header ON trashed (account_id, message_id);
