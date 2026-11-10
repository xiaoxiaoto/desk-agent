use crate::{
    applications::wecom::{center_wecom_window, launch_wecom},
    kernel::task::{
        TaskError, TaskErrorCode, TaskErrorResponse, TaskPreflightError, TaskRequest, TaskRequestError, TaskResponse,
    },
    runtime::{invoke_task, InvokeError},
};
use axum::{Extension, Json, Router, body::Bytes, extract::{DefaultBodyLimit, rejection::BytesRejection}, http::{HeaderMap, StatusCode, header::AUTHORIZATION}, routing::post};
use std::sync::Arc;
use super::local_auth::LocalCredential;

pub fn routes() -> Router {
    Router::new().route("/api/v2/rpa", post(handle_rpa)).layer(DefaultBodyLimit::max(1024 * 1024))
}

async fn handle_rpa(
    Extension(credential): Extension<Option<Arc<LocalCredential>>>,
    Extension(database): Extension<Option<Arc<sqlx::SqlitePool>>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<TaskResponse>, (StatusCode, Json<TaskErrorResponse>)> {
    let authenticated = headers.get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| credential.as_ref().is_some_and(|key| key.authenticate(token.as_bytes())));
    if !authenticated {
        return Err(rpa_error(StatusCode::UNAUTHORIZED, TaskErrorCode::Unauthorized, "Authentication required."));
    }
    let body = body.map_err(|_| rpa_error(StatusCode::BAD_REQUEST, TaskErrorCode::InvalidRequest, "Request body is invalid or exceeds the size limit."))?;
    let request: TaskRequest = serde_json::from_slice(&body).map_err(|_| {
        rpa_error(
            StatusCode::BAD_REQUEST,
            TaskErrorCode::InvalidRequest,
            "Request body does not match the RPA request schema.",
        )
    })?;
    let input = request.into_task_input().map_err(|error| {
        if error == TaskRequestError::UnsupportedVersion {
            rpa_error(StatusCode::BAD_REQUEST, TaskErrorCode::UnsupportedVersion, "Unsupported protocol version.")
        } else {
            rpa_error(
                StatusCode::BAD_REQUEST,
                TaskErrorCode::InvalidRequest,
                "The request must identify a valid app ID and provide messages or non-empty instructions.",
            )
        }
    })?;
    input.preflight().map_err(|error| match error {
        TaskPreflightError::InvalidRequest => rpa_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            TaskErrorCode::InvalidRequest,
            "The natural-language task is ambiguous or unsupported.",
        ),
        TaskPreflightError::CapabilityUnavailable => rpa_error(
            StatusCode::SERVICE_UNAVAILABLE,
            TaskErrorCode::CapabilityUnavailable,
            "The requested instruction sequence is not currently executable.",
        ),
    })?;
    let mode = input.mode();
    let app = input.app().to_owned();
    let request_id = uuid::Uuid::new_v4().to_string();
    let instruction = input.resolve_instruction().expect("preflight resolves supported input");
    match invoke_task(database.as_deref(), &request_id, input, launch_wecom, center_wecom_window).await {
        Ok((_, result)) => {
            let output = if let Some(output) = result {
                output
            } else if matches!(instruction, crate::kernel::instruction::TaskInstruction::Atomic(crate::kernel::instruction::AtomicInstruction::ProcessLaunch)) {
                serde_json::json!({ "open_command_succeeded": true })
            } else {
                serde_json::json!({ "window_position_submitted": true })
            };
            Ok(Json(TaskResponse::completed(request_id, &app, mode, instruction, output)))
        },
        Err(InvokeError::OutcomeUnknown) => Ok(Json(TaskResponse::unknown(request_id, &app, mode, instruction))),
        Err(error) => {
            Err(invoke_error_response(error, request_id))
        }
    }
}

pub(super) fn invoke_error(error: InvokeError) -> (StatusCode, TaskErrorCode, &'static str) {
    match error {
        InvokeError::Forbidden => (StatusCode::FORBIDDEN, TaskErrorCode::Forbidden, "The requested capability is not explicitly authorized."),
        InvokeError::AuditUnavailable => (StatusCode::SERVICE_UNAVAILABLE, TaskErrorCode::CapabilityUnavailable, "Authorization or audit storage is unavailable; operation not dispatched."),
        InvokeError::CapabilityUnavailable => (StatusCode::SERVICE_UNAVAILABLE, TaskErrorCode::CapabilityUnavailable, "The requested capability prerequisites are not met."),
        InvokeError::ActivationPreflight(_) => (StatusCode::SERVICE_UNAVAILABLE, TaskErrorCode::CapabilityUnavailable, "Accessibility target preflight prerequisites are not met; operation not dispatched."),
        InvokeError::InvalidInput => (StatusCode::UNPROCESSABLE_ENTITY, TaskErrorCode::InvalidRequest, "Task instruction is not executable."),
        InvokeError::OutcomeUnknown => (StatusCode::INTERNAL_SERVER_ERROR, TaskErrorCode::OutcomeUnknown, "Dispatch outcome is unknown."),
    }
}

pub(super) fn invoke_error_response(error: InvokeError, request_id: String) -> (StatusCode, Json<TaskErrorResponse>) {
    let (status, code, message) = invoke_error(error);
    let (status, Json(mut response)) = rpa_error_with_id(status, code, message, request_id);
    if let InvokeError::ActivationPreflight(diagnostic) = error {
        response.error.diagnostic_code = Some(diagnostic);
    }
    (status, Json(response))
}

fn rpa_error(
    status: StatusCode,
    code: TaskErrorCode,
    message: &'static str,
) -> (StatusCode, Json<TaskErrorResponse>) {
    rpa_error_with_id(status, code, message, uuid::Uuid::new_v4().to_string())
}

fn rpa_error_with_id(
    status: StatusCode,
    code: TaskErrorCode,
    message: &'static str,
    request_id: String,
) -> (StatusCode, Json<TaskErrorResponse>) {
    let trace_id = uuid::Uuid::new_v4().to_string();
    (
        status,
        Json(TaskErrorResponse::new(TaskError::new(
            code,
            message.to_owned(),
            request_id,
            trace_id,
        ))),
    )
}
