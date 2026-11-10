use serde::{Deserialize, Serialize};

use crate::kernel::instruction::{ApplicationInstruction, AtomicInstruction, Message, PerceptionInstruction, TaskInstruction};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    NaturalLanguage,
    ExactInstruction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskInput {
    NaturalLanguage { app: String, messages: Vec<String> },
    ExactInstruction { app: String, steps: Vec<TaskInstruction> },
}
pub const WECOM_APP_ID: &str = "wecom";
pub const PROTOCOL_VERSION: &str = "2";
pub const MAX_INSTRUCTIONS: usize = 100;
pub const MAX_STRING_BYTES: usize = 64 * 1024;
pub const MAX_WINDOW_OBJECTS: usize = 16;
pub const MAX_WINDOW_OBJECT_BYTES: usize = 256;

fn valid_app_id(app: &str) -> bool {
    !app.is_empty()
        && app.len() <= 128
        && app.as_bytes()[0].is_ascii_lowercase()
        && app.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-'))
}

fn valid_window_objects(objects: &[String]) -> bool {
    !objects.is_empty()
        && objects.len() <= MAX_WINDOW_OBJECTS
        && objects.iter().all(|object| !object.trim().is_empty() && object.len() <= MAX_WINDOW_OBJECT_BYTES)
}

fn valid_conversion(capture_id: &str, boxes: &[[f64; 4]]) -> bool {
    uuid::Uuid::parse_str(capture_id)
        .is_ok_and(|id| id.hyphenated().to_string() == capture_id)
        && !boxes.is_empty()
        && boxes.len() <= 100
        && boxes.iter().all(|b| {
            b.iter().all(|v| v.is_finite())
                && b[0] >= 0.0 && b[1] >= 0.0 && b[0] < b[2] && b[1] < b[3]
        })
}

fn valid_activation(point: &[f64; 2], expected_role: &str, expected_label: &Option<String>) -> bool {
    point.iter().all(|value| value.is_finite())
        && !expected_role.trim().is_empty() && expected_role.len() <= 256
        && expected_label.as_ref().map_or(true, |label| !label.trim().is_empty() && label.len() <= 256)
}
pub fn valid_key_input(key: &str, modifiers: &[String]) -> bool {
    key == "return" && modifiers.is_empty()
}


pub fn valid_app_input(point: &[f64; 2], text: &str) -> bool {
    point.iter().all(|value| value.is_finite() && (*value as f32) as f64 == *value)
        && !text.is_empty()
        && text.len() <= 256
        && !text.trim().is_empty()
        && !text.chars().any(char::is_control)
}

pub(crate) fn valid_open_chat(contact: &str) -> bool {
    !contact.is_empty()
        && contact.len() <= 256
        && !contact.trim().is_empty()
        && !contact.chars().any(char::is_control)
}

pub(crate) fn valid_chat_draft(contact: &str, text: &str) -> bool {
    valid_open_chat(contact)
        && !text.is_empty()
        && text.len() <= 4096
        && !text.trim().is_empty()
        && !text.chars().any(char::is_control)
}

pub fn valid_send_message_by_nickname(nickname: &str, messages: &[Message], interval_ms: u64) -> bool {
    valid_open_chat(nickname)
        && !messages.is_empty()
        && messages.len() <= 100
        && messages.iter().all(|message| match message {
            Message::Text { content } => valid_app_input(&[0.0, 0.0], content),
        })
        && interval_ms <= 60_000
        && (messages.len().saturating_sub(1) as u64) * interval_ms <= 600_000
}


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRequest {
    pub protocol_version: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_instructions"
    )]
    pub instructions: Option<Vec<TaskInstruction>>,
}

fn deserialize_instructions<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<TaskInstruction>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Vec::<TaskInstruction>::deserialize(deserializer).map(Some)
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRequestError {
    UnsupportedVersion,
    InvalidApp,
    InvalidMessages,
    EmptyInstructions,
}

impl TaskRequest {
    pub fn into_task_input(self) -> Result<TaskInput, TaskRequestError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(TaskRequestError::UnsupportedVersion);
        }
        if self.messages.iter().any(|s| s.len() > MAX_STRING_BYTES)
            || self.instructions.as_ref().is_some_and(|steps| steps.len() > MAX_INSTRUCTIONS || steps.iter().any(|step| match step {
                TaskInstruction::Perception(PerceptionInstruction::VisionDetect { objects }) => !valid_window_objects(objects),
                TaskInstruction::Perception(PerceptionInstruction::ScreenshotCoordinatesConvert { capture_id, boxes }) => !valid_conversion(capture_id, boxes),
                TaskInstruction::Atomic(AtomicInstruction::ClickButton { point, expected_role, expected_label }) | TaskInstruction::Atomic(AtomicInstruction::ClickFocus { point, expected_role, expected_label }) | TaskInstruction::Atomic(AtomicInstruction::AxInspect { point, expected_role, expected_label }) => !valid_activation(point, expected_role, expected_label),
                TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point, text }) => !valid_app_input(point, text),
                TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key, modifiers }) => !valid_key_input(key, modifiers),
                TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { nickname, messages, interval_ms }) => !valid_send_message_by_nickname(nickname, messages, *interval_ms),
                _ => false,
            }))
        {
            return Err(TaskRequestError::InvalidMessages);
        }
        if !valid_app_id(&self.app) {
            return Err(TaskRequestError::InvalidApp);
        }
        if self.instructions.as_ref().is_some_and(|steps| steps.iter().any(|step| matches!(step, TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { .. }))))
            && (self.app != WECOM_APP_ID || self.instructions.as_ref().is_some_and(|steps| steps.len() != 1))
        {
            return Err(TaskRequestError::InvalidMessages);
        }
        match self.instructions {
            Some(instructions) if !instructions.is_empty() => Ok(TaskInput::ExactInstruction {
                app: self.app,
                steps: instructions,
            }),
            Some(_) => Err(TaskRequestError::EmptyInstructions),
            None if !self.messages.is_empty() && self.messages.iter().all(|message| !message.trim().is_empty()) => {
                Ok(TaskInput::NaturalLanguage { app: self.app, messages: self.messages })
            }
            None => Err(TaskRequestError::InvalidMessages),
        }
}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPreflightError {
    InvalidRequest,
    CapabilityUnavailable,
}

