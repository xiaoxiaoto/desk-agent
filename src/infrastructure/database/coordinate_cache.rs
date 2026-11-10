//! Durable localization candidates only; callers must authorize and live-validate reuse.

use serde::Serialize;
use sqlx::{Row, Sqlite, SqlitePool, query::Query, sqlite::SqliteArguments};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    InstructionContent,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::InstructionContent => "instruction_content",
        }
    }

    fn limit(self) -> i64 {
        match self {
            Self::Global => 192,
            Self::InstructionContent => 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Key {
    pub app: String,
    pub route: String,
    pub scope: Scope,
    pub instruction: String,
    pub content_digest: String,
    pub control: String,
    pub layout_version: i64,
    pub geometry_key: String,
}

impl Key {
    fn validate(&self) -> Result<(), sqlx::Error> {
        let valid_scope = match self.scope {
            Scope::Global => {
                self.instruction.is_empty()
                    && self.content_digest.is_empty()
                    && matches!(self.control.as_str(), "message_navigation" | "search" | "composer")
            }
            Scope::InstructionContent => {
                self.instruction == "send_message_by_nickname"
                    && self.control == "contact_result"
                    && self.content_digest.strip_prefix("nickname-v1-sha256:").is_some_and(|digest| {
                        digest.len() == 64
                            && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    })
            }
        };
        if self.app.is_empty() || self.route.is_empty() || self.geometry_key.is_empty()
            || self.layout_version < 1 || !valid_scope
        {
            return Err(sqlx::Error::Protocol("invalid coordinate cache key".into()));
        }
        Ok(())
    }
}

fn bind_key<'q>(query: Query<'q, Sqlite, SqliteArguments<'q>>, key: &'q Key)
    -> Query<'q, Sqlite, SqliteArguments<'q>>
{
    query.bind(&key.app).bind(&key.route).bind(key.scope.as_str())
        .bind(&key.instruction).bind(&key.content_digest).bind(&key.control)
        .bind(key.layout_version).bind(&key.geometry_key)
}

fn valid_point(point: [f64; 2]) -> bool {
    point.into_iter().all(|value| value.is_finite() && value >= 0.0)
}

pub async fn initialize(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // This independent table does not change grant/audit schema versions or data.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS coordinate_cache (
            id INTEGER PRIMARY KEY,
            app TEXT NOT NULL CHECK(length(app) > 0),
            route TEXT NOT NULL CHECK(length(route) > 0),
            scope TEXT NOT NULL,
            instruction TEXT NOT NULL,
            content_digest TEXT NOT NULL,
            control TEXT NOT NULL,
            layout_version INTEGER NOT NULL CHECK(typeof(layout_version) = 'integer' AND layout_version >= 1),
            geometry_key TEXT NOT NULL CHECK(length(geometry_key) > 0),
            point_x REAL NOT NULL CHECK(typeof(point_x) IN ('integer', 'real') AND point_x >= 0 AND point_x <= 1.7976931348623157e308),
            point_y REAL NOT NULL CHECK(typeof(point_y) IN ('integer', 'real') AND point_y >= 0 AND point_y <= 1.7976931348623157e308),
            created_at INTEGER NOT NULL DEFAULT (unixepoch()) CHECK(typeof(created_at) = 'integer' AND created_at >= 0),
            last_validated_at INTEGER NOT NULL DEFAULT (unixepoch()) CHECK(typeof(last_validated_at) = 'integer' AND last_validated_at >= 0),
            CHECK(
                (scope = 'global' AND instruction = '' AND content_digest = '' AND control IN ('message_navigation', 'search', 'composer'))
                OR
                (scope = 'instruction_content' AND instruction = 'send_message_by_nickname' AND control = 'contact_result'
                    AND length(content_digest) = 83 AND substr(content_digest, 1, 19) = 'nickname-v1-sha256:'
                    AND substr(content_digest, 20) NOT GLOB '*[^0-9a-f]*')
            ),
            UNIQUE(app, route, scope, instruction, content_digest, control, layout_version, geometry_key)
        )",
    ).execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS coordinate_cache_eviction ON coordinate_cache(scope, last_validated_at, id)")
        .execute(pool).await?;
    Ok(())
}

