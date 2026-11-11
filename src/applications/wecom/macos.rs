use crate::kernel::platform::{DesktopApplication, Point, Size};
use crate::kernel::task::ActivationDiagnostic as Diagnostic;
use crate::platforms::macos::desktop::{self, CaptureTarget, Owned, ValueKind, DiagnosticResult, bounds};
use crate::platforms::macos::desktop::query::*;
use crate::platforms::macos::input::{PreparedInputPair, MouseMonitor, KeyboardMonitor, observe_for, click_verified, unicode_chunks};
use anyhow::{bail, ensure, Context, Result};
use std::time::{Duration, Instant};
use super::nickname_native::{ContactCode as C, ContactDispatch as D, ContactError};

const APPLICATION: DesktopApplication = DesktopApplication {
    bundle_identifier: c"com.tencent.WeWorkMac",
    display_name: "WeCom",
};
pub(super) use crate::platforms::macos::permissions::{trusted, screen_recording_allowed, input_monitoring_available};
pub(super) fn target() -> Result<desktop::Target> { desktop::target(APPLICATION) }
pub(super) fn center() -> Result<()> { desktop::center(APPLICATION) }
pub(super) fn preflight_keyboard_key_input() -> DiagnosticResult<()> {
    crate::platforms::macos::keyboard::preflight(APPLICATION)
}
pub(super) fn press_return_key() -> Result<()> {
    crate::platforms::macos::keyboard::press_return(APPLICATION)
}
pub(super) fn capture_target() -> Result<CaptureTarget> { desktop::capture_target(APPLICATION) }
pub(super) fn capture() -> Result<super::AppWindowCapture> { desktop::capture(APPLICATION) }

pub(super) fn screenshot_capture_available() -> bool {
    crate::platforms::macos::screenshot::available()
}

pub(super) fn nickname_geometry(captured: &super::AppWindowCapture) -> Result<super::nickname_native::Geometry> {
    let initial = capture_target()?;
    // The private capture record binds the actual image dimensions to its
    // PID/window/display identity. A caller-supplied geometry is not authority.
    let loaded = load_capture(&captured.capture_id)?;
    let current = capture_target()?;
    ensure!(same_search_window(&initial, &current)
        && loaded.window_bounds_points == bounds(current.position, current.size)
        && captured.window_bounds_points == loaded.window_bounds_points
        && captured.image_width == loaded.image_width && captured.image_height == loaded.image_height,
        "WeCom capture geometry changed");
    Ok(super::nickname_native::Geometry {
        pid: current.pid, window_number: current.number, display_id: current.display,
        window_bounds: bounds(current.position, current.size),
        display_bounds: bounds(current.display_bounds.origin, current.display_bounds.size),
        image_size: [loaded.image_width, loaded.image_height],
    })
}
pub(super) fn load_capture(id: &str) -> Result<super::AppWindowCapture> { desktop::load_capture(APPLICATION, id) }
pub(super) fn preflight_input_box(point: [f64; 2], text: &str) -> DiagnosticResult<()> {
    crate::platforms::macos::field::preflight(APPLICATION, point, text)
}
pub(super) fn input_box(point: [f64; 2], text: &str) -> Result<()> {
    crate::platforms::macos::field::input(APPLICATION, point, text)
}
fn window_target(capture: bool) -> Result<CaptureTarget> { desktop::window_target(APPLICATION, capture) }
use desktop::{frontmost, confirm_target_opaque, hit_target_element};
use desktop::query::children_attribute as search_children;
pub(super) fn activation_target_available() -> DiagnosticResult<()> {
    if !trusted() { return Err(Diagnostic::AccessibilityUnavailable); }
    let target = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    frontmost(&target)
}

fn element_at_point(point: [f64; 2], role: &str, label: Option<&str>) -> DiagnosticResult<Owned> {
    if !trusted() {
        return Err(Diagnostic::AccessibilityUnavailable);
    }
    if !point.iter().all(|value| value.is_finite())
        || role.is_empty() || role.len() > 256
        || label.is_some_and(|label| label.is_empty() || label.len() > 256) {
        return Err(Diagnostic::PreflightUnavailable);
    }
    // AX hit testing accepts floats; refuse rounding that could hit a
    // different element from the exact point sent to Core Graphics.
    if !point.iter().all(|value| (*value as f32) as f64 == *value) {
        return Err(Diagnostic::CoordinatePrecisionUnsupported);
    }
    let target = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    let [x, y, width, height] = bounds(target.position, target.size);
    if !(point[0] >= x && point[0] < x + width && point[1] >= y && point[1] < y + height) {
        return Err(Diagnostic::PointOutsideTargetWindow);
    }
    frontmost(&target)?;
    confirm_target_opaque(&target)?;
    let element = hit_target_element(&target, point)?;
    if !matches_string_attribute(element.as_ref(), b"AXRole\0", role)
        .map_err(|_| Diagnostic::AccessibilityRoleUnavailable)? {
        return Err(Diagnostic::AccessibilityRoleMismatch);
    }
    if let Some(label) = label {
        let name = accessible_name(element.as_ref(), &target)?;
        if !matches_string_value(name.as_ref(), label).map_err(|_| Diagnostic::AccessibilityTitleUnavailable)? {
            return Err(Diagnostic::AccessibilityTitleMismatch);
        }
    }
    // Recheck live window identity and geometry after AX reads, then
    // recheck frontmost, target opacity, and system-wide hit identity.
    let current = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    if !(current.pid == target.pid && current.number == target.number
        && (current.window.as_ref().equals(target.window.as_ref()))
        && bounds(current.position, current.size) == bounds(target.position, target.size)
        && current.display == target.display
        && bounds(current.display_bounds.origin, current.display_bounds.size)
            == bounds(target.display_bounds.origin, target.display_bounds.size)) {
        return Err(Diagnostic::TargetChanged);
    }
    frontmost(&current)?;
    confirm_target_opaque(&current)?;
    let repeated = hit_target_element(&current, point)?;
    if !(element.as_ref().equals(repeated.as_ref())) {
        return Err(Diagnostic::TargetElementMismatch);
    }
    Ok(element)
}

pub(super) fn preflight_element(point: [f64; 2], role: &str, label: Option<&str>) -> DiagnosticResult<()> {
    element_at_point(point, role, label).map(|_| ())
}
pub(super) fn opaque_target_available() -> DiagnosticResult<()> {
    if !trusted() {
        return Err(Diagnostic::AccessibilityUnavailable);
    }
    let target = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    frontmost(&target)?;
    confirm_target_opaque(&target)
}
fn search_parent(element: Element<'_>, target: &CaptureTarget, role: &str) -> DiagnosticResult<Owned> {
    let parent = attribute(element, b"AXParent\0").map_err(|_| Diagnostic::TargetElementMismatch)?;
    if parent.as_ref().kind() != ValueKind::Element { return Err(Diagnostic::TargetElementMismatch); }
    if parent.as_ref().pid().ok() != Some(target.pid)
        || !element_window_matches(parent.as_ref(), target).map_err(|_| Diagnostic::TargetElementMismatch)?
        || !matches_string_attribute(parent.as_ref(), b"AXRole\0", role)
            .map_err(|_| Diagnostic::AccessibilityRoleUnavailable)? {
        return Err(Diagnostic::TargetElementMismatch);
    }
    let children = search_children(parent.as_ref())?.ok_or(Diagnostic::TargetElementMismatch)?;
    let count = children.as_ref().array_count();
    if !(1..=2048).contains(&count) {
        return Err(Diagnostic::TargetElementMismatch);
    }
    let mut occurrences = 0;
    for index in 0..count {
        let child = children.as_ref().array_at(index);
        if child.is_null() || (child.kind() != ValueKind::Element) {
            return Err(Diagnostic::PreflightUnavailable);
        }
        if child.equals(element) { occurrences += 1; }
    }
    if occurrences != 1 {
        return Err(Diagnostic::TargetElementMismatch);
    }
    Ok(parent)
}
fn search_field_structure(target: &CaptureTarget, field: Element<'_>, point: [f64; 2]) -> DiagnosticResult<()> {
    if !matches_string_attribute(field, b"AXRole\0", "AXTextField")
        .map_err(|_| Diagnostic::AccessibilityRoleUnavailable)? {
        return Err(Diagnostic::AccessibilityRoleMismatch);
    }
    let (position, size) = geometry(field).map_err(|_| Diagnostic::PreflightUnavailable)?;
    let right = position.x + size.width;
    let bottom = position.y + size.height;
    let window_right = target.position.x + target.size.width;
    let header_bottom = target.position.y + target.size.height.min(80.0);
    if !right.is_finite() || !bottom.is_finite() || size.height > 60.0
        || position.x < target.position.x || right > window_right
        || position.y < target.position.y || bottom > header_bottom
        || point[0] < position.x || point[0] >= right
        || point[1] < position.y || point[1] >= bottom {
        return Err(Diagnostic::TargetElementMismatch);
    }
    if boolean_attribute(field, b"AXEnabled\0") != Some(true)
        || boolean_attribute(field, b"AXFocused\0").is_none() {
        return Err(Diagnostic::PreflightUnavailable);
    }
    attribute_writable(field, b"AXFocused\0")?;
    let value = attribute(field, b"AXValue\0").map_err(|_| Diagnostic::PreflightUnavailable)?;
    if value.as_ref().kind() != ValueKind::String { return Err(Diagnostic::PreflightUnavailable); }
    let parent = search_parent(field, target, "AXSplitGroup")?;
    let split = search_parent(parent.as_ref(), target, "AXSplitGroup")?;
    let window = search_parent(split.as_ref(), target, "AXWindow")?;
    if !(window.as_ref().equals(target.window.as_ref()))
        || (parent.as_ref().equals(split.as_ref())) {
        return Err(Diagnostic::TargetElementMismatch);
    }
    let children = search_children(parent.as_ref())?.ok_or(Diagnostic::TargetElementMismatch)?;
    let count = children.as_ref().array_count();
    if !(1..=2048).contains(&count) {
        return Err(Diagnostic::TargetElementMismatch);
    }
    // Current WeCom exposes the chat composer as a sibling AXTextField in the
    // header band. Ambiguity is resolved only by an observed vertical AXSplitter
    // that separates this field's left column from every other header text
    // field; anything else fails closed rather than guessing by position.
    let mut dividers = Vec::new();
    let mut header_fields = Vec::new();
    let mut target_occurrences = 0;
    for index in 0..count {
        let child = children.as_ref().array_at(index);
        if child.is_null() || (child.kind() != ValueKind::Element) {
            return Err(Diagnostic::PreflightUnavailable);
        }
        let role = if matches_string_attribute(child, b"AXRole\0", "AXSplitter")
            .map_err(|_| Diagnostic::AccessibilityRoleUnavailable)? {
            "AXSplitter"
        } else if matches_string_attribute(child, b"AXRole\0", "AXTextField")
            .map_err(|_| Diagnostic::AccessibilityRoleUnavailable)? {
            "AXTextField"
        } else {
            continue;
        };
        let (child_position, child_size) = geometry(child).map_err(|_| Diagnostic::PreflightUnavailable)?;
        if !child_size.width.is_finite() || !child_size.height.is_finite() {
            return Err(Diagnostic::PreflightUnavailable);
        }
        if role == "AXSplitter" {
            if child_size.width > 0.0 && child_size.height > child_size.width {
                dividers.push((child_position.x, child_position.y, child_position.y + child_size.height));
            }
            continue;
        }
        if child_size.width <= 0.0 || child_size.height <= 0.0 {
            return Err(Diagnostic::TargetElementMismatch);
        }
        if child.equals(field) { target_occurrences += 1; }
        if child_position.y < header_bottom && child_position.y + child_size.height > target.position.y {
            header_fields.push((
                (child.equals(field)),
                child_position.x,
                child_position.x + child_size.width,
                child_position.y,
                child_position.y + child_size.height,
            ));
        }
    }
    if target_occurrences != 1 {
        return Err(Diagnostic::TargetElementMismatch);
    }
    if !header_fields.iter().any(|(is_target, _, _, _, _)| *is_target) {
        return Err(Diagnostic::TargetElementMismatch);
    }
    if header_fields.len() > 1 {
        let Some((divider_x, _, _)) = dividers.iter().copied().find(|(x, top, bottom)| {
            *x > right && *top < header_bottom && *bottom > target.position.y
        }) else {
            return Err(Diagnostic::TargetElementMismatch);
        };
        if header_fields.iter().any(|(is_target, left_edge, _, _, _)| {
            !*is_target && *left_edge <= divider_x
        }) {
            return Err(Diagnostic::TargetElementMismatch);
        }
    }
    Ok(())
}

