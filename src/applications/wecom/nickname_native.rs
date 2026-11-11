//! Native ports for the already-authorized nickname workflow.
//! These functions neither grant authority nor retain native AX references.
use super::AppWindowCapture;
use crate::kernel::task::{valid_app_input, valid_open_chat};
use anyhow::{Result, ensure};

/// Immutable, task-private attachment snapshot prepared and checked by Runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub path: std::path::PathBuf,
    pub filename: String,
    pub mime_type: String,
    pub is_image: bool,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Opaque evidence of one pasted attachment preview; never crosses tasks.
pub struct AttachmentSession {
    #[cfg(target_os = "macos")]
    draft: super::macos_desktop::AttachmentDraft,
}

/// Sanitized evidence about the contact click, not about earlier GUI effects.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct ContactError {
    pub dispatch: ContactDispatch,
    pub code: ContactCode,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactDispatch { NotDispatched, Dispatched, Unknown }

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContactCode {
    ContactTargetUnavailable, InputMonitoringUnavailable, InputPreparationFailed,
    ContactTargetChanged, WindowUnavailable, WindowChanged, FocusLost,
    OpacityChanged, WindowListUnavailable, SearchPopupStillVisible,
    InputRouteUnconfirmed, ConversationHeaderUnconfirmed, InputMonitoringLost,
    ContactOperationFailed,
    MainWindowUnavailable, MainWindowNotFrontmost, SearchFocusUnavailable,
    SearchGeometryUnavailable, SearchAnchorUnavailable, SearchQueryMismatch,
    PopupUnavailable, PopupTreeUnavailable, ContactResultUnavailable,
    ContactResultAmbiguous, ContactResultBot, ContactSectionUnavailable,
    ContactCellUnavailable, ContactCellWindowMismatch, ContactCellGeometryUnavailable,
    ContactHitTestUnavailable, ContactHitMismatch, ContactPointOutside,
    ContactCaptureUnavailable, ContactCaptureChanged, ContactCaptureRecordUnavailable,
}

impl std::fmt::Display for ContactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("contact operation failed")
    }
}
impl std::error::Error for ContactError {}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    Messages,
    Search,
    Composer,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub pid: i32,
    pub window_number: u32,
    pub display_id: u32,
    pub window_bounds: [f64; 4],
    pub display_bounds: [f64; 4],
    pub image_size: [u32; 2],
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ContactGeometry {
    pub main: Geometry,
    pub popup_bounds: [f64; 4],
}

/// The whole app-window image captured while the exact contact popup is live.
pub struct ContactCapture {
    pub capture: std::sync::Arc<AppWindowCapture>,
    #[cfg(target_os = "macos")]
    pub(super) binding: super::macos_desktop::ContactBinding,
}

pub fn capture_contact(nickname: &str) -> Result<ContactCapture> {
    if !valid_open_chat(nickname) {
        return Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactTargetUnavailable }.into());
    }
    #[cfg(target_os = "macos")]
    { super::macos_desktop::capture_contact(nickname) }
    #[cfg(not(target_os = "macos"))]
    { Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactCaptureUnavailable }.into()) }
}

pub fn load_contact_capture(capture: &ContactCapture, nickname: &str) -> Result<AppWindowCapture> {
    #[cfg(target_os = "macos")]
    { super::macos_desktop::load_contact_capture(capture, nickname) }
    #[cfg(not(target_os = "macos"))]
    { let _ = (capture, nickname); Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactCaptureUnavailable }.into()) }
}

/// Rechecks the original private capture and its live contact-popup binding.
pub fn contact_geometry(capture: &ContactCapture, nickname: &str) -> Result<ContactGeometry> {
    if !valid_open_chat(nickname) {
        return Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactTargetUnavailable }.into());
    }
    #[cfg(target_os = "macos")]
    { super::macos_desktop::nickname_contact_geometry(capture, nickname) }
    #[cfg(not(target_os = "macos"))]
    { let _ = capture; Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactCaptureUnavailable }.into()) }
}

/// Only an invalid or stale point is a miss; unavailable or changed evidence fails closed.
pub fn validate_contact(capture: &ContactCapture, point: [f64; 2], nickname: &str) -> Result<bool> {
    if !valid_open_chat(nickname) {
        return Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactTargetUnavailable }.into());
    }
    #[cfg(target_os = "macos")]
    { super::macos_desktop::nickname_validate_contact(capture, point, nickname) }
    #[cfg(not(target_os = "macos"))]
    { let _ = (capture, point); Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactCaptureUnavailable }.into()) }
}

/// Uses the actual authorized capture, not a guessed screen scale.
pub fn geometry(capture: &AppWindowCapture) -> Result<Geometry> {
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::nickname_geometry(capture)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = capture;
        anyhow::bail!("WeCom nickname workflow requires macOS")
    }
}

