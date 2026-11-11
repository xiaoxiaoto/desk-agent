//! Bounded delegation for an approved nickname transaction (single-use or install-time standing parent grant).
use super::{InvokeError, ROUTE};
use crate::applications::wecom::{
    self, AppWindowCapture,
    nickname_native::{self as native, Control, Geometry},
};
use crate::kernel::instruction::Message;
use crate::infrastructure::database::coordinate_cache::{self, Key as CacheKey, Scope as CacheScope};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::sync::{Arc, LazyLock, Mutex};

#[path = "media.rs"]
mod media;

const SCOPE: (&str, &str, &str) = (
    "wecom.message.send_by_nickname",
    "exact_nickname_conversation",
    "send_messages",
);
const PREFIX: &str = "send-by-nickname-v2-sha256:";
const AUTHORIZATION: &str = "instruction:send_message_by_nickname:v2";
const NAV_QUERY: &str = "窗口最左侧竖栏中，消息气泡图标旁写“消息”的导航按钮";
const SEARCH_QUERY: &str = "消息页左上方、放大镜图标旁的搜索输入框";
const COMPOSER_QUERY: &str = "聊天窗口最下方的白色空白区域";

// Share the final authorization/dispatch boundary with revocation. A successful
// revocation waits for an already-admitted native command to finish; no later
// command can begin after the revocation commits. Local-process coordination only.
static DISPATCH_GATE: LazyLock<Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Exact request identity for audit, delegation and revocation, not grant matching.
pub fn send_message_by_nickname_parameters(
    nickname: &str,
    messages: &[Message],
    interval_ms: u64,
) -> Result<String, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct Parameters<'a> {
        nickname: &'a str,
        messages: &'a [Message],
        interval_ms: u64,
    }
    Ok(format!(
        "{PREFIX}{:x}",
        Sha256::digest(serde_json::to_vec(&Parameters {
            nickname,
            messages,
            interval_ms
        })?)
    ))
}

pub async fn send_message_by_nickname_any_grant(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(super::grants::EXACT_OR_STANDING)
        .bind(SCOPE.0).bind(ROUTE).bind(SCOPE.1).bind(SCOPE.2).bind(AUTHORIZATION)
        .fetch_one(db).await.map(|v| v == 1)
}

/// Check an instruction-level grant (exact grant consumed, install-time standing grant kept); audit the exact request separately.
async fn authorize_parent(db: &SqlitePool, request: &str, request_digest: &str) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let insert = "INSERT INTO capability_audit (request_id, app, capability, route, target, effect, parameters, decision) VALUES (?, 'wecom', ?, ?, ?, ?, ?, ?)";
    sqlx::query(insert)
        .bind(request).bind(SCOPE.0).bind(ROUTE).bind(SCOPE.1).bind(SCOPE.2)
        .bind(request_digest).bind("authorization_check").execute(&mut *tx).await?;
    let granted = super::grants::consume(&mut tx, SCOPE.0, ROUTE, SCOPE.1, SCOPE.2, AUTHORIZATION).await?;
    sqlx::query(insert)
        .bind(request).bind(SCOPE.0).bind(ROUTE).bind(SCOPE.1).bind(SCOPE.2)
        .bind(request_digest).bind(if granted { "dispatch_authorized" } else { "denied" })
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(granted)
}

async fn parent_audit(
    db: &SqlitePool,
    request: &str,
    parameters: &str,
    decision: &str,
) -> Result<(), sqlx::Error> {
    super::audit_text_input(db, request, parameters, decision, SCOPE).await
}

/// Revokes the in-flight bounded delegation; a consumed grant is never restored and standing grants are untouched.
pub async fn revoke_send_message_by_nickname(
    db: &SqlitePool,
    request: &str,
    parameters: &str,
) -> Result<(), sqlx::Error> {
    let db = db.clone();
    let request = request.to_owned();
    let parameters = parameters.to_owned();
    // Dropping a caller's future must not release the gate while SQLite can
    // still commit its revocation on the connection worker.
    tokio::spawn(async move {
        let _dispatch = Arc::clone(&DISPATCH_GATE).lock_owned().await;
        parent_audit(&db, &request, &parameters, "delegation_revoked").await
    })
    .await
    .map_err(|_| sqlx::Error::Protocol("revocation worker stopped".into()))?
}

fn contact_query(nickname: &str) -> String {
    let descriptive = format!(
        "搜索结果中“联系人”分组下，姓名与“{nickname}”完全一致的联系人整行"
    );
    if descriptive.len() <= 256 {
        descriptive
    } else {
        nickname.to_owned()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Navigation,
    Search,
    Composer,
}
impl Slot {
    fn index(self) -> usize {
        match self {
            Self::Navigation => 0,
            Self::Search => 1,
            Self::Composer => 2,
        }
    }
    fn control(self) -> Control {
        match self {
            Self::Navigation => Control::Messages,
            Self::Search => Control::Search,
            Self::Composer => Control::Composer,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Navigation => "message_navigation",
            Self::Search => "search",
            Self::Composer => "composer",
        }
    }
    fn query(self) -> &'static str {
        match self {
            Self::Navigation => NAV_QUERY,
            Self::Search => SEARCH_QUERY,
            Self::Composer => COMPOSER_QUERY,
        }
    }
}

