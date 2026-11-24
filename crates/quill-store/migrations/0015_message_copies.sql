-- One message is often in several folders of an account: Gmail keeps
-- everything in All Mail as well as where it is shown. A copy that has
-- been downloaded is looked for by the message's own id before the
-- server is asked for it again.
CREATE INDEX messages_by_header ON messages (message_id) WHERE message_id IS NOT NULL;
