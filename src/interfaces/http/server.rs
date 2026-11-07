use crate::applications::wecom::{app_element_activation_capability, app_window_capture_capability, center_capability, center_wecom_window, launch_wecom, wecom_search_query_capability};
use crate::interfaces::ipc::{self, IpcRequest, IpcResponse};
use crate::kernel::task::{
    TaskError, TaskErrorCode, TaskErrorResponse, TaskPreflightError, TaskRequest, TaskResponse,
};
use crate::runtime::{invoke_task, app_element_activation_any_grant, app_element_diagnosis_any_grant, app_window_capture_authorized, app_window_conversion_any_grant, center_authorized, wecom_search_query_any_grant, InvokeError};
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
    let search_query = wecom_search_query_capability();
    let search_query_grant = match state.security_database.as_deref() {
        Some(database) => wecom_search_query_any_grant(database).await.ok(),
        None => None,
    };
    let search_query_available = matches!(search_query.status, CapabilityStatus::Available) && search_query_grant == Some(true);
    let search_query_reason = match search_query_grant {
        None => Some("Authorization storage is unavailable"),
        Some(false) => Some("No one-shot exact WeCom search query grant exists"),
        Some(true) => search_query.reason,
    };
    let vision_model_available = capture_available && local_vision_model_available().await;
    let vision_reason = if !capture_available {
        capture_reason
    } else if !vision_model_available {
        Some("Local vision model availability could not be confirmed via /v1/models; dispatch may still be attempted")
    } else {
        None
    };
    let entries = [
        ("launch_app", "wecom.client.launch", "app.wecom.launch", available, reason),
        ("center_app_window", "wecom.window.center", "app.wecom.window.manage", center_available, center_reason),
        ("capture_app_window", "desktop.window.capture", "desktop.window.capture", capture_available, capture_reason),
        ("detection_app_window_objects", "vision.window.locate", "vision.window.locate", vision_model_available, vision_reason),
        ("convert_app_window_coordinates", "desktop.window.coordinates.convert", "desktop.window.coordinates.convert", conversion_available, conversion_reason),
        ("click_app_element", "wecom.element.activate", "wecom.element.activate", activation_available, activation_reason),
        ("inspect_app_element", "wecom.element.diagnose", "wecom.element.diagnose", diagnosis_available, diagnosis_reason),
        ("input_app_element", "wecom.search.query.set", "wecom.search.query.set", search_query_available, search_query_reason),
        ("search_contact", "wecom.contact.search", "wecom.contact.read", false, Some("Contact search instruction is not implemented")),
        ("open_chat", "wecom.chat.open", "wecom.chat.read", false, Some("Open chat instruction is not implemented")),
        ("send_text", "wecom.message.send", "wecom.message.send", false, Some("Send text instruction is not implemented")),
        ("verify_message_sent", "wecom.message.verify", "wecom.message.read", false, Some("Message verification instruction is not implemented")),
    ];
    Json(CapabilityCatalog {
        protocol_version: "1",
        capabilities: entries.into_iter().map(|(action, id, scope, available, reason)| InstructionCapability {
            capability_id: id,
            instruction_action: action,
            app: "wecom",
            schema_version: "1",
            input_schema_ref: "https://wecom-use.local/schemas/task-instruction-v1.json",
            input_schema: if action == "input_app_element" { Some(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["action", "params"],
                "properties": {
                    "action": { "const": "input_app_element" },
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
            })) } else { None },
            expected_result: if action == "input_app_element" { Some(serde_json::json!({ "text_input": true })) } else { None },
            available,
            required_scopes: if action == "detection_app_window_objects" { vec!["desktop.window.capture", scope] } else { vec![scope] },
            channels: if action == "detection_app_window_objects" { vec!["gui", "local_vision"] } else if action == "launch_app" || action == "center_app_window" || action == "capture_app_window" || action == "convert_app_window_coordinates" || action == "click_app_element" || action == "inspect_app_element" || action == "input_app_element" { vec!["gui"] } else { vec![] },
            risk: if action == "input_app_element" { "Inputs sensitive text into the controlled WeCom header search field through Accessibility; never presses Enter, edits a chat composer, sends a message, or emits mouse/keyboard events" } else if action == "inspect_app_element" { "Reads the live accessibility element at the specified point without activating it; coordinate-only mode does not establish message identity" } else if action == "click_app_element" { "Clicks the specified WeCom element and may cause an external effect; coordinate-only mode does not establish message identity" } else if action == "detection_app_window_objects" { "Captures sensitive WeCom window content and discloses it to the local vision service" } else if action == "convert_app_window_coordinates" { "Reads capture-time WeCom window geometry; coordinates are snapshots, not authorization to act" } else if action == "center_app_window" { "Moves an existing WeCom window without resizing" } else if action == "capture_app_window" { "Captures sensitive WeCom window content to a private local file" } else { "No risk policy is implemented" },
            confirmation: if action == "input_app_element" { "Separate explicit one-shot local wecom.search.query.set grant for the exact point and unmodified text digest required; capture/click grants are not reused; availability is advisory and cannot authorize any request" } else if action == "inspect_app_element" { "Separate explicit local grant for the exact point, role and optional label, including null, required; coordinate-only mode requires independent caller screenshot review and approval of the exact point/role; the model and catalog cannot authorize a request" } else if action == "click_app_element" { "Explicit one-shot local grant for the exact point, role and optional label, including null, required; coordinate-only mode requires independent caller screenshot review and approval of the exact point/role; the model and catalog cannot authorize a request" } else if action == "detection_app_window_objects" { "Separate explicit local capture and exact object-list vision grants required" } else if action == "convert_app_window_coordinates" { "Explicit local grant for the exact capture ID and boxes required; catalog availability does not authorize a particular request" } else if action == "center_app_window" || action == "capture_app_window" { "Explicit local grant required" } else { "No confirmation policy is implemented" },
            verification: if action == "input_app_element" { "Adapter checks exact AXValue readback within 500ms and unchanged active app/window; this is not independent verification, so task verification remains not_performed; any dispatched failure is unknown and never retried" } else if action == "inspect_app_element" { "Reports whether the read-only preflight matched at inspection time; does not authorize later activation or establish message identity in coordinate-only mode" } else if action == "click_app_element" { "The same target must have an actual AXFocused or AXSelected false-to-true transition; otherwise the outcome is unknown; coordinate-only mode does not establish message identity" } else { "Independent verification is not implemented" },
            prerequisites: if action == "input_app_element" { vec!["macOS", "Accessibility permission; no Screen Recording permission or screenshot required", "Exactly one running WeCom process with a main or sole frontmost unobstructed window", "Current WeCom GUI only: point hits the unique enabled AXTextField in the top 80 logical points of the window, height <=60 and positive width, parent AXSplitGroup, and a vertical AXSplitter separating it from every other header AXTextField; ambiguity is rejected", "Writable AXFocused and AXValue; exact application focused-element identity and live window/geometry revalidation", "Separate explicit one-shot exact search-field text grant; native and grant availability are advisory"] } else if action == "inspect_app_element" { vec!["macOS", "Accessibility permission", "Exactly one running WeCom process with a main or sole window", "Global macOS top-left screen logical point within the frontmost unobstructed WeCom window and a matching live accessibility element", "Explicit exact inspection grant"] } else if action == "click_app_element" { vec!["macOS", "Accessibility permission", "Exactly one running WeCom process with a main or sole window", "Global macOS top-left screen logical point within the frontmost unobstructed WeCom window and a matching live accessibility element", "Explicit one-shot exact click grant"] } else if action == "convert_app_window_coordinates" { vec!["macOS", "A recent recorded app window capture with unchanged target and geometry", "Accessibility permission", "Screen recording permission already granted", "Explicit exact conversion grant"] } else if action == "detection_app_window_objects" { vec!["macOS", "Exactly one running WeCom process with a main or sole window", "Accessibility permission", "Screen recording permission", "Explicit app window capture grant", "Explicit vision.window.locate grant for the exact object list", "Local vision service at http://127.0.0.1/v1/chat/completions with model wevisone-vl-4b"] } else if action == "launch_app" { vec!["macOS", "Installed WeCom desktop application"] } else if action == "center_app_window" { vec!["macOS", "Running WeCom with a controllable window", "Accessibility permission", "Explicit local grant"] } else if action == "capture_app_window" { vec!["macOS", "Exactly one running WeCom process with a main or sole window", "Accessibility permission", "Screen recording permission", "Explicit app window capture grant"] } else { vec![] },
            unavailable_reason: if available { None } else { Some(reason.unwrap_or("Capability unavailable")) },
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
            } else if matches!(instruction, crate::kernel::task::TaskInstruction::LaunchApp) {
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
    let app = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/system/info", get(system_info))
        .route("/api/v1/capabilities", get(capabilities))
        .merge(super::rpa::routes().with_state::<AppState>(()))
        .with_state(AppState { database, security_database: security_database.clone() })
        .layer(axum::Extension(security_database))
        .layer(axum::Extension(credential));
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
    async fn search_catalog_is_separately_scoped_and_explicitly_advisory() {
        let db = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE local_capability_grants (app TEXT, capability TEXT, route TEXT, target TEXT, effect TEXT, parameters TEXT)").execute(&db).await.unwrap();
        let state = AppState { database: db.clone(), security_database: Some(Arc::new(db.clone())) };
        let Json(catalog) = capabilities(State(state.clone())).await;
        let search = catalog.capabilities.iter().find(|entry| entry.instruction_action == "input_app_element").unwrap();
        assert!(!search.available);
        assert_eq!(search.unavailable_reason, Some("No one-shot exact WeCom search query grant exists"));
        assert_eq!(search.capability_id, "wecom.search.query.set");
        assert_eq!(search.required_scopes, ["wecom.search.query.set"]);
        assert_eq!(search.channels, ["gui"]);
        assert_eq!(search.expected_result, Some(serde_json::json!({ "text_input": true })));
        let schema = search.input_schema.as_ref().unwrap();
        assert_eq!(schema["properties"]["params"]["required"], serde_json::json!(["point", "text"]));
        assert_eq!(schema["properties"]["params"]["additionalProperties"], false);
        assert!(search.confirmation.contains("one-shot"));
        assert!(search.confirmation.contains("advisory"));
        assert!(search.verification.contains("not_performed"));
        assert!(search.prerequisites.iter().any(|requirement| requirement.contains("Current WeCom GUI only")));
        assert!(search.prerequisites.iter().any(|requirement| requirement.contains("no Screen Recording")));
        let digest = format!("search-query-sha256:{}", "a".repeat(64));
        sqlx::query("INSERT INTO local_capability_grants VALUES ('wecom', 'wecom.search.query.set', 'macos_gui', 'wecom_search_field', 'set_search_query', ?)").bind(digest).execute(&db).await.unwrap();
        let Json(catalog) = capabilities(State(state)).await;
        let search = catalog.capabilities.iter().find(|entry| entry.instruction_action == "input_app_element").unwrap();
        assert_ne!(search.unavailable_reason, Some("No one-shot exact WeCom search query grant exists"));
        let Json(catalog) = capabilities(State(AppState { database: db, security_database: None })).await;
        let search = catalog.capabilities.iter().find(|entry| entry.instruction_action == "input_app_element").unwrap();
        assert!(!search.available);
        assert_eq!(search.unavailable_reason, Some("Authorization storage is unavailable"));
    }
}