/// Read-only live hit, role, ancestry and business-target validation.
pub fn validate_control(control: Control, point: [f64; 2], nickname: &str) -> bool {
    if !valid_app_input(&point, "x") || !valid_open_chat(nickname) {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::nickname_validate_control(control, point, nickname).is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = control;
        false
    }
}

pub fn activate_messages(point: [f64; 2]) -> Result<()> {
    ensure!(valid_app_input(&point, "x"), "Invalid WeCom coordinate");
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::nickname_activate_messages(point)
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("WeCom nickname workflow requires macOS")
    }
}

pub fn input_search(point: [f64; 2], nickname: &str) -> Result<()> {
    ensure!(
        valid_app_input(&point, nickname) && valid_open_chat(nickname),
        "Invalid WeCom search"
    );
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::set_search_query(point, nickname)
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("WeCom nickname workflow requires macOS")
    }
}

/// Opens a live-validated vision or cached candidate under this request's capture binding.
pub fn open_contact(capture: &ContactCapture, point: [f64; 2], nickname: &str) -> Result<()> {
    if !valid_app_input(&point, "x") || !valid_open_chat(nickname) {
        return Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactTargetUnavailable }.into());
    }
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::nickname_open_contact(&capture.binding, point, nickname)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = capture;
        Err(ContactError { dispatch: ContactDispatch::NotDispatched, code: ContactCode::ContactTargetUnavailable }.into())
    }
}

pub fn write_draft(point: [f64; 2], nickname: &str, content: &str) -> Result<()> {
    ensure!(
        valid_app_input(&point, content) && valid_open_chat(nickname),
        "Invalid WeCom draft"
    );
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::nickname_validate_control(Control::Composer, point, nickname)?;
        super::macos_desktop::write_chat_draft(nickname, content)
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("WeCom nickname workflow requires macOS")
    }
}

pub fn validate_submission(nickname: &str, content: &str) -> Result<()> {
    ensure!(
        valid_open_chat(nickname) && valid_app_input(&[0.0, 0.0], content),
        "Invalid WeCom submission"
    );
    super::preflight_send_chat_draft(nickname, content)
        .map_err(|_| anyhow::anyhow!("WeCom submission validation failed"))
}

/// One synchronous native operation owns allocation, snapshots, final checks,
/// Return posting and appended-history observation. Any error stops the workflow.
pub fn send_draft(nickname: &str, content: &str) -> Result<()> {
    ensure!(
        valid_open_chat(nickname) && valid_app_input(&[0.0, 0.0], content),
        "Invalid WeCom submission"
    );
    super::send_chat_draft(nickname, content)
}

fn valid_attachment(nickname: &str, attachment: &Attachment) -> bool {
    valid_open_chat(nickname) && attachment.path.is_absolute() && attachment.size_bytes > 0
        && attachment.path.file_name().and_then(|n| n.to_str()) == Some(attachment.filename.as_str())
        && (!attachment.is_image || matches!(attachment.mime_type.as_str(), "image/png" | "image/jpeg"))
}

/// Pastes one Runtime-pinned snapshot into the live-validated empty composer.
pub fn write_attachment(point: [f64; 2], nickname: &str, attachment: &Attachment) -> Result<AttachmentSession> {
    ensure!(valid_app_input(&point, "x") && valid_attachment(nickname, attachment), "Invalid WeCom attachment");
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::nickname_validate_control(Control::Composer, point, nickname)?;
        Ok(AttachmentSession { draft: super::macos_desktop::write_chat_attachment(nickname, attachment)? })
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("WeCom nickname workflow requires macOS")
    }
}

pub fn validate_attachment_submission(nickname: &str, attachment: &Attachment, session: &AttachmentSession) -> Result<()> {
    ensure!(valid_attachment(nickname, attachment), "Invalid WeCom attachment");
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::preflight_send_chat_attachment(nickname, &session.draft)
            .map_err(|_| anyhow::anyhow!("WeCom attachment submission validation failed"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = session;
        anyhow::bail!("WeCom nickname workflow requires macOS")
    }
}

/// One Return for the exact pasted preview; any error after dispatch is unknown.
pub fn send_attachment(nickname: &str, attachment: &Attachment, session: &AttachmentSession) -> Result<()> {
    ensure!(valid_attachment(nickname, attachment), "Invalid WeCom attachment");
    #[cfg(target_os = "macos")]
    {
        super::macos_desktop::send_chat_attachment(nickname, &session.draft)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = session;
        anyhow::bail!("WeCom nickname workflow requires macOS")
    }
}