fn same_search_window(a: &CaptureTarget, b: &CaptureTarget) -> bool {
    a.pid == b.pid && a.number == b.number && (a.window.as_ref().equals(b.window.as_ref()))
        && bounds(a.position, a.size) == bounds(b.position, b.size)
        && a.display == b.display
        && bounds(a.display_bounds.origin, a.display_bounds.size) == bounds(b.display_bounds.origin, b.display_bounds.size)
}

fn search_field_at_point(point: [f64; 2]) -> DiagnosticResult<(CaptureTarget, Owned)> {
    if !trusted() {
        return Err(Diagnostic::AccessibilityUnavailable);
    }
    if !point.iter().all(|value| value.is_finite()) {
        return Err(Diagnostic::PreflightUnavailable);
    }
    if !point.iter().all(|value| (*value as f32) as f64 == *value) {
        return Err(Diagnostic::CoordinatePrecisionUnsupported);
    }
    let target = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    if point[0] < target.position.x || point[0] >= target.position.x + target.size.width
        || point[1] < target.position.y || point[1] >= target.position.y + target.size.height {
        return Err(Diagnostic::PointOutsideTargetWindow);
    }
    frontmost(&target)?;
    confirm_target_opaque(&target)?;
    let field = hit_target_element(&target, point)?;
    let before = geometry(field.as_ref()).map_err(|_| Diagnostic::PreflightUnavailable)?;
    search_field_structure(&target, field.as_ref(), point)?;
    let current = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    if !same_search_window(&target, &current) {
        return Err(Diagnostic::TargetChanged);
    }
    frontmost(&current)?;
    confirm_target_opaque(&current)?;
    let repeated = hit_target_element(&current, point)?;
    let after = geometry(repeated.as_ref()).map_err(|_| Diagnostic::PreflightUnavailable)?;
    if !(field.as_ref().equals(repeated.as_ref()))
        || bounds(before.0, before.1) != bounds(after.0, after.1) {
        return Err(Diagnostic::TargetElementMismatch);
    }
    Ok((current, field))
}

fn point_in_rect(point: [f64; 2], rect: [f64; 4]) -> bool {
    point.iter().chain(rect.iter()).all(|v| v.is_finite())
        && point.iter().all(|v| (*v as f32) as f64 == *v)
        && rect[2] > 0.0 && rect[3] > 0.0
        && point[0] >= rect[0] && point[0] < rect[0] + rect[2]
        && point[1] >= rect[1] && point[1] < rect[1] + rect[3]
}

fn nickname_messages_target(point: [f64; 2]) -> Result<Owned> {
    // Current WeCom exposes this navigation item as an unnamed AXCell.
    // Semantic localization comes from fresh vision on a cache miss; reuse
    // still requires a live cell in the main window's left navigation column,
    // not a search-popup or conversation-history cell.
    let cell = element_at_point(point, "AXCell", None)
        .map_err(|_| anyhow::anyhow!("WeCom navigation target unavailable"))?;
    let main = window_target(false)?;
    let (p, s) = geometry(cell.as_ref())?;
    ensure!(point_in_rect(point, bounds(p, s))
        && p.x == main.position.x && s.width <= main.size.width / 3.0
        && p.y >= main.position.y && p.y + s.height <= main.position.y + main.size.height,
        "WeCom navigation cell outside navigation column");
    let mut child = cell.as_ref().retained();
    for role in ["AXRow", "AXTable", "AXScrollArea", "AXSplitGroup", "AXWindow"] {
        child = search_parent(child.as_ref(), &main, role)
            .map_err(|_| anyhow::anyhow!("WeCom navigation ancestry changed"))?;
    }
    ensure!(child.as_ref().equals(main.window.as_ref()), "WeCom navigation window changed");
    let repeated = element_at_point(point, "AXCell", None)
        .map_err(|_| anyhow::anyhow!("WeCom navigation target changed"))?;
    ensure!(cell.as_ref().equals(repeated.as_ref()), "WeCom navigation target changed");
    Ok(cell)
}

pub(super) fn nickname_validate_control(
    control: super::nickname_native::Control, point: [f64; 2], nickname: &str,
) -> Result<()> {
    match control {
        super::nickname_native::Control::Messages => { nickname_messages_target(point)?; }
        super::nickname_native::Control::Search => {
            search_field_at_point(point).map_err(|_| anyhow::anyhow!("WeCom search target unavailable"))?;
        }
        super::nickname_native::Control::Composer => {
            let a = draft_target(nickname)?;
            ensure!(point_in_rect(point, a.rect) && draft_value_matches(a.editor.as_ref(), "")?,
                "WeCom composer point or empty draft mismatch");
            let hit = hit_target_element(&a.main, point).map_err(|_| anyhow::anyhow!("WeCom composer hit unavailable"))?;
            ensure!(hit.as_ref().equals(a.editor.as_ref()), "WeCom composer point mismatch");
            let b = draft_target(nickname)?;
            ensure!(same_draft(&a, &b) && draft_value_matches(b.editor.as_ref(), "")?,
                "WeCom composer changed");
        }
    }
    Ok(())
}

pub(super) fn nickname_activate_messages(point: [f64; 2]) -> Result<()> {
    let initial = nickname_messages_target(point)?;
    let token = i64::from_ne_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
    let tap = MouseMonitor::new(initial.as_ref().pid()?, point, token)
        .context("Input monitoring unavailable")?;
    ensure!(tap.enabled(), "Input monitoring unavailable");
    let click = PreparedInputPair::mouse(point, token)?;
    let current = nickname_messages_target(point)?;
    ensure!(initial.as_ref().equals(current.as_ref()) && tap.enabled(),
        "WeCom navigation changed before dispatch");
    click.post();
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        ensure!(Instant::now() < deadline && tap.enabled(), "WeCom navigation outcome unknown");
        let current = nickname_messages_target(point)?;
        ensure!(initial.as_ref().equals(current.as_ref()), "WeCom navigation changed after dispatch");
        if tap.complete() { return Ok(()); }
        observe_for(Duration::from_millis(10));
    }
}


fn search_focus_matches(app: Element<'_>, field: Element<'_>) -> Result<bool> {
    let focused = attribute(app, b"AXFocusedUIElement\0")?;
    Ok((focused.as_ref().kind() == ValueKind::Element && focused.as_ref().equals(field))
        && boolean_attribute(field, b"AXFocused\0") == Some(true))
}

fn search_value_matches(field: Element<'_>, query: &str) -> Result<bool> {
    let actual = attribute(field, b"AXValue\0")?;
    bounded_utf8_matches(actual.as_ref(), query)
}

