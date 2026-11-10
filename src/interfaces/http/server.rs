use crate::applications::wecom::{send_message_by_nickname_capability, app_element_input_capability, app_element_focus_capability, keyboard_key_input_capability, app_element_activation_capability, app_window_capture_capability, center_capability, center_wecom_window, launch_wecom};
use crate::interfaces::ipc::{self, IpcRequest, IpcResponse};
use crate::kernel::task::{
    TaskError, TaskErrorCode, TaskErrorResponse, TaskPreflightError, TaskRequest, TaskResponse,
};
use crate::runtime::{send_message_by_nickname_any_grant, invoke_task, app_element_input_any_grant, app_element_activation_any_grant, app_element_diagnosis_any_grant, app_element_focus_any_grant, app_key_input_any_grant, app_window_capture_authorized, app_window_conversion_any_grant, center_authorized, InvokeError};
use crate::{
    config::loader::RuntimeConfig,
    infrastructure::database,
    kernel::capability::{CapabilityStatus, initial_capabilities},
    observability::tracing::init,
};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::{os::unix::fs::{MetadataExt, PermissionsExt}, path::PathBuf};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

#[derive(Clone)]
struct AppState {
    database: SqlitePool,
    security_database: Option<Arc<SqlitePool>>,
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct SystemInfo {
    app_version: &'static str,
    os: &'static str,
    architecture: &'static str,
}

#[derive(Serialize)]
struct InstructionCapability {
    capability_id: &'static str,
    instruction_action: &'static str,
    app: &'static str,
    schema_version: &'static str,
    input_schema_ref: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_result: Option<serde_json::Value>,
    available: bool,
    required_scopes: Vec<&'static str>,
    channels: Vec<&'static str>,
    risk: &'static str,
    confirmation: &'static str,
    verification: &'static str,
    prerequisites: Vec<&'static str>,
    unavailable_reason: Option<&'static str>,
}

#[derive(Serialize)]
struct CapabilityCatalog {
    protocol_version: &'static str,
    capabilities: Vec<InstructionCapability>,
}

async fn health(State(state): State<AppState>) -> Result<Json<Health>, (StatusCode, &'static str)> {
    database::ping(&state.database)
        .await
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"))?;
    Ok(Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    }))
}

async fn system_info() -> Json<SystemInfo> {
    Json(SystemInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
    })
}

// Discovery sends no screen content and never follows a redirect or a proxy.
// It is advisory: dispatch does not depend on this endpoint supporting /v1/models.
async fn local_vision_model_available() -> bool {
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(1))
        .build() {
        Ok(client) => client,
        Err(_) => return false,
    };
    let mut response = match client.get("http://127.0.0.1/v1/models").send().await {
        Ok(response) if response.status().is_success() => response,
        _ => return false,
    };
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len().saturating_add(chunk.len()) <= 64 * 1024 => body.extend_from_slice(&chunk),
            Ok(Some(_)) | Err(_) => return false,
            Ok(None) => break,
        }
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body) else { return false; };
    value.get("data").and_then(|data| data.as_array()).is_some_and(|models| {
        models.iter().any(|model| model.get("id").and_then(|id| id.as_str()) == Some("wevisone-vl-4b"))
    })
}

