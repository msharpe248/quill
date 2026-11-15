//! Numbered, forward-only migrations, run at startup inside one transaction.
//!
//! The schema version is SQLite's `user_version`. To change the schema, add a
//! file and a line here. Never edit a migration that has shipped.

use rusqlite::Connection;

use crate::StoreError;

/// `(version, sql)`, contiguous from 1.
pub(crate) const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../migrations/0001_initial.sql")),
    (2, include_str!("../migrations/0002_blob_hash.sql")),
    (3, include_str!("../migrations/0003_operations.sql")),
    (4, include_str!("../migrations/0004_remote_senders.sql")),
    (5, include_str!("../migrations/0005_search.sql")),
    (6, include_str!("../migrations/0006_operation_batch.sql")),
    (7, include_str!("../migrations/0007_server_copies.sql")),
];

#[cfg(test)]
pub(crate) fn latest_version() -> u32 {
    MIGRATIONS.last().map_or(0, |(version, _)| *version)
}

pub(crate) fn current_version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Brings the database up to [`latest_version`]. A database written by a newer
/// Quill is refused, never downgraded.
pub(crate) fn run(conn: &mut Connection) -> Result<(), StoreError> {
    apply(conn, MIGRATIONS)
}

pub(crate) fn apply(conn: &mut Connection, migrations: &[(u32, &str)]) -> Result<(), StoreError> {
    let current = current_version(conn)?;
    let latest = migrations.last().map_or(0, |(version, _)| *version);
    if current > latest {
        return Err(StoreError::NewerSchema {
            found: current,
            supported: latest,
        });
    }
    if current == latest {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for (version, sql) in migrations {
        if *version > current {
            tx.execute_batch(sql)
                .map_err(|source| StoreError::Migration {
                    version: *version,
                    source,
                })?;
        }
    }
    tx.pragma_update(None, "user_version", latest)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_numbered_contiguously_from_one() {
        for (index, (version, sql)) in MIGRATIONS.iter().enumerate() {
            assert_eq!(*version as usize, index + 1);
            assert!(!sql.trim().is_empty());
        }
    }

    #[test]
    fn a_failing_migration_rolls_back_every_migration_in_the_run() {
        let mut conn = Connection::open_in_memory().unwrap();
        let migrations = [
            (1, "CREATE TABLE a (x INTEGER) STRICT;"),
            (2, "CREATE TABLE b (x INTEGER) STRICT; THIS IS NOT SQL;"),
        ];
        let err = apply(&mut conn, &migrations).unwrap_err();
        assert!(
            matches!(err, StoreError::Migration { version: 2, .. }),
            "{err:?}"
        );
        assert_eq!(current_version(&conn).unwrap(), 0);
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
    }

    #[test]
    fn only_pending_migrations_run() {
        let mut conn = Connection::open_in_memory().unwrap();
        let first = [(1, "CREATE TABLE a (x INTEGER) STRICT;")];
        apply(&mut conn, &first).unwrap();
        // Re-running version 1 would fail with "table a already exists".
        let both = [first[0], (2, "CREATE TABLE b (x INTEGER) STRICT;")];
        apply(&mut conn, &both).unwrap();
        assert_eq!(current_version(&conn).unwrap(), 2);
        apply(&mut conn, &both).unwrap();
    }
}