fn revalidate_search_field(target: &CaptureTarget, field: Element<'_>, point: [f64; 2]) -> Result<()> {
    let (current, hit) = search_field_at_point(point)
        .map_err(|_| anyhow::anyhow!("WeCom search target validation failed"))?;
    ensure!(same_search_window(target, &current) && (field.equals(hit.as_ref())),
        "WeCom search target changed");
    Ok(())
}
fn validate_search_input_target(
    target: &CaptureTarget, field: Element<'_>, app: Element<'_>, point: [f64; 2],
    original_geometry: (Point, Size),
) -> Result<()> {
    revalidate_search_field(target, field, point)?;
    let current_geometry = geometry(field)?;
    ensure!(bounds(original_geometry.0, original_geometry.1) == bounds(current_geometry.0, current_geometry.1),
        "WeCom search field geometry changed");
    frontmost(target).map_err(|_| anyhow::anyhow!("WeCom search window is no longer active"))?;
    ensure!(search_focus_matches(app, field)?, "WeCom search focus changed before input");
    Ok(())
}
pub(super) fn set_search_query(point: [f64; 2], query: &str) -> Result<()> {
    let (target, field) = search_field_at_point(point)
        .map_err(|_| anyhow::anyhow!("WeCom search target validation failed"))?;
    let original_geometry = geometry(field.as_ref())?;
    let app = application(target.pid)?;
    // Allocate every event before focus or keyboard side effects.
    let focus_token = i64::from_ne_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
    let focus_tap = MouseMonitor::new(target.pid, point, focus_token)
        .ok_or_else(|| anyhow::anyhow!("WeCom search focus observation unavailable"))?;
    let focus_click = PreparedInputPair::mouse(point, focus_token)?;
    let select_all = PreparedInputPair::keyboard(0, 1 << 20, None, None)?;
    let chunks = unicode_chunks(query);
    let token = i64::from_ne_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
    let tap = KeyboardMonitor::for_key(target.pid, token, 0)
        .ok_or_else(|| anyhow::anyhow!("WeCom search input observation unavailable"))?;
    let events = chunks.iter().enumerate().map(|(index, chunk)| {
        PreparedInputPair::keyboard(0, 0, Some(chunk), (index + 1 == chunks.len()).then_some(token))
    }).collect::<Result<Vec<_>>>()?;
    ensure!(tap.enabled() && focus_tap.enabled(), "WeCom search observation unavailable");
    revalidate_search_field(&target, field.as_ref(), point)?;
    // From this call onward every error is an unknown outcome; never retry.
    // A real, single activation reopens search results even for an unchanged query.
    // AXFocused alone can leave the field focused while its popup stays dismissed.
    focus_click.post();
    let focus_deadline = Instant::now() + Duration::from_millis(500);
    loop {
        ensure!(focus_tap.enabled() && tap.enabled(), "WeCom search observation lost");
        revalidate_search_field(&target, field.as_ref(), point)?;
        if focus_tap.complete() && search_focus_matches(app.as_ref(), field.as_ref())? { break; }
        ensure!(Instant::now() < focus_deadline, "WeCom search activation unconfirmed");
        observe_for(Duration::from_millis(10).min(focus_deadline.saturating_duration_since(Instant::now())));
    }
    validate_search_input_target(&target, field.as_ref(), app.as_ref(), point, original_geometry)?;
    select_all.post();
    for event in events {
        validate_search_input_target(&target, field.as_ref(), app.as_ref(), point, original_geometry)?;
        event.post();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        // Read-only observation, not another input attempt.
        ensure!(tap.enabled(), "WeCom search input observation lost");
        let current = window_target(false).context("WeCom search window unavailable after writing")?;
        ensure!(same_search_window(&target, &current), "WeCom search window changed after writing");
        frontmost(&current).map_err(|_| anyhow::anyhow!("WeCom search window is no longer active"))?;
        confirm_target_opaque(&current).map_err(|_| anyhow::anyhow!("WeCom search window is no longer opaque"))?;
        let hit = hit_target_element(&current, point)
            .map_err(|_| anyhow::anyhow!("WeCom search hit changed after writing"))?;
        ensure!((field.as_ref().equals(hit.as_ref())), "WeCom search target changed after writing");
        let field_geometry = geometry(field.as_ref())?;
        ensure!(bounds(original_geometry.0, original_geometry.1) == bounds(field_geometry.0, field_geometry.1),
            "WeCom search field geometry changed after writing");
        ensure!(search_focus_matches(app.as_ref(), field.as_ref())?, "WeCom search focus changed after writing");
        let matches = search_value_matches(field.as_ref(), query)?;
        frontmost(&target).map_err(|_| anyhow::anyhow!("WeCom search window is no longer active"))?;
        ensure!(Instant::now() < deadline, "WeCom search readback timed out");
        if matches && tap.complete() {
            return Ok(());
        }
        observe_for(Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())));
    }
}
pub(super) fn activate_element(point: [f64; 2], role: &str, label: Option<&str>) -> Result<()> {
    let initial = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
    let location = point;
    let pid = initial.as_ref().pid()?;
    ensure!(pid > 0,
        "Could not identify activation target process");
    let token = i64::from_ne_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
    let tap = MouseMonitor::new(pid, location, token);
    // Allocate both events before dispatch so allocation failure cannot leave
    // a pressed button behind. There is no retry after either event is posted.
    let click = PreparedInputPair::mouse(point, token)?;
    let element = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
    ensure!((initial.as_ref().equals(element.as_ref())), "The hit-tested element changed during validation");
    let focused = boolean_attribute(element.as_ref(), b"AXFocused\0");
    let selected = boolean_attribute(element.as_ref(), b"AXSelected\0");
    let dispatch_element = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
    ensure!((element.as_ref().equals(dispatch_element.as_ref())), "The hit-tested element changed before dispatch");
    click.post();
    let verified = (|| -> Result<bool> {
        // Observe only; never dispatch another click while waiting.
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if Instant::now() >= deadline {
                return Ok(false);
            }
            let current = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
            ensure!((current.as_ref().equals(element.as_ref())), "Target changed after dispatch");
            if tap.as_ref().is_some_and(|tap| !tap.enabled()) {
                return Ok(false);
            }
            let routed = tap.as_ref().is_some_and(|tap| tap.complete());
            let verified = click_verified(focused, boolean_attribute(current.as_ref(), b"AXFocused\0"),
                selected, boolean_attribute(current.as_ref(), b"AXSelected\0"), routed);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            if verified {
                return Ok(true);
            }
            if tap.is_some() {
                observe_for(remaining.min(Duration::from_millis(20)));
            } else {
                std::thread::sleep(Duration::from_millis(20).min(remaining));
            }
        }
    })();
    ensure!(matches!(verified, Ok(true)),
        "Activation was dispatched but its outcome is unknown");
    Ok(())
}
pub(super) fn focus_element(point: [f64; 2], role: &str, label: Option<&str>) -> Result<()> {
    let initial = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Focus validation failed"))?;
    let location = point;
    let pid = initial.as_ref().pid()?;
    ensure!(pid > 0, "Could not identify focus target process");
    let token = i64::from_ne_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
    let tap = MouseMonitor::new(pid, location, token);
    let click = PreparedInputPair::mouse(point, token)?;
    let element = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Focus validation failed"))?;
    ensure!(initial.as_ref().equals(element.as_ref()), "The hit-tested element changed during validation");
    let dispatch_element = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Focus validation failed"))?;
    ensure!(element.as_ref().equals(dispatch_element.as_ref()), "The hit-tested element changed before dispatch");
    let app = application(pid)?;
    click.post();
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        if Instant::now() >= deadline {
            bail!("Focus click was dispatched but focus did not arrive");
        }
        let current = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Focus validation failed"))?;
        ensure!(current.as_ref().equals(element.as_ref()), "Target changed after dispatch");
        if tap.as_ref().is_some_and(|t| !t.enabled()) {
            bail!("Focus click tap monitoring lost");
        }
        let routed = tap.as_ref().is_some_and(|t| t.complete());
        if routed && search_focus_matches(app.as_ref(), current.as_ref())? {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("Focus click was dispatched but focus outcome is unknown");
        }
        if tap.is_some() {
            observe_for(remaining.min(Duration::from_millis(20)));
        } else {
            std::thread::sleep(Duration::from_millis(20).min(remaining));
        }
    }
}


struct ChatTarget {
    main: CaptureTarget, field: Owned, popup: Owned, number: u32,
    popup_bounds: [f64; 4], cell: Owned, cell_bounds: [f64; 4], point: [f64; 2],
}

fn chat_children(element: Element<'_>) -> Result<Vec<Owned>> {
    let array = search_children(element).map_err(|_| anyhow::anyhow!("Cannot read accessibility children"))?
        .context("Missing accessibility children")?;
    let count = array.as_ref().array_count();
    ensure!((0..=2048).contains(&count), "Invalid accessibility child count");
    let mut result = Vec::with_capacity(count as usize);
    for i in 0..count {
        let child = array.as_ref().array_at(i);
        ensure!(!child.is_null() && (child.kind() == ValueKind::Element), "Invalid accessibility child");
        result.push(child.retained());
    }
    Ok(result)
}

fn chat_role(element: Element<'_>, role: &str) -> Result<bool> {
    matches_string_attribute(element, b"AXRole\0", role)
}

// Search rows may contain unnamed AXStaticText placeholders. Absence is not
// identity evidence; malformed values and failed AX reads still fail closed.
fn optional_chat_name<T>(
    title: desktop::NameAttribute<T>,
    value: impl FnOnce() -> DiagnosticResult<desktop::NameAttribute<T>>,
) -> DiagnosticResult<Option<T>> {
    use desktop::NameAttribute::{Invalid, MissingOrEmpty, Nonempty};
    match title {
        Nonempty(name) => Ok(Some(name)),
        Invalid => Err(Diagnostic::AccessibilityTitleUnavailable),
        MissingOrEmpty => match value()? {
            Nonempty(name) => Ok(Some(name)),
            MissingOrEmpty => Ok(None),
            Invalid => Err(Diagnostic::AccessibilityTitleUnavailable),
        },
    }
}

