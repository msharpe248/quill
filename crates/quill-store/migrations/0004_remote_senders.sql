-- Senders whose mail may load remote content (pictures and the like from
-- the web) without being asked each time. Remote content is blocked for
-- everyone else: loading it tells the sender the message was opened, and
-- when, and from where.
CREATE TABLE remote_senders (
    -- The sender's address, lower case.
    address TEXT PRIMARY KEY,
    added   INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