async fn capabilities(State(state): State<AppState>) -> Json<CapabilityCatalog> {
    let launch = initial_capabilities().into_iter().find(|c| c.id == "wecom.client.launch");
    let available = launch.as_ref().is_some_and(|c| matches!(c.status, CapabilityStatus::Available));
    let reason = launch.as_ref().and_then(|c| c.reason);
    let center = center_capability();
    let authorization = match state.security_database.as_deref() {
        Some(database) => center_authorized(database).await.ok(),
        None => None,
    };
    let center_available = matches!(center.status, CapabilityStatus::Available) && authorization == Some(true);
    let center_reason = match authorization {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("Window centering is not explicitly authorized"),
        Some(true) => center.reason,
    };
    let capture = app_window_capture_capability();
    let capture_grant = match state.security_database.as_deref() {
        Some(database) => app_window_capture_authorized(database).await.ok(),
        None => None,
    };
    let capture_available = matches!(capture.status, CapabilityStatus::Available) && capture_grant == Some(true);
    let capture_reason = match capture_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("App window capture is not explicitly authorized"),
        Some(true) => capture.reason,
    };
    // The same native window/permission prerequisites are used by capture-record
    // loading, but conversion does not require a capture grant or request permission.
    let conversion_grant = match state.security_database.as_deref() {
        Some(database) => app_window_conversion_any_grant(database).await.ok(),
        None => None,
    };
    let conversion_available = matches!(capture.status, CapabilityStatus::Available) && conversion_grant == Some(true);
    let conversion_reason = match conversion_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No exact app window coordinate conversion grant exists"),
        Some(true) => capture.reason,
    };
    // Discovery is advisory: it does not inspect a caller's point or target.
    let activation = app_element_activation_capability();
    let activation_grant = match state.security_database.as_deref() {
        Some(database) => app_element_activation_any_grant(database).await.ok(),
        None => None,
    };
    let activation_available = matches!(activation.status, CapabilityStatus::Available) && activation_grant == Some(true);
    let activation_reason = match activation_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No one-shot app element activation grant exists"),
        Some(true) => activation.reason,
    };
    let focus = app_element_focus_capability();
    let focus_grant = match state.security_database.as_deref() {
        Some(database) => app_element_focus_any_grant(database).await.ok(),
        None => None,
    };
    let focus_available = matches!(focus.status, CapabilityStatus::Available) && focus_grant == Some(true);
    let focus_reason = match focus_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No one-shot app element focus grant exists"),
        Some(true) => focus.reason,
    };
    let diagnosis_grant = match state.security_database.as_deref() {
        Some(database) => app_element_diagnosis_any_grant(database).await.ok(),
        None => None,
    };
    let diagnosis_available = matches!(activation.status, CapabilityStatus::Available) && diagnosis_grant == Some(true);
    let diagnosis_reason = match diagnosis_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No exact app element diagnosis grant exists"),
        Some(true) => activation.reason,
    };
    let element_input = app_element_input_capability();
    let element_input_grant = match state.security_database.as_deref() {
        Some(database) => app_element_input_any_grant(database).await.ok(),
        None => None,
    };
    let element_input_available = matches!(element_input.status, CapabilityStatus::Available) && element_input_grant == Some(true);
    let element_input_reason = match element_input_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No one-shot exact app element input grant exists"),
        Some(true) => element_input.reason,
    };
    let key_input = keyboard_key_input_capability();
    let key_input_grant = match state.security_database.as_deref() {
        Some(database) => app_key_input_any_grant(database).await.ok(),
        None => None,
    };
    let key_input_available = matches!(key_input.status, CapabilityStatus::Available) && key_input_grant == Some(true);
    let key_input_reason = match key_input_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No one-shot exact key input grant exists"),
        Some(true) => key_input.reason,
    };
    let nickname_send = send_message_by_nickname_capability();
    let nickname_send_grant = match state.security_database.as_deref() {
        Some(database) => send_message_by_nickname_any_grant(database).await.ok(),
        None => None,
    };
    let nickname_native_and_grant = matches!(nickname_send.status, CapabilityStatus::Available) && nickname_send_grant == Some(true);
    // A parent grant delegates its bounded children: independent standalone
    // capture/vision/input grants are neither required nor reused here.
    // Discovery cannot predict live-validated coordinate hits. Vision is required
    // only when execution actually needs uncached localization.
    let local_model_available = capture_available && local_vision_model_available().await;
    let nickname_send_available = nickname_native_and_grant;
    let nickname_send_reason = match nickname_send_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No one-shot WeCom send-by-nickname parent grant exists"),
        Some(true) if !matches!(nickname_send.status, CapabilityStatus::Available) => nickname_send.reason,
        Some(true) => None,
    };
    let vision_model_available = capture_available && local_model_available;
    let vision_reason = if !capture_available {
        capture_reason
    } else if !vision_model_available {
        Some("Local vision model availability could not be confirmed via /v1/models; dispatch may still be attempted")
    } else {
        None
    };
    let entries = [
        ("process_launch", "wecom.client.launch", "app.wecom.launch", available, reason),
        ("window_center", "wecom.window.center", "app.wecom.window.manage", center_available, center_reason),
        ("window_capture", "desktop.window.capture", "desktop.window.capture", capture_available, capture_reason),
        ("vision_detect", "vision.window.locate", "vision.window.locate", vision_model_available, vision_reason),
        ("screenshot_coordinates_convert", "desktop.window.coordinates.convert", "desktop.window.coordinates.convert", conversion_available, conversion_reason),
        ("click_button", "wecom.element.activate", "wecom.element.activate", activation_available, activation_reason),
        ("click_focus", "desktop.element.focus", "desktop.element.focus", focus_available, focus_reason),
        ("ax_inspect", "wecom.element.diagnose", "wecom.element.diagnose", diagnosis_available, diagnosis_reason),
        ("keyboard_text_input", "desktop.element.input", "desktop.element.input", element_input_available, element_input_reason),
        ("keyboard_key_input", "desktop.keyboard.key_input", "desktop.keyboard.key_input", key_input_available, key_input_reason),
        ("send_message_by_nickname", "wecom.message.send_by_nickname", "wecom.message.send_by_nickname", nickname_send_available, nickname_send_reason),
    ];
    Json(CapabilityCatalog {
        protocol_version: "2",
        capabilities: entries.into_iter().map(|(action, id, scope, available, reason)| {
            if action == "send_message_by_nickname" {
                return InstructionCapability {
                    capability_id: id,
                    instruction_action: action,
                    app: "wecom",
                    schema_version: "1",
                    input_schema_ref: "https://wecom-use.local/schemas/task-instruction-v2.json",
                    input_schema: Some(serde_json::json!({
                        "type": "object", "additionalProperties": false, "required": ["action", "params"],
                        "properties": {
                            "action": { "const": "send_message_by_nickname" },
                            "params": {
                                "type": "object", "additionalProperties": false, "required": ["nickname", "messages", "interval_ms"],
                                "properties": {
                                    "nickname": { "type": "string", "minLength": 1, "maxLength": 256, "allOf": [{ "pattern": "\\S" }, { "not": { "pattern": "[\\u0000-\\u001f\\u007f-\\u009f]" } }], "description": "Exact nonblank control-free nickname, 1..256 UTF-8 bytes; preserve all original values" },
                                    "messages": {
                                        "type": "array", "minItems": 1, "maxItems": 100,
                                        "items": {
                                            "type": "object", "additionalProperties": false, "required": ["type", "content"],
                                            "properties": {
                                                "type": { "const": "text" },
                                                "content": { "type": "string", "minLength": 1, "maxLength": 256, "allOf": [{ "pattern": "\\S" }, { "not": { "pattern": "[\\u0000-\\u001f\\u007f-\\u009f]" } }], "description": "Exact nonblank control-free text, 1..256 UTF-8 bytes" }
                                            }
                                        }
                                    },
                                    "interval_ms": { "type": "integer", "minimum": 0, "maximum": 60000, "description": "Wait only between completed sends; (messages.length - 1) * interval_ms <= 600000" }
                                }
                            }
                        }
                    })),
                    expected_result: Some(serde_json::json!({
                        "outcome": "completed",
                        "outcomes": [{ "index": 0, "submitted": true, "message_observed": true }],
                        "completed_count": 1,
                        "unknown_index": null,
                        "progress": { "phase": "completed", "operation": "parent_audit", "message_index": null, "status": "completed", "failure": null, "contact_diagnostic": null },
                        "message_states": [{ "index": 0, "state": "observed" }],
                        "cache_hits": { "message_navigation": false, "search": false, "composer": false, "contact_result": false }
                    })),
                    available,
                    required_scopes: vec![scope],
                    channels: vec!["gui", "local_vision"],
                    risk: "High-risk external-message transmission: launch and center, navigate/search a unique exact nickname and send each authorized text once in order; stop on any failure or unknown effect and never resend",
                    confirmation: "Separate explicit single-use parent wecom.message.send_by_nickname grant binds app wecom, route macos_gui, target exact_nickname_conversation, effect send_text_messages and fixed parameters instruction:send_message_by_nickname:v1 independent of all call parameters; content-bound digest grants cannot authorize this instruction and are not converted or automatically regranted; send-by-nickname-v1-sha256 over compact UTF-8 serde struct fields nickname,messages,interval_ms in that order is immutable request audit/delegation/revocation identity only; atomically check and consume with intent before any launch; recheck exact immutable current-request digest/lease and app/route/action restrictions before each child without per-child approval, ambient or minted broad child grants",
                    verification: "Mandatory process_launch then window_center and real measurement before phase-specific private SQLite navigation/search/contact/composer lookup; stable geometry keys exclude PID/window identities, which must pass live guards; true cache_hits flags require live-validated reuse, not candidate matches; every screenshot is the whole WeCom window including anchored child windows; exact-nickname contact candidates require a fresh ContactCapture, unique ordinary-contact AX identity and original-binding checks before dispatch, with vision/conversion only for missing or stale points; ordered submitted/message_observed outcomes retain completed_count/cache_hits/unknown_index on stopped unknown TaskResponse; progress retains enclosing phase, fixed operation, nullable message_index, completed|stopped status and null|authorization|audit|operation|postcondition failure; required nullable contact_diagnostic covers contact capture/readiness, local inference, conversion prechecks and exact_contact failures, null outside contact failures and retained across final audit failure; {dispatch:not_dispatched|dispatched|unknown,code:fixed sanitized identifier} describes only contact click, never prior parent effects; proven pre-click failures including readiness timeout remain not_dispatched, never permit retry or change parent unknown; unexpected errors use contact_operation_failed, with not_dispatched for read-only pre-click operations and unknown only for possible contact-click dispatch; codes are contact_target_unavailable,input_monitoring_unavailable,input_preparation_failed,contact_target_changed,window_unavailable,window_changed,focus_lost,opacity_changed,window_list_unavailable,search_popup_still_visible,input_route_unconfirmed,conversation_header_unconfirmed,input_monitoring_lost,contact_operation_failed,main_window_unavailable,main_window_not_frontmost,search_focus_unavailable,search_geometry_unavailable,search_anchor_unavailable,search_query_mismatch,popup_unavailable,popup_tree_unavailable,contact_result_unavailable,contact_cell_unavailable,contact_cell_window_mismatch,contact_cell_geometry_unavailable,contact_hit_test_unavailable,contact_hit_mismatch,contact_point_outside,contact_capture_unavailable,contact_capture_changed,contact_capture_record_unavailable; timeout retains latest concrete typed reason; no raw OS/AX errors, nickname, text or coordinates; message_states covers every requested message as not_started|preparing|submission_unknown|observed; local evidence is not delivery/read receipt and independent verification remains not_performed; never blindly retry",
                    prerequisites: vec![
                        "macOS, Accessibility/Input Monitoring/Screen Recording permissions and supported WeCom GUI",
                        "ScreenCaptureKit whole app-window capture APIs (macOS 14.2+), exact window ID/PID selection, child windows kept so the anchored popup is inside the app-window image, no shadows/cursor, actual point/pixel scale, at most 15 seconds asynchronous screenshot wait; API availability is read-only",
                        "Explicitly absent/empty AXTitle and AXValue are nonmatching placeholders, not contact/section/BOT identity; nonempty titles remain authoritative, invalid types and AX read failures fail closed",
                        "Usable authorization/audit storage and a matching independent one-shot parent grant; standalone child grants do not substitute",
                        "Always process_launch then window_center before cache lookup, including already running/centered clients",
                        "SQLite stores one main-window-relative logical point per global row and one popup-relative logical candidate per instruction_content row; keys bind app/route/control/layout version, display bounds, main logical size and actual capture pixels, plus exact-nickname digest and popup dimensions/offset for contact candidates; no durable PID/window identity and no plaintext nickname/messages",
                        "Existing same-user private security storage; unique index/UPSERT, 192 global and 1024 content rows, least-recently-successfully-validated eviction, 60-second validation touch throttling, no second memory cache; cache-only failures may degrade to real localization but never bypass authorization/audit or replay input",
                        "Every screenshot is the whole WeCom app window, including the anchored search-result popup as a child window; image dimensions, window bounds and coordinate conversion use the whole main-window frame, never a popup image/crop, screen region or full-screen fallback; private binding rechecks live main identity/geometry, popup identity/geometry, exact query and unique exact ordinary contact before inference/conversion/click",
                        "Native contact activation requires original ContactCapture binding; both initial and final live target checks compare main and popup identity/geometry to the original screenshot before click dispatch, not just to the current target",
                        "Contact readiness is read-only and bounded to at most 2 seconds with latest typed failure retained, no input replay or broad desktop grants; the contact-result capture uses the existing whole-window capture/conversion capabilities with purpose contact_result, no public helper and no separate popup image",
                        "The fresh contact-result whole-window capture never replaces main measurement; all four validated hits retain two captures and zero vision/conversion calls, fully cold successful flow reuses measurement for four captures and four vision/conversion calls; these counts are not real-send or performance evidence",
                        "Fixed contact diagnostic codes include contact_result_ambiguous, contact_result_bot, contact_section_unavailable; ambiguous exact results, bot results or an unconfirmed ordinary-contact section fail before contact click",
                        "Cache state is unknown during discovery; local vision is required only when any missing or stale coordinate, including contact_result, needs localization; unavailable required vision stops execution, without promising hits",
                        "Unique exact ordinary-contact nickname result and live AX identity, focus/editor, ordered key routing and newly appended message evidence",
                        "Only 1..100 text messages, 1..256 UTF-8 bytes each; interval 0..60000ms with total delay <=600000ms; standalone exact WeCom instruction only",
                    ],
                    unavailable_reason: if available { None } else { Some(reason.unwrap_or("Capability unavailable")) },
                };
            }
            InstructionCapability {
            capability_id: id,
            instruction_action: action,
            app: "wecom",
            schema_version: "1",
            input_schema_ref: "https://wecom-use.local/schemas/task-instruction-v2.json",
            input_schema: if action == "keyboard_key_input" { Some(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["action", "params"],
                "properties": {
                    "action": { "const": "keyboard_key_input" },
                    "params": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["key", "modifiers"],
                        "properties": {
                            "key": { "const": "return", "description": "Only Return is exposed; no other key or shortcut" },
                            "modifiers": { "type": "array", "maxItems": 0, "items": { "type": "string" }, "description": "Must be empty; modified keystrokes are not supported" }
                        }
                    }
                }
            })) } else if action == "keyboard_text_input" { Some(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["action", "params"],
                "properties": {
                    "action": { "const": action },
                    "params": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["point", "text"],
                        "properties": {
                            "point": { "type": "array", "minItems": 2, "maxItems": 2, "items": { "type": "number" }, "description": "Finite, exactly f32-representable global macOS top-left logical screen points" },
                            "text": { "type": "string", "description": "1..256 UTF-8 bytes, nonblank and no control characters including CR/LF; preserved exactly without trimming" }
                        }
                    }
                }
            })) } else if action == "click_button" || action == "click_focus" || action == "ax_inspect" { Some(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["action", "params"],
                "properties": {
                    "action": { "const": action },
                    "params": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["point", "expected_role"],
                        "properties": {
                            "point": { "type": "array", "minItems": 2, "maxItems": 2, "items": { "type": "number" }, "description": "Finite global macOS top-left logical screen points inside the frontmost unobstructed WeCom window" },
                            "expected_role": { "type": "string", "description": "1..256 bytes; exact live AX role at the point" },
                            "expected_label": { "type": ["string", "null"], "description": "Optional exact AX label; null or omitted selects coordinate-and-role identity without reading a name" }
                        }
                    }
                }
            })) } else { None },
            expected_result: if action == "keyboard_text_input" { Some(serde_json::json!({ "text_input": true })) } else if action == "keyboard_key_input" { Some(serde_json::json!({ "pressed": true })) } else if action == "click_focus" { Some(serde_json::json!({ "focused": true })) } else { None },
            available,
            required_scopes: if action == "vision_detect" { vec!["desktop.window.capture", scope] } else { vec![scope] },
            channels: if action == "vision_detect" { vec!["gui", "local_vision"] } else { vec!["gui"] },
            risk: if action == "keyboard_text_input" { "Replaces the complete value of the selected editable field, potentially including a draft; no Enter or message send" } else if action == "keyboard_key_input" { "Dispatches exactly one preallocated zero-modifier Return key-down/up pair in the current focus context, which may trigger an application action such as sending; no text, focus change, shortcut, other key or retry" } else if action == "ax_inspect" { "Reads the live accessibility element at the specified point without activating it; coordinate-only mode does not establish message identity" } else if action == "click_button" { "Clicks the specified WeCom element and may cause an external effect; coordinate-only mode does not establish message identity" } else if action == "click_focus" { "Clicks the specified WeCom element once and shifts keyboard focus to it, which may change subsequent input destination; coordinate-only mode does not establish message identity" } else if action == "vision_detect" { "Captures sensitive WeCom window content and discloses it to the local vision service" } else if action == "screenshot_coordinates_convert" { "Reads capture-time WeCom window geometry; coordinates are snapshots, not authorization to act" } else if action == "window_center" { "Moves an existing WeCom window without resizing" } else if action == "window_capture" { "Captures sensitive WeCom window content to a private local file" } else { "No risk policy is implemented" },
            confirmation: if action == "keyboard_text_input" { "Separate one-shot desktop.element.input grant bound to exact point and text with element-input-v1-sha256; search, key and draft grants do not authorize this action" } else if action == "keyboard_key_input" { "Separate explicit one-shot local desktop.keyboard.key_input grant for route macos_gui, target main_or_single_app_window, effect key_input and key-input-v1-sha256: over compact UTF-8 JSON in key,modifiers order; text-input, click, focus and draft-send grants cannot authorize this action; the model and catalog cannot authorize a request" } else if action == "ax_inspect" { "Separate explicit local grant for the exact point, role and optional label, including null, required; coordinate-only mode requires independent caller screenshot review and approval of the exact point/role; the model and catalog cannot authorize a request" } else if action == "click_button" { "Explicit one-shot local grant for the exact point, role and optional label, including null, required; coordinate-only mode requires independent caller screenshot review and approval of the exact point/role; the model and catalog cannot authorize a request" } else if action == "click_focus" { "Separate explicit one-shot local desktop.element.focus grant for the exact point, role and optional label, including null, with focus-v1-sha256; click and input grants cannot authorize this action; the model and catalog cannot authorize a request" } else if action == "vision_detect" { "Separate explicit local capture and exact object-list vision grants required" } else if action == "screenshot_coordinates_convert" { "Explicit local grant for the exact capture ID and boxes required; catalog availability does not authorize a particular request" } else if action == "window_center" || action == "window_capture" { "Explicit local grant required" } else { "No confirmation policy is implemented" },
            verification: if action == "keyboard_text_input" { "Revalidates the stable target and focused field and reads back the exact value; adapter evidence is not independent verification" } else if action == "keyboard_key_input" { "Requires a stable unique frontmost opaque window and passive per-process observation of this request's exact correlated Return key-down then key-up pair (matching tag, PID, keycode and zero flags) within 500ms; success means only that the pair was routed, not that the application processed it; verification remains not_performed and any failure after posting is unknown without retry" } else if action == "ax_inspect" { "Reports whether the read-only preflight matched at inspection time; does not authorize later activation or establish message identity in coordinate-only mode" } else if action == "click_button" { "Requires a stable target and either an actual AXFocused/AXSelected false-to-true transition or passive per-process observation of this click's exact correlated mouse-down then mouse-up pair (matching tag, PID, point, left button, zero flags and click count one); unchanged focus alone is insufficient; unavailable tap falls back only to transition evidence; no retries, extra focus writes or relaxed window/overlay guards; routing is not app semantic processing or independent verification, so verification remains not_performed; coordinate-only mode does not establish message identity" } else if action == "click_focus" { "Requires a stable target, the correlated mouse-down/up pair routed to the target process, and the application's AXFocusedUIElement exactly matching the clicked element with AXFocused true within 500ms; focus is observed, never written through AX; no retry or second click; this is local UI evidence, so verification remains not_performed" } else { "Independent verification is not implemented" },
            prerequisites: if action == "keyboard_text_input" { vec!["macOS and Accessibility permission", "Unique frontmost WeCom main window and live editable text field at the exact point", "Readable and writable value and writable focus", "Separate one-shot exact point and text grant"] } else if action == "keyboard_key_input" { vec!["macOS", "Accessibility permission and Input Monitoring for passive key routing evidence", "Exactly one running WeCom process with a main or sole frontmost opaque window", "Focus is already on the intended target; key input never sets or changes focus", "Exactly one preallocated zero-modifier Return pair; no other key or modifier", "Separate one-shot exact key-input grant; catalog availability is advisory"] } else if action == "ax_inspect" { vec!["macOS", "Accessibility permission", "Exactly one running WeCom process with a main or sole window", "Global macOS top-left screen logical point within the frontmost unobstructed WeCom window and a matching live accessibility element", "Explicit exact inspection grant"] } else if action == "click_button" { vec!["macOS", "Accessibility permission", "Exactly one running WeCom process with a main or sole window", "Global macOS top-left screen logical point within the frontmost unobstructed WeCom window and a matching live accessibility element", "Explicit one-shot exact click grant"] } else if action == "click_focus" { vec!["macOS", "Accessibility permission and Input Monitoring for passive click routing evidence", "Exactly one running WeCom process with a main or sole window", "Global macOS top-left screen logical point within the frontmost unobstructed WeCom window and a matching live accessibility element", "Target must accept keyboard focus and report itself through AXFocusedUIElement", "Explicit one-shot exact focus grant"] } else if action == "screenshot_coordinates_convert" { vec!["macOS", "A recent recorded app window capture with unchanged target and geometry", "Accessibility permission", "Screen recording permission already granted", "Explicit exact conversion grant"] } else if action == "vision_detect" { vec!["macOS", "Exactly one running WeCom process with a main or sole window", "Accessibility permission", "Screen recording permission", "Explicit app window capture grant", "Explicit vision.window.locate grant for the exact object list", "Local vision service at http://127.0.0.1/v1/chat/completions with model wevisone-vl-4b"] } else if action == "process_launch" { vec!["macOS", "Installed WeCom desktop application"] } else if action == "window_center" { vec!["macOS", "Running WeCom with a controllable window", "Accessibility permission", "Explicit local grant"] } else if action == "window_capture" { vec!["macOS", "Exactly one running WeCom process with a main or sole window", "Accessibility permission", "Screen recording permission", "Explicit app window capture grant"] } else { vec![] },
            unavailable_reason: if available { None } else { Some(reason.unwrap_or("Capability unavailable")) },
            }
        }).collect(),
    })
}

