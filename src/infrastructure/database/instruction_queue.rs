//! Durable FIFO instruction queue records in the single private project database.
//!
//! The payload (the original `TaskInput`, which may contain message text) is kept only while a
//! record is `pending`/`executing` and is purged when the record reaches a terminal state.
//! `merge_key` holds a digest, never a nickname. Queue state never authorizes execution.

use sqlx::{Row, SqlitePool};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStatus {
    /// 待执行
    Pending,
    /// 执行中: claimed and handed to the Runtime; never re-dispatched after a crash.
    Executing,
    /// 执行成功
    Succeeded,
    /// 执行失败: rejected or failed with no dispatched effect for this record.
    Failed,
    /// 取消执行: never dispatched because an earlier part of its merged batch stopped.
    Cancelled,
    /// 结果未知: an effect may have occurred; must be reconciled, never blindly retried.
    Unknown,
}

impl QueueStatus {
    pub const ALL: [Self; 6] = [Self::Pending, Self::Executing, Self::Succeeded, Self::Failed, Self::Cancelled, Self::Unknown];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Executing => "executing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == value)
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Executing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRecord<'a> {
    pub request_id: &'a str,
    pub app: &'a str,
    pub action: &'a str,
    pub merge_key: Option<&'a str>,
    pub request_digest: &'a str,
    pub payload: &'a str,
    pub enqueued_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRecord {
    pub id: i64,
    pub request_id: String,
    pub app: String,
    pub action: String,
    pub merge_key: Option<String>,
    pub request_digest: String,
    pub payload: Option<String>,
    pub enqueued_at_ms: i64,
}

#[derive(Debug)]
pub enum EnqueueError {
    Full,
    Storage(sqlx::Error),
}

impl From<sqlx::Error> for EnqueueError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error)
    }
}

pub async fn initialize(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS instruction_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            request_id TEXT NOT NULL CHECK(length(request_id) > 0),
            app TEXT NOT NULL CHECK(length(app) > 0),
            action TEXT NOT NULL CHECK(length(action) > 0),
            merge_key TEXT,
            request_digest TEXT NOT NULL,
            payload TEXT,
            enqueued_at_ms INTEGER NOT NULL CHECK(typeof(enqueued_at_ms) = 'integer'),
            status TEXT NOT NULL CHECK(status IN ('pending', 'executing', 'succeeded', 'failed', 'cancelled', 'unknown')),
            batch_id TEXT,
            batch_position INTEGER,
            message_offset INTEGER,
            updated_at_ms INTEGER NOT NULL,
            CHECK(status IN ('pending', 'executing') OR payload IS NULL)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS instruction_queue_status ON instruction_queue(status, id)")
        .execute(pool).await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS instruction_queue_merge ON instruction_queue(status, app, action, merge_key, id)")
        .execute(pool).await?;
    Ok(())
}

/// Inserts a pending record unless the configured pending bound is reached.
pub async fn enqueue(pool: &SqlitePool, record: &NewRecord<'_>, max_pending: u32) -> Result<i64, EnqueueError> {
    let mut tx = pool.begin().await?;
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM instruction_queue WHERE status = 'pending'")
        .fetch_one(&mut *tx).await?;
    if pending >= i64::from(max_pending) {
        return Err(EnqueueError::Full);
    }
    let id = sqlx::query(
        "INSERT INTO instruction_queue (request_id, app, action, merge_key, request_digest, payload, enqueued_at_ms, status, updated_at_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?)",
    )
    .bind(record.request_id).bind(record.app).bind(record.action).bind(record.merge_key)
    .bind(record.request_digest).bind(record.payload).bind(record.enqueued_at_ms).bind(record.enqueued_at_ms)
    .execute(&mut *tx).await?
    .last_insert_rowid();
    tx.commit().await?;
    Ok(id)
}

fn pending_record(row: sqlx::sqlite::SqliteRow) -> Result<PendingRecord, sqlx::Error> {
    Ok(PendingRecord {
        id: row.try_get("id")?,
        request_id: row.try_get("request_id")?,
        app: row.try_get("app")?,
        action: row.try_get("action")?,
        merge_key: row.try_get("merge_key")?,
        request_digest: row.try_get("request_digest")?,
        payload: row.try_get("payload")?,
        enqueued_at_ms: row.try_get("enqueued_at_ms")?,
    })
}