/// A match is a candidate, never evidence of target identity or authorization.
pub async fn read(pool: &SqlitePool, key: &Key) -> Result<Option<[f64; 2]>, sqlx::Error> {
    key.validate()?;
    let row = bind_key(sqlx::query(
        "SELECT point_x, point_y, created_at, last_validated_at FROM coordinate_cache
         WHERE app=? AND route=? AND scope=? AND instruction=? AND content_digest=? AND control=? AND layout_version=? AND geometry_key=?",
    ), key).fetch_optional(pool).await?;
    let Some(row) = row else { return Ok(None); };
    // SQLite is dynamically typed: malformed or externally modified rows are misses.
    let point = match (row.try_get::<f64, _>("point_x"), row.try_get::<f64, _>("point_y")) {
        (Ok(x), Ok(y)) if valid_point([x, y]) => [x, y],
        _ => return Ok(None),
    };
    for column in ["created_at", "last_validated_at"] {
        if !matches!(row.try_get::<i64, _>(column), Ok(timestamp) if timestamp >= 0) {
            return Ok(None);
        }
    }
    Ok(Some(point))
}

/// Replace one successfully validated relative point and evict within its scope atomically.
pub async fn write(pool: &SqlitePool, key: &Key, point: [f64; 2]) -> Result<(), sqlx::Error> {
    key.validate()?;
    if !valid_point(point) {
        return Err(sqlx::Error::Protocol("invalid coordinate cache relative point".into()));
    }
    let mut transaction = pool.begin().await?;
    bind_key(sqlx::query(
        "INSERT INTO coordinate_cache(app, route, scope, instruction, content_digest, control, layout_version, geometry_key, point_x, point_y)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(app, route, scope, instruction, content_digest, control, layout_version, geometry_key)
         DO UPDATE SET point_x=excluded.point_x, point_y=excluded.point_y, last_validated_at=unixepoch()",
    ), key).bind(point[0]).bind(point[1]).execute(&mut *transaction).await?;
    sqlx::query(
        "DELETE FROM coordinate_cache WHERE id IN (
            SELECT id FROM coordinate_cache WHERE scope=? ORDER BY last_validated_at DESC, id DESC LIMIT -1 OFFSET ?
         )",
    ).bind(key.scope.as_str()).bind(key.scope.limit()).execute(&mut *transaction).await?;
    transaction.commit().await
}

/// Persist successful validation at most once per minute; reads do not refresh recency.
pub async fn touch(pool: &SqlitePool, key: &Key) -> Result<(), sqlx::Error> {
    key.validate()?;
    bind_key(sqlx::query(
        "UPDATE coordinate_cache SET last_validated_at=unixepoch()
         WHERE app=? AND route=? AND scope=? AND instruction=? AND content_digest=? AND control=? AND layout_version=? AND geometry_key=?
           AND last_validated_at <= unixepoch() - 60",
    ), key).execute(pool).await?;
    Ok(())
}

