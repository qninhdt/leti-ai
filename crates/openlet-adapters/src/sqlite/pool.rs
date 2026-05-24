//! SQLite connection pool helpers.
//!
//! Pragmas applied at connect:
//!   - `journal_mode=WAL`        — concurrent reads alongside a writer.
//!   - `synchronous=NORMAL`      — durability sufficient with WAL.
//!   - `foreign_keys=ON`         — enforce ON DELETE CASCADE rows.
//!   - `busy_timeout=5000`       — wait up to 5s on lock contention.

use std::path::Path;
use std::str::FromStr;

use sqlx::ConnectOptions;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use openlet_core::error::MemoryError;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn open_pool(db_path: &Path, max_connections: u32) -> Result<SqlitePool, MemoryError> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MemoryError::Io(format!("create db dir: {e}")))?;
        }
    }

    let url = format!("sqlite://{}", db_path.display());
    let opts = SqliteConnectOptions::from_str(&url)
        .map_err(|e| MemoryError::Io(format!("parse db url: {e}")))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5))
        .log_statements(tracing::log::LevelFilter::Trace);

    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(opts)
        .await
        .map_err(|e| MemoryError::Io(format!("open sqlite pool: {e}")))
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<(), MemoryError> {
    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| MemoryError::Io(format!("run migrations: {e}")))
}

pub async fn open_in_memory() -> Result<SqlitePool, MemoryError> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .map_err(|e| MemoryError::Io(format!("parse memory url: {e}")))?
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .map_err(|e| MemoryError::Io(format!("open in-memory pool: {e}")))?;
    run_migrations(&pool).await?;
    Ok(pool)
}
