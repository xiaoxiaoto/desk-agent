pub mod permission;
pub mod grants;
mod nickname;
pub mod queue;
// All local runtime GUI tasks share this guard. Composite children never recurse
// through invoke_task, so the whole batch (including waits) owns it exactly once.
static GUI_EXECUTOR: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub use nickname::{send_message_by_nickname_any_grant, send_message_by_nickname_parameters, revoke_send_message_by_nickname};

use crate::kernel::instruction::{ApplicationInstruction, AtomicInstruction, PerceptionInstruction, TaskInstruction};
use crate::kernel::task::{valid_key_input, ActivationDiagnostic, TaskInput};
use sqlx::SqlitePool;
use sha2::{Digest, Sha256};
use crate::applications::wecom::AppWindowCapture;
use std::time::{SystemTime, UNIX_EPOCH};

const ROUTE: &str = "macos_gui";
const PARAMETERS: &str = "none:v1";
const CENTER: (&str, &str, &str) = ("wecom.window.center", "main_or_single_wecom_window", "move_without_resize");
const WINDOW_CAPTURE: (&str, &str, &str) = ("desktop.window.capture", "main_or_single_app_window", "capture");
const VISION_ROUTE: &str = "local_vision";
const WINDOW_LOCATE: (&str, &str, &str) = ("vision.window.locate", "main_or_single_app_window", "local_inference");
const WINDOW_CONVERT: (&str, &str, &str) = ("desktop.window.coordinates.convert", "main_or_single_app_window", "read_geometry");
const ELEMENT_ACTIVATE: (&str, &str, &str) = ("wecom.element.activate", "main_or_single_app_window", "activate_element");
const ELEMENT_DIAGNOSE: (&str, &str, &str) = ("wecom.element.diagnose", "main_or_single_app_window", "inspect_element");
const ELEMENT_FOCUS: (&str, &str, &str) = ("desktop.element.focus", "main_or_single_app_window", "focus_element");
const KEY_INPUT: (&str, &str, &str) = ("desktop.keyboard.key_input", "main_or_single_app_window", "key_input");
const SEARCH_QUERY: (&str, &str, &str) = ("wecom.search.query.set", "wecom_search_field", "set_search_query");
const ELEMENT_INPUT: (&str, &str, &str) = ("desktop.element.input", "main_or_single_app_window", "replace_text");
const CHAT_OPEN: (&str, &str, &str) = ("wecom.chat.open", "search_result_contact", "open_chat");
const CHAT_DRAFT: (&str, &str, &str) = ("wecom.chat.draft.write", "current_contact_empty_message_editor", "write_chat_draft");
const CHAT_DRAFT_SEND: (&str, &str, &str) = ("wecom.chat.draft.send", "current_contact_exact_message_editor", "send_chat_draft");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeError {
    Forbidden,
    AuditUnavailable,
    CapabilityUnavailable,
    ActivationPreflight(ActivationDiagnostic),
    OutcomeUnknown,
    InvalidInput,
    /// The instruction queue reached its configured pending bound; nothing was enqueued.
    RateLimited,
}

async fn authorized(database: &SqlitePool, scope: (&str, &str, &str)) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
        .bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(PARAMETERS)
        .fetch_one(database).await.map(|v| v == 1)
}

pub async fn center_authorized(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    authorized(database, CENTER).await
}

pub async fn app_window_capture_authorized(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    authorized(database, WINDOW_CAPTURE).await
}

/// Advisory catalog check; dispatch always checks the exact request digest again.
pub async fn app_window_conversion_any_grant(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM local_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4 AND length(parameters) = 82 AND substr(parameters, 1, 18) = 'conversion-sha256:' AND substr(parameters, 19) NOT GLOB '*[^0-9a-f]*') OR EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4)")
        .bind(WINDOW_CONVERT.0).bind(ROUTE).bind(WINDOW_CONVERT.1).bind(WINDOW_CONVERT.2)
        .fetch_one(database).await.map(|v| v == 1)
}
/// Advisory only: an exact grant is specific to one request and consumed on use; a standing grant covers the scope.
pub async fn app_element_activation_any_grant(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM local_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4 AND length(parameters) = 83 AND substr(parameters, 1, 19) = 'activate-v3-sha256:' AND substr(parameters, 20) NOT GLOB '*[^0-9a-f]*') OR EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4)")
        .bind(ELEMENT_ACTIVATE.0).bind(ROUTE).bind(ELEMENT_ACTIVATE.1).bind(ELEMENT_ACTIVATE.2)
        .fetch_one(database).await.map(|v| v == 1)
}

/// Advisory only: diagnosis requires an exact, separately scoped grant.
pub async fn app_element_diagnosis_any_grant(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM local_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4 AND length(parameters) = 83 AND substr(parameters, 1, 19) = 'diagnose-v3-sha256:' AND substr(parameters, 20) NOT GLOB '*[^0-9a-f]*') OR EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4)")
        .bind(ELEMENT_DIAGNOSE.0).bind(ROUTE).bind(ELEMENT_DIAGNOSE.1).bind(ELEMENT_DIAGNOSE.2)
        .fetch_one(database).await.map(|v| v == 1)
}

/// Advisory only: focus requires an exact, separately scoped grant.
pub async fn app_element_focus_any_grant(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM local_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4 AND length(parameters) = 80 AND substr(parameters, 1, 16) = 'focus-v1-sha256:' AND substr(parameters, 17) NOT GLOB '*[^0-9a-f]*') OR EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4)")
        .bind(ELEMENT_FOCUS.0).bind(ROUTE).bind(ELEMENT_FOCUS.1).bind(ELEMENT_FOCUS.2)
        .fetch_one(database).await.map(|v| v == 1)
}

/// Advisory only: key input requires an exact, separately scoped grant.
pub async fn app_key_input_any_grant(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM local_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4 AND length(parameters) = 84 AND substr(parameters, 1, 20) = 'key-input-v1-sha256:' AND substr(parameters, 21) NOT GLOB '*[^0-9a-f]*') OR EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4)")
        .bind(KEY_INPUT.0).bind(ROUTE).bind(KEY_INPUT.1).bind(KEY_INPUT.2)
        .fetch_one(database).await.map(|v| v == 1)
}