// This private command set is the entire delegable surface. No arbitrary task,
// capability, application, provider, SQL grant, or keyboard shortcut is accepted.
enum Command {
    Launch,
    Center,
    Capture,
    CaptureContact,
    Geometry(Arc<AppWindowCapture>),
    ContactGeometry,
    CacheRead(Arc<CacheKey>),
    Validate(Slot, [f64; 2]),
    ValidateContact([f64; 2]),
    Locate(Arc<AppWindowCapture>, String),
    Convert(Arc<AppWindowCapture>, [f64; 4]),
    ConvertContact(Arc<AppWindowCapture>, [f64; 4]),
    CacheWrite(Arc<CacheKey>, [f64; 2]),
    CacheTouch(Arc<CacheKey>),
    CacheInvalidate(Arc<CacheKey>),
    Navigation([f64; 2]),
    Search([f64; 2]),
    Contact([f64; 2]),
    Draft([f64; 2], usize),
    SendGuard([f64; 2], usize),
    Send(usize),
    Delay,
}
impl Command {
    fn operation(&self) -> &'static str {
        match self {
            Self::Launch => "process_launch",
            Self::Center => "window_center",
            Self::Capture | Self::CaptureContact => "window_capture",
            Self::Geometry(_) | Self::ContactGeometry => "live_geometry",
            Self::CacheRead(..) => "cache_read",
            Self::Validate(..) | Self::ValidateContact(_) => "live_control_guard",
            Self::Locate(..) => "vision_detect",
            Self::Convert(..) | Self::ConvertContact(..) => "screenshot_coordinates_convert",
            Self::CacheWrite(..) | Self::CacheTouch(_) | Self::CacheInvalidate(_) => "cache_store",
            Self::Navigation(_) => "message_navigation",
            Self::Search(_) => "search_query",
            Self::Contact(_) => "exact_contact",
            Self::Draft(..) => "empty_draft_write",
            Self::SendGuard(..) => "exact_draft_guard",
            Self::Send(_) => "return_and_observe",
            Self::Delay => "inter_message_delay",
        }
    }
    fn expected(&self) -> u8 {
        match self {
            Self::Capture | Self::CaptureContact => 1,
            Self::Geometry(_) => 2,
            Self::ContactGeometry => 7,
            Self::CacheRead(..) => 3,
            Self::Validate(..) | Self::ValidateContact(_) => 4,
            Self::Locate(..) => 5,
            Self::Convert(..) | Self::ConvertContact(..) => 6,
            _ => 0,
        }
    }
    fn accepts(expected: u8, reply: &Reply) -> bool {
        matches!((expected, reply), (0, Reply::Unit) | (1, Reply::Capture(_))
            | (2, Reply::Geometry(_)) | (3, Reply::Point(_)) | (4, Reply::Valid(_))
            | (5, Reply::Box(_)) | (6, Reply::Point(Some(_))) | (7, Reply::ContactGeometry(_)))
    }
}

struct Progress {
    phase: &'static str,
    operation: &'static str,
    message_index: Option<usize>,
    failure: Option<&'static str>,
    contact_diagnostic: Option<native::ContactError>,
}
impl Progress {
    fn phase(&mut self, phase: &'static str, index: Option<usize>) {
        self.phase = phase;
        self.message_index = index;
    }
    fn postcondition(&mut self) -> InvokeError {
        self.failure = Some("postcondition");
        InvokeError::OutcomeUnknown
    }
}

enum Reply {
    Unit,
    Capture(Arc<AppWindowCapture>),
    Geometry(Geometry),
    ContactGeometry(native::ContactGeometry),
    Point(Option<[f64; 2]>),
    Valid(bool),
    Box([f64; 4]),
}

trait Backend: Send + 'static {
    fn execute(
        &mut self,
        command: Command,
        nickname: &str,
        messages: &[Message],
        attachments: &media::PreparedMessages,
        interval: u64,
    ) -> anyhow::Result<Reply>;
}

