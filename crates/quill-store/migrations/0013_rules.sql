-- Rules: which mail the rules have been asked about, and the mailing list
-- a message came through.

-- When the rules were asked about a message, by themselves: as it arrived,
-- or catching up. Empty until they are. A message is asked once, so that
-- one the user put back in the inbox stays there.
ALTER TABLE messages ADD COLUMN ruled_at INTEGER;

-- The message's `List-Id`, read when it is downloaded. Empty text: it was
-- looked for and the message has none. No value: not looked for yet.
ALTER TABLE messages ADD COLUMN list_id TEXT;
