use std::{path::Path, time::Duration};

use anyhow::Result;
use sqlx::{
    ConnectOptions, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};

/// Open the standalone service database and apply checked-in migrations.
pub async fn open_database(path: &Path) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5))
        // SQL statements and parameters must not enter the log.
        .disable_statement_logging();
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}