pub async fn invalidate(pool: &SqlitePool, key: &Key) -> Result<(), sqlx::Error> {
    key.validate()?;
    bind_key(sqlx::query(
        "DELETE FROM coordinate_cache
         WHERE app=? AND route=? AND scope=? AND instruction=? AND content_digest=? AND control=? AND layout_version=? AND geometry_key=?",
    ), key).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn database() -> SqlitePool {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        initialize(&pool).await.unwrap();
        pool
    }

    fn global() -> Key {
        Key {
            app: "wecom".into(), route: "macos_gui".into(), scope: Scope::Global,
            instruction: String::new(), content_digest: String::new(), control: "search".into(),
            layout_version: 1, geometry_key: "geometry-v1:main".into(),
        }
    }

    fn content(index: usize) -> Key {
        Key {
            scope: Scope::InstructionContent, instruction: "send_message_by_nickname".into(),
            content_digest: format!("nickname-v1-sha256:{index:064x}"), control: "contact_result".into(),
            ..global()
        }
    }

    async fn count(pool: &SqlitePool, scope: Scope) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM coordinate_cache WHERE scope=?")
            .bind(scope.as_str()).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn unique_replacement_and_exact_invalidation() {
        let pool = database().await;
        let key = global();
        write(&pool, &key, [1.0, 2.0]).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM coordinate_cache").fetch_one(&pool).await.unwrap();
        write(&pool, &key, [3.0, 4.0]).await.unwrap();
        assert_eq!(count(&pool, Scope::Global).await, 1);
        assert_eq!(read(&pool, &key).await.unwrap(), Some([3.0, 4.0]));
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT id FROM coordinate_cache").fetch_one(&pool).await.unwrap(), id);
        write(&pool, &content(1), [5.0, 6.0]).await.unwrap();
        invalidate(&pool, &key).await.unwrap();
        invalidate(&pool, &key).await.unwrap();
        assert_eq!(read(&pool, &key).await.unwrap(), None);
        assert_eq!(read(&pool, &content(1)).await.unwrap(), Some([5.0, 6.0]));
    }

    #[tokio::test]
    async fn scope_content_and_every_geometry_key_field_are_isolated() {
        let pool = database().await;
        let first = global();
        let mut keys = vec![first.clone(), content(1), content(2)];
        let mut changed = first.clone(); changed.app = "another_app".into(); keys.push(changed);
        let mut changed = first.clone(); changed.route = "another_route".into(); keys.push(changed);
        let mut changed = first.clone(); changed.control = "composer".into(); keys.push(changed);
        let mut changed = first.clone(); changed.layout_version = 2; keys.push(changed);
        let mut changed = first.clone(); changed.geometry_key = "geometry-v1:other".into(); keys.push(changed);
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(read(&pool, key).await.unwrap(), None);
            write(&pool, key, [index as f64, 10.0]).await.unwrap();
        }
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(read(&pool, key).await.unwrap(), Some([index as f64, 10.0]));
        }
        assert_eq!(count(&pool, Scope::Global).await, 6);
        assert_eq!(count(&pool, Scope::InstructionContent).await, 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn close_reopen_preserves_cache_and_existing_security_data() {
        use std::os::unix::fs::DirBuilderExt;
        let directory = std::env::temp_dir().join(format!("desk-agent-coordinate-cache-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let path = directory.join("private.sqlite3");
        let pool = crate::infrastructure::database::open_security(&path).await.unwrap();
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom','capability','route','target','effect','digest')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO capability_audit(request_id, app, capability, route, target, effect, parameters, decision) VALUES ('request','wecom','capability','route','target','effect','digest','intent')")
            .execute(&pool).await.unwrap();
        write(&pool, &global(), [3.0, 4.0]).await.unwrap();
        write(&pool, &content(1), [5.0, 6.0]).await.unwrap();
        pool.close().await;
        let pool = crate::infrastructure::database::open_security(&path).await.unwrap();
        initialize(&pool).await.unwrap();
        assert_eq!(read(&pool, &global()).await.unwrap(), Some([3.0, 4.0]));
        assert_eq!(read(&pool, &content(1)).await.unwrap(), Some([5.0, 6.0]));
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM local_capability_grants").fetch_one(&pool).await.unwrap(), 1);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM capability_audit").fetch_one(&pool).await.unwrap(), 1);
        pool.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn incompatible_cache_does_not_disable_authorization_and_audit_on_reopen() {
        use std::os::unix::fs::DirBuilderExt;
        let directory = std::env::temp_dir().join(format!("desk-agent-cache-fault-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let path = directory.join("private.sqlite3");
        let pool = crate::infrastructure::database::open_security(&path).await.unwrap();
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom','capability','route','target','effect','digest')")
            .execute(&pool).await.unwrap();
        sqlx::query("DROP TABLE coordinate_cache").execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE coordinate_cache (id INTEGER PRIMARY KEY)").execute(&pool).await.unwrap();
        pool.close().await;

        let reopened = crate::infrastructure::database::open_security(&path).await;
        if let Ok(pool) = &reopened {
            assert!(read(pool, &global()).await.is_err());
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM local_capability_grants").fetch_one(pool).await.unwrap(), 1);
            sqlx::query("INSERT INTO capability_audit(request_id, app, capability, route, target, effect, parameters, decision) VALUES ('request','wecom','capability','route','target','effect','digest','intent')")
                .execute(pool).await.unwrap();
            assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM capability_audit").fetch_one(pool).await.unwrap(), 1);
            pool.close().await;
        }
        std::fs::remove_dir_all(directory).unwrap();
        assert!(reopened.is_ok(), "cache-only schema failure must preserve usable authorization/audit storage: {reopened:?}");
    }

    #[tokio::test]
    async fn bounded_eviction_is_scope_wide_and_uses_validation_then_id() {
        let pool = database().await;
        for scope in [Scope::Global, Scope::InstructionContent] {
            let limit = scope.limit() as usize;
            let key_at = |index: usize| match scope {
                Scope::Global => Key { geometry_key: format!("geometry-v1:{index}"), ..global() },
                Scope::InstructionContent => content(index),
            };
            for index in 0..limit {
                write(&pool, &key_at(index), [index as f64, 1.0]).await.unwrap();
            }
            // The least recently validated row must go first, not the earliest inserted.
            sqlx::query("UPDATE coordinate_cache SET last_validated_at=10 WHERE scope=?")
                .bind(scope.as_str()).execute(&pool).await.unwrap();
            bind_key(sqlx::query("UPDATE coordinate_cache SET last_validated_at=1 WHERE app=? AND route=? AND scope=? AND instruction=? AND content_digest=? AND control=? AND layout_version=? AND geometry_key=?"), &key_at(1))
                .execute(&pool).await.unwrap();
            write(&pool, &key_at(limit), [1.0, 1.0]).await.unwrap();
            assert_eq!(read(&pool, &key_at(1)).await.unwrap(), None);
            assert!(read(&pool, &key_at(0)).await.unwrap().is_some());
            // Equal timestamps use ascending id as deterministic eviction order.
            write(&pool, &key_at(limit + 1), [2.0, 2.0]).await.unwrap();
            assert_eq!(read(&pool, &key_at(0)).await.unwrap(), None);
            assert_eq!(count(&pool, scope).await, limit as i64);
        }
        assert_eq!(count(&pool, Scope::Global).await, 192);
        assert_eq!(count(&pool, Scope::InstructionContent).await, 1024);
    }

    #[tokio::test]
    async fn touch_throttles_updates_and_reads_never_refresh_timestamps() {
        let pool = database().await;
        let key = global();
        touch(&pool, &key).await.unwrap();
        assert_eq!(count(&pool, Scope::Global).await, 0);
        write(&pool, &key, [0.0, 0.0]).await.unwrap();
        sqlx::query("CREATE TABLE cache_updates (id INTEGER PRIMARY KEY)").execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER count_cache_updates AFTER UPDATE ON coordinate_cache BEGIN INSERT INTO cache_updates(id) VALUES (NULL); END")
            .execute(&pool).await.unwrap();
        let initial: i64 = sqlx::query_scalar("SELECT last_validated_at FROM coordinate_cache").fetch_one(&pool).await.unwrap();
        touch(&pool, &key).await.unwrap();
        read(&pool, &key).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM cache_updates").fetch_one(&pool).await.unwrap(), 0);
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT last_validated_at FROM coordinate_cache").fetch_one(&pool).await.unwrap(), initial);
        sqlx::query("UPDATE coordinate_cache SET last_validated_at=unixepoch()-61").execute(&pool).await.unwrap();
        touch(&pool, &key).await.unwrap();
        touch(&pool, &key).await.unwrap();
        assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM cache_updates").fetch_one(&pool).await.unwrap(), 2);
        let refreshed: i64 = sqlx::query_scalar("SELECT last_validated_at FROM coordinate_cache").fetch_one(&pool).await.unwrap();
        assert!(refreshed >= initial);
    }

    #[tokio::test]
    async fn corrupt_rows_are_misses_and_bad_points_and_keys_are_rejected() {
        let pool = database().await;
        let key = global();
        for point in [[-1.0, 0.0], [0.0, -1.0], [f64::NAN, 1.0], [f64::INFINITY, 1.0], [1.0, f64::NEG_INFINITY]] {
            assert!(write(&pool, &key, point).await.is_err());
        }
        assert_eq!(count(&pool, Scope::Global).await, 0);
        let mut invalid_keys = Vec::new();
        let mut invalid = global(); invalid.instruction = "send_message_by_nickname".into(); invalid_keys.push(invalid);
        let mut invalid = global(); invalid.content_digest = content(1).content_digest; invalid_keys.push(invalid);
        let mut invalid = global(); invalid.control = "contact_result".into(); invalid_keys.push(invalid);
        let mut invalid = content(1); invalid.instruction = "other_instruction".into(); invalid_keys.push(invalid);
        let mut invalid = content(1); invalid.control = "search".into(); invalid_keys.push(invalid);
        for digest in ["plaintext nickname".to_owned(), format!("nickname-v1-sha256:{}", "A".repeat(64)), format!("nickname-v2-sha256:{}", "a".repeat(64))] {
            let mut invalid = content(1); invalid.content_digest = digest; invalid_keys.push(invalid);
        }
        for invalid in invalid_keys {
            assert!(write(&pool, &invalid, [1.0, 1.0]).await.is_err());
            assert!(read(&pool, &invalid).await.is_err());
            assert!(touch(&pool, &invalid).await.is_err());
            assert!(invalidate(&pool, &invalid).await.is_err());
        }
        write(&pool, &key, [1.0, 2.0]).await.unwrap();
        sqlx::query("PRAGMA ignore_check_constraints=ON").execute(&pool).await.unwrap();
        for corrupt in ["UPDATE coordinate_cache SET point_x='broken'", "UPDATE coordinate_cache SET point_x=-1", "UPDATE coordinate_cache SET point_x=1e999", "UPDATE coordinate_cache SET last_validated_at='broken'", "UPDATE coordinate_cache SET created_at=-1"] {
            write(&pool, &key, [1.0, 2.0]).await.unwrap();
            sqlx::query("UPDATE coordinate_cache SET created_at=unixepoch()").execute(&pool).await.unwrap();
            sqlx::query(corrupt).execute(&pool).await.unwrap();
            assert_eq!(read(&pool, &key).await.unwrap(), None);
        }
        sqlx::query("PRAGMA ignore_check_constraints=OFF").execute(&pool).await.unwrap();
    }
}