async fn execute_ipc_task(request: TaskRequest, database: Option<&SqlitePool>) -> IpcResponse {
    let input = match request.into_task_input() {
        Ok(input) => input,
        Err(error) => {
            let code = if error == crate::kernel::task::TaskRequestError::UnsupportedVersion {
                TaskErrorCode::UnsupportedVersion
            } else {
                TaskErrorCode::InvalidRequest
            };
            return ipc_task_error(code, "Invalid or unsupported task request.");
        }
    };
    if let Err(error) = input.preflight() {
        return match error {
            TaskPreflightError::InvalidRequest => ipc_task_error(
                TaskErrorCode::InvalidRequest,
                "The natural-language task is ambiguous or unsupported.",
            ),
            TaskPreflightError::CapabilityUnavailable => ipc_task_error(
                TaskErrorCode::CapabilityUnavailable,
                "The requested instruction sequence is not currently executable.",
            ),
        };
    }
    let mode = input.mode();
    let app = input.app().to_owned();
    let instruction = input.resolve_instruction().expect("preflight resolves supported input");
    let request_id = uuid::Uuid::new_v4().to_string();
    match invoke_task(database, &request_id, input, launch_wecom, center_wecom_window).await {
        Ok((_, result)) => {
            let output = if let Some(output) = result {
                output
            } else if matches!(instruction, crate::kernel::instruction::TaskInstruction::Atomic(crate::kernel::instruction::AtomicInstruction::ProcessLaunch)) {
                serde_json::json!({ "open_command_succeeded": true })
            } else {
                serde_json::json!({ "window_position_submitted": true })
            };
            IpcResponse::Task(TaskResponse::completed(request_id, &app, mode, instruction, output))
        },
        Err(InvokeError::OutcomeUnknown) => IpcResponse::Task(TaskResponse::unknown(request_id, &app, mode, instruction)),
        Err(error) => {
            ipc_invoke_error(error, request_id)
        }
    }
}