const COLUMNS: &str = "id, request_id, app, action, merge_key, request_digest, payload, enqueued_at_ms";

/// Oldest pending record (FIFO by insertion id).
pub async fn head(pool: &SqlitePool) -> Result<Option<PendingRecord>, sqlx::Error> {
    sqlx::query(&format!("SELECT {COLUMNS} FROM instruction_queue WHERE status = 'pending' ORDER BY id LIMIT 1"))
        .fetch_optional(pool).await?
        .map(pending_record).transpose()
}

/// Pending records after `head` with the same app, action and merge key whose enqueue time is
/// inside `[head.enqueued_at_ms, head.enqueued_at_ms + window_ms]`, in FIFO order.
pub async fn merge_candidates(pool: &SqlitePool, head: &PendingRecord, window_ms: i64, limit: i64) -> Result<Vec<PendingRecord>, sqlx::Error> {
    let Some(merge_key) = head.merge_key.as_deref() else { return Ok(Vec::new()) };
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM instruction_queue
         WHERE status = 'pending' AND id > ? AND app = ? AND action = ? AND merge_key = ? AND enqueued_at_ms <= ?
         ORDER BY id LIMIT ?"
    ))
    .bind(head.id).bind(&head.app).bind(&head.action).bind(merge_key)
    .bind(head.enqueued_at_ms.saturating_add(window_ms)).bind(limit)
    .fetch_all(pool).await?
    .into_iter().map(pending_record).collect()
}

/// Durably claims every record of a batch (`pending` → `executing`) before dispatch.
/// Fails without claiming anything if any record is no longer pending.
pub async fn claim(pool: &SqlitePool, batch_id: &str, members: &[(i64, i64)], now_ms: i64) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    for (position, (id, message_offset)) in members.iter().enumerate() {
        let updated = sqlx::query(
            "UPDATE instruction_queue SET status = 'executing', batch_id = ?, batch_position = ?, message_offset = ?, updated_at_ms = ?
             WHERE id = ? AND status = 'pending'",
        )
        .bind(batch_id).bind(position as i64).bind(message_offset).bind(now_ms).bind(id)
        .execute(&mut *tx).await?
        .rows_affected();
        if updated != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
    }
    tx.commit().await?;
    Ok(true)
}

/// Records a terminal state and purges the payload.
pub async fn finish(pool: &SqlitePool, id: i64, status: QueueStatus, now_ms: i64) -> Result<(), sqlx::Error> {
    if !status.is_terminal() {
        return Err(sqlx::Error::Protocol("queue finish requires a terminal status".into()));
    }
    sqlx::query("UPDATE instruction_queue SET status = ?, payload = NULL, updated_at_ms = ? WHERE id = ? AND status IN ('pending', 'executing')")
        .bind(status.as_str()).bind(now_ms).bind(id)
        .execute(pool).await?;
    Ok(())
}

/// After a restart no waiter or Runtime task owns an `executing` record; its effect may have
/// happened, so it becomes `unknown` and is never dispatched again. Returns the affected count.
pub async fn recover(pool: &SqlitePool, now_ms: i64) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("UPDATE instruction_queue SET status = 'unknown', payload = NULL, updated_at_ms = ? WHERE status = 'executing'")
        .bind(now_ms).execute(pool).await?.rows_affected())
}

pub async fn purge_terminal(pool: &SqlitePool, before_ms: i64) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM instruction_queue WHERE status NOT IN ('pending', 'executing') AND updated_at_ms < ?")
        .bind(before_ms).execute(pool).await?.rows_affected())
}

pub async fn status(pool: &SqlitePool, id: i64) -> Result<Option<QueueStatus>, sqlx::Error> {
    let value: Option<String> = sqlx::query_scalar("SELECT status FROM instruction_queue WHERE id = ?")
        .bind(id).fetch_optional(pool).await?;
    Ok(value.as_deref().and_then(QueueStatus::parse))
}
