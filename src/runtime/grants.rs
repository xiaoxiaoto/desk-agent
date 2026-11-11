//! Install-time standing capability grants.
//!
//! `desk-agent setup` records one standing grant per public capability scope
//! (app, capability, route, target, effect). A standing grant authorizes any
//! call parameters for that scope, is never consumed and stays valid until it is
//! revoked with `desk-agent setup --revoke`. Exact one-shot grants in
//! `local_capability_grants` keep working and are still consumed first.
//! Every check, dispatch and outcome is audited exactly as before.
use sqlx::{SqliteConnection, SqlitePool};

pub(crate) const STANDING_SOURCE: &str = "install:v1";

/// Public capability scopes authorized at install time: (capability, route, target, effect).
pub const STANDING_SCOPES: &[(&str, &str, &str, &str)] = &[
    ("wecom.window.center", "macos_gui", "main_or_single_wecom_window", "move_without_resize"),
    ("desktop.window.capture", "macos_gui", "main_or_single_app_window", "capture"),
    ("vision.window.locate", "local_vision", "main_or_single_app_window", "local_inference"),
    ("desktop.window.coordinates.convert", "macos_gui", "main_or_single_app_window", "read_geometry"),
    ("wecom.element.activate", "macos_gui", "main_or_single_app_window", "activate_element"),
    ("wecom.element.diagnose", "macos_gui", "main_or_single_app_window", "inspect_element"),
    ("desktop.element.focus", "macos_gui", "main_or_single_app_window", "focus_element"),
    ("desktop.keyboard.key_input", "macos_gui", "main_or_single_app_window", "key_input"),
    ("desktop.element.input", "macos_gui", "main_or_single_app_window", "replace_text"),
    ("wecom.message.send_by_nickname", "macos_gui", "exact_nickname_conversation", "send_messages"),
];

pub const CREATE_TABLE: &str = "CREATE TABLE IF NOT EXISTS standing_capability_grants (app TEXT NOT NULL, capability TEXT NOT NULL, route TEXT NOT NULL, target TEXT NOT NULL, effect TEXT NOT NULL, source TEXT NOT NULL, granted_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY(app, capability, route, target, effect))";

/// Exact grant (?1..?5) or a standing grant for the same scope (?1..?4).
pub(crate) const EXACT_OR_STANDING: &str = "SELECT EXISTS(SELECT 1 FROM local_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4 AND parameters = ?5) OR EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4)";

const STANDING: &str = "SELECT EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ? AND route = ? AND target = ? AND effect = ?)";

pub(crate) async fn standing(conn: &mut SqliteConnection, capability: &str, route: &str, target: &str, effect: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(STANDING).bind(capability).bind(route).bind(target).bind(effect)
        .fetch_one(conn).await.map(|v| v == 1)
}

/// Consumes an exact one-shot grant when present; otherwise accepts an unconsumed standing grant.
pub(crate) async fn consume(conn: &mut SqliteConnection, capability: &str, route: &str, target: &str, effect: &str, parameters: &str) -> Result<bool, sqlx::Error> {
    let consumed = sqlx::query("DELETE FROM local_capability_grants WHERE app = 'wecom' AND capability = ? AND route = ? AND target = ? AND effect = ? AND parameters = ?")
        .bind(capability).bind(route).bind(target).bind(effect).bind(parameters)
        .execute(&mut *conn).await?.rows_affected() == 1;
    if consumed { return Ok(true); }
    standing(conn, capability, route, target, effect).await
}

/// Records every standing scope; idempotent.
pub async fn install(db: &SqlitePool) -> Result<usize, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query(CREATE_TABLE).execute(&mut *tx).await?;
    for (capability, route, target, effect) in STANDING_SCOPES {
        sqlx::query("INSERT OR IGNORE INTO standing_capability_grants (app, capability, route, target, effect, source) VALUES ('wecom', ?, ?, ?, ?, ?)")
            .bind(capability).bind(route).bind(target).bind(effect).bind(STANDING_SOURCE)
            .execute(&mut *tx).await?;
        sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES ('setup', 'wecom', ?, ?, ?, ?, ?, 'standing_grant_installed')")
            .bind(capability).bind(route).bind(target).bind(effect).bind(STANDING_SOURCE)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(STANDING_SCOPES.len())
}