struct NativeBackend<L, C> {
    launch: Option<L>,
    center: Option<C>,
    contact_capture: Option<native::ContactCapture>,
    attachment_sessions: Vec<Option<native::AttachmentSession>>,
}
impl<L, C> Backend for NativeBackend<L, C>
where
    L: FnOnce() -> anyhow::Result<()> + Send + 'static,
    C: FnOnce() -> anyhow::Result<()> + Send + 'static,
{
    fn execute(
        &mut self,
        command: Command,
        nickname: &str,
        messages: &[Message],
        attachments: &media::PreparedMessages,
        _interval: u64,
    ) -> anyhow::Result<Reply> {
        match command {
            Command::Launch => (self
                .launch
                .take()
                .ok_or_else(|| anyhow::anyhow!("launch already consumed"))?)(
            )?,
            Command::Center => (self
                .center
                .take()
                .ok_or_else(|| anyhow::anyhow!("center already consumed"))?)(
            )?,
            Command::Capture => return Ok(Reply::Capture(Arc::new(wecom::capture_app_window()?))),
            Command::CaptureContact => {
                let binding = native::capture_contact(nickname)?;
                let capture = Arc::clone(&binding.capture);
                self.contact_capture = Some(binding);
                return Ok(Reply::Capture(capture));
            }
            Command::Geometry(capture) => return Ok(Reply::Geometry(native::geometry(&capture)?)),
            Command::ContactGeometry => {
                let binding = self.contact_capture.as_ref().ok_or_else(|| anyhow::anyhow!("contact capture unavailable"))?;
                return Ok(Reply::ContactGeometry(native::contact_geometry(binding, nickname)?));
            }
            Command::ValidateContact(point) => {
                let binding = self.contact_capture.as_ref().ok_or_else(|| anyhow::anyhow!("contact capture unavailable"))?;
                return Ok(Reply::Valid(native::validate_contact(binding, point, nickname)?));
            }
            Command::CacheRead(_) | Command::CacheWrite(..) | Command::CacheTouch(_) | Command::CacheInvalidate(_) => {
                anyhow::bail!("cache operations must run through the runtime store");
            }
            Command::Validate(slot, point) => {
                return Ok(Reply::Valid(native::validate_control(
                    slot.control(),
                    point,
                    nickname,
                )));
            }
            Command::Locate(capture, query) => {
                if query == contact_query(nickname) {
                    let binding = self.contact_capture.as_ref().ok_or_else(|| anyhow::anyhow!("contact capture unavailable"))?;
                    let current = native::load_contact_capture(binding, nickname)?;
                    anyhow::ensure!(current.capture_id == capture.capture_id, "contact capture changed");
                }
                let response = crate::providers::vlm::detect_app_window_objects(
                    &capture.path,
                    std::slice::from_ref(&query),
                )?;
                return Ok(Reply::Box(unique_prediction(&response, &capture, &query)?));
            }
            Command::Convert(capture, bounds) => {
                // Loading the private capture rechecks live process/window geometry.
                let current = wecom::load_app_window_capture(&capture.capture_id)?;
                anyhow::ensure!(
                    current.window_bounds_points == capture.window_bounds_points
                        && current.image_width == capture.image_width
                        && current.image_height == capture.image_height,
                    "capture changed"
                );
                let point = generated_point(&capture, bounds)?;
                return Ok(Reply::Point(Some(point)));
            }
            Command::ConvertContact(capture, bounds) => {
                let binding = self.contact_capture.as_ref().ok_or_else(|| anyhow::anyhow!("contact capture unavailable"))?;
                let current = native::load_contact_capture(binding, nickname)?;
                anyhow::ensure!(current.capture_id == capture.capture_id, "contact capture changed");
                return Ok(Reply::Point(Some(generated_point(&current, bounds)?)));
            }
            Command::Navigation(point) => native::activate_messages(point)?,
            Command::Search(point) => native::input_search(point, nickname)?,
            Command::Contact(point) => {
                let binding = self.contact_capture.as_ref().ok_or_else(|| anyhow::anyhow!("contact capture unavailable"))?;
                native::open_contact(binding, point, nickname)?;
            }
            Command::Draft(point, index) => {
                match &messages[index] {
                    Message::Text { content } => native::write_draft(point, nickname, content)?,
                    Message::Image { .. } | Message::File { .. } => {
                        let attachment = attachments.get(index).ok_or_else(|| anyhow::anyhow!("Attachment unavailable"))?;
                        let session = native::write_attachment(point, nickname, attachment)?;
                        self.attachment_sessions[index] = Some(session);
                    }
                }
            }
            Command::SendGuard(_, index) => {
                match &messages[index] {
                    Message::Text { content } => native::validate_submission(nickname, content)?,
                    Message::Image { .. } | Message::File { .. } => {
                        let attachment = attachments.get(index).ok_or_else(|| anyhow::anyhow!("Attachment unavailable"))?;
                        let session = self.attachment_sessions[index].as_ref().ok_or_else(|| anyhow::anyhow!("Attachment preview unavailable"))?;
                        native::validate_attachment_submission(nickname, attachment, session)?;
                    }
                }
            }
            Command::Send(index) => {
                match &messages[index] {
                    Message::Text { content } => native::send_draft(nickname, content)?,
                    Message::Image { .. } | Message::File { .. } => {
                        let attachment = attachments.get(index).ok_or_else(|| anyhow::anyhow!("Attachment unavailable"))?;
                        let session = self.attachment_sessions[index].as_ref().ok_or_else(|| anyhow::anyhow!("Attachment preview unavailable"))?;
                        native::send_attachment(nickname, attachment, session)?;
                        self.attachment_sessions[index] = None;
                    }
                }
            }
            Command::Delay => anyhow::bail!("delay must run asynchronously"),
        }
        Ok(Reply::Unit)
    }
}

fn unique_prediction(
    response: &Value,
    capture: &AppWindowCapture,
    query: &str,
) -> anyhow::Result<[f64; 4]> {
    anyhow::ensure!(
        response.get("image_size") == Some(&json!([capture.image_width, capture.image_height])),
        "vision image mismatch"
    );
    let objects = response
        .get("objects")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("missing objects"))?;
    anyhow::ensure!(
        objects.len() == 1 && objects[0].get("query").and_then(Value::as_str) == Some(query),
        "vision query mismatch"
    );
    let predictions = objects[0]
        .get("predictions")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("missing predictions"))?;
    anyhow::ensure!(predictions.len() == 1, "result must be unique");
    let bounds: [f64; 4] = serde_json::from_value(
        predictions[0]
            .get("coords")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing bounds"))?,
    )?;
    anyhow::ensure!(
        super::screen_box(
            [capture.image_width, capture.image_height],
            capture.window_bounds_points,
            bounds
        )
        .is_some(),
        "invalid prediction"
    );
    Ok(bounds)
}

/// Only internally generated vision centers are quantized for AX's f32 hit
/// testing. Public conversion and caller-provided points retain exact f64 math.
fn generated_point(capture: &AppWindowCapture, image_bounds: [f64; 4]) -> anyhow::Result<[f64; 2]> {
    let (bounds, center) = super::screen_box(
        [capture.image_width, capture.image_height],
        capture.window_bounds_points,
        image_bounds,
    )
    .ok_or_else(|| anyhow::anyhow!("invalid prediction bounds"))?;
    let point = center.map(|coordinate| f64::from(coordinate as f32));
    let [x, y, width, height] = capture.window_bounds_points;
    anyhow::ensure!(
        point.iter().all(|coordinate| coordinate.is_finite())
            && point[0] > bounds[0]
            && point[0] < bounds[2]
            && point[1] > bounds[1]
            && point[1] < bounds[3]
            && point[0] > x
            && point[0] < x + width
            && point[1] > y
            && point[1] < y + height,
        "prediction has no safely representable interior center"
    );
    Ok(point)
}