impl TaskInput {
    pub fn preflight(&self) -> Result<(), TaskPreflightError> {
        if !valid_app_id(self.app()) {
            return Err(TaskPreflightError::InvalidRequest);
        }
        match self {
            Self::ExactInstruction { steps, .. }
                if steps.iter().any(|step| matches!(step, TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { nickname, messages, interval_ms }) if !valid_send_message_by_nickname(nickname, messages, *interval_ms))) =>
            {
                Err(TaskPreflightError::InvalidRequest)
            }
            Self::ExactInstruction { app, steps }
                if matches!(steps.as_slice(), [TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { .. })]) =>
            {
                if app == WECOM_APP_ID { Ok(()) } else { Err(TaskPreflightError::CapabilityUnavailable) }
            }
            Self::ExactInstruction { steps, .. }
                if matches!(steps.as_slice(), [TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point, text })] if valid_app_input(point, text)) =>
            {
                Ok(())
            }
            Self::ExactInstruction { steps, .. }
                if steps.iter().any(|step| matches!(step, TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point, text }) if !valid_app_input(point, text))) =>
            {
                Err(TaskPreflightError::InvalidRequest)
            }
            Self::ExactInstruction { steps, .. }
                if matches!(steps.as_slice(), [TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key, modifiers })] if valid_key_input(key, modifiers)) =>
            {
                Ok(())
            }
            Self::ExactInstruction { steps, .. }
                if steps.iter().any(|step| matches!(step, TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key, modifiers }) if !valid_key_input(key, modifiers))) =>
            {
                Err(TaskPreflightError::InvalidRequest)
            }
            Self::ExactInstruction { steps, .. }
                if matches!(steps.as_slice(), [TaskInstruction::Atomic(AtomicInstruction::ClickButton { point, expected_role, expected_label }) | TaskInstruction::Atomic(AtomicInstruction::ClickFocus { point, expected_role, expected_label }) | TaskInstruction::Atomic(AtomicInstruction::AxInspect { point, expected_role, expected_label })] if valid_activation(point, expected_role, expected_label)) =>
            {
                Ok(())
            }
            Self::ExactInstruction { steps, .. }
                if steps.iter().any(|step| matches!(step, TaskInstruction::Atomic(AtomicInstruction::ClickButton { point, expected_role, expected_label }) | TaskInstruction::Atomic(AtomicInstruction::ClickFocus { point, expected_role, expected_label }) | TaskInstruction::Atomic(AtomicInstruction::AxInspect { point, expected_role, expected_label }) if !valid_activation(point, expected_role, expected_label))) =>
            {
                Err(TaskPreflightError::InvalidRequest)
            }
            Self::ExactInstruction { steps, .. }
                if matches!(steps.as_slice(), [TaskInstruction::Perception(PerceptionInstruction::ScreenshotCoordinatesConvert { capture_id, boxes })] if valid_conversion(capture_id, boxes)) =>
            {
                Ok(())
            }
            Self::ExactInstruction { steps, .. }
                if steps.iter().any(|step| matches!(step, TaskInstruction::Perception(PerceptionInstruction::ScreenshotCoordinatesConvert { capture_id, boxes }) if !valid_conversion(capture_id, boxes))) =>
            {
                Err(TaskPreflightError::InvalidRequest)
            }
            Self::ExactInstruction { steps, .. }
                if matches!(steps.as_slice(), [TaskInstruction::Perception(PerceptionInstruction::VisionDetect { objects })] if valid_window_objects(objects)) =>
            {
                Ok(())
            }
            Self::ExactInstruction { steps, .. }
                if steps.iter().any(|step| matches!(step, TaskInstruction::Perception(PerceptionInstruction::VisionDetect { objects }) if !valid_window_objects(objects))) =>
            {
                Err(TaskPreflightError::InvalidRequest)
            }
            Self::ExactInstruction { steps, .. }
                if matches!(steps.as_slice(), [TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch) | TaskInstruction::Atomic(AtomicInstruction::WindowCenter) | TaskInstruction::Atomic(AtomicInstruction::WindowCapture)]) =>
            {
                Ok(())
            }
            Self::ExactInstruction { .. } => Err(TaskPreflightError::CapabilityUnavailable),
            Self::NaturalLanguage { .. } => self
                .resolve_instruction()
                .map(|_| ())
                .map_err(|_| TaskPreflightError::InvalidRequest),
        }
    }
}

