-- The search index: every message's subject, sender, recipients and body
-- text, as words. It holds no text of its own (the text is in `messages`
-- and `bodies`), only where each word occurs; its rowid is the message's id.
--
-- Triggers keep it in step, so no code that writes mail has to remember to:
-- a message is indexed when its headers arrive, and again when its body
-- does. Each takes the old entry out before putting the new one in. (An
-- "INSERT OR REPLACE" would do, except that inside a trigger the statement
-- that fired it decides how conflicts are handled, and the old words would
-- stay.)
CREATE VIRTUAL TABLE message_search USING fts5(
    subject,
    sender,
    recipients,
    body,
    content = '',
    contentless_delete = 1,
    -- Words are matched without regard to case or accents.
    tokenize = "unicode61 remove_diacritics 2"
);

-- The people in a JSON array of addresses, as text: names and addresses.
-- Written out in each trigger, since SQLite has no stored functions.

CREATE TRIGGER message_search_insert AFTER INSERT ON messages BEGIN
    DELETE FROM message_search WHERE rowid = NEW.id;
    INSERT INTO message_search (rowid, subject, sender, recipients, body)
    VALUES (
        NEW.id,
        NEW.subject,
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM json_each(NEW.from_addrs)),
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM (SELECT value FROM json_each(NEW.to_addrs)
                 UNION ALL SELECT value FROM json_each(NEW.cc_addrs))),
        coalesce((SELECT text FROM bodies WHERE message_id = NEW.id), '')
    );
END;

CREATE TRIGGER message_search_update
AFTER UPDATE OF subject, from_addrs, to_addrs, cc_addrs ON messages BEGIN
    DELETE FROM message_search WHERE rowid = NEW.id;
    INSERT INTO message_search (rowid, subject, sender, recipients, body)
    VALUES (
        NEW.id,
        NEW.subject,
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM json_each(NEW.from_addrs)),
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM (SELECT value FROM json_each(NEW.to_addrs)
                 UNION ALL SELECT value FROM json_each(NEW.cc_addrs))),
        coalesce((SELECT text FROM bodies WHERE message_id = NEW.id), '')
    );
END;

CREATE TRIGGER message_search_delete AFTER DELETE ON messages BEGIN
    DELETE FROM message_search WHERE rowid = OLD.id;
END;

CREATE TRIGGER message_search_body_insert AFTER INSERT ON bodies BEGIN
    DELETE FROM message_search WHERE rowid = NEW.message_id;
    INSERT INTO message_search (rowid, subject, sender, recipients, body)
    SELECT m.id, m.subject,
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM json_each(m.from_addrs)),
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM (SELECT value FROM json_each(m.to_addrs)
                 UNION ALL SELECT value FROM json_each(m.cc_addrs))),
        NEW.text
      FROM messages m WHERE m.id = NEW.message_id;
END;

CREATE TRIGGER message_search_body_update AFTER UPDATE OF text ON bodies BEGIN
    DELETE FROM message_search WHERE rowid = NEW.message_id;
    INSERT INTO message_search (rowid, subject, sender, recipients, body)
    SELECT m.id, m.subject,
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM json_each(m.from_addrs)),
        (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
                || coalesce(json_extract(value, '$.email'), ''), ' '), '')
           FROM (SELECT value FROM json_each(m.to_addrs)
                 UNION ALL SELECT value FROM json_each(m.cc_addrs))),
        NEW.text
      FROM messages m WHERE m.id = NEW.message_id;
END;

-- Mail that was here before the index was.
INSERT INTO message_search (rowid, subject, sender, recipients, body)
SELECT m.id, m.subject,
    (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
            || coalesce(json_extract(value, '$.email'), ''), ' '), '')
       FROM json_each(m.from_addrs)),
    (SELECT coalesce(group_concat(coalesce(json_extract(value, '$.name'), '') || ' '
            || coalesce(json_extract(value, '$.email'), ''), ' '), '')
       FROM (SELECT value FROM json_each(m.to_addrs)
             UNION ALL SELECT value FROM json_each(m.cc_addrs))),
    coalesce(b.text, '')
  FROM messages m LEFT JOIN bodies b ON b.message_id = m.id;