fn chat_text(element: Element<'_>, expected: &str) -> Result<bool> {
    let value = name_attribute(element, b"AXTitle\0")
        .and_then(|title| optional_chat_name(title, || name_attribute(element, b"AXValue\0")))
        .map_err(|_| contact_error(C::PopupTreeUnavailable))?;
    match value {
        Some(value) => matches_string_value(value.as_ref(), expected),
        None => Ok(false),
    }
}

#[cfg(test)]
mod contact_text_tests {
    use super::*;
    use desktop::NameAttribute::{Invalid, MissingOrEmpty, Nonempty};

    #[test]
    fn unnamed_text_does_not_supply_contact_section_or_bot_identity() {
        let absent = optional_chat_name::<&str>(MissingOrEmpty, || Ok(MissingOrEmpty)).unwrap();
        for required in ["联系人", "exact nickname", "BOT"] {
            assert!(!absent.is_some_and(|text| text == required));
        }
        assert_eq!(optional_chat_name(MissingOrEmpty, || Ok(Nonempty("exact nickname"))).unwrap(), Some("exact nickname"));
    }

    #[test]
    fn present_title_is_authoritative_and_invalid_or_failed_reads_stop_selection() {
        assert_eq!(optional_chat_name(Nonempty("other"), || panic!("must not replace an existing title")).unwrap(), Some("other"));
        assert!(optional_chat_name::<&str>(Invalid, || Ok(Nonempty("exact nickname"))).is_err());
        assert!(optional_chat_name::<&str>(MissingOrEmpty, || Ok(Invalid)).is_err());
        assert!(optional_chat_name::<&str>(MissingOrEmpty, || Err(Diagnostic::PreflightUnavailable)).is_err());
    }
}
fn chat_popup_anchored(main: [f64; 4], field: [f64; 4], popup: [f64; 4]) -> bool {
    main.iter().chain(field.iter()).chain(popup.iter()).all(|v| v.is_finite())
        && main[2] > 0.0 && main[3] > 0.0 && field[2] > 0.0 && field[3] > 0.0
        && popup[2] > 0.0 && popup[3] > 0.0
        && (popup[0] - field[0]).abs() <= 4.0
        && (popup[1] - field[1] - field[3]).abs() <= 4.0
        && popup[2] >= field[2] && popup[0] >= main[0] && popup[1] >= main[1]
        && popup[0] + popup[2] <= main[0] + main[2]
        && popup[1] + popup[3] <= main[1] + main[3]
}

#[cfg(test)]
mod chat_anchor_tests {
    use super::chat_popup_anchored;

    #[test]
    fn popup_requires_live_search_anchor_and_main_containment() {
        let main = [226.0, 171.0, 1060.0, 640.0];
        let field = [401.0, 186.0, 180.0, 34.0];
        assert!(chat_popup_anchored(main, field, [402.0, 219.0, 599.0, 149.0]));
        assert!(!chat_popup_anchored(main, field, [406.0, 219.0, 599.0, 149.0]));
        assert!(!chat_popup_anchored(main, field, [402.0, 225.0, 599.0, 149.0]));
        assert!(!chat_popup_anchored(main, field, [402.0, 219.0, 179.0, 149.0]));
        assert!(!chat_popup_anchored(main, field, [402.0, 219.0, 1000.0, 149.0]));
        assert!(!chat_popup_anchored(main, field, [f64::NAN, 219.0, 599.0, 149.0]));
    }

    #[test]
    fn projected_point_must_be_precise_and_inside_selected_target() {
        let rect = [184.0, 277.0, 160.0, 32.0];
        assert!(super::point_in_rect([200.0, 290.0], rect));
        assert!(!super::point_in_rect([344.0, 290.0], rect));
        assert!(!super::point_in_rect([200.0, 309.0], rect));
        assert!(!super::point_in_rect([183.0, 290.0], rect));
        assert!(!super::point_in_rect([f64::NAN, 290.0], rect));
        assert!(!super::point_in_rect([200.1, 290.0], rect));
        assert!(!super::point_in_rect([200.0, 290.0], [184.0, 277.0, 0.0, 32.0]));
    }
}

fn chat_popup(main: &CaptureTarget, field: Element<'_>) -> Result<(Owned, u32, [f64; 4])> {
    let (fp, fs) = geometry(field).map_err(|_| contact_error(C::SearchGeometryUnavailable))?;
    let app = application(main.pid).map_err(|_| contact_error(C::WindowListUnavailable))?;
    let windows = attribute(app.as_ref(), b"AXWindows\0").map_err(|_| contact_error(C::WindowListUnavailable))?;
    if windows.as_ref().kind() != ValueKind::Array { return Err(contact_error(C::WindowListUnavailable)); }
    let count = windows.as_ref().array_count();
    if !(1..=256).contains(&count) { return Err(contact_error(C::WindowListUnavailable)); }
    let mut found = None;
    for i in 0..count {
        let window = windows.as_ref().array_at(i);
        if window.is_null() || window.kind() != ValueKind::Element { return Err(contact_error(C::WindowListUnavailable)); }
        if window.equals(main.window.as_ref()) { continue; }
        let (p, s) = geometry(window).map_err(|_| contact_error(C::WindowListUnavailable))?;
        if !chat_popup_anchored(bounds(main.position, main.size), bounds(fp, fs), bounds(p, s)) { continue; }
        if !chat_role(window, "AXWindow").map_err(|_| contact_error(C::PopupTreeUnavailable))? {
            return Err(contact_error(C::PopupTreeUnavailable));
        }
        let id = desktop::visible_window_number(main.pid, p, s).map_err(|_| contact_error(C::PopupUnavailable))?;
        if found.is_some() { return Err(contact_error(C::PopupTreeUnavailable)); }
        found = Some((window.retained(), id, bounds(p, s)));
    }
    found.ok_or_else(|| contact_error(C::PopupUnavailable))
}

fn chat_has_bot(cell: Element<'_>) -> Result<bool> {
    let mut pending = vec![(cell.retained(), 0_usize)];
    let mut count = 0;
    let mut bot = false;
    while let Some((node, depth)) = pending.pop() {
        count += 1;
        ensure!(count <= 256 && depth <= 8, "Contact row tree exceeds limit");
        if (chat_role(node.as_ref(), "AXStaticText")? || chat_role(node.as_ref(), "AXTextArea")?)
            && chat_text(node.as_ref(), "BOT")? { bot = true; }
        if let Some(array) = search_children(node.as_ref()).map_err(|_| anyhow::anyhow!("Cannot read contact row"))? {
            let n = array.as_ref().array_count();
            ensure!((0..=256).contains(&n) && (depth < 8 || n == 0), "Contact row tree exceeds limit");
            for i in 0..n {
                let child = array.as_ref().array_at(i);
                ensure!(!child.is_null() && (child.kind() == ValueKind::Element), "Invalid contact row child");
                pending.push((child.retained(), depth + 1));
            }
        }
    }
    Ok(bot)
}

fn chat_cell(popup: Element<'_>, contact: &str) -> Result<Owned> {
    let mut pending = vec![(popup.retained(), 0_usize)];
    let mut tables = Vec::new();
    let mut visited = 0;
    while let Some((node, depth)) = pending.pop() {
        visited += 1;
        ensure!(visited <= 2048 && depth <= 8, "Popup tree exceeds limit");
        if chat_role(node.as_ref(), "AXTable")? { tables.push(node); continue; }
        if let Some(array) = search_children(node.as_ref()).map_err(|_| anyhow::anyhow!("Cannot read popup children"))? {
            let count = array.as_ref().array_count();
            ensure!((0..=2048).contains(&count) && (depth < 8 || count == 0), "Popup tree exceeds limit");
            for i in 0..count {
                let child = array.as_ref().array_at(i);
                ensure!(!child.is_null() && (child.kind() == ValueKind::Element), "Invalid popup child");
                pending.push((child.retained(), depth + 1));
            }
        }
    }
    if tables.is_empty() { return Err(contact_error(C::ContactResultUnavailable)); }
    if tables.len() > 1 { return Err(contact_error(C::ContactResultAmbiguous)); }
    let mut section = false;
    let mut headers = 0;
    let mut found = None;
    for row in chat_children(tables[0].as_ref())? {
        if chat_role(row.as_ref(), "AXColumn")? { continue; }
        ensure!(chat_role(row.as_ref(), "AXRow")?, "Invalid search row");
        let cells = chat_children(row.as_ref())?;
        ensure!(cells.len() == 1 && chat_role(cells[0].as_ref(), "AXCell")?, "Invalid search cell");
        let mut header = false;
        let mut exact = 0;
        let mut bot = false;
        for item in chat_children(cells[0].as_ref())? {
            if chat_role(item.as_ref(), "AXTextArea")? {
                header = true;
                section = chat_text(item.as_ref(), "联系人")?;
                if section { headers += 1; }
            } else if chat_role(item.as_ref(), "AXStaticText")? {
                if chat_text(item.as_ref(), contact)? { exact += 1; }
                if chat_text(item.as_ref(), "BOT")? { bot = true; }
            }
        }
        if !header && section && exact > 0 {
            if exact != 1 || found.is_some() { return Err(contact_error(C::ContactResultAmbiguous)); }
            if bot || chat_has_bot(cells[0].as_ref())? { return Err(contact_error(C::ContactResultBot)); }
            found = Some(cells[0].as_ref().retained());
        }
    }
    if headers != 1 { return Err(contact_error(C::ContactSectionUnavailable)); }
    found.ok_or_else(|| contact_error(C::ContactResultUnavailable))
}

fn contact_error(code: C) -> anyhow::Error {
    ContactError { dispatch: D::NotDispatched, code }.into()
}