impl TaskInput {
    pub fn app(&self) -> &str {
        match self {
            Self::NaturalLanguage { app, .. } | Self::ExactInstruction { app, .. } => app,
        }
    }

    pub fn mode(&self) -> TaskMode {
        match self {
            Self::NaturalLanguage { .. } => TaskMode::NaturalLanguage,
            Self::ExactInstruction { .. } => TaskMode::ExactInstruction,
        }
    }

    pub fn resolve_instruction(&self) -> Result<TaskInstruction, &'static str> {
        match self {
            Self::ExactInstruction { app, steps } => match steps.as_slice() {
                [instruction @ TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { nickname, messages, interval_ms })] if app == WECOM_APP_ID && valid_send_message_by_nickname(nickname, messages, *interval_ms) => Ok(instruction.clone()),
                [instruction @ TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point, text })] if valid_app_input(point, text) => Ok(instruction.clone()),
                [instruction @ TaskInstruction::Atomic(AtomicInstruction::KeyboardKeyInput { key, modifiers })] if valid_key_input(key, modifiers) => Ok(instruction.clone()),
                [instruction @ TaskInstruction::Atomic(AtomicInstruction::ClickButton { point, expected_role, expected_label })] if valid_activation(point, expected_role, expected_label) => Ok(instruction.clone()),
                [instruction @ TaskInstruction::Atomic(AtomicInstruction::ClickFocus { point, expected_role, expected_label })] if valid_activation(point, expected_role, expected_label) => Ok(instruction.clone()),
                [instruction @ TaskInstruction::Atomic(AtomicInstruction::AxInspect { point, expected_role, expected_label })] if valid_activation(point, expected_role, expected_label) => Ok(instruction.clone()),
                [instruction @ TaskInstruction::Perception(PerceptionInstruction::ScreenshotCoordinatesConvert { capture_id, boxes })] if valid_conversion(capture_id, boxes) => Ok(instruction.clone()),
                [instruction @ TaskInstruction::Perception(PerceptionInstruction::VisionDetect { objects })] if valid_window_objects(objects) => Ok(instruction.clone()),
                [instruction @ (TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch) | TaskInstruction::Atomic(AtomicInstruction::WindowCenter) | TaskInstruction::Atomic(AtomicInstruction::WindowCapture))] => Ok(instruction.clone()),
                _ => Err("unsupported exact task instruction sequence"),
            },
            Self::NaturalLanguage { app, messages }
                if app == WECOM_APP_ID && matches!(
                    messages.as_slice(),
                    [message] if matches!(
                        message.trim(),
                        "打开企业微信" | "唤起企业微信" | "唤醒企业微信" | "请打开企业微信" | "请唤起企业微信" | "请唤醒企业微信"
                    )
                ) =>
            {
                Ok(TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch))
            }
            Self::NaturalLanguage { app, messages }
                if app == WECOM_APP_ID && matches!(messages.as_slice(), [message] if matches!(
                    message.trim(),
                    "将企业微信窗口居中" | "企业微信窗口居中" | "将企微窗口居中" | "企微窗口居中"
                )) =>
            {
                Ok(TaskInstruction::Atomic(AtomicInstruction::WindowCenter))
            }
            Self::NaturalLanguage { app, messages }
                if app == WECOM_APP_ID && matches!(messages.as_slice(), [message] if message.trim() == "截取企业微信窗口") =>
            {
                Ok(TaskInstruction::Atomic(AtomicInstruction::WindowCapture))
            }
            Self::NaturalLanguage { .. } => Err("ambiguous or unsupported natural-language task"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Created,
    Validated,
    Planned,
    AwaitingApproval,
    Running,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    NotPerformed,
    Verified,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskErrorCode {
    UnsupportedVersion,
    InvalidRequest,
    Unauthorized,
    Forbidden,
    ApprovalRequired,
    NotFound,
    Conflict,
    CapabilityUnavailable,
    AmbiguousTarget,
    OperationFailed,
    OutcomeUnknown,
    RateLimited,
    Timeout,
    TransientFailure,
    VerificationFailed,
    CheckpointNotFound,
    CheckpointExpired,
    InvalidCheckpoint,
    ResumeConflict,
    RollbackUnavailable,
    RollbackFailed,
    RetryLimitExceeded,
    IdempotencyConflict,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationDiagnostic {
    AccessibilityUnavailable,
    TargetWindowUnavailable,
    CoordinatePrecisionUnsupported,
    PointOutsideTargetWindow,
    TargetNotFrontmost,
    PointObscured,
    HitTestUnavailable,
    TargetElementMismatch,
    AccessibilityRoleUnavailable,
    AccessibilityRoleMismatch,
    AccessibilityTitleUnavailable,
    AccessibilityTitleMismatch,
    TargetChanged,
    PreflightUnavailable,
}

impl ActivationDiagnostic {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AccessibilityUnavailable => "accessibility_unavailable",
            Self::TargetWindowUnavailable => "target_window_unavailable",
            Self::CoordinatePrecisionUnsupported => "coordinate_precision_unsupported",
            Self::PointOutsideTargetWindow => "point_outside_target_window",
            Self::TargetNotFrontmost => "target_not_frontmost",
            Self::PointObscured => "point_obscured",
            Self::HitTestUnavailable => "hit_test_unavailable",
            Self::TargetElementMismatch => "target_element_mismatch",
            Self::AccessibilityRoleUnavailable => "accessibility_role_unavailable",
            Self::AccessibilityRoleMismatch => "accessibility_role_mismatch",
            Self::AccessibilityTitleUnavailable => "accessibility_title_unavailable",
            Self::AccessibilityTitleMismatch => "accessibility_title_mismatch",
            Self::TargetChanged => "target_changed",
            Self::PreflightUnavailable => "preflight_unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskError {
    pub code: TaskErrorCode,
    pub message: String,
    pub request_id: String,
    pub task_id: Option<String>,
    pub operation_id: Option<String>,
    pub phase: String,
    pub outcome: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic_code: Option<ActivationDiagnostic>,
}

impl TaskError {
    pub fn new(code: TaskErrorCode, message: String, request_id: String, _trace_id: String) -> Self {
        let dispatched = matches!(code, TaskErrorCode::OperationFailed | TaskErrorCode::OutcomeUnknown | TaskErrorCode::Timeout);
        Self {
            code,
            message,
            request_id,
            task_id: None,
            operation_id: None,
            phase: if dispatched { "dispatch" } else if matches!(code, TaskErrorCode::Unauthorized | TaskErrorCode::Forbidden) { "authorization" } else { "validation" }.to_owned(),
            outcome: if dispatched { "unknown" } else { "not_dispatched" }.to_owned(),
            retryable: false,
            diagnostic_code: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskErrorResponse {
    pub error: TaskError,
}

impl TaskErrorResponse {
    pub fn new(error: TaskError) -> Self {
        Self { error }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub status: VerificationStatus,
    pub evidence_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStepExecution {
    pub step_id: String,
    pub sequence: u32,
    pub attempt: u32,
    pub instruction: TaskInstruction,
    pub status: ExecutionStatus,
    pub verification: Verification,
    pub output: Option<serde_json::Value>,
    pub error: Option<TaskError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskExecution {
    pub request_id: String,
    pub task_id: String,
    pub operation_id: String,
    pub trace_id: String,
    pub mode: TaskMode,
    pub status: ExecutionStatus,
    pub steps: Vec<TaskStepExecution>,
}

impl TaskExecution {
    pub fn completed(
        request_id: String,
        mode: TaskMode,
        instruction: TaskInstruction,
        output: serde_json::Value,
    ) -> Self {
        Self {
            request_id,
            task_id: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            mode,
            status: ExecutionStatus::Completed,
            steps: vec![TaskStepExecution {
                step_id: uuid::Uuid::new_v4().to_string(),
                sequence: 0,
                attempt: 1,
                instruction,
                status: ExecutionStatus::Completed,
                verification: Verification { status: VerificationStatus::NotPerformed, evidence_ref: None },
                output: Some(output),
                error: None,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResponse {
    #[serde(deserialize_with = "deserialize_protocol_version")]
    pub protocol_version: String,
    pub app: String,
    pub execution: TaskExecution,
}

fn deserialize_protocol_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = String::deserialize(deserializer)?;
    if version != PROTOCOL_VERSION {
        return Err(serde::de::Error::custom("unsupported protocol version"));
    }
    Ok(version)
}

impl TaskResponse {
    pub fn unknown(request_id: String, app: &str, mode: TaskMode, instruction: TaskInstruction) -> Self {
        let mut response = Self::completed(request_id, app, mode, instruction, serde_json::Value::Null);
        response.execution.status = ExecutionStatus::Unknown;
        let mut error = TaskError::new(
            TaskErrorCode::OutcomeUnknown,
            "The dispatched operation could not be confirmed.".to_owned(),
            response.execution.request_id.clone(),
            response.execution.trace_id.clone(),
        );
        error.task_id = Some(response.execution.task_id.clone());
        error.operation_id = Some(response.execution.operation_id.clone());
        let step = &mut response.execution.steps[0];
        step.status = ExecutionStatus::Unknown;
        step.verification.status = VerificationStatus::Unknown;
        step.output = None;
        step.error = Some(error);
        response
    }

    pub fn unknown_with_output(
        request_id: String,
        app: &str,
        mode: TaskMode,
        instruction: TaskInstruction,
        output: serde_json::Value,
    ) -> Self {
        let mut response = Self::unknown(request_id, app, mode, instruction);
        let step = &mut response.execution.steps[0];
        step.output = Some(output);
        step.verification.status = VerificationStatus::NotPerformed;
        response
    }

    pub fn completed(
        request_id: String,
        app: &str,
        mode: TaskMode,
        instruction: TaskInstruction,
        output: serde_json::Value,
    ) -> Self {
        if matches!(&instruction, TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname { .. }))
            && output.get("outcome").and_then(serde_json::Value::as_str) == Some("unknown")
        {
            return Self::unknown_with_output(request_id, app, mode, instruction, output);
        }
        Self {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            app: app.to_owned(),
            execution: TaskExecution::completed(request_id, mode, instruction, output),
        }
    }
}

#[cfg(test)]
mod vision_detect_tests {
    use super::*;

    #[test]
    fn exact_instruction_validates_and_resolves() {
        let request: TaskRequest = serde_json::from_str(
            r#"{"protocol_version":"2","app":"wecom","instructions":[{"action":"vision_detect","params":{"objects":["按钮","search"]}}]}"#,
        ).unwrap();
        let input = request.into_task_input().unwrap();
        assert_eq!(input.preflight(), Ok(()));
        assert_eq!(input.resolve_instruction().unwrap(), TaskInstruction::Perception(PerceptionInstruction::VisionDetect {
            objects: vec!["按钮".into(), "search".into()],
        }));
        assert!(serde_json::from_str::<TaskRequest>(
            r#"{"protocol_version":"2","app":"wecom","instructions":[{"action":"vision_detect","params":{"objects":["x"],"extra":true}}]}"#,
        ).is_err());
    }

    #[test]
    fn window_capture_has_no_legacy_aliases() {
        assert_eq!(
            serde_json::from_str::<TaskInstruction>(r#"{"action":"window_capture"}"#).unwrap(),
            TaskInstruction::Atomic(AtomicInstruction::WindowCapture)
        );
        assert!(serde_json::from_str::<TaskInstruction>(r#"{"action":"capture_screen"}"#).is_err());
        assert!(serde_json::from_str::<TaskInstruction>(r#"{"action":"locate_app_window_objects","params":{"objects":["x"]}}"#).is_err());
        assert_eq!(
            TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["截取企业微信窗口".into()] }.resolve_instruction(),
            Ok(TaskInstruction::Atomic(AtomicInstruction::WindowCapture))
        );
        assert!(TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["截取屏幕".into()] }.resolve_instruction().is_err());
    }

    #[test]
    fn rejects_invalid_object_bounds_in_request_and_preflight() {
        for objects in [vec![], vec!["  ".into()], vec!["x".repeat(257)], vec!["x".into(); 17], vec!["界".repeat(86)]] {
            let step = TaskInstruction::Perception(PerceptionInstruction::VisionDetect { objects });
            let request = TaskRequest {
                protocol_version: "2".into(), app: "wecom".into(),
                messages: vec![], instructions: Some(vec![step.clone()]),
            };
            assert_eq!(request.into_task_input(), Err(TaskRequestError::InvalidMessages));
            assert_eq!(TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![step] }.preflight(), Err(TaskPreflightError::InvalidRequest));
        }
        let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![TaskInstruction::Perception(PerceptionInstruction::VisionDetect {
            objects: vec!["x".repeat(256); 16],
        })] };
        assert_eq!(input.preflight(), Ok(()));
    }
}

#[cfg(test)]
mod app_element_tests {
    use super::*;

    #[test]
    fn optional_label_explicitly_selects_identity_mode() {
        for action in ["click_button", "click_focus", "ax_inspect"] {
            for label in [None, Some(serde_json::Value::Null), Some(serde_json::json!("Search"))] {
                let mut params = serde_json::json!({"point": [100.0, 200.0], "expected_role": "AXCell"});
                if let Some(label) = label {
                    params["expected_label"] = label;
                }
                let step: TaskInstruction = serde_json::from_value(serde_json::json!({
                    "action": action, "params": params,
                })).unwrap();
                let expected_label = params.get("expected_label").and_then(|label| label.as_str());
                assert_eq!(
                    serde_json::to_value(&step).unwrap()["params"]["expected_label"],
                    serde_json::json!(expected_label)
                );
                let request = TaskRequest {
                    protocol_version: "2".into(), app: "wecom".into(),
                    messages: vec![], instructions: Some(vec![step.clone()]),
                };
                let input = request.into_task_input().unwrap();
                assert_eq!(input.preflight(), Ok(()));
                assert_eq!(input.resolve_instruction(), Ok(step));
            }
        }
    }

    #[test]
    fn rejects_present_blank_or_oversized_labels() {
        for action in ["click_button", "click_focus", "ax_inspect"] {
            for label in ["".to_owned(), " \t\n".to_owned(), "x".repeat(257), "界".repeat(86)] {
                let step: TaskInstruction = serde_json::from_value(serde_json::json!({
                    "action": action,
                    "params": {"point": [100.0, 200.0], "expected_role": "AXCell", "expected_label": label},
                })).unwrap();
                let request = TaskRequest {
                    protocol_version: "2".into(), app: "wecom".into(),
                    messages: vec![], instructions: Some(vec![step.clone()]),
                };
                assert_eq!(request.into_task_input(), Err(TaskRequestError::InvalidMessages));
                let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![step] };
                assert_eq!(input.preflight(), Err(TaskPreflightError::InvalidRequest));
                assert!(input.resolve_instruction().is_err());
            }
        }
    }
}

#[cfg(test)]
mod app_target_tests {
    use super::*;
    #[test]
    fn key_input_accepts_only_return_without_modifiers() {
        for (key, modifiers, valid) in [
            ("return", vec![], true),
            ("enter", vec![], false),
            ("Return", vec![], false),
            ("return", vec!["command".to_owned()], false),
            ("", vec![], false),
        ] {
            let step: TaskInstruction = serde_json::from_value(serde_json::json!({
                "action": "keyboard_key_input",
                "params": {"key": key, "modifiers": modifiers},
            })).unwrap();
            let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![step.clone()] };
            assert_eq!(input.preflight().is_ok(), valid, "{key} {modifiers:?}");
            assert_eq!(input.resolve_instruction().is_ok(), valid, "{key} {modifiers:?}");
            let request = TaskRequest { protocol_version: PROTOCOL_VERSION.into(), app: "wecom".into(), messages: vec![], instructions: Some(vec![step]) };
            assert_eq!(request.into_task_input().is_ok(), valid, "{key} {modifiers:?}");
        }
    }
    #[test]
    fn atomic_text_input_limits_and_standalone_boundary() {
        let action = "keyboard_text_input";
            for (point, text, valid) in [
                ([1.0, 2.0], "界".repeat(85), true),
                ([1.0, 2.0], "界".repeat(86), false),
                ([0.1, 2.0], "Alice".to_owned(), false),
                ([1.0, 2.0], " \t".to_owned(), false),
            ] {
                let step: TaskInstruction = serde_json::from_value(serde_json::json!({
                    "action": action, "params": {"point": point, "text": text}
                })).unwrap();
                let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![step.clone()] };
                assert_eq!(input.preflight().is_ok(), valid);
                assert_eq!(input.resolve_instruction().is_ok(), valid);
                let request = TaskRequest { protocol_version: PROTOCOL_VERSION.into(), app: "wecom".into(), messages: vec![], instructions: Some(vec![step.clone()]) };
                assert_eq!(request.into_task_input().is_ok(), valid);
                if valid {
                    let sequence = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![step.clone(), step] };
                    assert_eq!(sequence.preflight(), Err(TaskPreflightError::CapabilityUnavailable));
                }
            }
    }


    #[test]
    fn generic_instructions_have_no_wecom_aliases_or_target_parameters() {
        for (action, instruction) in [
            ("process_launch", TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)),
            ("window_center", TaskInstruction::Atomic(AtomicInstruction::WindowCenter)),
            ("keyboard_text_input", TaskInstruction::Atomic(AtomicInstruction::KeyboardTextInput { point: [100.0, 200.0], text: "Alice".into() })),
        ] {
            let value = serde_json::to_value(&instruction).unwrap();
            assert_eq!(value["action"], action);
            assert_eq!(serde_json::from_value::<TaskInstruction>(value).unwrap(), instruction);
            let request = TaskRequest {
                protocol_version: PROTOCOL_VERSION.into(), app: "other.app-1".into(),
                messages: vec![], instructions: Some(vec![instruction.clone()]),
            };
            let input = request.into_task_input().unwrap();
            assert_eq!(input.app(), "other.app-1");
            assert_eq!(input.preflight(), Ok(()));
            assert_eq!(input.resolve_instruction(), Ok(instruction));
        }
        for action in ["launch_wecom", "center_wecom_window", "set_wecom_search_query", "locate_app_window_objects", "activate_app_element", "diagnose_app_element", "set_app_search_query", "keyboard_input", "mouse_click"] {
            assert!(serde_json::from_value::<TaskInstruction>(
                serde_json::json!({"action": action})
            ).is_err());
        }
        assert!(serde_json::from_value::<TaskInstruction>(serde_json::json!({
            "action": "keyboard_text_input",
            "params": {"app": "wecom", "point": [100.0, 200.0], "text": "Alice"}
        })).is_err());
    }

    #[test]
    fn app_ids_validate_syntax_without_claiming_adapter_support() {
        for app in ["a".to_owned(), "wecom".to_owned(), "other.app-1_2".to_owned(), "a".repeat(128)] {
            let input = TaskRequest {
                protocol_version: PROTOCOL_VERSION.into(), app: app.clone(),
                messages: vec![], instructions: Some(vec![TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)]),
            }.into_task_input().unwrap();
            assert_eq!(input.app(), app);
            assert_eq!(input.preflight(), Ok(()));
        }
        for app in ["".to_owned(), "Wecom".to_owned(), "1app".to_owned(), "_app".to_owned(),
            "app name".to_owned(), "app/name".to_owned(), "应用".to_owned(), "a".repeat(129)] {
            assert_eq!(TaskRequest {
                protocol_version: PROTOCOL_VERSION.into(), app: app.clone(),
                messages: vec![], instructions: Some(vec![TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)]),
            }.into_task_input(), Err(TaskRequestError::InvalidApp));
            assert_eq!(TaskInput::ExactInstruction {
                app, steps: vec![TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)],
            }.preflight(), Err(TaskPreflightError::InvalidRequest));
        }
    }

    #[test]
    fn deserialized_inputs_require_an_explicit_app() {
        for value in [
            serde_json::json!({"mode": "natural_language", "messages": ["打开企业微信"]}),
            serde_json::json!({"mode": "exact_instruction", "steps": [{"action": "process_launch"}]}),
        ] {
            assert!(serde_json::from_value::<TaskInput>(value.clone()).is_err());
            let mut targeted = value;
            targeted["app"] = serde_json::json!("other");
            assert_eq!(serde_json::from_value::<TaskInput>(targeted).unwrap().app(), "other");
        }
    }

    #[test]
    fn wecom_natural_language_is_scoped_to_wecom_target() {
        for (message, instruction) in [
            ("打开企业微信", TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch)),
            ("将企业微信窗口居中", TaskInstruction::Atomic(AtomicInstruction::WindowCenter)),
            ("截取企业微信窗口", TaskInstruction::Atomic(AtomicInstruction::WindowCapture)),
        ] {
            for app in ["wecom", "other"] {
                let input = TaskRequest {
                    protocol_version: PROTOCOL_VERSION.into(), app: app.into(),
                    messages: vec![message.into()], instructions: None,
                }.into_task_input().unwrap();
                assert_eq!(input.app(), app);
                if app == WECOM_APP_ID {
                    assert_eq!(input.resolve_instruction(), Ok(instruction.clone()));
                    assert_eq!(input.preflight(), Ok(()));
                } else {
                    assert!(input.resolve_instruction().is_err());
                    assert_eq!(input.preflight(), Err(TaskPreflightError::InvalidRequest));
                }
            }
        }
    }

    #[test]
    fn responses_preserve_the_requested_app() {
        let completed = TaskResponse::completed(
            "request".into(), "other", TaskMode::ExactInstruction,
            TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch), serde_json::Value::Null,
        );
        assert_eq!(completed.app, "other");
        assert_eq!(completed.execution.status, ExecutionStatus::Completed);
        let unknown = TaskResponse::unknown(
            "request".into(), "other", TaskMode::ExactInstruction, TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch),
        );
        assert_eq!(unknown.app, "other");
        assert_eq!(unknown.execution.status, ExecutionStatus::Unknown);
    }
}

#[cfg(test)]
mod protocol_version_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_require_string_version_two_before_dispatch() {
        for version in ["1", "", "02", "2.0", "3"] {
            let request: TaskRequest = serde_json::from_value(json!({
                "protocol_version": version, "app": "wecom",
                "instructions": [{"action":"process_launch"}],
            })).unwrap();
            assert_eq!(request.into_task_input(), Err(TaskRequestError::UnsupportedVersion));
        }
        for wire in [
            json!({"app":"wecom","instructions":[{"action":"process_launch"}]}),
            json!({"protocol_version":2,"app":"wecom","instructions":[{"action":"process_launch"}]}),
            json!({"protocol_version":null,"app":"wecom","instructions":[{"action":"process_launch"}]}),
        ] {
            assert!(serde_json::from_value::<TaskRequest>(wire).is_err());
        }
    }

    #[test]
    fn responses_and_ipc_clients_require_version_two() {
        let response = TaskResponse::completed(
            "request".into(), "wecom", TaskMode::ExactInstruction,
            TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch), serde_json::Value::Null,
        );
        let mut wire = serde_json::to_value(response).unwrap();
        assert_eq!(wire["protocol_version"], "2");
        assert!(serde_json::from_value::<TaskResponse>(wire.clone()).is_ok());
        assert!(serde_json::from_value::<crate::interfaces::ipc::IpcResponse>(wire.clone()).is_ok());
        wire["protocol_version"] = json!("1");
        assert!(serde_json::from_value::<TaskResponse>(wire.clone()).is_err());
        assert!(serde_json::from_value::<crate::interfaces::ipc::IpcResponse>(wire).is_err());
    }
}

