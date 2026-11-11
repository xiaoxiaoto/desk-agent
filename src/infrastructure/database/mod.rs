pub mod coordinate_cache;
pub mod instruction_queue;

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::time::Duration;

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

/// Renames the pre-consolidation private database (with its WAL/SHM side files) to the single
/// project database when the latter does not exist yet, preserving grants, audit and cache.
pub fn adopt_legacy(target: &std::path::Path, legacy: Option<&std::path::Path>) -> anyhow::Result<bool> {
    let Some(legacy) = legacy else { return Ok(false) };
    if target.exists() || !legacy.is_file() {
        return Ok(false);
    }
    for suffix in ["-wal", "-shm"] {
        let side = std::path::PathBuf::from(format!("{}{suffix}", legacy.display()));
        if side.exists() {
            std::fs::rename(&side, format!("{}{suffix}", target.display()))?;
        }
    }
    std::fs::rename(legacy, target)?;
    Ok(true)
}

/// Opens the single private project database and creates every table. Grant and audit tables
/// are required; coordinate-cache and instruction-queue failures only disable those features.
pub async fn open(path: &std::path::Path) -> anyhow::Result<SqlitePool> {
    prepare_private_database(path)?;
    let options = SqliteConnectOptions::new().filename(path).busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY NOT NULL, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .execute(&pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS runtime_metadata (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL)")
        .execute(&pool).await?;
    sqlx::query("INSERT OR IGNORE INTO schema_migrations(version) VALUES (1)")
        .execute(&pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS local_capability_grants (app TEXT NOT NULL, capability TEXT NOT NULL, route TEXT NOT NULL, target TEXT NOT NULL, effect TEXT NOT NULL, parameters TEXT NOT NULL, PRIMARY KEY(app, capability, route, target, effect, parameters))")
        .execute(&pool).await?;
    sqlx::query(crate::runtime::grants::CREATE_TABLE).execute(&pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS capability_audit (id INTEGER PRIMARY KEY, request_id TEXT NOT NULL, occurred_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, app TEXT NOT NULL, capability TEXT NOT NULL, route TEXT NOT NULL, target TEXT NOT NULL, effect TEXT NOT NULL, parameters TEXT NOT NULL, decision TEXT NOT NULL)")
        .execute(&pool).await?;
    if coordinate_cache::initialize(&pool).await.is_err() {
        tracing::warn!("Coordinate cache initialization failed; authorization/audit storage remains available");
    }
    if instruction_queue::initialize(&pool).await.is_err() {
        tracing::warn!("Instruction queue initialization failed; authorization/audit storage remains available");
    }
    Ok(pool)
}

pub async fn ping(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(pool)
        .await?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::DirBuilderExt;

    #[tokio::test]
    async fn single_database_adopts_legacy_private_file_and_holds_every_table() {
        let dir = std::env::temp_dir().join(format!("desk-agent-single-db-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let legacy = dir.join("security.sqlite3");
        let target = dir.join("desk_agent.sqlite3");
        let pool = super::open(&legacy).await.unwrap();
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom','c','r','t','e','p')").execute(&pool).await.unwrap();
        pool.close().await;

        assert!(super::adopt_legacy(&target, Some(&legacy)).unwrap());
        assert!(!legacy.exists());
        assert!(!super::adopt_legacy(&target, Some(&legacy)).unwrap(), "adoption happens once");
        let pool = super::open(&target).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM local_capability_grants").fetch_one(&pool).await.unwrap(), 1);
        let tables: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .fetch_all(&pool).await.unwrap();
        for table in ["capability_audit", "coordinate_cache", "instruction_queue", "local_capability_grants", "runtime_metadata", "schema_migrations", "standing_capability_grants"] {
            assert!(tables.iter().any(|t| t == table), "{table} missing from {tables:?}");
        }
        pool.close().await;

        // An existing single database is never overwritten by a stray legacy file.
        std::fs::write(&legacy, b"").unwrap();
        assert!(!super::adopt_legacy(&target, Some(&legacy)).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
