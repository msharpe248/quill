-- Operations made by one action on several messages share a batch: the id of
-- the first of them. Undo takes the whole batch back. NULL for an operation
-- made by itself.
ALTER TABLE operations ADD COLUMN batch INTEGER;