fn chat_target(contact: &str) -> Result<ChatTarget> {
    let initial = window_target(false).map_err(|_| contact_error(C::MainWindowUnavailable))?;
    frontmost(&initial).map_err(|_| contact_error(C::MainWindowNotFrontmost))?;
    let app = application(initial.pid).map_err(|_| contact_error(C::SearchFocusUnavailable))?;
    let focused = attribute(app.as_ref(), b"AXFocusedUIElement\0").map_err(|_| contact_error(C::SearchFocusUnavailable))?;
    if focused.as_ref().kind() != ValueKind::Element { return Err(contact_error(C::SearchFocusUnavailable)); }
    let (p, s) = geometry(focused.as_ref()).map_err(|_| contact_error(C::SearchGeometryUnavailable))?;
    let (main, field) = search_field_at_point([p.x + s.width / 2.0, p.y + s.height / 2.0])
        .map_err(|_| contact_error(C::SearchAnchorUnavailable))?;
    if !same_search_window(&initial, &main) { return Err(contact_error(C::ContactTargetChanged)); }
    if !field.as_ref().equals(focused.as_ref())
        || !search_focus_matches(app.as_ref(), field.as_ref()).map_err(|_| contact_error(C::SearchFocusUnavailable))? {
        return Err(contact_error(C::SearchFocusUnavailable));
    }
    if !search_value_matches(field.as_ref(), contact).map_err(|_| contact_error(C::SearchQueryMismatch))? {
        return Err(contact_error(C::SearchQueryMismatch));
    }
    let (popup, number, popup_bounds) = chat_popup(&main, field.as_ref())?;
    let cell = chat_cell(popup.as_ref(), contact).map_err(|error| {
        if error.downcast_ref::<ContactError>().is_some() { error } else { contact_error(C::PopupTreeUnavailable) }
    })?;
    if cell.as_ref().pid().ok() != Some(main.pid)
        || boolean_attribute(cell.as_ref(), b"AXEnabled\0") != Some(true) {
        return Err(contact_error(C::ContactCellUnavailable));
    }
    let cell_window = attribute(cell.as_ref(), b"AXWindow\0").map_err(|_| contact_error(C::ContactCellWindowMismatch))?;
    if cell_window.as_ref().kind() != ValueKind::Element || !cell_window.as_ref().equals(popup.as_ref()) {
        return Err(contact_error(C::ContactCellWindowMismatch));
    }
    let (p, s) = geometry(cell.as_ref()).map_err(|_| contact_error(C::ContactCellGeometryUnavailable))?;
    let point = [p.x + s.width / 2.0, p.y + s.height / 2.0];
    if !point.iter().all(|v| v.is_finite() && (*v as f32) as f64 == *v)
        || p.x < popup_bounds[0] || p.y < popup_bounds[1]
        || p.x + s.width > popup_bounds[0] + popup_bounds[2]
        || p.y + s.height > popup_bounds[1] + popup_bounds[3] {
        return Err(contact_error(C::ContactCellGeometryUnavailable));
    }
    let hit = hit_test(point).map_err(|_| contact_error(C::ContactHitTestUnavailable))?;
    if !hit.as_ref().equals(cell.as_ref()) { return Err(contact_error(C::ContactHitMismatch)); }
    Ok(ChatTarget { main, field, popup, number, popup_bounds, cell, cell_bounds: bounds(p, s), point })
}

fn chat_target_at(contact: &str, point: [f64; 2]) -> Result<ChatTarget> {
    let target = chat_target(contact)?;
    if !point_in_rect(point, target.cell_bounds) { return Err(contact_error(C::ContactPointOutside)); }
    // Vision selects the independently identified row, not a child control
    // within it. chat_target has hit-tested the row's live AX center exactly;
    // retain that point for dispatch rather than the model's arbitrary center.
    Ok(target)
}

fn same_chat(a: &ChatTarget, b: &ChatTarget) -> bool {
    same_search_window(&a.main, &b.main) && (a.field.as_ref().equals(b.field.as_ref()) && a.popup.as_ref().equals(b.popup.as_ref()) && a.cell.as_ref().equals(b.cell.as_ref())) && a.number == b.number && a.popup_bounds == b.popup_bounds
        && a.cell_bounds == b.cell_bounds && a.point == b.point
}

#[derive(Clone, PartialEq)]
pub(super) struct ContactBinding {
    pid: i32,
    main_number: u32,
    display: u32,
    display_bounds: [f64; 4],
    main_bounds: [f64; 4],
    popup_number: u32,
    popup_bounds: [f64; 4],
    cell_bounds: [f64; 4],
}

fn contact_binding(target: &ChatTarget) -> ContactBinding {
    ContactBinding {
        pid: target.main.pid,
        main_number: target.main.number,
        display: target.main.display,
        display_bounds: bounds(target.main.display_bounds.origin, target.main.display_bounds.size),
        main_bounds: bounds(target.main.position, target.main.size),
        popup_number: target.number,
        popup_bounds: target.popup_bounds,
        cell_bounds: target.cell_bounds,
    }
}

fn live_contact_binding(contact: &str, expected: &ContactBinding) -> Result<()> {
    let target = chat_target(contact)?;
    if contact_binding(&target) != *expected {
        return Err(contact_error(C::ContactCaptureChanged));
    }
    Ok(())
}

pub(super) fn capture_contact(contact: &str) -> Result<super::nickname_native::ContactCapture> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let initial = loop {
        match chat_target(contact) {
            Ok(target) => break target,
            Err(error) => {
                if !matches!(error.downcast_ref::<ContactError>().map(|e| e.code), Some(
                    C::PopupUnavailable | C::PopupTreeUnavailable | C::ContactResultUnavailable
                    | C::ContactSectionUnavailable | C::ContactCellUnavailable
                    | C::ContactCellWindowMismatch | C::ContactCellGeometryUnavailable
                )) { return Err(error); }
                let now = Instant::now();
                if now >= deadline { return Err(error); }
                observe_for((deadline - now).min(Duration::from_millis(50)));
            }
        }
    };
    let binding = contact_binding(&initial);
    // The whole app-window image includes the anchored popup through child
    // windows; the image is only meaningful while that binding stays live.
    let capture = desktop::capture(APPLICATION).map_err(|error| {
        if error.downcast_ref::<ContactError>().is_some() { error } else { contact_error(C::ContactCaptureUnavailable) }
    })?;
    live_contact_binding(contact, &binding)?;
    Ok(super::nickname_native::ContactCapture { capture: std::sync::Arc::new(capture), binding })
}

/// Load the private image and obtain one live, fully identified contact snapshot.
fn bound_contact_target(capture: &super::nickname_native::ContactCapture, contact: &str) -> Result<(super::AppWindowCapture, ChatTarget)> {
    let loaded = desktop::load_capture(APPLICATION, &capture.capture.capture_id)
        .map_err(|_| contact_error(C::ContactCaptureRecordUnavailable))?;
    if loaded.window_bounds_points != capture.capture.window_bounds_points
        || loaded.image_width != capture.capture.image_width || loaded.image_height != capture.capture.image_height
        || loaded.window_bounds_points != capture.binding.main_bounds {
        return Err(contact_error(C::ContactCaptureChanged));
    }
    let target = chat_target(contact)?;
    if contact_binding(&target) != capture.binding {
        return Err(contact_error(C::ContactCaptureChanged));
    }
    Ok((loaded, target))
}

pub(super) fn load_contact_capture(capture: &super::nickname_native::ContactCapture, contact: &str) -> Result<super::AppWindowCapture> {
    bound_contact_target(capture, contact).map(|(loaded, _)| loaded)
}

pub(super) fn nickname_contact_geometry(capture: &super::nickname_native::ContactCapture, contact: &str) -> Result<super::nickname_native::ContactGeometry> {
    let (loaded, target) = bound_contact_target(capture, contact)?;
    Ok(super::nickname_native::ContactGeometry {
        main: super::nickname_native::Geometry {
            pid: target.main.pid,
            window_number: target.main.number,
            display_id: target.main.display,
            window_bounds: bounds(target.main.position, target.main.size),
            display_bounds: bounds(target.main.display_bounds.origin, target.main.display_bounds.size),
            image_size: [loaded.image_width, loaded.image_height],
        },
        popup_bounds: target.popup_bounds,
    })
}

pub(super) fn nickname_validate_contact(capture: &super::nickname_native::ContactCapture, point: [f64; 2], contact: &str) -> Result<bool> {
    // Unavailable, ambiguous or changed evidence is an error, not a cache miss.
    let (_, target) = bound_contact_target(capture, contact)?;
    // Match chat_target_at: select the identified row by containment; its live
    // AX center is independently hit-tested and is the point actually dispatched.
    Ok(point_in_rect(point, target.cell_bounds))
}


fn chat_header_for_field(main: &CaptureTarget, field: Element<'_>, contact: &str) -> Result<Option<(f64, f64)>> {
    let parent = search_parent(field, main, "AXSplitGroup")
        .map_err(|_| anyhow::anyhow!("Header structure unavailable"))?;
    let siblings = chat_children(parent.as_ref())?;
    let (fp, fs) = geometry(field)?;
    let mut dividers = Vec::new();
    for sibling in &siblings {
        if chat_role(sibling.as_ref(), "AXSplitter")? {
            let (p, s) = geometry(sibling.as_ref())?;
            if s.height > s.width && p.x > fp.x + fs.width
                && p.y < main.position.y + 80.0 && p.y + s.height > main.position.y {
                dividers.push(p.x + s.width);
            }
        }
    }
    // A text field to the right of the divider is not the search/header anchor.
    if dividers.is_empty() { return Ok(None); }
    ensure!(dividers.len() == 1, "Conversation divider missing or ambiguous");
    let mut title_bottom = None;
    for sibling in &siblings {
        let title_role = chat_role(sibling.as_ref(), "AXTextField")? || chat_role(sibling.as_ref(), "AXStaticText")?;
        if !title_role || (sibling.as_ref().equals(field)) { continue; }
        let (p, s) = geometry(sibling.as_ref())?;
        if p.x > dividers[0] && p.y >= main.position.y
            && p.y + s.height <= main.position.y + 80.0
            && p.x + s.width <= main.position.x + main.size.width
            && search_value_matches(sibling.as_ref(), contact)? {
            ensure!(title_bottom.is_none(), "Conversation title ambiguous");
            title_bottom = Some(p.y + s.height);
        }
    }
    Ok(title_bottom.map(|bottom| (dividers[0], bottom)))
}