/// Stable layout data excludes process/window IDs and the movable window origin.
fn cache_key(
    geometry: &Geometry,
    control: &str,
    contact: Option<(&str, [f64; 4])>,
) -> Result<CacheKey, InvokeError> {
    let [x, y, width, height] = geometry.window_bounds;
    if !geometry.window_bounds.iter().chain(geometry.display_bounds.iter()).all(|v| v.is_finite())
        || width <= 0.0 || height <= 0.0
        || geometry.display_bounds[2] <= 0.0 || geometry.display_bounds[3] <= 0.0
        || geometry.image_size.contains(&0)
    {
        return Err(InvokeError::OutcomeUnknown);
    }
    let popup = if let Some((_, bounds)) = contact {
        if !bounds.iter().all(|v| v.is_finite()) || bounds[2] <= 0.0 || bounds[3] <= 0.0 {
            return Err(InvokeError::OutcomeUnknown);
        }
        Some([bounds[0] - x, bounds[1] - y, bounds[2], bounds[3]])
    } else {
        None
    };
    #[derive(serde::Serialize)]
    struct Profile {
        version: u8,
        display_bounds: [f64; 4],
        window_size: [f64; 2],
        image_size: [u32; 2],
        popup: Option<[f64; 4]>,
    }
    let zero = |v: f64| if v == 0.0 { 0.0 } else { v };
    let geometry_key = serde_json::to_string(&Profile {
        version: 1,
        display_bounds: geometry.display_bounds.map(zero),
        window_size: [width, height],
        image_size: geometry.image_size,
        popup: popup.map(|bounds| bounds.map(zero)),
    }).map_err(|_| InvokeError::OutcomeUnknown)?;
    Ok(CacheKey {
        app: "wecom".into(),
        route: ROUTE.into(),
        scope: if contact.is_some() { CacheScope::InstructionContent } else { CacheScope::Global },
        instruction: if contact.is_some() { "send_message_by_nickname".into() } else { String::new() },
        content_digest: contact.map(|(nickname, _)| format!("nickname-v1-sha256:{:x}", Sha256::digest(nickname.as_bytes()))).unwrap_or_default(),
        control: control.into(),
        layout_version: 1,
        geometry_key,
    })
}

fn relative_point(anchor: [f64; 4], point: [f64; 2]) -> Option<[f64; 2]> {
    if !anchor.iter().chain(point.iter()).all(|v| v.is_finite())
        || anchor[2] <= 0.0 || anchor[3] <= 0.0
    {
        return None;
    }
    let relative = [point[0] - anchor[0], point[1] - anchor[1]];
    (relative[0] >= 0.0 && relative[0] < anchor[2]
        && relative[1] >= 0.0 && relative[1] < anchor[3]).then_some(relative)
}

fn screen_point(anchor: [f64; 4], relative: [f64; 2]) -> Option<[f64; 2]> {
    if !relative.iter().all(|v| v.is_finite() && *v >= 0.0)
        || relative[0] >= anchor[2] || relative[1] >= anchor[3]
    {
        return None;
    }
    let point = [
        f64::from((anchor[0] + relative[0]) as f32),
        f64::from((anchor[1] + relative[1]) as f32),
    ];
    relative_point(anchor, point).map(|_| point)
}