#[cfg(test)]
mod send_message_by_nickname_tests {
    use super::*;
    use serde_json::json;

    fn step(nickname: &str, contents: &[&str], interval_ms: u64) -> TaskInstruction {
        TaskInstruction::Application(ApplicationInstruction::SendMessageByNickname {
            nickname: nickname.into(),
            messages: contents.iter().map(|content| Message::Text { content: (*content).into() }).collect(),
            interval_ms,
        })
    }

    fn request(app: &str, steps: Vec<TaskInstruction>) -> TaskRequest {
        TaskRequest { protocol_version: PROTOCOL_VERSION.into(), app: app.into(), messages: vec![], instructions: Some(steps) }
    }

    #[test]
    fn strict_typed_messages_preserve_the_flat_wire_contract() {
        let instruction = step("  张=三  ", &["  e\u{301} 😀 \"研发\" \\ =  ", "second"], 1000);
        let wire = serde_json::to_value(&instruction).unwrap();
        assert_eq!(wire, json!({
            "action": "send_message_by_nickname",
            "params": {
                "nickname": "  张=三  ",
                "messages": [{"type":"text","content":"  e\u{301} 😀 \"研发\" \\ =  "},{"type":"text","content":"second"}],
                "interval_ms": 1000
            }
        }));
        assert_eq!(serde_json::from_value::<TaskInstruction>(wire).unwrap(), instruction);
        for params in [
            json!({"nickname":"Alice","messages":[{"type":"text","content":"hi"}]}),
            json!({"nickname":"Alice","interval_ms":0}),
            json!({"messages":[{"type":"text","content":"hi"}],"interval_ms":0}),
            json!({"nickname":"Alice","messages":["hi"],"interval_ms":0}),
            json!({"nickname":"Alice","messages":[{"type":"image","content":"hi"}],"interval_ms":0}),
            json!({"nickname":"Alice","messages":[{"type":"text","content":"hi","extra":true}],"interval_ms":0}),
            json!({"nickname":"Alice","messages":[{"content":"hi"}],"interval_ms":0}),
            json!({"nickname":"Alice","messages":[{"type":"text"}],"interval_ms":0}),
            json!({"nickname":"Alice","messages":[{"type":"text","content":"hi"}],"interval_ms":-1}),
            json!({"nickname":"Alice","messages":[{"type":"text","content":"hi"}],"interval_ms":1.5}),
            json!({"nickname":"Alice","messages":[{"type":"text","content":"hi"}],"interval_ms":0,"send":true}),
        ] {
            assert!(serde_json::from_value::<TaskInstruction>(json!({"action":"send_message_by_nickname","params":params})).is_err());
        }
    }

