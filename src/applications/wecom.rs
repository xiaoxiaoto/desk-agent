use crate::kernel::capability::{Capability, CapabilityChannel, CapabilityStatus};
use crate::kernel::task::{ActivationDiagnostic, valid_chat_draft};
pub(crate) mod nickname_native;

/// Local vision is required only when live-validated cached localization is unavailable.
pub fn send_message_by_nickname_capability() -> Capability {
    #[cfg(target_os = "macos")]
    let reason = if !macos_desktop::trusted() {
        Some("macOS Accessibility permission is required")
    } else if !macos_desktop::screen_recording_allowed() {
        Some("macOS Screen Recording permission is required")
    } else if !macos_desktop::input_monitoring_available() {
        Some("macOS Input Monitoring permission is required")
    } else if !macos_desktop::screenshot_capture_available() {
        Some("ScreenCaptureKit whole app-window screenshots require macOS 14.2 or later")
    } else { None };
    #[cfg(not(target_os = "macos"))]
    let reason = Some("WeCom desktop messaging is supported only on macOS");
    Capability {
        id: "wecom.message.send_by_nickname",
        status: if reason.is_none() { CapabilityStatus::Available } else { CapabilityStatus::Unavailable },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["wecom.message.send_by_nickname"],
        prerequisites: vec![
            "macOS Accessibility, Screen Recording and Input Monitoring permissions",
            "ScreenCaptureKit whole app-window screenshots including child windows (macOS 14.2 or later); no screen-region or full-screen fallback",
            "Installed WeCom, one unambiguous main window, and Return configured to submit messages",
            "Local wevisone-vl-4b vision endpoint when any coordinate requires cache-miss or stale-candidate localization; discovery cannot predict hits",
            "Install-time standing grant (desk-agent setup) or one explicit single-use instruction authorization with fixed parameters instruction:send_message_by_nickname:v2, independent of nickname/messages/interval",
        ],
        limitations: vec![
            "Launches, centers and captures actual geometry before querying private SQLite coordinates; persistent keys exclude process/window identities, which are always checked live.",
            "Every screenshot covers the whole WeCom window, including its anchored child popup; exact-nickname contact candidates require a fresh ContactCapture and live unique-contact checks before reuse.",
            "Requires a unique exact contact and an empty draft; stops without retry on every failure.",
            "Submitted and message_observed report local UI evidence, not delivery or read receipts.",
        ],
        reason,
    }
}

pub fn launch_capability() -> Capability {
    let available = cfg!(target_os = "macos");
    Capability {
        id: "wecom.client.launch",
        status: if available {
            CapabilityStatus::Available
        } else {
            CapabilityStatus::Unavailable
        },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["app.wecom.launch"],
        prerequisites: vec!["macOS", "Installed WeCom desktop application"],
        limitations: vec!["Launches the application; does not inspect or control its window."],
        reason: if available {
            None
        } else {
            Some("WeCom desktop launch is supported only on macOS")
        },
    }
}

pub fn launch_wecom() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        crate::platforms::macos::launch(crate::kernel::platform::LaunchTarget {
            application_name: "企业微信",
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("WeCom desktop launch is supported only on macOS")
    }
}

/// Reports availability of the controlled window-centering capability.
pub fn center_capability() -> Capability {
    #[cfg(target_os = "macos")]
    let reason = if !macos_desktop::trusted() {
        Some("macOS Accessibility permission is required for the runtime")
    } else if let Err(error) = macos_desktop::target() {
        let message = error.to_string();
        Some(if message.starts_with("Expected exactly one running WeCom instance") {
            "Exactly one running WeCom instance is required"
        } else if message.starts_with("Expected a main WeCom window") {
            "A main or single WeCom window is required"
        } else if message.starts_with("WeCom window position is not writable") {
            "The WeCom window position is not writable"
        } else if message.starts_with("WeCom window does not overlap") || message.starts_with("WeCom window overlaps") {
            "The WeCom window must overlap one unambiguous active display"
        } else {
            "The WeCom window or display geometry could not be read"
        })
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let reason = Some("WeCom window control is supported only on macOS");
    let available = reason.is_none();

    Capability {
        id: "wecom.window.center",
        status: if available { CapabilityStatus::Available } else { CapabilityStatus::Unavailable },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["app.wecom.window.manage"],
        prerequisites: vec![
            "macOS with Accessibility permission for the controlling process",
            "One running WeCom instance with a main or single movable window on an active display",
        ],
        limitations: vec![
            "Moves the window without activating WeCom or changing its size; reads back position and size after the move.",
            "Multiple windows require an identified main window; equally overlapping displays are not moved.",
        ],
        reason,
    }
}

/// Move the main or sole WeCom window to the center of its display's full bounds.
pub fn center_wecom_window() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::center()
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("WeCom window control is supported only on macOS")
    }
}