struct Delegation<B> {
    db: SqlitePool,
    request: String,
    parameters: String,
    active: String,
    nickname: Arc<str>,
    messages: Arc<Vec<Message>>,
    attachments: Arc<media::PreparedMessages>,
    interval: u64,
    next: u64,
    stopped: bool,
    backend: Arc<Mutex<B>>,
    last_send_observed: Option<usize>,
    latest_capture: Option<Arc<AppWindowCapture>>,
    progress: Progress,
    message_states: Vec<&'static str>,
}
impl<B: Backend> Delegation<B> {
    // Rebind every child to the immutable original transaction, its exact step,
    // and a narrow semantic capability. Only hashes enter the ledger.
    fn child(
        &self,
        command: &Command,
    ) -> Result<
        (
            (&'static str, &'static str, &'static str),
            &'static str,
            String,
        ),
        InvokeError,
    > {
        if self.stopped
            || (self.next == 0 && !matches!(command, Command::Launch))
            || (self.next == 1 && !matches!(command, Command::Center))
            || (self.next > 1 && matches!(command, Command::Launch | Command::Center))
        {
            return Err(InvokeError::Forbidden);
        }
        let (capability, route, payload) = match command {
            Command::Launch => (
                "wecom.client.launch",
                ROUTE,
                json!({"step":"process_launch","resources": (0..self.messages.len()).map(|i| self.attachments.binding(i)).collect::<Vec<_>>()}),
            ),
            Command::Center => (super::CENTER.0, ROUTE, json!({"step":"window_center"})),
            Command::Capture => (
                super::WINDOW_CAPTURE.0,
                ROUTE,
                json!({"step":"window_capture"}),
            ),
            Command::CaptureContact => (
                super::WINDOW_CAPTURE.0,
                ROUTE,
                json!({"step":"window_capture","purpose":"contact_result","nickname":self.nickname.as_ref()}),
            ),
            Command::Geometry(c) => (
                super::WINDOW_CONVERT.0,
                ROUTE,
                json!({"step":"live_geometry","capture":c.capture_id}),
            ),
            Command::ContactGeometry => (
                super::WINDOW_CONVERT.0,
                ROUTE,
                json!({"step":"live_geometry","purpose":"contact_result","nickname":self.nickname.as_ref()}),
            ),
            Command::CacheRead(key) => (
                super::ELEMENT_DIAGNOSE.0,
                ROUTE,
                json!({"step":"cache_read","key":key.as_ref()}),
            ),
            Command::ValidateContact(point) => (
                super::ELEMENT_DIAGNOSE.0,
                ROUTE,
                json!({"step":"live_control_guard","purpose":"contact_result","point":point,"nickname":self.nickname.as_ref()}),
            ),
            Command::Validate(s, p) => (
                super::ELEMENT_DIAGNOSE.0,
                ROUTE,
                json!({"step":"live_control_guard","slot":s.index(),"point":p}),
            ),
            Command::Locate(c, q) => {
                if q != NAV_QUERY
                    && q != SEARCH_QUERY
                    && q != COMPOSER_QUERY
                    && q != &contact_query(&self.nickname)
                {
                    return Err(InvokeError::Forbidden);
                }
                (
                    super::WINDOW_LOCATE.0,
                    super::VISION_ROUTE,
                    if q == &contact_query(&self.nickname) {
                        json!({"step":"vision_detect","purpose":"contact_result","capture":c.capture_id,"query":q})
                    } else {
                        json!({"step":"vision_detect","capture":c.capture_id,"query":q})
                    },
                )
            }
            Command::Convert(c, b) => (
                super::WINDOW_CONVERT.0,
                ROUTE,
                json!({"step":"screenshot_coordinates_convert","capture":c.capture_id,"boxes":[b],"point_derivation":"f32-strict-interior-v1"}),
            ),
            Command::ConvertContact(c, b) => (
                super::WINDOW_CONVERT.0,
                ROUTE,
                json!({"step":"screenshot_coordinates_convert","purpose":"contact_result","capture":c.capture_id,"boxes":[b],"point_derivation":"f32-strict-interior-v1"}),
            ),
            Command::CacheWrite(key, point) => (
                super::ELEMENT_DIAGNOSE.0,
                ROUTE,
                json!({"step":"cache_store","key":key.as_ref(),"point":point}),
            ),
            Command::CacheTouch(key) => (
                super::ELEMENT_DIAGNOSE.0,
                ROUTE,
                json!({"step":"cache_store","key":key.as_ref(),"validated":true}),
            ),
            Command::CacheInvalidate(key) => (
                super::ELEMENT_DIAGNOSE.0,
                ROUTE,
                json!({"step":"cache_store","key":key.as_ref(),"invalidated":true}),
            ),
            Command::Navigation(p) => (
                super::ELEMENT_ACTIVATE.0,
                ROUTE,
                json!({"step":"message_navigation","point":p}),
            ),
            Command::Search(p) => (
                super::SEARCH_QUERY.0,
                ROUTE,
                json!({"step":"search_query","point":p,"nickname":self.nickname.as_ref()}),
            ),
            Command::Contact(p) => (
                super::CHAT_OPEN.0,
                ROUTE,
                json!({"step":"exact_contact","point":p,"nickname":self.nickname.as_ref()}),
            ),
            Command::Draft(p, i) | Command::SendGuard(p, i) => {
                let message = self.messages.get(*i).ok_or(InvokeError::Forbidden)?;
                (
                    if matches!(command, Command::Draft(..)) {
                        super::CHAT_DRAFT.0
                    } else {
                        super::ELEMENT_DIAGNOSE.0
                    },
                    ROUTE,
                    json!({"step":if matches!(command,Command::Draft(..)) {"empty_draft_write"} else {"exact_draft_guard"},"point":p,"index":i,"message":message,"resource":self.attachments.binding(*i)}),
                )
            }
            Command::Send(i) => (
                super::CHAT_DRAFT_SEND.0,
                ROUTE,
                json!({"step":"return_and_observe","index":i,"message":self.messages.get(*i).ok_or(InvokeError::Forbidden)?,"resource":self.attachments.binding(*i)}),
            ),
            Command::Delay => (
                SCOPE.0,
                ROUTE,
                json!({"step":"inter_message_delay","interval_ms":self.interval}),
            ),
        };
        let scope = match capability {
            "wecom.client.launch" => ("wecom.client.launch", "wecom_client", "launch_process"),
            value if value == super::CENTER.0 => super::CENTER,
            value if value == super::WINDOW_CAPTURE.0 => super::WINDOW_CAPTURE,
            value if value == super::WINDOW_CONVERT.0 => super::WINDOW_CONVERT,
            value if value == super::WINDOW_LOCATE.0 => super::WINDOW_LOCATE,
            value if value == super::ELEMENT_DIAGNOSE.0 => super::ELEMENT_DIAGNOSE,
            value if value == super::ELEMENT_ACTIVATE.0 => super::ELEMENT_ACTIVATE,
            value if value == super::SEARCH_QUERY.0 => super::SEARCH_QUERY,
            value if value == super::CHAT_OPEN.0 => super::CHAT_OPEN,
            value if value == super::CHAT_DRAFT.0 => super::CHAT_DRAFT,
            value if value == super::CHAT_DRAFT_SEND.0 => super::CHAT_DRAFT_SEND,
            value if value == SCOPE.0 => SCOPE,
            _ => return Err(InvokeError::Forbidden),
        };
        let bytes = serde_json::to_vec(&json!({"parent":self.parameters,"lease":self.active,"sequence":self.next,"app":"wecom","capability":scope.0,"route":route,"target":scope.1,"effect":scope.2,"payload":payload})).map_err(|_| InvokeError::InvalidInput)?;
        Ok((
            scope,
            route,
            format!("nickname-child-v1-sha256:{:x}", Sha256::digest(bytes)),
        ))
    }
    async fn run(&mut self, command: Command) -> Result<Reply, InvokeError> {
        self.progress.operation = command.operation();
        self.progress.failure = Some("authorization");
        let expected = command.expected();
        let is_contact = matches!(&command, Command::CaptureContact | Command::ContactGeometry | Command::ValidateContact(_) | Command::ConvertContact(..) | Command::Contact(_))
            || matches!(&command, Command::Locate(_, query) if query == &contact_query(&self.nickname));
        let contact_dispatch = if matches!(&command, Command::Contact(_)) {
            native::ContactDispatch::Unknown
        } else {
            native::ContactDispatch::NotDispatched
        };
        let send_index = if let Command::Send(index) = &command {
            Some(*index)
        } else {
            None
        };
        let (scope, route, parameters) = self.child(&command)?;
        self.progress.failure = Some("audit");
        let dispatch = Arc::clone(&DISPATCH_GATE).lock_owned().await;
        let mut tx = self
            .db
            .begin()
            .await
            .map_err(|_| InvokeError::AuditUnavailable)?;
        let allowed = sqlx::query_scalar::<_,i64>("SELECT EXISTS(SELECT 1 FROM capability_audit WHERE request_id=? AND app='wecom' AND capability=? AND route=? AND target=? AND effect=? AND parameters=? AND decision=?) AND NOT EXISTS(SELECT 1 FROM capability_audit WHERE request_id=? AND app='wecom' AND capability=? AND parameters=? AND decision IN ('delegation_revoked','delegation_completed','delegation_stopped_unknown'))")
            .bind(&self.request).bind(SCOPE.0).bind(ROUTE).bind(SCOPE.1).bind(SCOPE.2).bind(&self.parameters).bind(&self.active)
            .bind(&self.request).bind(SCOPE.0).bind(&self.parameters).fetch_one(&mut *tx).await.map_err(|_| InvokeError::AuditUnavailable)? == 1;
        if !allowed {
            self.stopped = true;
            self.progress.failure = Some("authorization");
            return Err(InvokeError::Forbidden);
        }
        sqlx::query("INSERT INTO capability_audit(request_id,app,capability,route,target,effect,parameters,decision) VALUES (?,'wecom',?,?,?,?,?,'delegated_dispatch_intent')")
            .bind(&self.request).bind(scope.0).bind(route).bind(scope.1).bind(scope.2).bind(&parameters).execute(&mut *tx).await.map_err(|_| InvokeError::AuditUnavailable)?;
        tx.commit()
            .await
            .map_err(|_| InvokeError::AuditUnavailable)?;
        self.next += 1;
        if let Some(index) = send_index {
            self.message_states[index] = "submission_unknown";
        }
        self.progress.failure = Some("operation");
        let started = std::time::Instant::now();
        let result = if let Command::CacheRead(key) = &command {
            let point = match coordinate_cache::read(&self.db, key).await {
                Ok(point) => point,
                Err(_) => {
                    tracing::warn!(operation = "cache_read", "Coordinate cache unavailable; using live localization");
                    None
                }
            };
            tracing::debug!(control = key.control, candidate_match = point.is_some(), "Coordinate cache lookup");
            Ok(Reply::Point(point))
        } else if let Command::CacheWrite(key, point) = &command {
            if coordinate_cache::write(&self.db, key, *point).await.is_err() {
                tracing::warn!(operation = "cache_store", "Coordinate cache write failed; live evidence retained");
            }
            Ok(Reply::Unit)
        } else if let Command::CacheTouch(key) = &command {
            if coordinate_cache::touch(&self.db, key).await.is_err() {
                tracing::warn!(operation = "cache_store", "Coordinate cache validation timestamp unavailable");
            }
            tracing::debug!(control = key.control, validated_hit = true, "Coordinate cache reuse");
            Ok(Reply::Unit)
        } else if let Command::CacheInvalidate(key) = &command {
            if coordinate_cache::invalidate(&self.db, key).await.is_err() {
                tracing::warn!(operation = "cache_store", "Coordinate cache invalidation failed; candidate remains untrusted");
            }
            Ok(Reply::Unit)
        } else if matches!(command, Command::Delay) {
            // Waiting has no native effect; allow revocation during an interval.
            drop(dispatch);
            tokio::time::sleep(std::time::Duration::from_millis(self.interval)).await;
            Ok(Reply::Unit)
        } else {
            let backend = Arc::clone(&self.backend);
            let nickname = self.nickname.clone();
            let messages = self.messages.clone();
            let interval = self.interval;
            let attachments = Arc::clone(&self.attachments);
            tokio::task::spawn_blocking(move || {
                // The native worker keeps the gate even if its waiter disconnects.
                let _dispatch = dispatch;
                match &command {
                    Command::Launch => attachments.validate_all()?,
                    Command::Draft(_, index) | Command::SendGuard(_, index) | Command::Send(index) => attachments.validate(*index)?,
                    _ => {}
                }
                if let Command::Draft(_, index) = &command {
                    attachments.mark_paste_started(*index);
                }
                backend
                    .lock()
                    .map_err(|_| anyhow::anyhow!("backend poisoned"))?
                    .execute(command, &nickname, &messages, &attachments, interval)
            })
            .await
            .map_err(|_| anyhow::anyhow!("backend worker failed"))
            .and_then(|result| result)
            .map_err(|error| {
                if is_contact {
                    self.progress.contact_diagnostic = Some(
                        error.downcast_ref::<native::ContactError>().copied().unwrap_or(
                            native::ContactError {
                                dispatch: contact_dispatch,
                                code: native::ContactCode::ContactOperationFailed,
                            },
                        ),
                    );
                }
                InvokeError::OutcomeUnknown
            })
        };
        tracing::debug!(operation = self.progress.operation, elapsed_ms = started.elapsed().as_millis() as u64, "Nickname operation finished");
        let valid_reply = result.as_ref().map(|reply| Command::accepts(expected, reply)).unwrap_or(false);
        if valid_reply {
            self.last_send_observed = send_index;
            if let Some(index) = send_index {
                self.message_states[index] = "observed";
            }
        }
        let decision = if result.is_ok() {
            "delegated_dispatch_succeeded"
        } else {
            "delegated_dispatch_unknown"
        };
        let audit=sqlx::query("INSERT INTO capability_audit(request_id,app,capability,route,target,effect,parameters,decision) VALUES (?,'wecom',?,?,?,?,?,?)")
            .bind(&self.request).bind(scope.0).bind(route).bind(scope.1).bind(scope.2).bind(&parameters).bind(decision).execute(&self.db).await;
        if audit.is_err() {
            self.stopped = true;
            self.progress.failure = Some("audit");
            return Err(InvokeError::OutcomeUnknown);
        }
        if result.is_err() {
            self.stopped = true;
            return Err(InvokeError::OutcomeUnknown);
        }
        if !valid_reply {
            if is_contact {
                self.progress.contact_diagnostic = Some(native::ContactError {
                    dispatch: contact_dispatch,
                    code: native::ContactCode::ContactOperationFailed,
                });
            }
            self.stopped = true;
            return Err(self.progress.postcondition());
        }
        self.progress.failure = None;
        result
    }
    async fn capture(&mut self) -> Result<Arc<AppWindowCapture>, InvokeError> {
        let Reply::Capture(capture) = self.run(Command::Capture).await? else {
            return Err(InvokeError::OutcomeUnknown);
        };
        self.latest_capture = Some(Arc::clone(&capture));
        Ok(capture)
    }
    async fn locate(&mut self, query: String) -> Result<[f64; 2], InvokeError> {
        let capture = self.capture().await?;
        self.locate_capture(capture, query).await
    }
    async fn locate_capture(
        &mut self,
        capture: Arc<AppWindowCapture>,
        query: String,
    ) -> Result<[f64; 2], InvokeError> {
        let Reply::Box(bounds) = self
            .run(Command::Locate(Arc::clone(&capture), query))
            .await?
        else {
            return Err(InvokeError::OutcomeUnknown);
        };
        let Reply::Point(Some(point)) = self.run(Command::Convert(capture, bounds)).await? else {
            return Err(InvokeError::OutcomeUnknown);
        };
        Ok(point)
    }
    async fn control(
        &mut self,
        capture: &Arc<AppWindowCapture>,
        geometry: &Geometry,
        slot: Slot,
        hits: &mut [bool; 4],
    ) -> Result<[f64; 2], InvokeError> {
        // Recheck the latest actual capture against live geometry before every
        // cache lookup, preserving the initial geometry key without aging out
        // that first capture during several sequential vision requests.
        let latest = self
            .latest_capture
            .as_ref()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::clone(capture));
        let Reply::Geometry(current) = self.run(Command::Geometry(latest)).await? else {
            return Err(InvokeError::OutcomeUnknown);
        };
        if current != *geometry {
            return Err(self.progress.postcondition());
        }
        let key = Arc::new(cache_key(geometry, slot.name(), None)?);
        if let Reply::Point(Some(relative)) = self.run(Command::CacheRead(Arc::clone(&key))).await? {
            if let Some(point) = screen_point(geometry.window_bounds, relative) {
                if matches!(self.run(Command::Validate(slot, point)).await?, Reply::Valid(true)) {
                    hits[slot.index()] = true;
                    self.run(Command::CacheTouch(key)).await?;
                    return Ok(point);
                }
            } else {
                self.run(Command::CacheInvalidate(Arc::clone(&key))).await?;
            }
        }
        let point = if slot == Slot::Navigation {
            self.locate_capture(Arc::clone(capture), slot.query().to_owned()).await?
        } else {
            self.locate(slot.query().to_owned()).await?
        };
        let latest = self.latest_capture.as_ref().map(Arc::clone)
            .ok_or_else(|| self.progress.postcondition())?;
        let Reply::Geometry(current) = self.run(Command::Geometry(latest)).await? else {
            return Err(self.progress.postcondition());
        };
        if current != *geometry {
            return Err(self.progress.postcondition());
        }
        if !matches!(self.run(Command::Validate(slot, point)).await?, Reply::Valid(true)) {
            return Err(self.progress.postcondition());
        }
        let relative = relative_point(geometry.window_bounds, point)
            .ok_or_else(|| self.progress.postcondition())?;
        self.run(Command::CacheWrite(key, relative)).await?;
        Ok(point)
    }
    async fn contact(
        &mut self,
        geometry: &Geometry,
        hits: &mut [bool; 4],
    ) -> Result<[f64; 2], InvokeError> {
        let Reply::Capture(capture) = self.run(Command::CaptureContact).await? else {
            return Err(self.progress.postcondition());
        };
        let Reply::ContactGeometry(current) = self.run(Command::ContactGeometry).await? else {
            return Err(self.progress.postcondition());
        };
        if current.main != *geometry {
            self.progress.contact_diagnostic = Some(native::ContactError {
                dispatch: native::ContactDispatch::NotDispatched,
                code: native::ContactCode::ContactCaptureChanged,
            });
            return Err(self.progress.postcondition());
        }
        let key = Arc::new(cache_key(geometry, "contact_result", Some((&self.nickname, current.popup_bounds)))?);
        if let Reply::Point(Some(relative)) = self.run(Command::CacheRead(Arc::clone(&key))).await? {
            if let Some(point) = screen_point(current.popup_bounds, relative) {
                if matches!(self.run(Command::ValidateContact(point)).await?, Reply::Valid(true)) {
                    hits[3] = true;
                    self.run(Command::CacheTouch(key)).await?;
                    return Ok(point);
                }
            }
            self.run(Command::CacheInvalidate(Arc::clone(&key))).await?;
        }
        let Reply::Box(bounds) = self.run(Command::Locate(Arc::clone(&capture), contact_query(&self.nickname))).await? else {
            return Err(self.progress.postcondition());
        };
        let Reply::Point(Some(point)) = self.run(Command::ConvertContact(capture, bounds)).await? else {
            return Err(self.progress.postcondition());
        };
        if !matches!(self.run(Command::ValidateContact(point)).await?, Reply::Valid(true)) {
            self.progress.contact_diagnostic = Some(native::ContactError {
                dispatch: native::ContactDispatch::NotDispatched,
                code: native::ContactCode::ContactPointOutside,
            });
            return Err(self.progress.postcondition());
        }
        let relative = relative_point(current.popup_bounds, point)
            .ok_or_else(|| self.progress.postcondition())?;
        self.run(Command::CacheWrite(key, relative)).await?;
        Ok(point)
    }
    async fn workflow(
        &mut self,
        hits: &mut [bool; 4],
        outcomes: &mut Vec<Value>,
        unknown: &mut Option<usize>,
    ) -> Result<(), InvokeError> {
        self.progress.phase("launch", None);
        self.run(Command::Launch).await?;
        self.progress.phase("center", None);
        self.run(Command::Center).await?;
        self.progress.phase("measurement", None);
        // A measurement capture supplies actual pixel dimensions even on a warm
        // cache. On a cold cache the same image is reused for navigation vision.
        let capture = self.capture().await?;
        let Reply::Geometry(geometry) = self.run(Command::Geometry(Arc::clone(&capture))).await?
        else {
            return Err(InvokeError::OutcomeUnknown);
        };
        self.progress.phase("navigation", None);
        let navigation = self
            .control(&capture, &geometry, Slot::Navigation, hits)
            .await?;
        self.run(Command::Navigation(navigation)).await?;
        self.progress.phase("search", None);
        let search = self
            .control(&capture, &geometry, Slot::Search, hits)
            .await?;
        self.run(Command::Search(search)).await?;
        self.progress.phase("contact", None);
        let contact = self.contact(&geometry, hits).await?;
        self.run(Command::Contact(contact)).await?;
        self.progress.phase("composer", None);
        let composer = self
            .control(&capture, &geometry, Slot::Composer, hits)
            .await?;
        for index in 0..self.messages.len() {
            self.progress.phase("message_validation", Some(index));
            self.message_states[index] = "preparing";
            *unknown = Some(index);
            outcomes.push(json!({"index":index,"submitted":false,"message_observed":false}));
            if !matches!(
                self.run(Command::Validate(Slot::Composer, composer))
                    .await?,
                Reply::Valid(true)
            ) {
                return Err(self.progress.postcondition());
            }
            self.progress.phase("draft", Some(index));
            self.run(Command::Draft(composer, index)).await?;
            self.progress.phase("send_guard", Some(index));
            self.run(Command::SendGuard(composer, index)).await?;
            self.progress.phase("send", Some(index));
            let sent = self.run(Command::Send(index)).await;
            // Authorization/intent failures prove Send never began. Only an
            // admitted native attempt without completion evidence is unknown.
            if self.message_states[index] == "submission_unknown" {
                outcomes.last_mut().unwrap()["submitted"] = Value::Null;
            }
            if self.last_send_observed == Some(index) {
                *outcomes.last_mut().unwrap() =
                    json!({"index":index,"submitted":true,"message_observed":true});
            }
            sent?;
            *unknown = None;
            if index + 1 < self.messages.len() && self.interval > 0 {
                self.progress.phase("interval", Some(index));
                self.run(Command::Delay).await?;
            }
        }
        Ok(())
    }
}

