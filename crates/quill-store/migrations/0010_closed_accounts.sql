-- The accounts whose folders are folded away in the accounts pane. A row
-- is a closed account; an account with no row is open, so one that has
-- just been added starts open. How the window was left, not a setting: it
-- is kept here and not in the config.
CREATE TABLE closed_accounts (
    account_id INTEGER PRIMARY KEY REFERENCES accounts (id) ON DELETE CASCADE
) STRICT;