/// Removes all standing grants; exact one-shot grants are left untouched.
pub async fn revoke(db: &SqlitePool) -> Result<u64, sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query(CREATE_TABLE).execute(&mut *tx).await?;
    let removed = sqlx::query("DELETE FROM standing_capability_grants WHERE app = 'wecom'").execute(&mut *tx).await?.rows_affected();
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES ('setup', 'wecom', '*', '*', '*', '*', ?, 'standing_grants_revoked')")
        .bind(STANDING_SOURCE).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(removed)
}

pub async fn installed(db: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT capability FROM standing_capability_grants WHERE app = 'wecom' ORDER BY capability")
        .fetch_all(db).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn private_db() -> (SqlitePool, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("desk-agent-grants-{}", uuid::Uuid::new_v4()));
        let path = dir.join("desk_agent.sqlite3");
        (crate::infrastructure::database::open(&path).await.unwrap(), dir)
    }

    #[tokio::test]
    async fn standing_grant_authorizes_any_parameters_without_consumption_until_revoked() {
        let (db, dir) = private_db().await;
        let digest = crate::runtime::app_element_activation_parameters([1.0, 2.0], "AXButton", None).unwrap();
        let (capability, route, target, effect) = ("wecom.element.activate", "macos_gui", "main_or_single_app_window", "activate_element");
        let check = |p: String| { let db = db.clone(); async move {
            sqlx::query_scalar::<_, i64>(EXACT_OR_STANDING).bind(capability).bind(route).bind(target).bind(effect).bind(p).fetch_one(&db).await.unwrap() == 1
        }};
        assert!(!check(digest.clone()).await);
        assert!(!crate::runtime::app_element_activation_any_grant(&db).await.unwrap());
        assert_eq!(install(&db).await.unwrap(), STANDING_SCOPES.len());
        assert_eq!(install(&db).await.unwrap(), STANDING_SCOPES.len(), "idempotent");
        assert_eq!(installed(&db).await.unwrap().len(), STANDING_SCOPES.len());
        assert!(check(digest.clone()).await);
        assert!(check("activate-v3-sha256:other".into()).await);
        assert!(crate::runtime::app_element_activation_any_grant(&db).await.unwrap());
        assert!(crate::runtime::send_message_by_nickname_any_grant(&db).await.unwrap());
        assert!(crate::runtime::center_authorized(&db).await.unwrap());
        for _ in 0..3 {
            let mut conn = db.acquire().await.unwrap();
            assert!(consume(&mut conn, capability, route, target, effect, &digest).await.unwrap());
        }
        // A standing grant never covers another scope.
        let mut conn = db.acquire().await.unwrap();
        assert!(!consume(&mut conn, capability, "other_route", target, effect, &digest).await.unwrap());
        drop(conn);
        assert_eq!(revoke(&db).await.unwrap(), STANDING_SCOPES.len() as u64);
        assert!(!check(digest.clone()).await);
        assert!(!crate::runtime::center_authorized(&db).await.unwrap());
        let decisions: Vec<String> = sqlx::query_scalar("SELECT DISTINCT decision FROM capability_audit WHERE request_id = 'setup' ORDER BY decision").fetch_all(&db).await.unwrap();
        assert_eq!(decisions, ["standing_grant_installed", "standing_grants_revoked"]);
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn exact_one_shot_grant_is_consumed_before_standing() {
        let (db, dir) = private_db().await;
        let (capability, route, target, effect) = ("desktop.element.input", "macos_gui", "main_or_single_app_window", "replace_text");
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', ?, ?, ?, ?, 'p')").bind(capability).bind(route).bind(target).bind(effect).execute(&db).await.unwrap();
        install(&db).await.unwrap();
        let mut conn = db.acquire().await.unwrap();
        assert!(consume(&mut conn, capability, route, target, effect, "p").await.unwrap());
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM local_capability_grants").fetch_one(&mut *conn).await.unwrap();
        assert_eq!(left, 0);
        assert!(consume(&mut conn, capability, route, target, effect, "p").await.unwrap());
        drop(conn); drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }
}