async fn invoke_backend<B: Backend>(
    db: &SqlitePool,
    request: &str,
    nickname: &str,
    messages: &[Message],
    interval: u64,
    backend: B,
) -> Result<Value, InvokeError> {
    if !crate::kernel::task::valid_send_message_by_nickname(nickname, messages, interval) {
        return Err(InvokeError::InvalidInput);
    }
    let parameters = send_message_by_nickname_parameters(nickname, messages, interval)
        .map_err(|_| InvokeError::InvalidInput)?;
    // Instruction authorization and exact-request intent commit before launch.
    if !authorize_parent(db, request, &parameters)
        .await
        .map_err(|_| InvokeError::AuditUnavailable)?
    {
        return Err(InvokeError::Forbidden);
    }
    let messages = Arc::new(messages.to_vec());
    let original_messages = Arc::clone(&messages);
    let attachments = match tokio::task::spawn_blocking(move || media::PreparedMessages::prepare(&original_messages)).await {
        Ok(Ok(attachments)) => attachments,
        _ => {
            parent_audit(db, request, &parameters, "attachment_preflight_rejected")
                .await.map_err(|_| InvokeError::AuditUnavailable)?;
            return Err(InvokeError::InvalidInput);
        }
    };
    let attachments = Arc::new(attachments);
    let active = format!("delegation_active:{}", uuid::Uuid::new_v4());
    parent_audit(db, request, &parameters, &active)
        .await
        .map_err(|_| InvokeError::AuditUnavailable)?;
    let mut delegation = Delegation {
        db: db.clone(),
        request: request.to_owned(),
        parameters,
        active,
        nickname: Arc::from(nickname),
        messages: Arc::clone(&messages),
        attachments,
        interval,
        next: 0,
        stopped: false,
        backend: Arc::new(Mutex::new(backend)),
        last_send_observed: None,
        latest_capture: None,
        progress: Progress { phase: "launch", operation: "process_launch", message_index: None, failure: None, contact_diagnostic: None },
        message_states: vec!["not_started"; messages.len()],
    };
    let mut hits = [false; 4];
    let mut outcomes = Vec::new();
    let mut unknown = None;
    let result = delegation
        .workflow(&mut hits, &mut outcomes, &mut unknown)
        .await;
    if result.is_ok() {
        delegation.attachments.release_completed();
    }
    delegation.stopped = true;
    let completed = outcomes
        .iter()
        .filter(|entry| entry["message_observed"] == true)
        .count();
    let terminal = parent_audit(
        db,
        request,
        &delegation.parameters,
        if result.is_ok() {
            "delegation_completed"
        } else {
            "delegation_stopped_unknown"
        },
    )
    .await;
    if terminal.is_err() {
        delegation.progress.phase("final_audit", None);
        delegation.progress.operation = "parent_audit";
        delegation.progress.failure = Some("audit");
    } else if result.is_ok() {
        delegation.progress.phase("completed", None);
        delegation.progress.operation = "parent_audit";
        delegation.progress.failure = None;
    }
    let message_states: Vec<Value> = delegation.message_states.iter().enumerate()
        .map(|(index, state)| json!({"index": index, "state": state})).collect();
    Ok(
        json!({"outcome":if result.is_ok() && terminal.is_ok() {"completed"}else{"unknown"},"outcomes":outcomes,"completed_count":completed,"unknown_index":unknown,"cache_hits":{"message_navigation":hits[0],"search":hits[1],"composer":hits[2],"contact_result":hits[3]},"verification":"local_ui_only","progress":{"phase":delegation.progress.phase,"operation":delegation.progress.operation,"message_index":delegation.progress.message_index,"status":if result.is_ok() && terminal.is_ok() {"completed"} else {"stopped"},"failure":delegation.progress.failure,"contact_diagnostic":delegation.progress.contact_diagnostic},"message_states":message_states}),
    )
}

pub(super) async fn invoke(
    db: &SqlitePool,
    request: &str,
    nickname: &str,
    messages: &[Message],
    interval: u64,
    launch: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
    center: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
) -> Result<Value, InvokeError> {
    invoke_backend(
        db,
        request,
        nickname,
        messages,
        interval,
        NativeBackend {
            launch: Some(launch),
            center: Some(center),
            contact_capture: None,
            attachment_sessions: (0..messages.len()).map(|_| None).collect(),
        },
    )
    .await
}

#[cfg(test)]
#[path = "nickname_tests.rs"]
mod tests;