fn ipc_invoke_error(error: InvokeError, request_id: String) -> IpcResponse {
    let (_, Json(response)) = super::rpa::invoke_error_response(error, request_id);
    IpcResponse::Error(response)
}

fn ipc_task_error(code: TaskErrorCode, message: &'static str) -> IpcResponse {
    ipc_task_error_with_id(code, message, uuid::Uuid::new_v4().to_string())
}

fn ipc_task_error_with_id(code: TaskErrorCode, message: &'static str, request_id: String) -> IpcResponse {
    let trace_id = uuid::Uuid::new_v4().to_string();
    IpcResponse::Error(TaskErrorResponse::new(TaskError::new(
        code,
        message.to_owned(),
        request_id,
        trace_id,
    )))
}

async fn run_ipc(path: PathBuf, database: SqlitePool, security_database: Option<Arc<SqlitePool>>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    if tokio::fs::try_exists(&path).await? {
        match tokio::net::UnixStream::connect(&path).await {
            Ok(_) => anyhow::bail!("IPC socket is already in use: {}", path.display()),
            Err(_) => tokio::fs::remove_file(&path).await?,
        }
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let socket_owner = std::fs::metadata(&path)?.uid();
    tracing::info!(path = %path.display(), "IPC listening");
    loop {
        let (mut stream, _) = listener.accept().await?;
        if stream.peer_cred().map_or(true, |peer| peer.uid() != socket_owner) {
            tracing::warn!("rejecting IPC connection with unverified peer identity");
            continue;
        }
        let database = database.clone();
        let security_database = security_database.clone();
        tokio::spawn(async move {
            let read = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let length = stream.read_u32().await? as usize;
                if length == 0 || length > ipc::MAX_FRAME_BYTES {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid IPC frame size"));
                }
                let mut data = vec![0; length];
                stream.read_exact(&mut data).await?;
                Ok::<_, std::io::Error>(data)
            }).await;
            let data = match read {
                Ok(Ok(data)) => data,
                Ok(Err(error)) => {
                    tracing::debug!(%error, "IPC frame failed");
                    return;
                }
                Err(_) => return,
            };
            let response = match serde_json::from_slice::<IpcRequest>(&data) {
                Ok(IpcRequest::Status { method: ipc::StatusMethod::Status }) => IpcResponse::Status {
                    available: database::ping(&database).await.is_ok(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                },
                Ok(IpcRequest::Rpa(request)) => execute_ipc_task(request, security_database.as_deref()).await,
                Err(_) => ipc_task_error(TaskErrorCode::InvalidRequest, "Invalid IPC request."),
            };
            let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let data = ipc::encode(&response).map_err(std::io::Error::other)?;
                stream.write_u32(data.len() as u32).await?;
                stream.write_all(&data).await
            }).await;
            if let Ok(Err(error)) = result {
                tracing::debug!(%error, "IPC frame failed");
            }
        });
    }
}
fn build_router(state: AppState, credential: Option<Arc<super::local_auth::LocalCredential>>) -> Router {
    let security_database = state.security_database.clone();
    Router::new()
        .route("/api/v2/health", get(health))
        .route("/api/v2/system/info", get(system_info))
        .route("/api/v2/capabilities", get(capabilities))
        .merge(super::rpa::routes().with_state::<AppState>(()))
        .with_state(state)
        .layer(axum::Extension(security_database))
        .layer(axum::Extension(credential))
}