/// Advisory only: dispatch still checks the exact point and text.
pub async fn app_element_input_any_grant(database: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM local_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4 AND length(parameters) = 88 AND substr(parameters, 1, 24) = 'element-input-v1-sha256:' AND substr(parameters, 25) NOT GLOB '*[^0-9a-f]*') OR EXISTS(SELECT 1 FROM standing_capability_grants WHERE app = 'wecom' AND capability = ?1 AND route = ?2 AND target = ?3 AND effect = ?4)")
        .bind(ELEMENT_INPUT.0).bind(ROUTE).bind(ELEMENT_INPUT.1).bind(ELEMENT_INPUT.2)
        .fetch_one(database).await.map(|v| v == 1)
}


pub fn app_element_input_parameters(point: [f64; 2], text: &str) -> Result<String, serde_json::Error> {
    text_input_parameters(point, text, "element-input-v1-sha256:")
}

fn text_input_parameters(point: [f64; 2], text: &str, prefix: &str) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct Parameters<'a> {
        point: [f64; 2],
        text: &'a str,
    }
    let encoded = serde_json::to_vec(&Parameters { point, text })?;
    Ok(format!("{prefix}{:x}", Sha256::digest(encoded)))
}

async fn audit_text_input(database: &SqlitePool, request_id: &str, parameters: &str, decision: &str, scope: (&str, &str, &str)) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(parameters).bind(decision)
        .execute(database).await?;
    Ok(())
}

// The consuming recheck (exact grant consumed, standing grant kept) and its durable audit commit together before any focus/write.
async fn authorize_text_input(database: &SqlitePool, request_id: &str, parameters: &str, consume: bool, scope: (&str, &str, &str)) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(parameters).bind("authorization_check").execute(&mut *tx).await?;
    let granted = if consume {
        grants::consume(&mut tx, scope.0, ROUTE, scope.1, scope.2, parameters).await?
    } else {
        sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
            .bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(parameters)
            .fetch_one(&mut *tx).await? == 1
    };
    let decision = if !granted { "denied" } else if consume { "dispatch_authorized" } else { "grant_allowed" };
    sqlx::query(insert).bind(request_id).bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(parameters).bind(decision).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn text_input_authorized(
    database: &SqlitePool,
    request_id: &str,
    point: [f64; 2],
    text: String,
    scope: (&str, &str, &str),
    prefix: &str,
    preflight: impl FnOnce([f64; 2], &str) -> Result<(), ActivationDiagnostic> + Send + 'static,
    dispatch: impl FnOnce([f64; 2], &str) -> anyhow::Result<()> + Send + 'static,
) -> Result<(), InvokeError> {
    if !crate::kernel::task::valid_app_input(&point, &text) {
        return Err(InvokeError::InvalidInput);
    }
    let parameters = text_input_parameters(point, &text, prefix).map_err(|_| InvokeError::InvalidInput)?;
    if !authorize_text_input(database, request_id, &parameters, false, scope).await.map_err(|_| InvokeError::AuditUnavailable)? {
        return Err(InvokeError::Forbidden);
    }
    // No native inspection, permission request, focus or write precedes the exact grant.
    let result = tokio::task::spawn_blocking(move || {
        let result = preflight(point, &text);
        (text, result)
    }).await;
    let text = match result {
        Ok((text, Ok(()))) => text,
        failure => {
            let diagnostic = match failure {
                Ok((_, Err(diagnostic))) => diagnostic,
                _ => ActivationDiagnostic::PreflightUnavailable,
            };
            let decision = format!("not_dispatched_{}", diagnostic.as_str());
            audit_text_input(database, request_id, &parameters, &decision, scope).await.map_err(|_| InvokeError::AuditUnavailable)?;
            return Err(InvokeError::ActivationPreflight(diagnostic));
        }
    };
    if !authorize_text_input(database, request_id, &parameters, true, scope).await.map_err(|_| InvokeError::AuditUnavailable)? {
        return Err(InvokeError::Forbidden);
    }
    let result = tokio::task::spawn_blocking(move || dispatch(point, &text)).await;
    let succeeded = matches!(result, Ok(Ok(())));
    // No error contents or plaintext text enter ordinary audit or task responses.
    audit_text_input(database, request_id, &parameters, if succeeded { "dispatch_succeeded" } else { "dispatch_outcome_unknown" }, scope)
        .await.map_err(|_| InvokeError::OutcomeUnknown)?;
    if !succeeded { return Err(InvokeError::OutcomeUnknown); }
    Ok(())
}


/// Field order, explicit null labels and compact JSON serialization are part of the authorization contract.
pub fn app_element_activation_parameters(point: [f64; 2], expected_role: &str, expected_label: Option<&str>) -> Result<String, serde_json::Error> {
    app_element_parameters("activate-v3-sha256:", point, expected_role, expected_label)
}

pub fn app_element_diagnosis_parameters(point: [f64; 2], expected_role: &str, expected_label: Option<&str>) -> Result<String, serde_json::Error> {
    app_element_parameters("diagnose-v3-sha256:", point, expected_role, expected_label)
}

pub fn app_element_focus_parameters(point: [f64; 2], expected_role: &str, expected_label: Option<&str>) -> Result<String, serde_json::Error> {
    app_element_parameters("focus-v1-sha256:", point, expected_role, expected_label)
}

pub fn app_key_input_parameters(key: &str, modifiers: &[String]) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct Parameters<'a> {
        key: &'a str,
        modifiers: &'a [String],
    }
    let encoded = serde_json::to_vec(&Parameters { key, modifiers })?;
    Ok(format!("key-input-v1-sha256:{:x}", Sha256::digest(encoded)))
}

fn app_element_parameters(prefix: &str, point: [f64; 2], expected_role: &str, expected_label: Option<&str>) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct Parameters<'a> {
        point: [f64; 2],
        expected_role: &'a str,
        expected_label: Option<&'a str>,
    }
    let encoded = serde_json::to_vec(&Parameters { point, expected_role, expected_label })?;
    Ok(format!("{prefix}{:x}", Sha256::digest(encoded)))
}

async fn audit_activation(database: &SqlitePool, request_id: &str, parameters: &str, decision: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(ELEMENT_ACTIVATE.0).bind(ROUTE).bind(ELEMENT_ACTIVATE.1).bind(ELEMENT_ACTIVATE.2).bind(parameters).bind(decision)
        .execute(database).await?;
    Ok(())
}