fn chat_header(target: &ChatTarget, contact: &str) -> Result<bool> {
    Ok(chat_header_for_field(&target.main, target.field.as_ref(), contact)?.is_some())
}


pub(super) fn nickname_open_contact(binding: &ContactBinding, point: [f64; 2], contact: &str) -> Result<()> {
    open_chat_at(binding, contact, point)
}

fn open_chat_at(binding: &ContactBinding, contact: &str, point: [f64; 2]) -> Result<()> {
    let pre = |code| ContactError { dispatch: D::NotDispatched, code };
    let initial = chat_target_at(contact, point)?;
    if contact_binding(&initial) != *binding { return Err(pre(C::ContactCaptureChanged).into()); }
    let location = initial.point;
    let token = i64::from_ne_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
    let tap = MouseMonitor::new(initial.main.pid, location, token).ok_or_else(|| pre(C::InputMonitoringUnavailable))?;
    if !tap.enabled() { return Err(pre(C::InputMonitoringUnavailable).into()); }
    let click = PreparedInputPair::mouse(initial.point, token).map_err(|_| pre(C::InputPreparationFailed))?;
    let current = chat_target_at(contact, point)?;
    if contact_binding(&current) != *binding { return Err(pre(C::ContactCaptureChanged).into()); }
    if !same_chat(&initial, &current) || !tap.enabled() {
        return Err(pre(C::ContactTargetChanged).into());
    }
    click.post();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last = C::InputRouteUnconfirmed;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { break; }
        if !tap.enabled() { last = C::InputMonitoringLost; break; }
        observe_for(remaining.min(Duration::from_millis(20)));
        let verified = (|| -> std::result::Result<(), C> {
            let main = window_target(false).map_err(|_| C::WindowUnavailable)?;
            if !same_search_window(&initial.main, &main) { return Err(C::WindowChanged); }
            frontmost(&main).map_err(|_| C::FocusLost)?;
            confirm_target_opaque(&main).map_err(|_| C::OpacityChanged)?;
            let app = application(main.pid).map_err(|_| C::WindowListUnavailable)?;
            let windows = attribute(app.as_ref(), b"AXWindows\0").map_err(|_| C::WindowListUnavailable)?;
            if windows.as_ref().kind() != ValueKind::Array { return Err(C::WindowListUnavailable); }
            let count = windows.as_ref().array_count();
            if !(1..=256).contains(&count) { return Err(C::WindowListUnavailable); }
            let gone = !(0..count).any(|i| {
                let window = windows.as_ref().array_at(i);
                !window.is_null() && (window.equals(initial.popup.as_ref()))
            });
            if !gone { return Err(C::SearchPopupStillVisible); }
            if !tap.complete() { return Err(C::InputRouteUnconfirmed); }
            if !chat_header(&initial, contact).map_err(|_| C::ConversationHeaderUnconfirmed)? {
                return Err(C::ConversationHeaderUnconfirmed);
            }
            Ok(())
        })();
        match verified {
            Ok(()) => return Ok(()),
            Err(reason) => last = reason,
        }
    }
    Err(ContactError { dispatch: D::Dispatched, code: last }.into())
}

struct DraftTarget {
    main: CaptureTarget,
    editor: Owned,
    rect: [f64; 4],
    point: [f64; 2],
}

fn draft_parent(child: Element<'_>, main: &CaptureTarget, role: &str) -> Result<Owned> {
    search_parent(child, main, role).map_err(|_| anyhow::anyhow!("Draft ancestry unavailable"))
}

fn draft_value_matches(editor: Element<'_>, expected: &str) -> Result<bool> {
    let value = attribute(editor, b"AXValue\0")?;
    ensure!((value.as_ref().kind() == ValueKind::String), "Invalid draft value");
    matches_string_value(value.as_ref(), expected)
}

fn draft_children(element: Element<'_>, remaining: &mut usize) -> Result<Vec<Owned>> {
    let children = chat_children(element)?;
    *remaining = remaining.checked_sub(children.len())
        .context("Draft accessibility tree exceeds limit")?;
    Ok(children)
}

// Only the editor group's direct scroll areas below its horizontal divider
// and toolbar are eligible; message-history text areas are never traversed.
fn draft_editor_in_group(group: Element<'_>, remaining: &mut usize) -> Result<Option<(Owned, Owned)>> {
    let children = draft_children(group, remaining)?;
    let (gp, gs) = geometry(group)?;
    let mut dividers = Vec::new();
    let mut buttons = Vec::new();
    let mut scrolls = Vec::new();
    for child in children {
        if chat_role(child.as_ref(), "AXSplitter")? {
            let (p, s) = geometry(child.as_ref())?;
            if s.width > s.height && s.height > 0.0 {
                dividers.push(bounds(p, s));
            }
        } else if chat_role(child.as_ref(), "AXButton")? {
            let (p, s) = geometry(child.as_ref())?;
            buttons.push(bounds(p, s));
        } else if chat_role(child.as_ref(), "AXScrollArea")? {
            let (p, s) = geometry(child.as_ref())?;
            scrolls.push((child, bounds(p, s)));
        }
    }
    let mut found = None;
    for (scroll, r) in scrolls {
        if !super::draft_scroll_geometry(bounds(gp, gs), r, &dividers, &buttons) { continue; }
        for editor in draft_children(scroll.as_ref(), remaining)? {
            if !chat_role(editor.as_ref(), "AXTextArea")? { continue; }
            let (p, s) = geometry(editor.as_ref())?;
            ensure!(bounds(p, s) == r, "Editor geometry mismatch");
            ensure!(found.is_none(), "Draft editor ambiguous");
            found = Some((editor, scroll.as_ref().retained()));
        }
    }
    Ok(found)
}

fn unique_header_boundary(
    candidates: impl IntoIterator<Item = Result<Option<(f64, f64)>>>,
) -> Result<(f64, f64)> {
    let mut boundary = None;
    for candidate in candidates {
        if let Some(candidate) = candidate? {
            ensure!(boundary.is_none(), "Conversation title ambiguous");
            boundary = Some(candidate);
        }
    }
    boundary.context("Conversation title mismatch")
}

#[cfg(test)]
mod header_boundary_tests {
    use super::*;

    #[test]
    fn failed_or_ambiguous_header_candidate_cannot_reuse_an_earlier_match() {
        assert!(unique_header_boundary([
            Ok(Some((100.0, 40.0))),
            Err(anyhow::anyhow!("injected AX read failure")),
        ]).is_err());
        assert!(unique_header_boundary([
            Ok(Some((100.0, 40.0))), Ok(Some((100.0, 41.0))),
        ]).is_err());
        assert!(unique_header_boundary([Ok(None)]).is_err());
        assert_eq!(unique_header_boundary([
            Ok(None), Ok(Some((100.0, 40.0))), Ok(None),
        ]).unwrap(), (100.0, 40.0));
    }
}

fn draft_target(contact: &str) -> Result<DraftTarget> {
    ensure!(trusted(), "Accessibility unavailable");
    let main = window_target(false)?;
    frontmost(&main).map_err(|_| anyhow::anyhow!("Window not frontmost"))?;
    confirm_target_opaque(&main).map_err(|_| anyhow::anyhow!("Window not opaque"))?;
    let mut found = None;
    let mut remaining = 2048;
    // The only searched path is window / split / header split / outer split /
    // editor split / scroll / text area, with bounded direct child arrays.
    for root in draft_children(main.window.as_ref(), &mut remaining)? {
        if !chat_role(root.as_ref(), "AXSplitGroup")? { continue; }
        for header in draft_children(root.as_ref(), &mut remaining)? {
            if !chat_role(header.as_ref(), "AXSplitGroup")? { continue; }
            for outer in draft_children(header.as_ref(), &mut remaining)? {
                if !chat_role(outer.as_ref(), "AXSplitGroup")? { continue; }
                for group in draft_children(outer.as_ref(), &mut remaining)? {
                    if !chat_role(group.as_ref(), "AXSplitGroup")? { continue; }
                    let Some((editor, selected_scroll)) = draft_editor_in_group(group.as_ref(), &mut remaining)? else { continue; };
                    ensure!(found.is_none(), "Draft editor ambiguous");
                    let scroll = draft_parent(editor.as_ref(), &main, "AXScrollArea")?;
                    let actual_group = draft_parent(scroll.as_ref(), &main, "AXSplitGroup")?;
                    let actual_outer = draft_parent(actual_group.as_ref(), &main, "AXSplitGroup")?;
                    let actual_header = draft_parent(actual_outer.as_ref(), &main, "AXSplitGroup")?;
                    let actual_root = draft_parent(actual_header.as_ref(), &main, "AXSplitGroup")?;
                    let actual_window = draft_parent(actual_root.as_ref(), &main, "AXWindow")?;
                    ensure!((scroll.as_ref().equals(selected_scroll.as_ref())
                        && actual_group.as_ref().equals(group.as_ref()) && actual_outer.as_ref().equals(outer.as_ref())
                        && actual_header.as_ref().equals(header.as_ref()) && actual_root.as_ref().equals(root.as_ref())
                        && actual_window.as_ref().equals(main.window.as_ref())), "Draft ancestry changed");
                    let (gp, gs) = geometry(group.as_ref())?;
                    let (op, os) = geometry(outer.as_ref())?;
                    ensure!(bounds(gp, gs) == bounds(op, os), "Draft split geometry mismatch");
                    let fields = draft_children(header.as_ref(), &mut remaining)?;
                    let (divider_right, title_bottom) = unique_header_boundary(fields.iter().map(|field| {
                        if !chat_role(field.as_ref(), "AXTextField")? { return Ok(None); }
                        // The header helper reads the direct siblings and the
                        // parent membership again; charge both bounded scans.
                        remaining = remaining.checked_sub(fields.len().saturating_mul(2))
                            .context("Draft accessibility tree exceeds limit")?;
                        chat_header_for_field(&main, field.as_ref(), contact)
                    }))?;
                    attribute_writable(editor.as_ref(), b"AXValue\0").map_err(|_| anyhow::anyhow!("Draft value not writable"))?;
                    attribute_writable(editor.as_ref(), b"AXFocused\0").map_err(|_| anyhow::anyhow!("Draft focus not writable"))?;
                    let (p, s) = geometry(editor.as_ref())?;
                    let (rp, rs) = geometry(root.as_ref())?;
                    let (hp, hs) = geometry(header.as_ref())?;
                    ensure!(super::draft_column_geometry(bounds(main.position, main.size),
                        bounds(rp, rs), bounds(hp, hs), bounds(gp, gs), bounds(p, s),
                        divider_right, title_bottom), "Draft outside conversation column");
                    let point = [p.x + s.width / 2.0, p.y + s.height / 2.0];
                    ensure!(point.iter().all(|v| v.is_finite() && (*v as f32) as f64 == *v), "Invalid editor point");
                    let hit = hit_target_element(&main, point).map_err(|_| anyhow::anyhow!("Editor hit unavailable"))?;
                    ensure!((hit.as_ref().equals(editor.as_ref())), "Editor hit mismatch");
                    ensure!(found.is_none(), "Draft editor ambiguous");
                    found = Some((editor, bounds(p, s), point));
                }
            }
        }
    }
    let (editor, rect, point) = found.context("Draft editor unavailable")?;
    Ok(DraftTarget { main, editor, rect, point })
}