pub async fn serve() -> anyhow::Result<()> {
    init();
    let config = RuntimeConfig::from_env()?;
    let database = database::open(&config.database_url).await?;
    let security_database = match database::open_security(&config.security_database_path).await {
        Ok(pool) => Some(Arc::new(pool)),
        Err(_) => {
            tracing::warn!("private authorization/audit storage unavailable; window centering is disabled");
            None
        }
    };
    let credential = match super::local_auth::LocalCredential::load_or_create() {
        Ok(value) => Some(Arc::new(value)),
        Err(_) => {
            tracing::warn!("local credential store unavailable; HTTP task requests are disabled");
            None
        }
    };
    let ipc_path = config.socket_path.clone();
    let ipc_database = database.clone();
    let ipc_security_database = security_database.clone();
    let ipc_task = tokio::spawn(async move { run_ipc(ipc_path, ipc_database, ipc_security_database).await });
    let app = build_router(AppState { database, security_database }, credential);
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    tracing::info!(addr = %config.bind, "runtime listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    ipc_task.abort();
    let _ = tokio::fs::remove_file(config.socket_path).await;
    Ok(())
}

#[cfg(test)]
mod diagnostic_transport_tests {
    use super::*;
    use crate::kernel::task::ActivationDiagnostic;

    #[test]
    fn http_and_ipc_serialize_identical_sanitized_preflight_errors() {
        let codes = [
            ActivationDiagnostic::AccessibilityUnavailable,
            ActivationDiagnostic::TargetWindowUnavailable,
            ActivationDiagnostic::CoordinatePrecisionUnsupported,
            ActivationDiagnostic::PointOutsideTargetWindow,
            ActivationDiagnostic::TargetNotFrontmost,
            ActivationDiagnostic::PointObscured,
            ActivationDiagnostic::HitTestUnavailable,
            ActivationDiagnostic::TargetElementMismatch,
            ActivationDiagnostic::AccessibilityRoleUnavailable,
            ActivationDiagnostic::AccessibilityRoleMismatch,
            ActivationDiagnostic::AccessibilityTitleUnavailable,
            ActivationDiagnostic::AccessibilityTitleMismatch,
            ActivationDiagnostic::TargetChanged,
            ActivationDiagnostic::PreflightUnavailable,
        ];
        for code in codes {
            let error = InvokeError::ActivationPreflight(code);
            let (status, Json(http)) = super::super::rpa::invoke_error_response(error, "request".into());
            let ipc = ipc_invoke_error(error, "request".into());
            let http = serde_json::to_value(http).unwrap();
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(http, serde_json::to_value(ipc).unwrap());
            assert_eq!(http["error"]["code"], "capability_unavailable");
            assert_eq!(http["error"]["diagnostic_code"], code.as_str());
            assert_eq!(http["error"]["phase"], "validation");
            assert_eq!(http["error"]["outcome"], "not_dispatched");
            assert_eq!(http["error"]["retryable"], false);
            assert_eq!(http["error"]["request_id"], "request");
        }
        let (_, Json(other)) = super::super::rpa::invoke_error_response(InvokeError::CapabilityUnavailable, "request".into());
        assert!(serde_json::to_value(other).unwrap()["error"].get("diagnostic_code").is_none());
    }

    #[tokio::test]
    async fn catalog_contains_exactly_the_public_instruction_set() {
        let db = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        let Json(catalog) = capabilities(State(AppState { database: db, security_database: None })).await;
        let actual: std::collections::BTreeSet<_> = catalog.capabilities.iter().map(|entry| entry.instruction_action).collect();
        let expected = ["process_launch", "window_center", "window_capture", "click_button", "click_focus", "ax_inspect", "keyboard_text_input", "keyboard_key_input", "vision_detect", "screenshot_coordinates_convert", "send_message_by_nickname"].into_iter().collect();
        assert_eq!(actual, expected);
        assert_eq!(catalog.capabilities.len(), 11);
    }
}

#[cfg(test)]
mod protocol_transport_tests {
    use super::*;

    #[tokio::test]
    async fn production_router_exposes_only_version_two_and_authenticates_execution() {
        let database = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1).connect("sqlite::memory:").await.unwrap();
        let app = build_router(AppState { database, security_database: None }, None);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5)).build().unwrap();
        let base = format!("http://{address}");

        for path in ["health", "system/info"] {
            let response = client.get(format!("{base}/api/v2/{path}")).send().await.unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK, "{path}");
        }
        let response = client.get(format!("{base}/api/v2/capabilities")).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let catalog: serde_json::Value = response.json().await.unwrap();
        assert_eq!(catalog["protocol_version"], "2");
        let actions: Vec<_> = catalog["capabilities"].as_array().unwrap().iter()
            .map(|entry| entry["instruction_action"].as_str().unwrap()).collect();
        for action in [
            "process_launch", "window_center", "window_capture", "ax_inspect",
            "click_button", "click_focus", "keyboard_text_input", "keyboard_key_input", "vision_detect", "screenshot_coordinates_convert",
        ] {
            assert!(actions.contains(&action), "{action}");
        }
        let response = client.post(format!("{base}/api/v2/rpa")).json(&serde_json::json!({
            "protocol_version":"2", "app":"wecom", "instructions":[{"action":"process_launch"}],
        })).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
        let error: serde_json::Value = response.json().await.unwrap();
        assert_eq!(error["error"]["code"], "unauthorized");
        assert_eq!(error["error"]["outcome"], "not_dispatched");

        for path in ["health", "system/info", "capabilities"] {
            let response = client.get(format!("{base}/api/v1/{path}")).send().await.unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND, "{path}");
        }
        let response = client.post(format!("{base}/api/v1/rpa")).json(&serde_json::json!({
            "protocol_version":"1", "app":"wecom", "instructions":[{"action":"launch_app"}],
        })).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn ipc_rejects_protocol_one_before_runtime_dispatch() {
        let request: TaskRequest = serde_json::from_value(serde_json::json!({
            "protocol_version":"1", "app":"wecom", "instructions":[{"action":"process_launch"}],
        })).unwrap();
        let response = serde_json::to_value(execute_ipc_task(request, None).await).unwrap();
        assert_eq!(response["error"]["code"], "unsupported_version");
        assert_eq!(response["error"]["outcome"], "not_dispatched");
        assert_eq!(response["error"]["retryable"], false);
    }
}