// Both initial authorization and the consuming recheck durably audit their decision.
// grants::consume deletes an exact grant or accepts a standing one inside the same
// transaction, so the check rolls back if the audit fails.
async fn authorize_activation(database: &SqlitePool, request_id: &str, parameters: &str, consume: bool) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(ELEMENT_ACTIVATE.0).bind(ROUTE).bind(ELEMENT_ACTIVATE.1).bind(ELEMENT_ACTIVATE.2).bind(parameters).bind("authorization_check").execute(&mut *tx).await?;
    let granted = if consume {
        grants::consume(&mut tx, ELEMENT_ACTIVATE.0, ROUTE, ELEMENT_ACTIVATE.1, ELEMENT_ACTIVATE.2, parameters).await?
    } else {
        sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
            .bind(ELEMENT_ACTIVATE.0).bind(ROUTE).bind(ELEMENT_ACTIVATE.1).bind(ELEMENT_ACTIVATE.2).bind(parameters)
            .fetch_one(&mut *tx).await? == 1
    };
    let decision = if !granted { "denied" } else if consume { "dispatch_authorized" } else { "grant_allowed" };
    sqlx::query(insert).bind(request_id).bind(ELEMENT_ACTIVATE.0).bind(ROUTE).bind(ELEMENT_ACTIVATE.1).bind(ELEMENT_ACTIVATE.2).bind(parameters).bind(decision).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn authorize_diagnosis(database: &SqlitePool, request_id: &str, parameters: &str, allowed_decision: &str) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(ELEMENT_DIAGNOSE.0).bind(ROUTE).bind(ELEMENT_DIAGNOSE.1).bind(ELEMENT_DIAGNOSE.2).bind(parameters).bind("authorization_check").execute(&mut *tx).await?;
    let granted = sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
        .bind(ELEMENT_DIAGNOSE.0).bind(ROUTE).bind(ELEMENT_DIAGNOSE.1).bind(ELEMENT_DIAGNOSE.2).bind(parameters).fetch_one(&mut *tx).await? == 1;
    sqlx::query(insert).bind(request_id).bind(ELEMENT_DIAGNOSE.0).bind(ROUTE).bind(ELEMENT_DIAGNOSE.1).bind(ELEMENT_DIAGNOSE.2).bind(parameters).bind(if granted { allowed_decision } else { "denied" }).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn audit_diagnosis(database: &SqlitePool, request_id: &str, parameters: &str, decision: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(ELEMENT_DIAGNOSE.0).bind(ROUTE).bind(ELEMENT_DIAGNOSE.1).bind(ELEMENT_DIAGNOSE.2).bind(parameters).bind(decision)
        .execute(database).await?;
    Ok(())
}

async fn audit_focus(database: &SqlitePool, request_id: &str, parameters: &str, decision: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(ELEMENT_FOCUS.0).bind(ROUTE).bind(ELEMENT_FOCUS.1).bind(ELEMENT_FOCUS.2).bind(parameters).bind(decision)
        .execute(database).await?;
    Ok(())
}

async fn authorize_focus(database: &SqlitePool, request_id: &str, parameters: &str, consume: bool) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(ELEMENT_FOCUS.0).bind(ROUTE).bind(ELEMENT_FOCUS.1).bind(ELEMENT_FOCUS.2).bind(parameters).bind("authorization_check").execute(&mut *tx).await?;
    let granted = if consume {
        grants::consume(&mut tx, ELEMENT_FOCUS.0, ROUTE, ELEMENT_FOCUS.1, ELEMENT_FOCUS.2, parameters).await?
    } else {
        sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
            .bind(ELEMENT_FOCUS.0).bind(ROUTE).bind(ELEMENT_FOCUS.1).bind(ELEMENT_FOCUS.2).bind(parameters)
            .fetch_one(&mut *tx).await? == 1
    };
    let decision = if !granted { "denied" } else if consume { "dispatch_authorized" } else { "grant_allowed" };
    sqlx::query(insert).bind(request_id).bind(ELEMENT_FOCUS.0).bind(ROUTE).bind(ELEMENT_FOCUS.1).bind(ELEMENT_FOCUS.2).bind(parameters).bind(decision).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn audit_key_input(database: &SqlitePool, request_id: &str, parameters: &str, decision: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(KEY_INPUT.0).bind(ROUTE).bind(KEY_INPUT.1).bind(KEY_INPUT.2).bind(parameters).bind(decision)
        .execute(database).await?;
    Ok(())
}

async fn authorize_key_input(database: &SqlitePool, request_id: &str, parameters: &str, consume: bool) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(KEY_INPUT.0).bind(ROUTE).bind(KEY_INPUT.1).bind(KEY_INPUT.2).bind(parameters).bind("authorization_check").execute(&mut *tx).await?;
    let granted = if consume {
        grants::consume(&mut tx, KEY_INPUT.0, ROUTE, KEY_INPUT.1, KEY_INPUT.2, parameters).await?
    } else {
        sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
            .bind(KEY_INPUT.0).bind(ROUTE).bind(KEY_INPUT.1).bind(KEY_INPUT.2).bind(parameters)
            .fetch_one(&mut *tx).await? == 1
    };
    let decision = if !granted { "denied" } else if consume { "dispatch_authorized" } else { "grant_allowed" };
    sqlx::query(insert).bind(request_id).bind(KEY_INPUT.0).bind(ROUTE).bind(KEY_INPUT.1).bind(KEY_INPUT.2).bind(parameters).bind(decision).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}


async fn audit(database: &SqlitePool, request_id: &str, scope: (&str, &str, &str), decision: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(PARAMETERS).bind(decision)
        .execute(database).await?;
    Ok(())
}

/// The exact object array, including ordering and spelling, is part of the local grant.
pub fn app_window_locate_parameters(objects: &[String]) -> Result<String, serde_json::Error> {
    let encoded = serde_json::to_vec(objects)?;
    Ok(format!("objects-sha256:{:x}", Sha256::digest(encoded)))
}

/// The compact JSON object and its field order are part of the grant contract.
pub fn app_window_convert_parameters(capture_id: &str, boxes: &[[f64; 4]]) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct Parameters<'a> {
        capture_id: &'a str,
        boxes: &'a [[f64; 4]],
    }
    let encoded = serde_json::to_vec(&Parameters { capture_id, boxes })?;
    Ok(format!("conversion-sha256:{:x}", Sha256::digest(encoded)))
}