/// Reports whether the selected WeCom window can be captured.
pub fn app_window_capture_capability() -> Capability {
    #[cfg(target_os = "macos")]
    let reason = if !macos_desktop::screen_recording_allowed() {
        Some("macOS Screen Recording permission is required for the runtime")
    } else if !macos_desktop::trusted() {
        Some("macOS Accessibility permission is required for the runtime")
    } else if macos_desktop::capture_target().is_err() {
        Some("A uniquely identifiable visible WeCom window fully contained in one unambiguous active display is required")
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let reason = Some("App window capture is supported only on macOS");

    Capability {
        id: "desktop.window.capture",
        status: if reason.is_none() { CapabilityStatus::Available } else { CapabilityStatus::Unavailable },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["desktop.window.capture"],
        prerequisites: vec![
            "macOS with Screen Recording and Accessibility permission for the controlling process",
            "Exactly one running WeCom instance with a uniquely identifiable visible main or single window",
            "The window must be fully contained in exactly one active display, without overlapping another display",
            "A writable private capture directory under ~/.desk-agent",
        ],
        limitations: vec![
            "Captures the selected WeCom window including its own child windows, without other windows that may occlude it.",
            "Protected content may be redacted by macOS and cannot always be detected; availability does not guarantee a subsequent capture will succeed.",
            "Stores sensitive PNG files in a private directory; callers are responsible for their retention and deletion.",
        ],
        reason,
    }
}

/// Preflight-only Screen Recording check; dispatch never shows an OS prompt.
pub fn screen_recording_allowed() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::screen_recording_allowed()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// A private window image and its capture-time platform geometry.
pub use crate::kernel::platform::WindowCapture as AppWindowCapture;

/// Capture the selected WeCom window into a private PNG file.
pub fn capture_app_window() -> anyhow::Result<AppWindowCapture> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::capture()
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("App window capture is supported only on macOS")
    }
}

/// Load a recent capture only while the same WeCom window geometry remains valid.
pub fn load_app_window_capture(capture_id: &str) -> anyhow::Result<AppWindowCapture> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::load_capture(capture_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = capture_id;
        anyhow::bail!("App window capture is supported only on macOS")
    }
}