    #[test]
    fn bounds_are_enforced_at_request_preflight_and_resolution() {
        let utf8_256 = format!("{}a", "界".repeat(85));
        for (nickname, contents, interval, valid) in [
            ("Alice".to_owned(), vec!["hi".to_owned()], 0, true),
            (utf8_256.clone(), vec![utf8_256], 60_000, true),
            ("Alice".to_owned(), vec!["hi".to_owned(); 100], 0, true),
            ("Alice".to_owned(), vec!["hi".to_owned(); 11], 60_000, true),
            ("Alice".to_owned(), vec!["hi".to_owned(); 12], 60_000, false),
            ("Alice".to_owned(), vec!["hi".to_owned()], 60_001, false),
            ("Alice".to_owned(), vec!["hi".to_owned()], u64::MAX, false),
            ("Alice".to_owned(), vec![], 0, false),
            ("Alice".to_owned(), vec!["hi".to_owned(); 101], 0, false),
            ("".to_owned(), vec!["hi".to_owned()], 0, false),
            (" \t".to_owned(), vec!["hi".to_owned()], 0, false),
            ("界".repeat(86), vec!["hi".to_owned()], 0, false),
            ("Alice\n".to_owned(), vec!["hi".to_owned()], 0, false),
            ("Alice".to_owned(), vec!["".to_owned()], 0, false),
            ("Alice".to_owned(), vec![" ".to_owned()], 0, false),
            ("Alice".to_owned(), vec!["界".repeat(86)], 0, false),
            ("Alice".to_owned(), vec!["hello\u{85}".to_owned()], 0, false),
        ] {
            let contents = contents.iter().map(String::as_str).collect::<Vec<_>>();
            let instruction = step(&nickname, &contents, interval);
            assert_eq!(request("wecom", vec![instruction.clone()]).into_task_input().is_ok(), valid);
            let input = TaskInput::ExactInstruction { app: "wecom".into(), steps: vec![instruction] };
            assert_eq!(input.preflight().is_ok(), valid);
            assert_eq!(input.resolve_instruction().is_ok(), valid);
        }
    }