async fn authorize_conversion_and_record(database: &SqlitePool, request_id: &str, parameters: &str, allowed_decision: &str) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(WINDOW_CONVERT.0).bind(ROUTE).bind(WINDOW_CONVERT.1).bind(WINDOW_CONVERT.2).bind(parameters).bind("authorization_check").execute(&mut *tx).await?;
    let granted = sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
        .bind(WINDOW_CONVERT.0).bind(ROUTE).bind(WINDOW_CONVERT.1).bind(WINDOW_CONVERT.2).bind(parameters).fetch_one(&mut *tx).await? == 1;
    sqlx::query(insert).bind(request_id).bind(WINDOW_CONVERT.0).bind(ROUTE).bind(WINDOW_CONVERT.1).bind(WINDOW_CONVERT.2).bind(parameters).bind(if granted { allowed_decision } else { "denied" }).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn audit_conversion(database: &SqlitePool, request_id: &str, parameters: &str, decision: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(WINDOW_CONVERT.0).bind(ROUTE).bind(WINDOW_CONVERT.1).bind(WINDOW_CONVERT.2).bind(parameters).bind(decision)
        .execute(database).await?;
    Ok(())
}

async fn audit_vision(database: &SqlitePool, request_id: &str, parameters: &str, decision: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)")
        .bind(request_id).bind(WINDOW_LOCATE.0).bind(VISION_ROUTE).bind(WINDOW_LOCATE.1).bind(WINDOW_LOCATE.2).bind(parameters).bind(decision)
        .execute(database).await?;
    Ok(())
}

async fn authorize_vision_and_record(database: &SqlitePool, request_id: &str, parameters: &str, allowed_decision: &str) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(WINDOW_LOCATE.0).bind(VISION_ROUTE).bind(WINDOW_LOCATE.1).bind(WINDOW_LOCATE.2).bind(parameters).bind("authorization_check").execute(&mut *tx).await?;
    let granted = sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
        .bind(WINDOW_LOCATE.0).bind(VISION_ROUTE).bind(WINDOW_LOCATE.1).bind(WINDOW_LOCATE.2).bind(parameters).fetch_one(&mut *tx).await? == 1;
    sqlx::query(insert).bind(request_id).bind(WINDOW_LOCATE.0).bind(VISION_ROUTE).bind(WINDOW_LOCATE.1).bind(WINDOW_LOCATE.2).bind(parameters).bind(if granted { allowed_decision } else { "denied" }).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn authorize_and_record(database: &SqlitePool, request_id: &str, scope: (&str, &str, &str), allowed_decision: &str) -> Result<bool, sqlx::Error> {
    let mut tx = database.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert).bind(request_id).bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(PARAMETERS).bind("authorization_check").execute(&mut *tx).await?;
    let granted = sqlx::query_scalar::<_, i64>(grants::EXACT_OR_STANDING)
        .bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(PARAMETERS).fetch_one(&mut *tx).await? == 1;
    sqlx::query(insert).bind(request_id).bind(scope.0).bind(ROUTE).bind(scope.1).bind(scope.2).bind(PARAMETERS).bind(if granted { allowed_decision } else { "denied" }).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn capture_authorized(database: &SqlitePool, request_id: &str) -> Result<(AppWindowCapture, u128), InvokeError> {
    if !authorize_and_record(database, request_id, WINDOW_CAPTURE, "grant_allowed").await.map_err(|_| InvokeError::AuditUnavailable)? {
        return Err(InvokeError::Forbidden);
    }
    // OS permissions are requested once by `desk-agent setup`; dispatch only preflights and never prompts.
    let permission = tokio::task::spawn_blocking(crate::applications::wecom::screen_recording_allowed)
        .await.map_err(|_| InvokeError::CapabilityUnavailable)?;
    if !permission {
        audit(database, request_id, WINDOW_CAPTURE, "not_dispatched_screen_recording_permission").await.map_err(|_| InvokeError::AuditUnavailable)?;
        return Err(InvokeError::CapabilityUnavailable);
    }
    if !matches!(crate::applications::wecom::app_window_capture_capability().status, crate::kernel::capability::CapabilityStatus::Available) {
        audit(database, request_id, WINDOW_CAPTURE, "not_dispatched_prerequisites").await.map_err(|_| InvokeError::AuditUnavailable)?;
        return Err(InvokeError::CapabilityUnavailable);
    }
    if !authorize_and_record(database, request_id, WINDOW_CAPTURE, "dispatch_authorized").await.map_err(|_| InvokeError::AuditUnavailable)? {
        return Err(InvokeError::Forbidden);
    }
    let result = tokio::task::spawn_blocking(|| {
        let capture = crate::applications::wecom::capture_app_window()?;
        let captured_at = SystemTime::now().duration_since(UNIX_EPOCH)?;
        Ok::<_, anyhow::Error>((capture, captured_at.as_millis()))
    }).await;
    let succeeded = matches!(result, Ok(Ok(_)));
    audit(database, request_id, WINDOW_CAPTURE, if succeeded { "dispatch_succeeded" } else { "dispatch_outcome_unknown" })
        .await.map_err(|_| InvokeError::OutcomeUnknown)?;
    result.map_err(|_| InvokeError::OutcomeUnknown)?.map_err(|_| InvokeError::OutcomeUnknown)
}

/// Map a captured-image box into global macOS points at capture time.
fn screen_box(image_size: [u32; 2], window: [f64; 4], coords: [f64; 4]) -> Option<([f64; 4], [f64; 2])> {
    let [width, height] = image_size;
    let [x, y, w, h] = window;
    let [x1, y1, x2, y2] = coords;
    if width == 0 || height == 0 || !window.iter().all(|v| v.is_finite())
        || w <= 0.0 || h <= 0.0 || !(x + w).is_finite() || !(y + h).is_finite()
        || !coords.iter().all(|v| v.is_finite())
        || x1 < 0.0 || y1 < 0.0 || x1 >= x2 || y1 >= y2
        || x2 > f64::from(width) || y2 > f64::from(height)
    {
        return None;
    }
    let sx = w / f64::from(width);
    let sy = h / f64::from(height);
    let bounds = [x + x1 * sx, y + y1 * sy, x + x2 * sx, y + y2 * sy];
    let center = [bounds[0] / 2.0 + bounds[2] / 2.0, bounds[1] / 2.0 + bounds[3] / 2.0];
    if !bounds.iter().chain(center.iter()).all(|v| v.is_finite())
        || bounds[0] >= bounds[2] || bounds[1] >= bounds[3]
    {
        return None;
    }
    Some((bounds, center))
}

fn convert_boxes(image_size: [u32; 2], window: [f64; 4], boxes: &[[f64; 4]]) -> Option<Vec<serde_json::Value>> {
    boxes.iter().map(|coords| {
        let (bounds, center) = screen_box(image_size, window, *coords)?;
        Some(serde_json::json!({
            "image_bounds_pixels": coords,
            "screen_bounds_points": bounds,
            "screen_center_points": center
        }))
    }).collect()
}