fn same_draft(a: &DraftTarget, b: &DraftTarget) -> bool {
    same_search_window(&a.main, &b.main) && a.rect == b.rect && a.point == b.point
        && (a.editor.as_ref().equals(b.editor.as_ref()))
}

struct History { scroll: Owned, table: Owned, rect: [f64; 4], rows: Vec<Owned> }

fn history_membership(child: Element<'_>, parent: Element<'_>, main: &CaptureTarget) -> Result<()> {
    let actual = attribute(child, b"AXParent\0")?;
    ensure!((actual.as_ref().kind() == ValueKind::Element
        && actual.as_ref().equals(parent)) && child.pid().ok() == Some(main.pid) && element_window_matches(child, main)?, "History ancestry changed");
    Ok(())
}

fn history_rows(table: Element<'_>, remaining: &mut usize) -> Result<Vec<Owned>> {
    let array = attribute(table, b"AXRows\0")?;
    ensure!((array.as_ref().kind() == ValueKind::Array), "Invalid history rows");
    let count = array.as_ref().array_count();
    ensure!((0..=2048).contains(&count), "Invalid history row count");
    *remaining = remaining.checked_sub(count as usize).context("History exceeds limit")?;
    let mut rows = Vec::with_capacity(count as usize);
    for i in 0..count {
        let row = array.as_ref().array_at(i);
        ensure!(!row.is_null() && (row.kind() == ValueKind::Element)
            && chat_role(row, "AXRow")?, "Invalid history row");
        rows.push(row.retained());
    }
    Ok(rows)
}

fn history_snapshot(target: &DraftTarget) -> Result<History> {
    let editor_scroll = draft_parent(target.editor.as_ref(), &target.main, "AXScrollArea")?;
    let group = draft_parent(editor_scroll.as_ref(), &target.main, "AXSplitGroup")?;
    let mut remaining = 2048;
    let mut result = None;
    for child in draft_children(group.as_ref(), &mut remaining)? {
        if !chat_role(child.as_ref(), "AXScrollArea")? || (child.as_ref().equals(editor_scroll.as_ref())) { continue; }
        let (p, s) = geometry(child.as_ref())?;
        let rect = bounds(p, s);
        if rect[0] != target.rect[0] || rect[2] != target.rect[2]
            || rect[1] + rect[3] > target.rect[1] || rect[2] <= 0.0 || rect[3] <= 0.0 { continue; }
        history_membership(child.as_ref(), group.as_ref(), &target.main)?;
        for table in draft_children(child.as_ref(), &mut remaining)? {
            if !chat_role(table.as_ref(), "AXTable")? { continue; }
            history_membership(table.as_ref(), child.as_ref(), &target.main)?;
            ensure!(result.is_none(), "History ambiguous");
            let rows = history_rows(table.as_ref(), &mut remaining)?;
            let children = draft_children(table.as_ref(), &mut remaining)?;
            for row in &rows {
                history_membership(row.as_ref(), table.as_ref(), &target.main)?;
                ensure!(children.iter().filter(|child| child.as_ref().equals(row.as_ref())).count() == 1,
                    "History row membership changed");
            }
            result = Some(History { scroll: child.as_ref().retained(), table, rect, rows });
        }
    }
    result.context("History unavailable")
}

fn history_prefix(a: &History, b: &History) -> bool {
    a.rect == b.rect && (a.scroll.as_ref().equals(b.scroll.as_ref()) && a.table.as_ref().equals(b.table.as_ref()))
        && a.rows.len() <= b.rows.len()
        && a.rows.iter().zip(&b.rows).all(|(x, y)| x.as_ref().equals(y.as_ref()))
}

fn history_appended_message(baseline: &History, current: &History, target: &DraftTarget, content: &str) -> Result<bool> {
    ensure!(history_prefix(baseline, current), "History changed");
    let mut remaining = 2048usize.checked_sub(current.rows.len()).context("History exceeds limit")?;
    let mut found = false;
    for row in &current.rows[baseline.rows.len()..] {
        for cell in draft_children(row.as_ref(), &mut remaining)? {
            if !chat_role(cell.as_ref(), "AXCell")? { continue; }
            history_membership(cell.as_ref(), row.as_ref(), &target.main)?;
            for text in draft_children(cell.as_ref(), &mut remaining)? {
                if !chat_role(text.as_ref(), "AXTextArea")? { continue; }
                history_membership(text.as_ref(), cell.as_ref(), &target.main)?;
                let (p, s) = geometry(text.as_ref())?;
                let r = bounds(p, s);
                if r.iter().all(|v| v.is_finite()) && r[2] > 0.0 && r[3] > 0.0
                    && r[0] < current.rect[0] + current.rect[2] && r[0] + r[2] > current.rect[0]
                    && r[1] < current.rect[1] + current.rect[3] && r[1] + r[3] > current.rect[1]
                    && draft_value_matches(text.as_ref(), content)? { found = true; }
            }
        }
    }
    Ok(found)
}

pub(super) fn preflight_send_chat_draft(contact: &str, content: &str) -> DiagnosticResult<()> {
    if !trusted() { return Err(Diagnostic::AccessibilityUnavailable); }
    let a = draft_target(contact).map_err(|_| Diagnostic::TargetElementMismatch)?;
    let app = application(a.main.pid)
        .map_err(|_| Diagnostic::PreflightUnavailable)?;
    if !search_focus_matches(app.as_ref(), a.editor.as_ref()).map_err(|_| Diagnostic::PreflightUnavailable)?
        || !draft_value_matches(a.editor.as_ref(), content).map_err(|_| Diagnostic::PreflightUnavailable)? {
        return Err(Diagnostic::TargetElementMismatch);
    }
    let history = history_snapshot(&a).map_err(|_| Diagnostic::PreflightUnavailable)?;
    let b = draft_target(contact).map_err(|_| Diagnostic::TargetChanged)?;
    let other = history_snapshot(&b).map_err(|_| Diagnostic::TargetChanged)?;
    if !same_draft(&a, &b) || !history_prefix(&history, &other) || history.rows.len() != other.rows.len()
        || !search_focus_matches(app.as_ref(), b.editor.as_ref()).map_err(|_| Diagnostic::PreflightUnavailable)?
        || !draft_value_matches(b.editor.as_ref(), content).map_err(|_| Diagnostic::PreflightUnavailable)? {
        return Err(Diagnostic::TargetChanged);
    }
    crate::platforms::macos::keyboard::preflight(APPLICATION)?;
    Ok(())
}

pub(super) fn send_chat_draft(contact: &str, content: &str) -> Result<()> {
    let initial = draft_target(contact)?;
    let app = application(initial.main.pid)?;
    ensure!(search_focus_matches(app.as_ref(), initial.editor.as_ref())? && draft_value_matches(initial.editor.as_ref(), content)?,
        "Draft or focus changed");
    let baseline = history_snapshot(&initial)?;
    // The key primitive allocates its sole event pair and monitoring tap before
    // this final business check. AX references stay local to this synchronous
    // operation; no async task can carry or reuse a stale prepared target.
    crate::platforms::macos::keyboard::press_return_guarded(APPLICATION, || {
        let current = draft_target(contact)?;
        let final_history = history_snapshot(&current)?;
        ensure!(same_draft(&initial, &current) && history_prefix(&baseline, &final_history)
            && baseline.rows.len() == final_history.rows.len()
            && search_focus_matches(app.as_ref(), current.editor.as_ref())?
            && draft_value_matches(current.editor.as_ref(), content)?, "Draft changed before dispatch");
        Ok(())
    })?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        ensure!(Instant::now() < deadline, "Submission outcome unknown");
        observe_for(Duration::from_millis(10));
        let current = draft_target(contact)?;
        ensure!(same_draft(&initial, &current), "Submission target changed");
        let history = history_snapshot(&current)?;
        let appended = history_appended_message(&baseline, &history, &current, content)?;
        if draft_value_matches(current.editor.as_ref(), "")? && appended {
            ensure!(Instant::now() < deadline, "Submission outcome unknown");
            return Ok(());
        }
    }
}