/// Reports the prerequisites for a separately authorized, point-specific activation.
pub fn app_element_activation_capability() -> Capability {
    #[cfg(target_os = "macos")]
    let reason = if !macos_desktop::trusted() {
        Some("macOS Accessibility permission is required for the runtime")
    } else if macos_desktop::activation_target_available().is_err() {
        Some("A visible, frontmost WeCom main or single window is required")
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let reason = Some("WeCom element activation is supported only on macOS");

    Capability {
        id: "wecom.element.activate",
        status: if reason.is_none() { CapabilityStatus::Available } else { CapabilityStatus::Unavailable },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["wecom.element.activate"],
        prerequisites: vec![
            "macOS Accessibility permission for the controlling process",
            "One running WeCom instance with a visible, frontmost main or single window",
            "An exact accessibility role at the requested screen point, with an optional exact semantic accessibility name",
        ],
        limitations: vec![
            "Does not launch, activate, or raise WeCom; system-wide accessibility hit testing must identify the target window and element.",
            "Requires a visible, opaque target window; overlays absent from accessibility hit testing cannot always be detected.",
            "Coordinates must be exactly representable as accessibility hit-test floats.",
            "With an expected label, the name is nonempty AXTitle, otherwise the same-window AXTitleUIElement's AXStaticText title or value; missing names are rejected.",
            "Without an expected label, authorization identifies only the exact point and role, not the message or other content at that point.",
            "A click is verified by a focused or selected state transition, or by observing the tagged mouse-down and mouse-up routed to the target process; routing does not prove application processing.",
        ],
        reason,
    }
}

/// Validate a point-specific activation without dispatching input.
/// `None` explicitly selects coordinate-and-role identity without reading a name.
pub fn preflight_app_element(point: [f64; 2], expected_role: &str, expected_label: Option<&str>) -> Result<(), ActivationDiagnostic> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::preflight_element(point, expected_role, expected_label)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (point, expected_role, expected_label);
        Err(ActivationDiagnostic::PreflightUnavailable)
    }
}

/// Revalidate and click the exact authorized element; require a state transition or correlated input-routing evidence.
/// `None` explicitly selects coordinate-and-role identity without reading a name.
pub fn activate_app_element(point: [f64; 2], expected_role: &str, expected_label: Option<&str>) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::activate_element(point, expected_role, expected_label)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (point, expected_role, expected_label);
        anyhow::bail!("WeCom element activation is supported only on macOS")
    }
}

/// Click and confirm AXFocusedUIElement matches the target — input focus acquisition.
/// `None` explicitly selects coordinate-and-role identity without reading a name.
pub fn focus_app_element(point: [f64; 2], expected_role: &str, expected_label: Option<&str>) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::focus_element(point, expected_role, expected_label)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (point, expected_role, expected_label);
        anyhow::bail!("WeCom element focus is supported only on macOS")
    }
}

/// Advisory availability for the click-focus atomic instruction.
pub fn app_element_focus_capability() -> Capability {
    #[cfg(target_os = "macos")]
    let reason = if !macos_desktop::trusted() {
        Some("macOS Accessibility permission is required for the runtime")
    } else if macos_desktop::activation_target_available().is_err() {
        Some("A visible, frontmost WeCom main or single window is required")
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let reason = Some("WeCom element focus is supported only on macOS");
    Capability {
        id: "desktop.element.focus",
        status: if reason.is_none() { CapabilityStatus::Available } else { CapabilityStatus::Unavailable },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["desktop.element.focus"],
        prerequisites: vec![
            "macOS Accessibility and Input Monitoring permissions",
            "A visible, frontmost WeCom main or single window",
            "A live AXTextField or AXTextArea at the exact point",
        ],
        limitations: vec![
            "Dispatches exactly one preallocated mouse-down/up pair and never retries.",
            "Succeeds only when the application reports the clicked element as AXFocusedUIElement.",
            "Focus acquisition is local UI evidence, not independent verification.",
        ],
        reason,
    }
}

/// Advisory availability for the single bounded Return key press.
pub fn keyboard_key_input_capability() -> Capability {
    #[cfg(target_os = "macos")]
    let reason = if !macos_desktop::trusted() {
        Some("macOS Accessibility permission is required for the runtime")
    } else if !macos_desktop::input_monitoring_available() {
        Some("macOS Input Monitoring permission is required for passive key routing evidence")
    } else if macos_desktop::opaque_target_available().is_err() {
        Some("A visible, opaque, frontmost WeCom main or single window is required")
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let reason = Some("WeCom key input is supported only on macOS");
    Capability {
        id: "desktop.keyboard.key_input",
        status: if reason.is_none() { CapabilityStatus::Available } else { CapabilityStatus::Unavailable },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["desktop.keyboard.key_input"],
        prerequisites: vec![
            "macOS Accessibility and Input Monitoring permissions",
            "A visible, opaque, frontmost WeCom main or single window",
        ],
        limitations: vec![
            "Currently accepts only key return with an empty modifier list; no text, shortcuts or other keys.",
            "Dispatches exactly one preallocated key-down/up pair and requires passive routing evidence; never retries.",
            "Does not change focus; routing is local UI evidence, not application processing or independent verification.",
        ],
        reason,
    }
}

/// Read-only validation of the bounded key press without dispatching input.
pub fn preflight_keyboard_key_input() -> Result<(), ActivationDiagnostic> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::preflight_keyboard_key_input()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(ActivationDiagnostic::PreflightUnavailable)
    }
}

