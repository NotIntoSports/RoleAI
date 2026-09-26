use std::{
    path::Path,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use thiserror::Error;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../migrations/0001_foundation.sql")),
    (2, include_str!("../../migrations/0002_materials.sql")),
    (3, include_str!("../../migrations/0003_sessions.sql")),
    (
        4,
        include_str!("../../migrations/0004_transport_livekit.sql"),
    ),
    (
        5,
        include_str!("../../migrations/0005_session_event_metadata.sql"),
    ),
    (
        6,
        include_str!("../../migrations/0006_voice_references.sql"),
    ),
    (
        7,
        include_str!("../../migrations/0007_session_context_summary.sql"),
    ),
];
const LATEST_SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Error)]
pub enum DatabaseError {
    #[error("Database schema is newer than this application")]
    NewerVersion,
    #[error("Database operation failed")]
    Operation,
}

impl DatabaseError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NewerVersion => "DATABASE_VERSION_NEWER",
            Self::Operation => "DATABASE_OPERATION_FAILED",
        }
    }
}

fn current_schema_version(connection: &Connection) -> Result<i64, DatabaseError> {
    let has_migrations: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| DatabaseError::Operation)?;
    if !has_migrations {
        return Ok(0);
    }
    Ok(connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .optional()
        .map_err(|_| DatabaseError::Operation)?
        .flatten()
        .unwrap_or(0))
}

pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        register_sqlite_vec()?;
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|_| DatabaseError::Operation)?;
        }
        let connection = Connection::open(path).map_err(|_| DatabaseError::Operation)?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|_| DatabaseError::Operation)?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|_| DatabaseError::Operation)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn migrate(&self) -> Result<(), DatabaseError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        let current = current_schema_version(&connection)?;
        if current > LATEST_SCHEMA_VERSION {
            return Err(DatabaseError::NewerVersion);
        }
        for &(version, sql) in MIGRATIONS {
            if version <= current {
                continue;
            }
            // SQLite ignores PRAGMA foreign_keys inside a transaction, so rebuild
            // migrations must turn FKs off before BEGIN and back on after COMMIT.
            connection
                .pragma_update(None, "foreign_keys", false)
                .map_err(|_| DatabaseError::Operation)?;
            let apply = (|| {
                let transaction = connection
                    .transaction()
                    .map_err(|_| DatabaseError::Operation)?;
                transaction
                    .execute_batch(sql)
                    .map_err(|_| DatabaseError::Operation)?;
                transaction
                    .execute(
                        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                        params![version, chrono::Utc::now().to_rfc3339()],
                    )
                    .map_err(|_| DatabaseError::Operation)?;
                transaction.commit().map_err(|_| DatabaseError::Operation)
            })();
            let restore = connection.pragma_update(None, "foreign_keys", true);
            apply?;
            restore.map_err(|_| DatabaseError::Operation)?;
        }
        Ok(())
    }

    pub fn integrity_check(&self) -> Result<String, DatabaseError> {
        self.pragma_string("integrity_check")
    }

    pub fn with_connection<T>(
        &self,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        work(&connection).map_err(|_| DatabaseError::Operation)
    }

    pub fn with_connection_mut<T>(
        &self,
        work: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> Result<T, DatabaseError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        work(&mut connection).map_err(|_| DatabaseError::Operation)
    }

    pub fn with_transaction<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, DatabaseError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        let transaction = connection
            .transaction()
            .map_err(|_| DatabaseError::Operation)?;
        let value = work(&transaction).map_err(|_| DatabaseError::Operation)?;
        transaction.commit().map_err(|_| DatabaseError::Operation)?;
        Ok(value)
    }

    #[cfg(test)]
    pub fn busy_timeout(&self) -> Duration {
        BUSY_TIMEOUT
    }

    #[cfg(test)]
    fn schema_version(&self) -> Result<i64, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        let version = current_schema_version(&connection)?;
        if version == 0 {
            Err(DatabaseError::Operation)
        } else {
            Ok(version)
        }
    }

    fn pragma_string(&self, name: &str) -> Result<String, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        connection
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .map_err(|_| DatabaseError::Operation)
    }

    #[cfg(test)]
    fn pragma_i64(&self, name: &str) -> Result<i64, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        connection
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .map_err(|_| DatabaseError::Operation)
    }

    #[cfg(test)]
    fn application_table_names(&self) -> Result<Vec<String>, DatabaseError> {
        self.query_strings(
            "SELECT name FROM sqlite_schema
             WHERE type='table'
               AND name NOT LIKE 'sqlite_%'
               AND name NOT LIKE '%_fts_%'
             ORDER BY name",
        )
    }

    #[cfg(test)]
    fn execute_batch(&self, sql: &str) -> Result<(), DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        connection
            .execute_batch(sql)
            .map_err(|_| DatabaseError::Operation)
    }

    #[cfg(test)]
    fn query_string(&self, sql: &str) -> Result<String, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        connection
            .query_row(sql, [], |row| row.get(0))
            .map_err(|_| DatabaseError::Operation)
    }

    #[cfg(test)]
    fn query_strings(&self, sql: &str) -> Result<Vec<String>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        let mut statement = connection
            .prepare(sql)
            .map_err(|_| DatabaseError::Operation)?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|_| DatabaseError::Operation)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseError::Operation)
    }

    #[cfg(test)]
    fn column_names(&self) -> Result<Vec<String>, DatabaseError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| DatabaseError::Operation)?;
        let mut statement = connection.prepare(
            "SELECT p.name FROM sqlite_schema AS s JOIN pragma_table_info(s.name) AS p WHERE s.type='table'",
        ).map_err(|_| DatabaseError::Operation)?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|_| DatabaseError::Operation)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseError::Operation)
    }
}

fn register_sqlite_vec() -> Result<(), DatabaseError> {
    static REGISTER: OnceLock<Result<(), DatabaseError>> = OnceLock::new();
    REGISTER.get_or_init(register_sqlite_vec_once).clone()
}

fn register_sqlite_vec_once() -> Result<(), DatabaseError> {
    use rusqlite::auto_extension::{RawAutoExtension, register_auto_extension};
    use sqlite_vec::sqlite3_vec_init;
    // sqlite-vec exports a 0-arg init; rusqlite 0.40 requires the 3-arg RawAutoExtension.
    #[allow(clippy::missing_transmute_annotations)]
    unsafe {
        let raw_ext: RawAutoExtension = std::mem::transmute(sqlite3_vec_init as *const () as usize);
        register_auto_extension(raw_ext).map_err(|_| DatabaseError::Operation)
    }
}

#[cfg(test)]
mod tests;