    #[test]
    fn exact_wecom_sole_instruction_is_required_without_natural_language_expansion() {
        let instruction = step("Alice", &["hello"], 0);
        for (app, steps) in [
            ("other", vec![instruction.clone()]),
            ("wecom", vec![TaskInstruction::Atomic(AtomicInstruction::ProcessLaunch), instruction.clone()]),
            ("wecom", vec![instruction.clone(), instruction.clone()]),
        ] {
            assert!(request(app, steps.clone()).into_task_input().is_err());
            let input = TaskInput::ExactInstruction { app: app.into(), steps };
            assert!(input.preflight().is_err());
            assert!(input.resolve_instruction().is_err());
        }
        let input = request("wecom", vec![instruction.clone()]).into_task_input().unwrap();
        assert_eq!(input.preflight(), Ok(()));
        assert_eq!(input.resolve_instruction(), Ok(instruction));
        let input = TaskInput::NaturalLanguage { app: "wecom".into(), messages: vec!["给Alice发送hello".into()] };
        assert!(input.resolve_instruction().is_err());
        let outer: TaskRequest = serde_json::from_value(json!({"protocol_version":"2","app":"wecom","messages":["打开企业微信"]})).unwrap();
        assert_eq!(outer.messages, vec!["打开企业微信"]);
    }

    #[test]
    fn stopped_workflow_retains_partial_outcomes_in_shared_http_ipc_response() {
        let output = json!({
            "outcome":"unknown",
            "outcomes":[{"index":0,"submitted":true,"message_observed":true},{"index":1,"submitted":false,"message_observed":false}],
            "completed_count":1,"unknown_index":1,"cache_hits":{"message_navigation":false,"search":false,"composer":false,"contact_result":false}
        });
        let response = TaskResponse::completed("request".into(), "wecom", TaskMode::ExactInstruction, step("Alice", &["one", "two"], 0), output.clone());
        assert_eq!(response.execution.status, ExecutionStatus::Unknown);
        let execution = &response.execution.steps[0];
        assert_eq!(execution.status, ExecutionStatus::Unknown);
        assert_eq!(execution.output, Some(output));
        assert_eq!(execution.verification.status, VerificationStatus::NotPerformed);
        assert_eq!(execution.error.as_ref().unwrap().code, TaskErrorCode::OutcomeUnknown);
        assert!(!execution.error.as_ref().unwrap().retryable);
        let wire = serde_json::to_value(response).unwrap();
        assert!(serde_json::from_value::<crate::interfaces::ipc::IpcResponse>(wire).is_ok());
    }
}