/// Dispatch the single preallocated Return pair; success requires routing evidence.
pub fn press_return_key() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::press_return_key()
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("WeCom key input is supported only on macOS")
    }
}


/// Advisory availability for bounded native input-box replacement.
pub fn app_element_input_capability() -> Capability {
    #[cfg(target_os = "macos")]
    let reason = if !macos_desktop::trusted() {
        Some("macOS Accessibility permission is required for the runtime")
    } else if macos_desktop::opaque_target_available().is_err() {
        Some("A visible, opaque, frontmost WeCom main or single window is required")
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let reason = Some("Native input-box replacement is supported only on macOS");
    Capability {
        id: "desktop.element.input",
        status: if reason.is_none() { CapabilityStatus::Available } else { CapabilityStatus::Unavailable },
        channel: CapabilityChannel::MacosGui,
        required_scopes: vec!["desktop.element.input"],
        prerequisites: vec![
            "macOS Accessibility permission for the controlling process",
            "One running WeCom instance with a visible, opaque, frontmost main or single window",
            "An AXTextField or AXTextArea with a readable string value and writable AXValue and AXFocused at the authorized point",
        ],
        limitations: vec![
            "Only the selected application's current main or single window is eligible; this does not add support for other applications.",
            "AXEnabled may be absent but must not be false; the exact element, role, field geometry, window and focus are revalidated before keyboard input.",
            "Sets AX focus once, then replaces text using Command+A and exact Unicode keyboard events; no mouse click, clipboard, Enter, search selector or business action.",
            "Requires exactly float-representable global top-left logical coordinates; overlays not exposed through Accessibility cannot always be detected.",
            "Exact AXValue readback is bounded to 2 seconds and is not independent visual verification; dispatched failures are unknown and are never retried.",
        ],
        reason,
    }
}

/// Validate the independently authorized native input-box operation without effects.
pub fn preflight_app_element_input(point: [f64; 2], text: &str) -> Result<(), ActivationDiagnostic> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::preflight_input_box(point, text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (point, text);
        Err(ActivationDiagnostic::PreflightUnavailable)
    }
}

/// Dispatch a single native input-box replacement under Runtime authorization.
pub fn input_app_element_text(point: [f64; 2], text: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::input_box(point, text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (point, text);
        anyhow::bail!("Native input-box replacement is supported only on macOS")
    }
}

fn preflight_send_chat_draft(contact: &str, content: &str) -> Result<(), ActivationDiagnostic> {
    if !valid_chat_draft(contact, content) { return Err(ActivationDiagnostic::PreflightUnavailable); }
    #[cfg(target_os = "macos")]
    { macos_desktop::preflight_send_chat_draft(contact, content) }
    #[cfg(not(target_os = "macos"))]
    { Err(ActivationDiagnostic::PreflightUnavailable) }
}

fn send_chat_draft(contact: &str, content: &str) -> anyhow::Result<()> {
    anyhow::ensure!(valid_chat_draft(contact, content), "Invalid WeCom chat draft");
    #[cfg(target_os = "macos")]
    { macos_desktop::send_chat_draft(contact, content) }
    #[cfg(not(target_os = "macos"))]
    { anyhow::bail!("WeCom draft sending is supported only on macOS") }
}

/// A pasted attachment must appear as exactly one object-replacement
/// placeholder; any text means the pasteboard or editor was not ours.
#[cfg(any(target_os = "macos", test))]
fn attachment_draft_value(value: &str) -> bool {
    value == "\u{FFFC}"
}