#[cfg(test)]
mod nickname_send_catalog_tests {
    use super::*;

    #[tokio::test]
    async fn nickname_catalog_rejects_wrong_scope_and_legacy_content_bound_grants() {
        let db = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE local_capability_grants (app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT)").execute(&db).await.unwrap();
        let state = AppState { database: db.clone(), security_database: Some(Arc::new(db.clone())) };
        let Json(catalog) = capabilities(State(state.clone())).await;
        let entry = catalog.capabilities.iter().find(|entry| entry.instruction_action == "send_message_by_nickname").unwrap();
        assert!(!entry.available);
        assert_eq!(entry.capability_id, "wecom.message.send_by_nickname");
        assert_eq!(entry.required_scopes, ["wecom.message.send_by_nickname"]);
        assert_eq!(entry.channels, ["gui", "local_vision"]);
        assert_eq!(entry.unavailable_reason, Some("No one-shot WeCom send-by-nickname parent grant exists"));
        let params = &entry.input_schema.as_ref().unwrap()["properties"]["params"];
        assert_eq!(params["required"], serde_json::json!(["nickname", "messages", "interval_ms"]));
        assert_eq!(params["additionalProperties"], false);
        assert_eq!(params["properties"]["messages"]["items"]["additionalProperties"], false);
        assert_eq!(params["properties"]["messages"]["items"]["properties"]["type"]["const"], "text");
        assert_eq!(
            entry.expected_result.as_ref().unwrap()["cache_hits"],
            serde_json::json!({
                "message_navigation": false, "search": false, "composer": false,
                "contact_result": false,
            }),
        );
        let valid_marker = "instruction:send_message_by_nickname:v1".to_owned();
        for (app, capability, route, target, effect, parameters) in [
            ("wecom", "desktop.keyboard.key_input", "macos_gui", "main_or_single_app_window", "key_input", valid_marker.clone()),
            ("other", "wecom.message.send_by_nickname", "macos_gui", "exact_nickname_conversation", "send_text_messages", valid_marker.clone()),
            ("wecom", "wecom.message.send_by_nickname", "other", "exact_nickname_conversation", "send_text_messages", valid_marker.clone()),
            ("wecom", "wecom.message.send_by_nickname", "macos_gui", "other", "send_text_messages", valid_marker.clone()),
            ("wecom", "wecom.message.send_by_nickname", "macos_gui", "exact_nickname_conversation", "other", valid_marker),
            ("wecom", "wecom.message.send_by_nickname", "macos_gui", "exact_nickname_conversation", "send_text_messages", format!("send-by-nickname-v1-sha256:{}", "a".repeat(64))),
            ("wecom", "wecom.message.send_by_nickname", "macos_gui", "exact_nickname_conversation", "send_text_messages", format!("send-by-nickname-v1-sha256:{}", "A".repeat(64))),
            ("wecom", "wecom.message.send_by_nickname", "macos_gui", "exact_nickname_conversation", "send_text_messages", format!("send-by-nickname-v1-sha256:{}", "a".repeat(63))),
        ] {
            sqlx::query("INSERT INTO local_capability_grants VALUES (?, ?, ?, ?, ?, ?)").bind(app).bind(capability).bind(route).bind(target).bind(effect).bind(parameters).execute(&db).await.unwrap();
            let Json(catalog) = capabilities(State(state.clone())).await;
            let entry = catalog.capabilities.iter().find(|entry| entry.instruction_action == "send_message_by_nickname").unwrap();
            assert!(!entry.available);
            assert_eq!(entry.unavailable_reason, Some("No one-shot WeCom send-by-nickname parent grant exists"));
        }
        let Json(catalog) = capabilities(State(AppState { database: db, security_database: None })).await;
        let entry = catalog.capabilities.iter().find(|entry| entry.instruction_action == "send_message_by_nickname").unwrap();
        assert!(!entry.available);
        assert_eq!(entry.unavailable_reason, Some("Authorization storage is unavailable"));
    }
}
