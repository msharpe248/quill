-- For accounts whose mail lives here and only passes through a server
-- (POP3): the messages still believed to be on the server, and when each
-- was first seen there. An account's policy for deleting from the server
-- works from this. A row goes when the server's copy does.
CREATE TABLE server_copies (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    remote_id  TEXT NOT NULL,
    seen_at    INTEGER NOT NULL,
    PRIMARY KEY (account_id, remote_id)
) STRICT, WITHOUT ROWID;