#[cfg(test)]
mod attachment_draft_tests {
    #[test]
    fn only_a_single_placeholder_is_an_attachment_preview() {
        assert!(super::attachment_draft_value("\u{FFFC}"));
        for value in ["", "x", "\u{FFFC}\u{FFFC}", "\u{FFFC}x", " \u{FFFC}", "[图片]"] {
            assert!(!super::attachment_draft_value(value));
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn draft_contains(outer: [f64; 4], inner: [f64; 4]) -> bool {
    outer.iter().chain(inner.iter()).all(|v| v.is_finite())
        && outer[2] > 0.0 && outer[3] > 0.0 && inner[2] > 0.0 && inner[3] > 0.0
        && inner[0] >= outer[0] && inner[1] >= outer[1]
        && inner[0] + inner[2] <= outer[0] + outer[2]
        && inner[1] + inner[3] <= outer[1] + outer[3]
}

#[cfg(any(target_os = "macos", test))]
fn draft_scroll_geometry(group: [f64; 4], scroll: [f64; 4], dividers: &[[f64; 4]], buttons: &[[f64; 4]]) -> bool {
    if !draft_contains(group, scroll) { return false; }
    let relevant = |d: &&[f64; 4]| d.iter().all(|v| v.is_finite())
        && d[2] > d[3] && d[3] > 0.0 && d[0] <= scroll[0]
        && d[0] + d[2] >= scroll[0] + scroll[2] && d[1] + d[3] <= scroll[1];
    let mut matching = dividers.iter().filter(relevant);
    let Some(divider) = matching.next() else { return false; };
    if matching.next().is_some() { return false; }
    buttons.iter().any(|b| b.iter().all(|v| v.is_finite())
        && b[2] > 0.0 && b[3] > 0.0 && b[0] >= scroll[0]
        && b[0] + b[2] <= scroll[0] + scroll[2]
        && b[1] >= divider[1] + divider[3] && b[1] + b[3] <= scroll[1])
}

#[cfg(any(target_os = "macos", test))]
fn draft_column_geometry(main: [f64; 4], root: [f64; 4], header: [f64; 4],
    group: [f64; 4], editor: [f64; 4], divider_right: f64, title_bottom: f64) -> bool {
    divider_right.is_finite() && title_bottom.is_finite()
        && draft_contains(main, root) && draft_contains(root, header)
        && draft_contains(header, group) && draft_contains(group, editor)
        && group[0] >= divider_right && group[1] >= title_bottom
}

#[cfg(test)]
mod draft_geometry_tests {
    use super::{draft_column_geometry, draft_scroll_geometry};

    #[test]
    fn editor_is_below_divider_and_toolbar_in_conversation_column() {
        let main = [226.0, 171.0, 1060.0, 640.0];
        let header = [386.0, 171.0, 900.0, 640.0];
        let group = [637.0, 235.0, 649.0, 576.0];
        let editor = [637.0, 736.0, 649.0, 70.0];
        let history = [637.0, 235.0, 649.0, 460.0];
        let divider = [637.0, 695.0, 649.0, 1.0];
        let button = [648.0, 700.0, 28.0, 28.0];
        assert!(draft_scroll_geometry(group, editor, &[divider], &[button]));
        assert!(!draft_scroll_geometry(group, history, &[divider], &[button]));
        assert!(!draft_scroll_geometry(group, editor, &[divider], &[]));
        assert!(!draft_scroll_geometry(group, editor, &[divider, divider], &[button]));
        assert!(draft_column_geometry(main, main, header, group, editor, 637.0, 215.0));
        assert!(!draft_column_geometry(main, main, header, group, editor, 638.0, 215.0));
        assert!(!draft_column_geometry(main, main, header, group, editor, 637.0, 236.0));
        assert!(!draft_column_geometry(main, main, header, group, [637.0, 736.0, 650.0, 70.0], 637.0, 215.0));
    }
}

#[cfg(target_os = "macos")]
#[path = "wecom/macos.rs"]
mod macos_desktop;