pub async fn invoke_task(
    database: Option<&SqlitePool>,
    request_id: &str,
    input: TaskInput,
    launch: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
    center: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
) -> Result<(TaskInstruction, Option<serde_json::Value>), InvokeError> {
    let database = database.cloned();
    let request_id = request_id.to_owned();
    // A disconnect only drops the waiter, not the admitted operation or its
    // GUI executor guard. Native input cannot be cancelled by dropping a future.
    tokio::spawn(async move {
        invoke_task_owned(database.as_ref(), &request_id, input, launch, center).await
    })
    .await
    .map_err(|_| InvokeError::OutcomeUnknown)?
}

async fn invoke_task_owned(
    database: Option<&SqlitePool>,
    request_id: &str,
    input: TaskInput,
    launch: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
    center: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
) -> Result<(TaskInstruction, Option<serde_json::Value>), InvokeError> {
    if input.app() != "wecom" {
        return Err(InvokeError::CapabilityUnavailable);
    }
    let instruction = input.resolve_instruction().map_err(|_| InvokeError::InvalidInput)?;
    let _gui = GUI_EXECUTOR.lock().await;
    let mut output = None;
    match &instruction {
        TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch) => {
            tokio::task::spawn_blocking(launch).await.map_err(|_| InvokeError::OutcomeUnknown)?.map_err(|_| InvokeError::OutcomeUnknown)?;
        }
        TaskInstruction::Atomic(AtomicInstruction::WindowCenter) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            if !authorize_and_record(database, request_id, CENTER, "grant_allowed").await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            if !matches!(crate::applications::wecom::center_capability().status, crate::kernel::capability::CapabilityStatus::Available) {
                audit(database, request_id, CENTER, "not_dispatched_prerequisites").await.map_err(|_| InvokeError::AuditUnavailable)?;
                return Err(InvokeError::CapabilityUnavailable);
            }
            // Recheck revocation and persist the dispatch decision immediately before dispatch.
            if !authorize_and_record(database, request_id, CENTER, "dispatch_authorized").await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let result = tokio::task::spawn_blocking(center).await;
            let succeeded = matches!(result, Ok(Ok(())));
            // The effect may already have happened; an audit failure cannot be
            // represented as not dispatched or as success.
            audit(database, request_id, CENTER, if succeeded { "dispatch_succeeded" } else { "dispatch_outcome_unknown" })
                .await.map_err(|_| InvokeError::OutcomeUnknown)?;
            if !succeeded { return Err(InvokeError::OutcomeUnknown); }
        }
        TaskInstruction::Atomic(AtomicInstruction::WindowCapture) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            let (capture, _) = capture_authorized(database, request_id).await?;
            output = Some(serde_json::json!({ "path": capture.path, "capture_id": capture.capture_id }));
        }
        TaskInstruction::Perception(PerceptionInstruction::ScreenshotCoordinatesConvert{ capture_id, boxes }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            if boxes.is_empty() || boxes.len() > 100 || boxes.iter().any(|b| {
                !b.iter().all(|v| v.is_finite()) || b[0] < 0.0 || b[1] < 0.0 || b[0] >= b[2] || b[1] >= b[3]
            }) {
                return Err(InvokeError::InvalidInput);
            }
            let parameters = app_window_convert_parameters(capture_id, boxes).map_err(|_| InvokeError::InvalidInput)?;
            if !authorize_conversion_and_record(database, request_id, &parameters, "grant_allowed").await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            if !authorize_conversion_and_record(database, request_id, &parameters, "dispatch_authorized").await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let id = capture_id.clone();
            let capture = tokio::task::spawn_blocking(move || crate::applications::wecom::load_app_window_capture(&id)).await;
            let capture = match capture {
                Ok(Ok(capture)) => capture,
                _ => {
                    audit_conversion(database, request_id, &parameters, "dispatch_unavailable").await.map_err(|_| InvokeError::AuditUnavailable)?;
                    return Err(InvokeError::CapabilityUnavailable);
                }
            };
            let dimensions = [capture.image_width, capture.image_height];
            let window = capture.window_bounds_points;
            let converted = convert_boxes(dimensions, window, boxes);
            let Some(converted) = converted else {
                audit_conversion(database, request_id, &parameters, "invalid_image_bounds").await.map_err(|_| InvokeError::AuditUnavailable)?;
                return Err(InvokeError::InvalidInput);
            };
            audit_conversion(database, request_id, &parameters, "dispatch_succeeded").await.map_err(|_| InvokeError::AuditUnavailable)?;
            output = Some(serde_json::json!({
                "capture_id": capture_id,
                "boxes": converted,
                "window_bounds_points": window,
                "screen_coordinate_space": "macos_global_top_left_points_at_capture"
            }));
        }
        TaskInstruction::Perception(PerceptionInstruction::VisionDetect{ objects }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            let parameters = app_window_locate_parameters(objects).map_err(|_| InvokeError::InvalidInput)?;
            // Disclosure authorization is required before window capture (which never prompts).
            if !authorize_vision_and_record(database, request_id, &parameters, "grant_allowed").await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let (capture, captured_at_unix_ms) = capture_authorized(database, request_id).await?;
            let dimensions = [capture.image_width, capture.image_height];
            let window = capture.window_bounds_points;
            let mut image_size = None;
            let mut located = Vec::with_capacity(objects.len());
            for query in objects {
                let provider_path = capture.path.clone();
                let queries = vec![query.clone()];
                // Each object is a separate disclosure. Check the original full-list grant
                // immediately before every request, including after earlier responses.
                if !authorize_vision_and_record(database, request_id, &parameters, "dispatch_authorized").await.map_err(|_| InvokeError::AuditUnavailable)? {
                    return Err(InvokeError::Forbidden);
                }
                let response = tokio::task::spawn_blocking(move || crate::providers::vlm::detect_app_window_objects(&provider_path, &queries)).await;
                let result = match response {
                    Ok(Ok(mut value)) => (|| {
                        let value = value.as_object_mut()?;
                        let size = value.remove("image_size")?;
                        if size != serde_json::json!(dimensions) {
                            return None;
                        }
                        if image_size.as_ref().is_some_and(|previous| previous != &size) {
                            return None;
                        }
                        let serde_json::Value::Array(mut results) = value.remove("objects")? else { return None; };
                        if results.len() != 1 {
                            return None;
                        }
                        let object = results.pop()?;
                        if object.get("query").and_then(|q| q.as_str()) != Some(query.as_str()) {
                            return None;
                        }
                        Some((size, object))
                    })(),
                    _ => None,
                };
                audit_vision(database, request_id, &parameters, if result.is_some() { "dispatch_succeeded" } else { "dispatch_outcome_unknown" })
                    .await.map_err(|_| InvokeError::OutcomeUnknown)?;
                let (size, object) = result.ok_or(InvokeError::OutcomeUnknown)?;
                image_size = Some(size);
                located.push(object);
            }
            output = Some(serde_json::json!({
                "image_size": image_size.ok_or(InvokeError::OutcomeUnknown)?,
                "objects": located,
                "capture_path": capture.path,
                "capture_id": capture.capture_id,
                "captured_at_unix_ms": captured_at_unix_ms,
                "target": "main_or_single_app_window",
                "coordinate_space": "captured_image_pixels",
                "window_bounds_points": window,
                "screen_coordinate_space": "macos_global_top_left_points_at_capture"
            }));
        }
        TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { nickname, messages, interval_ms }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            output = Some(nickname::invoke(database, request_id, nickname, messages, *interval_ms, launch, center).await?);
        }
        TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point, text }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            text_input_authorized(
                database, request_id, *point, text.clone(), ELEMENT_INPUT, "element-input-v1-sha256:",
                crate::applications::wecom::preflight_app_element_input,
                crate::applications::wecom::input_app_element_text,
            ).await?;
            output = Some(serde_json::json!({ "text_input": true }));
        }
        TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key, modifiers }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            if !valid_key_input(key, modifiers) { return Err(InvokeError::InvalidInput); }
            let parameters = app_key_input_parameters(key, modifiers).map_err(|_| InvokeError::InvalidInput)?;
            if !authorize_key_input(database, request_id, &parameters, false).await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let preflight = tokio::task::spawn_blocking(crate::applications::wecom::preflight_keyboard_key_input).await;
            let diagnostic = match preflight {
                Ok(Ok(())) => None,
                Ok(Err(diagnostic)) => Some(diagnostic),
                Err(_) => Some(ActivationDiagnostic::PreflightUnavailable),
            };
            if let Some(diagnostic) = diagnostic {
                let decision = format!("not_dispatched_{}", diagnostic.as_str());
                audit_key_input(database, request_id, &parameters, &decision).await.map_err(|_| InvokeError::AuditUnavailable)?;
                return Err(InvokeError::ActivationPreflight(diagnostic));
            }
            if !authorize_key_input(database, request_id, &parameters, true).await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let result = tokio::task::spawn_blocking(crate::applications::wecom::press_return_key).await;
            let succeeded = matches!(result, Ok(Ok(())));
            audit_key_input(database, request_id, &parameters, if succeeded { "dispatch_succeeded" } else { "dispatch_outcome_unknown" })
                .await.map_err(|_| InvokeError::OutcomeUnknown)?;
            if !succeeded { return Err(InvokeError::OutcomeUnknown); }
            output = Some(serde_json::json!({ "pressed": true }));
        }
        TaskInstruction::Atomic(AtomicInstruction::ClickButton{ point, expected_role, expected_label }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            if !point.iter().all(|v| v.is_finite())
                || expected_role.is_empty() || expected_role.len() > 256
                || expected_label.as_ref().is_some_and(|label| label.is_empty() || label.len() > 256)
            {
                return Err(InvokeError::InvalidInput);
            }
            let parameters = app_element_activation_parameters(*point, expected_role, expected_label.as_deref()).map_err(|_| InvokeError::InvalidInput)?;
            if !authorize_activation(database, request_id, &parameters, false).await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let (role, label, position) = (expected_role.clone(), expected_label.clone(), *point);
            let preflight = tokio::task::spawn_blocking(move || crate::applications::wecom::preflight_app_element(position, &role, label.as_deref())).await;
            let diagnostic = match preflight {
                Ok(Ok(())) => None,
                Ok(Err(diagnostic)) => Some(diagnostic),
                Err(_) => Some(ActivationDiagnostic::PreflightUnavailable),
            };
            if let Some(diagnostic) = diagnostic {
                let decision = format!("not_dispatched_{}", diagnostic.as_str());
                audit_activation(database, request_id, &parameters, &decision).await.map_err(|_| InvokeError::AuditUnavailable)?;
                return Err(InvokeError::ActivationPreflight(diagnostic));
            }
            let (role, label, position) = (expected_role.clone(), expected_label.clone(), *point);
            if !authorize_activation(database, request_id, &parameters, true).await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let result = tokio::task::spawn_blocking(move || crate::applications::wecom::activate_app_element(position, &role, label.as_deref())).await;
            let succeeded = matches!(result, Ok(Ok(())));
            audit_activation(database, request_id, &parameters, if succeeded { "dispatch_succeeded" } else { "dispatch_outcome_unknown" })
                .await.map_err(|_| InvokeError::OutcomeUnknown)?;
            if !succeeded { return Err(InvokeError::OutcomeUnknown); }
            output = Some(serde_json::json!({ "clicked": true }));
        }
        TaskInstruction::Atomic(AtomicInstruction::ClickFocus{ point, expected_role, expected_label }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            if !point.iter().all(|v| v.is_finite())
                || expected_role.is_empty() || expected_role.len() > 256
                || expected_label.as_ref().is_some_and(|label| label.is_empty() || label.len() > 256)
            {
                return Err(InvokeError::InvalidInput);
            }
            let parameters = app_element_focus_parameters(*point, expected_role, expected_label.as_deref()).map_err(|_| InvokeError::InvalidInput)?;
            if !authorize_focus(database, request_id, &parameters, false).await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let (role, label, position) = (expected_role.clone(), expected_label.clone(), *point);
            let preflight = tokio::task::spawn_blocking(move || crate::applications::wecom::preflight_app_element(position, &role, label.as_deref())).await;
            let diagnostic = match preflight {
                Ok(Ok(())) => None,
                Ok(Err(diagnostic)) => Some(diagnostic),
                Err(_) => Some(ActivationDiagnostic::PreflightUnavailable),
            };
            if let Some(diagnostic) = diagnostic {
                let decision = format!("not_dispatched_{}", diagnostic.as_str());
                audit_focus(database, request_id, &parameters, &decision).await.map_err(|_| InvokeError::AuditUnavailable)?;
                return Err(InvokeError::ActivationPreflight(diagnostic));
            }
            let (role, label, position) = (expected_role.clone(), expected_label.clone(), *point);
            if !authorize_focus(database, request_id, &parameters, true).await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let result = tokio::task::spawn_blocking(move || crate::applications::wecom::focus_app_element(position, &role, label.as_deref())).await;
            let succeeded = matches!(result, Ok(Ok(())));
            audit_focus(database, request_id, &parameters, if succeeded { "dispatch_succeeded" } else { "dispatch_outcome_unknown" })
                .await.map_err(|_| InvokeError::OutcomeUnknown)?;
            if !succeeded { return Err(InvokeError::OutcomeUnknown); }
            output = Some(serde_json::json!({ "focused": true }));
        }
        TaskInstruction::Atomic(AtomicInstruction::AxInspect{ point, expected_role, expected_label }) => {
            let database = database.ok_or(InvokeError::AuditUnavailable)?;
            if !point.iter().all(|v| v.is_finite())
                || expected_role.is_empty() || expected_role.len() > 256
                || expected_label.as_ref().is_some_and(|label| label.is_empty() || label.len() > 256)
            {
                return Err(InvokeError::InvalidInput);
            }
            let parameters = app_element_diagnosis_parameters(*point, expected_role, expected_label.as_deref()).map_err(|_| InvokeError::InvalidInput)?;
            if !authorize_diagnosis(database, request_id, &parameters, "grant_allowed").await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            if !authorize_diagnosis(database, request_id, &parameters, "dispatch_authorized").await.map_err(|_| InvokeError::AuditUnavailable)? {
                return Err(InvokeError::Forbidden);
            }
            let (role, label, position) = (expected_role.clone(), expected_label.clone(), *point);
            let result = tokio::task::spawn_blocking(move || crate::applications::wecom::preflight_app_element(position, &role, label.as_deref())).await;
            let diagnostic = match result {
                Ok(Ok(())) => None,
                Ok(Err(diagnostic)) => Some(diagnostic),
                Err(_) => Some(ActivationDiagnostic::PreflightUnavailable),
            };
            let decision = match diagnostic {
                Some(code) => format!("not_dispatched_{}", code.as_str()),
                None => "diagnosis_matched".to_owned(),
            };
            audit_diagnosis(database, request_id, &parameters, &decision).await.map_err(|_| InvokeError::AuditUnavailable)?;
            if let Some(diagnostic) = diagnostic {
                return Err(InvokeError::ActivationPreflight(diagnostic));
            }
            output = Some(serde_json::json!({ "diagnostic_code": "matched" }));
        }
    }
    Ok((instruction, output))
}

