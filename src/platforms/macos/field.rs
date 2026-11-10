//! Bounded native text-field input for an already-authorized application target.
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};
use crate::kernel::platform::DesktopApplication;
use crate::kernel::task::{valid_app_input, ActivationDiagnostic as Diagnostic};
use super::desktop::{self, query, CaptureTarget, Owned, ValueKind};
use super::input::{unicode_chunks, PreparedInputPair};

type Checked<T> = std::result::Result<T, Diagnostic>;
struct Field {
    target: CaptureTarget,
    element: Owned,
    role: Owned,
    geometry: [f64; 4],
}

fn allowed_role(role: &str) -> bool { matches!(role, "AXTextField" | "AXTextArea") }
fn enabled(value: Option<bool>) -> bool { value != Some(false) }
fn contains(rect: [f64; 4], point: [f64; 2]) -> bool {
    rect.iter().all(|v| v.is_finite()) && rect[2] > 0.0 && rect[3] > 0.0
        && point[0] >= rect[0] && point[1] >= rect[1]
        && point[0] < rect[0] + rect[2] && point[1] < rect[1] + rect[3]
}
fn contained(inner: [f64; 4], outer: [f64; 4]) -> bool {
    inner.iter().all(|v| v.is_finite()) && inner[2] > 0.0 && inner[3] > 0.0
        && inner[0] >= outer[0] && inner[1] >= outer[1]
        && inner[0] + inner[2] <= outer[0] + outer[2]
        && inner[1] + inner[3] <= outer[1] + outer[3]
}
fn inspect(application: DesktopApplication, point: [f64; 2]) -> Checked<Field> {
    if !desktop::trusted() { return Err(Diagnostic::AccessibilityUnavailable); }
    if !point.iter().all(|v| v.is_finite() && (*v as f32) as f64 == *v) {
        return Err(Diagnostic::CoordinatePrecisionUnsupported);
    }
    let target = desktop::window_target(application, false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
    let window = desktop::bounds(target.position, target.size);
    if !contains(window, point) { return Err(Diagnostic::PointOutsideTargetWindow); }
    desktop::frontmost(&target)?;
    desktop::confirm_target_opaque(&target)?;
    let element = desktop::hit_target_element(&target, point)?;
    let role = query::attribute(element.as_ref(), b"AXRole\0").map_err(|_| Diagnostic::AccessibilityRoleUnavailable)?;
    let mut accepted = false;
    for name in ["AXTextField", "AXTextArea"] {
        if allowed_role(name) && query::bounded_utf8_matches(role.as_ref(), name).map_err(|_| Diagnostic::AccessibilityRoleUnavailable)? {
            accepted = true;
        }
    }
    if !accepted { return Err(Diagnostic::AccessibilityRoleMismatch); }
    if !enabled(query::optional_boolean_attribute(element.as_ref(), b"AXEnabled\0")?) {
        return Err(Diagnostic::PreflightUnavailable);
    }
    let value = query::attribute(element.as_ref(), b"AXValue\0").map_err(|_| Diagnostic::PreflightUnavailable)?;
    if value.as_ref().kind() != ValueKind::String { return Err(Diagnostic::PreflightUnavailable); }
    query::attribute_writable(element.as_ref(), b"AXValue\0")?;
    query::attribute_writable(element.as_ref(), b"AXFocused\0")?;
    let (position, size) = query::geometry(element.as_ref()).map_err(|_| Diagnostic::PreflightUnavailable)?;
    let geometry = desktop::bounds(position, size);
    if !contains(geometry, point) || !contained(geometry, window) { return Err(Diagnostic::TargetElementMismatch); }
    Ok(Field { target, element, role, geometry })
}
fn same(a: &Field, b: &Field) -> bool {
    a.target.pid == b.target.pid && a.target.number == b.target.number
        && a.target.window.as_ref().equals(b.target.window.as_ref())
        && desktop::bounds(a.target.position, a.target.size) == desktop::bounds(b.target.position, b.target.size)
        && a.target.display == b.target.display
        && desktop::bounds(a.target.display_bounds.origin, a.target.display_bounds.size)
            == desktop::bounds(b.target.display_bounds.origin, b.target.display_bounds.size)
        && a.element.as_ref().equals(b.element.as_ref()) && a.role.as_ref().equals(b.role.as_ref())
        && a.geometry == b.geometry
}
fn focused(field: &Field, app: &Owned) -> Result<bool> {
    let focus = query::attribute(app.as_ref(), b"AXFocusedUIElement\0")?;
    Ok(query::boolean_attribute(field.element.as_ref(), b"AXFocused\0") == Some(true)
        && focus.as_ref().kind() == ValueKind::Element && focus.as_ref().equals(field.element.as_ref()))
}
fn revalidate(application: DesktopApplication, point: [f64; 2], original: &Field, app: &Owned) -> Result<()> {
    let current = inspect(application, point).map_err(|_| anyhow::anyhow!("Text input target unavailable"))?;
    ensure!(same(original, &current) && focused(&current, app)?, "Text input target or focus changed");
    Ok(())
}

pub(crate) fn preflight(application: DesktopApplication, point: [f64; 2], text: &str) -> Checked<()> {
    if point.iter().all(|value| value.is_finite())
        && !point.iter().all(|value| (*value as f32) as f64 == *value) {
        return Err(Diagnostic::CoordinatePrecisionUnsupported);
    }
    if !valid_app_input(&point, text) { return Err(Diagnostic::PreflightUnavailable); }
    let first = inspect(application, point)?;
    let second = inspect(application, point)?;
    if !same(&first, &second) { return Err(Diagnostic::TargetChanged); }
    Ok(())
}


pub(crate) fn input(application: DesktopApplication, point: [f64; 2], text: &str) -> Result<()> {
    ensure!(valid_app_input(&point, text), "Invalid text input parameters");
    let original = inspect(application, point).map_err(|_| anyhow::anyhow!("Text input validation failed"))?;
    let app = query::application(original.target.pid)?;
    // All event allocation precedes the first focus or keyboard effect.
    let select_all = PreparedInputPair::keyboard(0, 1 << 20, None, None)?;
    let events = unicode_chunks(text).iter().map(|chunk| PreparedInputPair::keyboard(0, 0, Some(chunk), None))
        .collect::<Result<Vec<_>>>()?;
    let current = inspect(application, point).map_err(|_| anyhow::anyhow!("Text input validation failed"))?;
    ensure!(same(&original, &current), "Text input target changed");
    // Any failure from focus onward is an unknown outcome; never retry input.
    original.element.as_ref().set_focused()?;
    revalidate(application, point, &original, &app)?;
    select_all.post();
    for event in events {
        revalidate(application, point, &original, &app)?;
        event.post();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        revalidate(application, point, &original, &app)?;
        let value = query::attribute(original.element.as_ref(), b"AXValue\0")?;
        let matches = query::bounded_utf8_matches(value.as_ref(), text)?;
        revalidate(application, point, &original, &app)?;
        ensure!(Instant::now() < deadline, "Text input readback timed out");
        if matches { return Ok(()); }
        std::thread::sleep(Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_standard_editable_text_roles() {
        assert!(allowed_role("AXTextField"));
        assert!(allowed_role("AXTextArea"));
        for role in ["AXComboBox", "AXStaticText", "AXButton", "", "AXSearchField"] { assert!(!allowed_role(role)); }
    }
    #[test]
    fn absent_enabled_is_allowed_but_disabled_is_not() {
        assert!(enabled(None));
        assert!(enabled(Some(true)));
        assert!(!enabled(Some(false)));
    }
    #[test]
    fn requires_positive_contained_field_geometry() {
        assert!(contained([1.0, 1.0, 5.0, 5.0], [0.0, 0.0, 10.0, 10.0]));
        assert!(!contained([1.0, 1.0, 0.0, 5.0], [0.0, 0.0, 10.0, 10.0]));
        assert!(!contained([1.0, 1.0, 10.0, 5.0], [0.0, 0.0, 10.0, 10.0]));
        assert!(!contains([0.0, 0.0, 10.0, 10.0], [10.0, 1.0]));
    }
}
