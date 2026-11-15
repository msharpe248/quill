-- Snoozing: a message put aside until a time. While `snoozed_until` is set
-- the message is out of its folder's list and counts, and in the Snoozed
-- mailbox. When the time comes it is cleared and `surfaced` is set, which
-- puts the message at the top of its folder as if it had just arrived.
--
-- This is kept here only. The message stays where it is on its server.
ALTER TABLE messages ADD COLUMN snoozed_until INTEGER;
ALTER TABLE messages ADD COLUMN surfaced INTEGER;

CREATE INDEX messages_snoozed ON messages (snoozed_until) WHERE snoozed_until IS NOT NULL;