#[cfg(test)]
mod activation_authorization_tests {
    use super::*;

    #[tokio::test]
    async fn consumption_is_single_use_and_audit_failure_rolls_back() {
        let db = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE local_capability_grants (app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT, PRIMARY KEY(app, capability, route, target, effect, parameters))").execute(&db).await.unwrap(); sqlx::query(crate::runtime::grants::CREATE_TABLE).execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE capability_audit (request_id TEXT, app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT, decision TEXT)").execute(&db).await.unwrap();
        let digest = app_element_activation_parameters([1.0, 2.0], "AXButton", Some("private label")).unwrap();
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&digest).execute(&db).await.unwrap();
        assert!(authorize_activation(&db, "first", &digest, false).await.unwrap());
        for (app, capability, route, target, effect) in [
            ("other", "wecom.element.activate", "macos_gui", "main_or_single_app_window", "activate_element"),
            ("wecom", "other", "macos_gui", "main_or_single_app_window", "activate_element"),
            ("wecom", "wecom.element.activate", "other", "main_or_single_app_window", "activate_element"),
            ("wecom", "wecom.element.activate", "macos_gui", "other", "activate_element"),
            ("wecom", "wecom.element.activate", "macos_gui", "main_or_single_app_window", "other"),
        ] {
            sqlx::query("INSERT INTO local_capability_grants VALUES (?, ?, ?, ?, ?, ?)").bind(app).bind(capability).bind(route).bind(target).bind(effect).bind(&digest).execute(&db).await.unwrap();
        }
        sqlx::query("DELETE FROM local_capability_grants WHERE app = 'wecom' AND capability = 'wecom.element.activate' AND route = 'macos_gui' AND target = 'main_or_single_app_window' AND effect = 'activate_element'").execute(&db).await.unwrap();
        assert!(!authorize_activation(&db, "revoked", &digest, true).await.unwrap());
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&digest).execute(&db).await.unwrap();
        assert!(authorize_activation(&db, "first", &digest, true).await.unwrap());
        assert!(!authorize_activation(&db, "second", &digest, true).await.unwrap());
        let decisions: Vec<String> = sqlx::query_scalar("SELECT decision FROM capability_audit WHERE request_id = 'first'").fetch_all(&db).await.unwrap();
        assert_eq!(decisions, ["authorization_check", "grant_allowed", "authorization_check", "dispatch_authorized"]);
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&digest).execute(&db).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_dispatch BEFORE INSERT ON capability_audit WHEN NEW.decision = 'dispatch_authorized' BEGIN SELECT RAISE(ABORT, 'audit failed'); END").execute(&db).await.unwrap();
        assert!(authorize_activation(&db, "audit-failure", &digest, true).await.is_err());
        assert!(authorize_activation(&db, "still-present", &digest, false).await.unwrap());
        let leaked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_audit WHERE parameters LIKE '%private label%'").fetch_one(&db).await.unwrap();
        assert_eq!(leaked, 0);
    }

    #[tokio::test]
    async fn coordinate_and_named_grants_are_consumed_independently() {
        let db = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE local_capability_grants (app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT, PRIMARY KEY(app, capability, route, target, effect, parameters))").execute(&db).await.unwrap(); sqlx::query(crate::runtime::grants::CREATE_TABLE).execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE capability_audit (request_id TEXT, app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT, decision TEXT)").execute(&db).await.unwrap();
        let named = app_element_activation_parameters([1.0, 2.0], "AXCell", Some("message")).unwrap();
        let coordinate = app_element_activation_parameters([1.0, 2.0], "AXCell", None).unwrap();
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&coordinate).execute(&db).await.unwrap();
        assert!(!authorize_activation(&db, "named-denied", &named, true).await.unwrap());
        assert!(authorize_activation(&db, "coordinate-present", &coordinate, false).await.unwrap());
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.element.activate', 'macos_gui', 'main_or_single_app_window', 'activate_element', ?)").bind(&named).execute(&db).await.unwrap();
        assert!(authorize_activation(&db, "named", &named, true).await.unwrap());
        assert!(!authorize_activation(&db, "named-replay", &named, true).await.unwrap());
        assert!(authorize_activation(&db, "coordinate", &coordinate, true).await.unwrap());
        assert!(!authorize_activation(&db, "coordinate-replay", &coordinate, true).await.unwrap());
    }
}

