pub mod coordinate_cache;

use anyhow::Context;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{str::FromStr, time::Duration};

#[cfg(unix)]
fn prepare_private_database(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    // A local grant is meaningful only when another OS user cannot alter it.
    unsafe extern "C" { fn geteuid() -> u32; }
    let uid = unsafe { geteuid() };
    anyhow::ensure!(path != std::path::Path::new(":memory:") && !path.as_os_str().is_empty(), "a persistent private database is required");
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    if !parent.exists() {
        std::fs::DirBuilder::new().mode(0o700).create(parent)?;
    }
    let dir = std::fs::symlink_metadata(parent)?;
    anyhow::ensure!(dir.is_dir() && dir.uid() == uid && dir.mode() & 0o077 == 0, "database directory must be owned by the current user and private (mode 0700)");
    match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let file = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(file.is_file() && file.uid() == uid && file.mode() & 0o077 == 0, "database file must be owned by the current user and private (mode 0600)");
    Ok(())
}

#[cfg(not(unix))]
fn prepare_private_database(_path: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("private local authorization storage is unsupported on this platform")
}

pub async fn open(database_url: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)
        .context("invalid DATABASE_URL")?
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY NOT NULL, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .execute(&pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS runtime_metadata (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)")
        .execute(&pool).await?;
    sqlx::query("INSERT OR IGNORE INTO schema_migrations(version) VALUES (1)")
        .execute(&pool)
        .await?;
    Ok(pool)
}

pub async fn open_security(path: &std::path::Path) -> anyhow::Result<SqlitePool> {
    prepare_private_database(path)?;
    let options = SqliteConnectOptions::new().filename(path).busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS local_capability_grants (app TEXT NOT NULL, capability TEXT NOT NULL, route TEXT NOT NULL, target TEXT NOT NULL, effect TEXT NOT NULL, parameters TEXT NOT NULL, PRIMARY KEY(app, capability, route, target, effect, parameters))")
        .execute(&pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS capability_audit (id INTEGER PRIMARY KEY, request_id TEXT NOT NULL, occurred_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, app TEXT NOT NULL, capability TEXT NOT NULL, route TEXT NOT NULL, target TEXT NOT NULL, effect TEXT NOT NULL, parameters TEXT NOT NULL, decision TEXT NOT NULL)")
        .execute(&pool).await?;
    if coordinate_cache::initialize(&pool).await.is_err() {
        tracing::warn!("Coordinate cache initialization failed; authorization/audit storage remains available");
    }
    Ok(pool)
}

pub async fn ping(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(pool)
        .await?;
    Ok(())
}