pub(super) fn write_chat_draft(contact: &str, text: &str) -> Result<()> {
    let initial = draft_target(contact)?;
    ensure!(draft_value_matches(initial.editor.as_ref(), "")?, "Draft is not empty");
    let chunks = unicode_chunks(text);
    let events = chunks.iter().map(|chunk| PreparedInputPair::keyboard(0, 0, Some(chunk), None))
        .collect::<Result<Vec<_>>>()?;
    let mut characters = text.char_indices();
    let mut ends = Vec::with_capacity(chunks.len());
    let mut end = 0;
    for chunk in &chunks {
        let mut units = chunk.len();
        while units > 0 {
            let (index, ch) = characters.next().context("Invalid draft chunk")?;
            units = units.checked_sub(ch.len_utf16()).context("Invalid draft chunk boundary")?;
            end = index + ch.len_utf8();
        }
        ends.push(end);
    }
    ensure!(end == text.len(), "Invalid draft chunk coverage");
    let app = application(initial.main.pid)?;
    let current = draft_target(contact)?;
    ensure!(same_draft(&initial, &current) && draft_value_matches(initial.editor.as_ref(), "")?,
        "Draft changed before dispatch");

    // From the AX focus request onward any error is an unknown outcome. Focus is
    // requested exactly once; no click, retry or second focus write is permitted.
    initial.editor.as_ref().set_focused()?;
    let focus_deadline = Instant::now() + Duration::from_millis(500);
    while !search_focus_matches(app.as_ref(), initial.editor.as_ref())? {
        ensure!(Instant::now() < focus_deadline, "Draft focus was requested but its outcome is unknown");
        std::thread::sleep(Duration::from_millis(10));
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut previous_len = 0;
    for (&end, event) in ends.iter().zip(events) {
        ensure!(Instant::now() < deadline, "Draft readback timed out");
        let current = draft_target(contact)?;
        ensure!(same_draft(&initial, &current) && search_focus_matches(app.as_ref(), initial.editor.as_ref())?
            && draft_value_matches(initial.editor.as_ref(), &text[..previous_len])?, "Draft target changed after focus");
        event.post();
        loop {
            ensure!(Instant::now() < deadline, "Draft readback timed out");
            let current = draft_target(contact)?;
            ensure!(same_draft(&initial, &current) && search_focus_matches(app.as_ref(), initial.editor.as_ref())?,
                "Draft target changed during readback");
            ensure!(Instant::now() < deadline, "Draft readback timed out");
            if draft_value_matches(initial.editor.as_ref(), &text[..end])? { break; }
            ensure!(draft_value_matches(initial.editor.as_ref(), &text[..previous_len])?,
                "Draft changed during readback");
            std::thread::sleep(Duration::from_millis(10));
        }
        previous_len = end;
    }
    Ok(())
}


/// Task-local evidence for one pasted attachment. It holds no AX reference;
/// every later step re-resolves the live editor and compares this snapshot.
pub(super) struct AttachmentDraft {
    token: crate::platforms::macos::clipboard::ClipboardToken,
    value: String,
}

const ATTACHMENT_DRAFT_LIMIT: isize = 64;

fn draft_value(editor: Element<'_>) -> Result<String> {
    let value = attribute(editor, b"AXValue\0")?;
    bounded_string(value.as_ref(), ATTACHMENT_DRAFT_LIMIT)
}

fn verified_snapshot(attachment: &super::nickname_native::Attachment) -> Result<Option<Vec<u8>>> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(&attachment.path).context("Attachment snapshot unavailable")?;
    ensure!(bytes.len() as u64 == attachment.size_bytes
        && format!("{:x}", Sha256::digest(&bytes)) == attachment.sha256, "Attachment snapshot changed");
    Ok(attachment.is_image.then_some(bytes))
}

/// Paste exactly one staged attachment into the verified empty editor of the
/// exact conversation. Any error after the pasteboard write is an unknown outcome.
pub(super) fn write_chat_attachment(contact: &str, attachment: &super::nickname_native::Attachment) -> Result<AttachmentDraft> {
    use crate::platforms::macos::{clipboard, keyboard};
    let initial = draft_target(contact)?;
    ensure!(draft_value_matches(initial.editor.as_ref(), "")?, "Draft is not empty");
    let app = application(initial.main.pid)?;
    let baseline = history_snapshot(&initial)?;
    let image = verified_snapshot(attachment)?;
    let current = draft_target(contact)?;
    ensure!(same_draft(&initial, &current) && draft_value_matches(current.editor.as_ref(), "")?,
        "Draft changed before dispatch");

    // From the pasteboard write onward the payload may be visible to other
    // local applications and any failure is reported as unknown by Runtime.
    let token = match &image {
        Some(bytes) => clipboard::write_image(bytes, &attachment.mime_type)?,
        None => clipboard::write_file(&attachment.path, &attachment.filename)?,
    };
    initial.editor.as_ref().set_focused()?;
    let focus_deadline = Instant::now() + Duration::from_millis(500);
    while !search_focus_matches(app.as_ref(), initial.editor.as_ref())? {
        ensure!(Instant::now() < focus_deadline, "Draft focus was requested but its outcome is unknown");
        std::thread::sleep(Duration::from_millis(10));
    }
    keyboard::press_paste_guarded(APPLICATION, || {
        let current = draft_target(contact)?;
        let history = history_snapshot(&current)?;
        ensure!(same_draft(&initial, &current) && history_prefix(&baseline, &history)
            && baseline.rows.len() == history.rows.len()
            && search_focus_matches(app.as_ref(), current.editor.as_ref())?
            && draft_value_matches(current.editor.as_ref(), "")?
            && clipboard::unchanged(token)?, "Draft or pasteboard changed before paste");
        Ok(())
    })?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        ensure!(Instant::now() < deadline, "Attachment paste outcome unknown");
        observe_for(Duration::from_millis(10));
        let current = draft_target(contact)?;
        let history = history_snapshot(&current)?;
        ensure!(same_draft(&initial, &current) && history_prefix(&baseline, &history)
            && baseline.rows.len() == history.rows.len()
            && search_focus_matches(app.as_ref(), current.editor.as_ref())?, "Draft target changed after paste");
        let value = draft_value(current.editor.as_ref())?;
        if value.is_empty() { continue; }
        ensure!(super::attachment_draft_value(&value), "Unexpected draft content after paste");
        return Ok(AttachmentDraft { token, value });
    }
}

pub(super) fn preflight_send_chat_attachment(contact: &str, draft: &AttachmentDraft) -> DiagnosticResult<()> {
    if !trusted() { return Err(Diagnostic::AccessibilityUnavailable); }
    let a = draft_target(contact).map_err(|_| Diagnostic::TargetElementMismatch)?;
    let app = application(a.main.pid).map_err(|_| Diagnostic::PreflightUnavailable)?;
    if !search_focus_matches(app.as_ref(), a.editor.as_ref()).map_err(|_| Diagnostic::PreflightUnavailable)?
        || !draft_value_matches(a.editor.as_ref(), &draft.value).map_err(|_| Diagnostic::PreflightUnavailable)? {
        return Err(Diagnostic::TargetElementMismatch);
    }
    let history = history_snapshot(&a).map_err(|_| Diagnostic::PreflightUnavailable)?;
    let b = draft_target(contact).map_err(|_| Diagnostic::TargetChanged)?;
    let other = history_snapshot(&b).map_err(|_| Diagnostic::TargetChanged)?;
    if !same_draft(&a, &b) || !history_prefix(&history, &other) || history.rows.len() != other.rows.len()
        || !search_focus_matches(app.as_ref(), b.editor.as_ref()).map_err(|_| Diagnostic::PreflightUnavailable)?
        || !draft_value_matches(b.editor.as_ref(), &draft.value).map_err(|_| Diagnostic::PreflightUnavailable)? {
        return Err(Diagnostic::TargetChanged);
    }
    crate::platforms::macos::keyboard::preflight(APPLICATION)?;
    Ok(())
}

/// One Return for the exact pasted preview, then observe an emptied editor and
/// at least one appended history row. This is local UI evidence, not delivery.
pub(super) fn send_chat_attachment(contact: &str, draft: &AttachmentDraft) -> Result<()> {
    let initial = draft_target(contact)?;
    let app = application(initial.main.pid)?;
    ensure!(search_focus_matches(app.as_ref(), initial.editor.as_ref())? && draft_value_matches(initial.editor.as_ref(), &draft.value)?,
        "Draft or focus changed");
    let baseline = history_snapshot(&initial)?;
    crate::platforms::macos::keyboard::press_return_guarded(APPLICATION, || {
        let current = draft_target(contact)?;
        let final_history = history_snapshot(&current)?;
        ensure!(same_draft(&initial, &current) && history_prefix(&baseline, &final_history)
            && baseline.rows.len() == final_history.rows.len()
            && search_focus_matches(app.as_ref(), current.editor.as_ref())?
            && draft_value_matches(current.editor.as_ref(), &draft.value)?, "Draft changed before dispatch");
        Ok(())
    })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        ensure!(Instant::now() < deadline, "Submission outcome unknown");
        observe_for(Duration::from_millis(10));
        let current = draft_target(contact)?;
        ensure!(same_draft(&initial, &current), "Submission target changed");
        let history = history_snapshot(&current)?;
        ensure!(history_prefix(&baseline, &history), "History changed");
        if draft_value_matches(current.editor.as_ref(), "")? && history.rows.len() > baseline.rows.len() {
            ensure!(Instant::now() < deadline, "Submission outcome unknown");
            // Best effort: only clears while the pasteboard is still ours.
            if crate::platforms::macos::clipboard::clear_if_unchanged(draft.token).is_err() {
                tracing::warn!("Attachment pasteboard cleanup unavailable");
            }
            return Ok(());
        }
    }
}