#[cfg(test)]
mod coordinate_tests {
    use super::{convert_boxes, screen_box};

    #[test]
    fn maps_offset_and_non_retina_scale() {
        assert_eq!(
            screen_box([1000, 800], [100.0, 50.0, 400.0, 320.0], [100.0, 200.0, 500.0, 600.0]),
            Some(([140.0, 130.0, 300.0, 290.0], [220.0, 210.0]))
        );
    }

    #[test]
    fn maps_negative_origins_and_fractional_points() {
        assert_eq!(
            screen_box([800, 400], [-500.0, -200.0, 400.0, 200.0], [1.0, 3.0, 11.0, 13.0]),
            Some(([-499.5, -198.5, -494.5, -193.5], [-497.0, -196.0]))
        );
    }

    #[test]
    fn converts_multiple_boxes_in_order_and_rejects_any_invalid_box() {
        let boxes = [[0.0, 0.0, 200.0, 100.0], [1.0, 3.0, 11.0, 13.0]];
        let converted = convert_boxes([800, 400], [-500.0, -200.0, 400.0, 200.0], &boxes).unwrap();
        assert_eq!(converted, vec![
            serde_json::json!({"image_bounds_pixels": [0.0, 0.0, 200.0, 100.0], "screen_bounds_points": [-500.0, -200.0, -400.0, -150.0], "screen_center_points": [-450.0, -175.0]}),
            serde_json::json!({"image_bounds_pixels": [1.0, 3.0, 11.0, 13.0], "screen_bounds_points": [-499.5, -198.5, -494.5, -193.5], "screen_center_points": [-497.0, -196.0]}),
        ]);
        assert!(convert_boxes([800, 400], [-500.0, -200.0, 400.0, 200.0], &[boxes[0], [0.0, 0.0, 801.0, 10.0]]).is_none());
    }

