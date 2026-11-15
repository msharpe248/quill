-- Conversations. A message's thread is found from the Message-IDs it
-- names: its own and those of the messages it follows. Every id that has
-- been seen belongs to one thread, and a message that names ids of two
-- threads joins them into one.
--
-- NULL until a message's ids are known. Mail synced before this existed
-- gets its thread when its downloaded copy is next read for them.
ALTER TABLE messages ADD COLUMN thread_id INTEGER;

CREATE INDEX messages_by_thread ON messages (thread_id);

CREATE TABLE thread_ids (
    -- A Message-ID, with its angle brackets, as a message gave it.
    message_id TEXT PRIMARY KEY,
    thread     INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX thread_ids_by_thread ON thread_ids (thread);
