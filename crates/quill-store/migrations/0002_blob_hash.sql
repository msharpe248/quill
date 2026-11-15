-- The content hash of a message's raw bytes, once downloaded. The bytes
-- themselves live as a file named by this hash, beside the database.
ALTER TABLE messages ADD COLUMN blob_hash TEXT;
