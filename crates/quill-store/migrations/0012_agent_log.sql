-- What programs outside Quill's window did with the mail: the commands for
-- a shell, and agents over MCP. It is how the user finds out afterwards, so
-- nothing but age takes a line out of it.
CREATE TABLE agent_log (
    id      INTEGER PRIMARY KEY,
    -- Seconds since the Unix epoch, UTC.
    at      INTEGER NOT NULL,
    -- 'shell' | 'mcp': which way in.
    way     TEXT NOT NULL,
    -- The name the caller gave. Its own claim, and nothing more.
    agent   TEXT NOT NULL,
    -- The command: 'move', 'search', ...
    action  TEXT NOT NULL,
    -- 1: the mail was changed, or mail was sent. 0: it was looked at.
    change  INTEGER NOT NULL,
    -- What was asked and what came of it, in a line. Never what a message
    -- said.
    summary TEXT NOT NULL,
    -- How many messages were changed, or came back.
    count   INTEGER NOT NULL,
    -- A change that can be taken back: what is needed to do so, as JSON.
    undo    TEXT,
    -- When it was taken back.
    undone  INTEGER,
    -- A message sent: when it was due to go, for keeping the wait between
    -- one and the next.
    due     INTEGER
) STRICT;

CREATE INDEX agent_log_by_time ON agent_log (at DESC, id DESC);

-- Mail sent from outside Quill's window: who sent it, by the name they
-- gave. NULL for the user's own. Such mail may also wait in the outbox in
-- the state 'held', which is never sent until the user lets it go.
ALTER TABLE drafts ADD COLUMN agent TEXT;
