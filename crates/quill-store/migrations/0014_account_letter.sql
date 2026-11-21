-- The one letter or number that stands for an account where mail of
-- several accounts is listed together, when the user has chosen one.
-- The settings file is where it is said; this is a copy for the list.
ALTER TABLE accounts ADD COLUMN letter TEXT;