    #[test]
    fn rejects_invalid_dimensions_and_bounds() {
        let image = [100, 100];
        let window = [10.0, 20.0, 100.0, 100.0];
        let coords = [0.0, 0.0, 50.0, 50.0];
        assert!(screen_box([0, 100], window, coords).is_none());
        assert!(screen_box([100, 0], window, coords).is_none());
        for invalid in [
            [10.0, 20.0, 0.0, 100.0],
            [10.0, 20.0, 100.0, -1.0],
            [f64::NAN, 20.0, 100.0, 100.0],
            [f64::MAX, 20.0, f64::MAX, 100.0],
        ] {
            assert!(screen_box(image, invalid, coords).is_none());
        }
        for invalid in [
            [-1.0, 0.0, 50.0, 50.0],
            [0.0, 0.0, 101.0, 50.0],
            [10.0, 0.0, 10.0, 50.0],
            [0.0, 20.0, 50.0, 10.0],
            [0.0, 0.0, f64::INFINITY, 50.0],
        ] {
            assert!(screen_box(image, window, invalid).is_none());
        }
    }
}

#[cfg(test)]
mod text_input_authorization_tests {
    use super::*;

    const QUERY: &str = "  私密 search  ";
    const POINT: [f64; 2] = [492.0, 203.0];

    async fn database() -> SqlitePool {
        let db = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE local_capability_grants (app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT, PRIMARY KEY(app, capability, route, target, effect, parameters))").execute(&db).await.unwrap(); sqlx::query(crate::runtime::grants::CREATE_TABLE).execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE capability_audit (request_id TEXT, app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT, decision TEXT)").execute(&db).await.unwrap();
        db
    }

    async fn grant(db: &SqlitePool) {
        let parameters = "parent-delegated-child-only";
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.search.query.set', 'macos_gui', 'wecom_search_field', 'set_search_query', ?)").bind(parameters).execute(db).await.unwrap();
    }


    #[tokio::test]
    async fn generic_input_requires_its_own_exact_one_shot_grant() {
        let db = database().await;
        grant(&db).await;
        let digest = app_element_input_parameters(POINT, QUERY).unwrap();
        let encoded = serde_json::to_vec(&serde_json::json!({"point": POINT, "text": QUERY})).unwrap();
        assert_eq!(digest, format!("element-input-v1-sha256:{:x}", Sha256::digest(encoded)));
        assert!(!app_element_input_any_grant(&db).await.unwrap());
        assert_eq!(text_input_authorized(&db, "wrong-scope", POINT, QUERY.into(), ELEMENT_INPUT, "element-input-v1-sha256:", |_, _| panic!("unauthorized preflight"), |_, _| panic!("unauthorized dispatch")).await, Err(InvokeError::Forbidden));
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'desktop.element.input', 'macos_gui', 'main_or_single_app_window', 'replace_text', ?)").bind(&digest).execute(&db).await.unwrap();
        assert!(app_element_input_any_grant(&db).await.unwrap());
        assert_eq!(text_input_authorized(&db, "preflight-failed", POINT, QUERY.into(), ELEMENT_INPUT, "element-input-v1-sha256:", |_, _| Err(ActivationDiagnostic::PreflightUnavailable), |_, _| panic!("failed preflight dispatch")).await, Err(InvokeError::ActivationPreflight(ActivationDiagnostic::PreflightUnavailable)));
        assert!(app_element_input_any_grant(&db).await.unwrap());
        assert_eq!(text_input_authorized(&db, "success", POINT, QUERY.into(), ELEMENT_INPUT, "element-input-v1-sha256:", |_, _| Ok(()), |point, text| { assert_eq!(point, POINT); assert_eq!(text, QUERY); Ok(()) }).await, Ok(()));
        assert!(!app_element_input_any_grant(&db).await.unwrap());
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_capability_grants WHERE capability = 'wecom.search.query.set'").fetch_one(&db).await.unwrap();
        assert_eq!(remaining, 1);
        assert_eq!(text_input_authorized(&db, "replay", POINT, QUERY.into(), ELEMENT_INPUT, "element-input-v1-sha256:", |_, _| panic!("replay preflight"), |_, _| panic!("replay dispatch")).await, Err(InvokeError::Forbidden));
        let decisions: Vec<String> = sqlx::query_scalar("SELECT decision FROM capability_audit WHERE request_id = 'success' ORDER BY rowid").fetch_all(&db).await.unwrap();
        assert_eq!(decisions, ["authorization_check", "grant_allowed", "authorization_check", "dispatch_authorized", "dispatch_succeeded"]);
    }

}

