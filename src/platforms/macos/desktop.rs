use crate::kernel::platform::{DesktopApplication, Point, Rect, Size, WindowCapture};
use crate::kernel::task::ActivationDiagnostic as Diagnostic;
use anyhow::{bail, ensure, Context, Result};
use std::ffi::{c_char, c_void};
use std::ptr;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

type Object = *const c_void;
type Element = *const c_void;

#[repr(C)]
struct CFRange { location: isize, length: isize }

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(alloc: Object, string: *const c_char, encoding: u32) -> Object;
    fn CFStringCreateWithBytes(alloc: Object, bytes: *const u8, length: isize, encoding: u32, external: bool) -> Object;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(string: Object) -> isize;
    fn CFStringGetBytes(string: Object, range: CFRange, encoding: u32, loss: u8, external: bool,
        buffer: *mut u8, capacity: isize, used: *mut isize) -> isize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Object) -> bool;
    fn CFArrayGetCount(array: Object) -> isize;
    fn CFArrayGetTypeID() -> usize;
    static kCFBooleanTrue: Object;
    fn CFArrayGetValueAtIndex(array: Object, index: isize) -> Object;
    fn CFRetain(value: Object) -> Object;
    fn CFRelease(value: Object);
    fn CFDictionaryGetValue(dictionary: Object, key: Object) -> Object;
    fn CFGetTypeID(value: Object) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFNumberGetValue(number: Object, kind: i32, value: *mut c_void) -> bool;
    fn CFEqual(a: Object, b: Object) -> bool;

}
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> Element;
    fn AXUIElementCreateSystemWide() -> Element;
    fn AXUIElementGetTypeID() -> usize;
    fn AXUIElementCopyElementAtPosition(element: Element, x: f32, y: f32, result: *mut Element) -> i32;
    fn AXUIElementGetPid(element: Element, pid: *mut i32) -> i32;
    fn AXUIElementCopyAttributeValue(element: Element, attribute: Object, value: *mut Object) -> i32;
    fn AXUIElementIsAttributeSettable(element: Element, attribute: Object, settable: *mut u8) -> i32;
    fn AXUIElementSetAttributeValue(element: Element, attribute: Object, value: Object) -> i32;
    fn AXValueGetValue(value: Object, kind: i32, output: *mut c_void) -> bool;
    fn AXValueGetTypeID() -> usize;
    fn AXValueCreate(kind: i32, value: *const c_void) -> Object;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(display: u32) -> Rect;
    fn CGWindowListCopyWindowInfo(options: u32, relative_to: u32) -> Object;
    fn CGRectMakeWithDictionaryRepresentation(dictionary: Object, rect: *mut Rect) -> bool;
    static kCGWindowOwnerPID: Object;
    static kCGWindowNumber: Object;
    static kCGWindowBounds: Object;
    static kCGWindowLayer: Object;
    static kCGWindowAlpha: Object;
    static kCGWindowSharingState: Object;
}
unsafe extern "C" {
    fn geteuid() -> u32;
}
// Owns Create/Copy-rule Core Foundation and Accessibility objects.
pub(crate) struct Owned(Object);
impl Drop for Owned {
    fn drop(&mut self) { unsafe { CFRelease(self.0) } }
}
fn owned(value: Object, what: &str) -> Result<Owned> {
    ensure!(!value.is_null(), "Could not create {what}");
    Ok(Owned(value))
}
fn string(value: &'static [u8]) -> Result<Owned> {
    let value = std::ffi::CStr::from_bytes_with_nul(value).context("Invalid native attribute name")?;
    owned(unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), 0x0800_0100) }, "CFString")
}
fn attribute(element: Element, name: &'static [u8]) -> Result<Owned> {
    let name = string(name)?;
    let mut value = ptr::null();
    let error = unsafe { AXUIElementCopyAttributeValue(element, name.0, &mut value) };
    ensure!(error == 0, "Could not read application accessibility attribute (AX error {error})");
    owned(value, "accessibility attribute")
}

fn running_pid(application: DesktopApplication) -> Result<i32> {
    super::process::running_pid(application.bundle_identifier, application.display_name)
}

pub(crate) struct Target { window: Owned, position: Point, original_size: Size }

fn geometry(window: Element) -> Result<(Point, Size)> {
    let pos = attribute(window, b"AXPosition\0")?;
    let size = attribute(window, b"AXSize\0")?;
    ensure!(unsafe { CFGetTypeID(pos.0) == AXValueGetTypeID() && CFGetTypeID(size.0) == AXValueGetTypeID() },
        "Invalid accessibility geometry attributes");
    let mut point = Point { x: 0.0, y: 0.0 };
    let mut dimensions = Size { width: 0.0, height: 0.0 };
    ensure!(unsafe { AXValueGetValue(pos.0, 1, (&mut point as *mut Point).cast()) }, "Invalid application window position");
    ensure!(unsafe { AXValueGetValue(size.0, 2, (&mut dimensions as *mut Size).cast()) }, "Invalid application window size");
    ensure!(point.x.is_finite() && point.y.is_finite() && dimensions.width.is_finite()
        && dimensions.height.is_finite() && dimensions.width > 0.0 && dimensions.height > 0.0,
        "Invalid application window geometry");
    Ok((point, dimensions))
}

pub(crate) fn trusted() -> bool {
    crate::platforms::macos::permissions::trusted()
}

fn selected_window(application: DesktopApplication) -> Result<(i32, Owned)> {
    let label = application.display_name;
    ensure!(trusted(), "macOS Accessibility permission is required for the controlling process");
    let pid = running_pid(application)?;
    let app = owned(unsafe { AXUIElementCreateApplication(pid) }, "application accessibility element")?;
    let windows = attribute(app.0, b"AXWindows\0")?;
    ensure!(unsafe { CFGetTypeID(windows.0) == CFArrayGetTypeID() }, "Invalid application window list");
    let count = unsafe { CFArrayGetCount(windows.0) };
    ensure!((1..=256).contains(&count), "Expected a bounded nonempty {label} window list");
    let window = if count == 1 {
        let value = unsafe { CFArrayGetValueAtIndex(windows.0, 0) };
        ensure!(!value.is_null(), "Could not identify the {label} window");
        Owned(unsafe { CFRetain(value) })
    } else {
        let name = string(b"AXMainWindow\0")?;
        let mut value = ptr::null();
        let error = unsafe { AXUIElementCopyAttributeValue(app.0, name.0, &mut value) };
        let main = if error == 0 {
            owned(value, "main application window")?
        } else {
            if !value.is_null() { unsafe { CFRelease(value) }; }
            anyhow::bail!("Expected a main {label} window (AX error {error})");
        };
        ensure!(unsafe { CFGetTypeID(main.0) == AXUIElementGetTypeID() }, "Invalid main application window");
        ensure!((0..count).any(|index| {
            let candidate = unsafe { CFArrayGetValueAtIndex(windows.0, index) };
            !candidate.is_null() && unsafe { CFEqual(candidate, main.0) }
        }), "Main {label} window is not in the application's window list");
        main
    };
    ensure!(unsafe { CFGetTypeID(window.0) == AXUIElementGetTypeID() }, "Invalid application window");
    Ok((pid, window))
}

pub(crate) fn target(application: DesktopApplication) -> Result<Target> {
    let label = application.display_name;
    let (_, window) = selected_window(application)?;
    let position_attribute = string(b"AXPosition\0")?;
    let mut settable = 0;
    let error = unsafe { AXUIElementIsAttributeSettable(window.0, position_attribute.0, &mut settable) };
    ensure!(error == 0 && settable != 0, "{label} window position is not writable (AX error {error})");
    let (pos, size) = geometry(window.0)?;

    let mut count = 0;
    let error = unsafe { CGGetActiveDisplayList(0, ptr::null_mut(), &mut count) };
    ensure!(error == 0 && count > 0, "Could not enumerate active displays (CG error {error})");
    let mut displays = vec![0; count as usize];
    let error = unsafe { CGGetActiveDisplayList(count, displays.as_mut_ptr(), &mut count) };
    ensure!(error == 0, "Could not read active displays (CG error {error})");
    displays.truncate(count as usize);
    let mut best: Option<(f64, Rect)> = None;
    let mut tied = false;
    // CGDisplayBounds and AXPosition both use the global, top-left-origin
    // display coordinate space, including negative origins on other screens.
    for display in displays {
        let bounds = unsafe { CGDisplayBounds(display) };
        let width = (pos.x + size.width).min(bounds.origin.x + bounds.size.width) - pos.x.max(bounds.origin.x);
        let height = (pos.y + size.height).min(bounds.origin.y + bounds.size.height) - pos.y.max(bounds.origin.y);
        let overlap = width.max(0.0) * height.max(0.0);
        if overlap > 0.0 {
            match best {
                None => best = Some((overlap, bounds)),
                Some((area, _)) if overlap > area => { best = Some((overlap, bounds)); tied = false; }
                Some((area, _)) if overlap == area => tied = true,
                _ => {}
            }
        }
    }
    let (_, display) = best.with_context(|| format!("{label} window does not overlap an active display"))?;
    ensure!(!tied, "{label} window overlaps multiple displays equally");
    let position = Point {
        x: display.origin.x + (display.size.width - size.width) / 2.0,
        y: display.origin.y + (display.size.height - size.height) / 2.0,
    };
    ensure!(position.x.is_finite() && position.y.is_finite(), "Invalid display geometry");
    Ok(Target { window, position, original_size: size })
}

pub(crate) fn screen_recording_allowed() -> bool {
    crate::platforms::macos::permissions::screen_recording_allowed()
}

pub(crate) struct CaptureTarget {
    pub(crate) pid: i32,
    pub(crate) window: Owned,
    pub(crate) number: u32,
    pub(crate) position: Point,
    pub(crate) size: Size,
    pub(crate) display: u32,
    pub(crate) display_bounds: Rect,
}

fn number(dictionary: Object, key: Object) -> Option<f64> {
    unsafe {
        let value = CFDictionaryGetValue(dictionary, key);
        if value.is_null() || CFGetTypeID(value) != CFNumberGetTypeID() {
            return None;
        }
        let mut result = 0.0_f64;
        if CFNumberGetValue(value, 13, (&mut result as *mut f64).cast()) && result.is_finite() {
            Some(result)
        } else {
            None
        }
    }
}

fn same_geometry(a: Point, b: Size, c: Point, d: Size) -> bool {
    (a.x - c.x).abs() <= 0.5 && (a.y - c.y).abs() <= 0.5
        && (b.width - d.width).abs() <= 0.5 && (b.height - d.height).abs() <= 0.5
}

pub(crate) fn capture_display(position: Point, size: Size) -> Result<(u32, Rect)> {
    let mut count = 0;
    let error = unsafe { CGGetActiveDisplayList(0, ptr::null_mut(), &mut count) };
    ensure!(error == 0 && count > 0, "Could not enumerate active displays (CG error {error})");
    let capacity = count;
    let mut displays = vec![0; capacity as usize];
    let error = unsafe { CGGetActiveDisplayList(capacity, displays.as_mut_ptr(), &mut count) };
    ensure!(error == 0 && count == capacity, "Active displays changed during enumeration");
    let mut actual_count = 0;
    let error = unsafe { CGGetActiveDisplayList(0, ptr::null_mut(), &mut actual_count) };
    ensure!(error == 0 && actual_count == capacity, "Active displays changed during enumeration");
    let right = position.x + size.width;
    let bottom = position.y + size.height;
    ensure!(right.is_finite() && bottom.is_finite(), "Invalid application window geometry");
    let mut selected = None;
    for display in displays {
        let bounds = unsafe { CGDisplayBounds(display) };
        let display_right = bounds.origin.x + bounds.size.width;
        let display_bottom = bounds.origin.y + bounds.size.height;
        ensure!(bounds.origin.x.is_finite() && bounds.origin.y.is_finite()
            && bounds.size.width.is_finite() && bounds.size.height.is_finite()
            && bounds.size.width > 0.0 && bounds.size.height > 0.0
            && display_right.is_finite() && display_bottom.is_finite(),
            "Invalid active display geometry");
        if position.x < display_right && right > bounds.origin.x
            && position.y < display_bottom && bottom > bounds.origin.y {
            ensure!(selected.is_none(), "application window overlaps multiple active displays");
            ensure!(position.x >= bounds.origin.x && position.y >= bounds.origin.y
                && right <= display_right && bottom <= display_bottom,
                "application window is not fully contained in an active display");
            selected = Some((display, bounds));
        }
    }
    selected.context("application window does not overlap an active display")
}

pub(crate) fn capture_target(application: DesktopApplication) -> Result<CaptureTarget> {
    window_target(application, true)
}

pub(crate) fn window_target(application: DesktopApplication, require_capture: bool) -> Result<CaptureTarget> {
    let (pid, window) = selected_window(application)?;
    let (position, size) = geometry(window.0)?;
    let (display, display_bounds) = capture_display(position, size)?;
    // On-screen windows only; including just the chosen window in the
    // image request avoids capturing windows that overlap it.
    let windows = owned(unsafe { CGWindowListCopyWindowInfo(1, 0) }, "on-screen window list")?;
    let count = unsafe { CFArrayGetCount(windows.0) };
    let mut matched = None;
    for index in 0..count {
        let entry = unsafe { CFArrayGetValueAtIndex(windows.0, index) };
        if entry.is_null() || unsafe { CFGetTypeID(entry) != CFDictionaryGetTypeID() } {
            continue;
        }
        if number(entry, unsafe { kCGWindowOwnerPID }) != Some(pid as f64)
            || number(entry, unsafe { kCGWindowLayer }) != Some(0.0) {
            continue;
        }
        let bounds = unsafe { CFDictionaryGetValue(entry, kCGWindowBounds) };
        if bounds.is_null() || unsafe { CFGetTypeID(bounds) != CFDictionaryGetTypeID() } {
            continue;
        }
        let mut rect = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: 0.0, height: 0.0 } };
        if !unsafe { CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) }
            || !same_geometry(position, size, rect.origin, rect.size) {
            continue;
        }
        ensure!(position.x == rect.origin.x && position.y == rect.origin.y
            && size.width == rect.size.width && size.height == rect.size.height,
            "application AX and CG window geometry do not match exactly");
        ensure!(matched.is_none(), "Multiple visible CG windows match the application window");
        let id = number(entry, unsafe { kCGWindowNumber }).context("application CG window has no number")?;
        ensure!(id > 0.0 && id <= u32::MAX as f64 && id.fract() == 0.0, "Invalid application CG window number");
        let alpha = number(entry, unsafe { kCGWindowAlpha }).context("application CG window has no alpha")?;
        ensure!(alpha > 0.0, "application window is transparent");
        if require_capture {
            let sharing = number(entry, unsafe { kCGWindowSharingState }).context("application CG window has no sharing state")?;
            ensure!(sharing != 0.0, "application window is protected from capture");
        }
        matched = Some(id as u32);
    }
    let number = matched.context("No unique visible CG window matches the application window")?;
    Ok(CaptureTarget { pid, window, number, position, size, display, display_bounds })
}

fn same_capture_target(a: &CaptureTarget, b: &CaptureTarget) -> bool {
    a.pid == b.pid && a.number == b.number && a.window.as_ref().equals(b.window.as_ref())
        && bounds(a.position, a.size) == bounds(b.position, b.size)
        && a.display == b.display
        && bounds(a.display_bounds.origin, a.display_bounds.size) == bounds(b.display_bounds.origin, b.display_bounds.size)
}


pub(crate) type DiagnosticResult<T> = std::result::Result<T, Diagnostic>;

pub(crate) fn frontmost(target: &CaptureTarget) -> DiagnosticResult<()> {
    let pid = crate::platforms::macos::process::frontmost_pid()
        .ok_or(Diagnostic::PreflightUnavailable)?;
    if pid != target.pid { return Err(Diagnostic::TargetNotFrontmost); }
    let app = owned(unsafe { AXUIElementCreateApplication(pid) }, "application accessibility element")
        .map_err(|_| Diagnostic::PreflightUnavailable)?;
    let window = attribute(app.0, b"AXFocusedWindow\0").map_err(|_| Diagnostic::PreflightUnavailable)?;
    if !unsafe { CFEqual(window.0, target.window.0) } {
        return Err(Diagnostic::TargetNotFrontmost);
    }
    Ok(())
}

// Confirm only the selected window's live visibility and opacity here.
// Other windows' bounding rectangles do not establish input ownership:
// system-wide AX hit testing below does. Non-AX overlays may be invisible
// to that hit test, so this is not a guarantee against every overlay.
pub(crate) fn confirm_target_opaque(target: &CaptureTarget) -> DiagnosticResult<()> {
    let windows = owned(unsafe { CGWindowListCopyWindowInfo(1, 0) }, "on-screen window list")
        .map_err(|_| Diagnostic::PreflightUnavailable)?;
    let count = unsafe { CFArrayGetCount(windows.0) };
    for index in 0..count {
        let entry = unsafe { CFArrayGetValueAtIndex(windows.0, index) };
        if entry.is_null() || unsafe { CFGetTypeID(entry) != CFDictionaryGetTypeID() } {
            continue;
        }
        if number(entry, unsafe { kCGWindowNumber }) != Some(target.number as f64) {
            continue;
        }
        return if number(entry, unsafe { kCGWindowOwnerPID }) == Some(target.pid as f64)
            && number(entry, unsafe { kCGWindowAlpha }) == Some(1.0) {
            Ok(())
        } else {
            Err(Diagnostic::PreflightUnavailable)
        };
    }
    Err(Diagnostic::TargetWindowUnavailable)
}

fn matches_string_value(actual: Object, expected: &str) -> Result<bool> {
    ensure!(unsafe { CFGetTypeID(actual) == CFStringGetTypeID() },
        "Invalid accessibility string attribute");
    let expected = owned(unsafe {
        CFStringCreateWithBytes(ptr::null(), expected.as_ptr(), expected.len() as isize, 0x0800_0100, false)
    }, "expected accessibility string")?;
    Ok(unsafe { CFEqual(actual, expected.0) })
}

fn matches_string_attribute(element: Element, name: &'static [u8], expected: &str) -> Result<bool> {
    let actual = attribute(element, name)?;
    matches_string_value(actual.0, expected)
}

pub(crate) enum NameAttribute<T> {
    MissingOrEmpty,
    Nonempty(T),
    Invalid,
}

// A present, nonempty name is authoritative even when it will not match
// the caller's expected label. Invalid attribute types are not absence.
pub(crate) fn select_name<T>(attribute: NameAttribute<T>, fallback: impl FnOnce() -> DiagnosticResult<T>) -> DiagnosticResult<T> {
    match attribute {
        NameAttribute::Nonempty(name) => Ok(name),
        NameAttribute::MissingOrEmpty => fallback(),
        NameAttribute::Invalid => Err(Diagnostic::AccessibilityTitleUnavailable),
    }
}

pub(crate) fn static_text_name<T>(
    is_static_text: bool,
    title: impl FnOnce() -> DiagnosticResult<NameAttribute<T>>,
    value: impl FnOnce() -> DiagnosticResult<NameAttribute<T>>,
) -> DiagnosticResult<T> {
    if !is_static_text {
        return Err(Diagnostic::AccessibilityTitleUnavailable);
    }
    select_name(title()?, || {
        select_name(value()?, || Err(Diagnostic::AccessibilityTitleUnavailable))
    })
}

#[cfg(test)]
mod accessible_name_tests {
    use super::{Diagnostic, DiagnosticResult, NameAttribute, select_name, static_text_name};

    #[test]
    fn direct_name_is_authoritative_even_when_expected_label_differs() {
        let selected = select_name(NameAttribute::Nonempty("Archive"), || {
            panic!("A nonempty direct title must not consult a matching related label");
        }).unwrap();
        assert_eq!(selected, "Archive");
        assert_ne!(selected, "Messages");
    }

    #[test]
    fn missing_or_empty_direct_title_uses_semantic_static_text_label() {
        let selected = select_name(NameAttribute::MissingOrEmpty, || {
            static_text_name(true,
                || Ok(NameAttribute::Nonempty("Messages")),
                || panic!("A nonempty related title must not consult its value"))
        }).unwrap();
        assert_eq!(selected, "Messages");
    }

    #[test]
    fn related_title_mismatch_is_not_replaced_by_matching_value() {
        let selected = static_text_name(true,
            || Ok(NameAttribute::Nonempty("Archive")),
            || panic!("A present related title is authoritative")).unwrap();
        assert_eq!(selected, "Archive");
        assert_ne!(selected, "Messages");
    }

    #[test]
    fn static_text_value_requires_missing_or_empty_related_title() {
        let selected = static_text_name(true,
            || Ok(NameAttribute::MissingOrEmpty),
            || Ok(NameAttribute::Nonempty("Messages"))).unwrap();
        assert_eq!(selected, "Messages");
    }

    #[test]
    fn non_static_text_relation_cannot_supply_a_name() {
        let selected: DiagnosticResult<&str> = static_text_name(false,
            || panic!("A control or image title must not be read as a static-text label"),
            || panic!("A control value must not be read as a static-text label"));
        assert_eq!(selected, Err(Diagnostic::AccessibilityTitleUnavailable));
    }

    #[test]
    fn ambiguous_attribute_types_do_not_trigger_fallback() {
        let direct: DiagnosticResult<&str> = select_name(NameAttribute::Invalid, || {
            panic!("An invalid direct title must not be treated as absent");
        });
        assert_eq!(direct, Err(Diagnostic::AccessibilityTitleUnavailable));
        let related: DiagnosticResult<&str> = static_text_name(true,
            || Ok(NameAttribute::Invalid),
            || panic!("An invalid related title must not fall back to a value"));
        assert_eq!(related, Err(Diagnostic::AccessibilityTitleUnavailable));
    }

    #[test]
    fn absent_semantic_names_remain_unavailable() {
        let without_relation: DiagnosticResult<&str> = select_name(NameAttribute::MissingOrEmpty,
            || Err(Diagnostic::AccessibilityTitleUnavailable));
        assert_eq!(without_relation, Err(Diagnostic::AccessibilityTitleUnavailable));
        let empty_relation: DiagnosticResult<&str> = static_text_name(true,
            || Ok(NameAttribute::MissingOrEmpty),
            || Ok(NameAttribute::MissingOrEmpty));
        assert_eq!(empty_relation, Err(Diagnostic::AccessibilityTitleUnavailable));
    }
}

fn name_attribute(element: Element, name: &'static [u8]) -> DiagnosticResult<NameAttribute<Owned>> {
    let name = string(name).map_err(|_| Diagnostic::AccessibilityTitleUnavailable)?;
    let mut value = ptr::null();
    let error = unsafe { AXUIElementCopyAttributeValue(element, name.0, &mut value) };
    let value = (!value.is_null()).then(|| Owned(value));
    // Only unsupported attributes and absent values mean "no name".
    // Transport failures and values of an ambiguous type fail closed.
    if error == -25205 || error == -25212 {
        return Ok(NameAttribute::MissingOrEmpty);
    }
    if error != 0 {
        return Err(Diagnostic::AccessibilityTitleUnavailable);
    }
    let value = value.ok_or(Diagnostic::AccessibilityTitleUnavailable)?;
    if unsafe { CFGetTypeID(value.0) != CFStringGetTypeID() } {
        return Ok(NameAttribute::Invalid);
    }
    if unsafe { CFStringGetLength(value.0) } == 0 {
        return Ok(NameAttribute::MissingOrEmpty);
    }
    Ok(NameAttribute::Nonempty(value))
}

fn element_window_matches(element: Element, target: &CaptureTarget) -> Result<bool> {
    if unsafe { CFEqual(element, target.window.0) } {
        return Ok(true);
    }
    let window = attribute(element, b"AXWindow\0")?;
    ensure!(unsafe { CFGetTypeID(window.0) == AXUIElementGetTypeID() },
        "Invalid accessibility window attribute");
    Ok(unsafe { CFEqual(window.0, target.window.0) })
}

fn accessible_name(element: Element, target: &CaptureTarget) -> DiagnosticResult<Owned> {
    select_name(name_attribute(element, b"AXTitle\0")?, || {
        // AXTitleUIElement is a semantic label relation, not an invitation
        // to search child badges, image descriptions, or descendants.
        let title = attribute(element, b"AXTitleUIElement\0")
            .map_err(|_| Diagnostic::AccessibilityTitleUnavailable)?;
        if unsafe { CFGetTypeID(title.0) != AXUIElementGetTypeID() } {
            return Err(Diagnostic::AccessibilityTitleUnavailable);
        }
        let mut pid = 0;
        if unsafe { AXUIElementGetPid(title.0, &mut pid) } != 0 || pid != target.pid
            || !element_window_matches(title.0, target)
                .map_err(|_| Diagnostic::AccessibilityTitleUnavailable)? {
            return Err(Diagnostic::AccessibilityTitleUnavailable);
        }
        let is_static_text = matches_string_attribute(title.0, b"AXRole\0", "AXStaticText")
            .map_err(|_| Diagnostic::AccessibilityTitleUnavailable)?;
        static_text_name(is_static_text,
            || name_attribute(title.0, b"AXTitle\0"),
            || name_attribute(title.0, b"AXValue\0"))
    })
}

pub(crate) fn hit_target_element(target: &CaptureTarget, point: [f64; 2]) -> DiagnosticResult<Owned> {
    let system = owned(unsafe { AXUIElementCreateSystemWide() }, "system accessibility element")
        .map_err(|_| Diagnostic::HitTestUnavailable)?;
    let mut hit = ptr::null();
    let error = unsafe { AXUIElementCopyElementAtPosition(system.0, point[0] as f32, point[1] as f32, &mut hit) };
    if error != 0 {
        return Err(Diagnostic::HitTestUnavailable);
    }
    let element = owned(hit, "accessibility hit-test result").map_err(|_| Diagnostic::HitTestUnavailable)?;
    let mut pid = 0;
    if unsafe { AXUIElementGetPid(element.0, &mut pid) } != 0 {
        return Err(Diagnostic::HitTestUnavailable);
    }
    if pid != target.pid {
        return Err(Diagnostic::PointObscured);
    }
    if !element_window_matches(element.0, target).map_err(|_| Diagnostic::HitTestUnavailable)? {
        return Err(Diagnostic::TargetElementMismatch);
    }
    Ok(element)
}
fn boolean_attribute(element: Element, name: &'static [u8]) -> Option<bool> {
    let value = attribute(element, name).ok()?;
    if unsafe { CFGetTypeID(value.0) != CFBooleanGetTypeID() } {
        return None;
    }
    Some(unsafe { CFBooleanGetValue(value.0) })
}

fn optional_boolean_value(error: i32, value: Option<bool>) -> DiagnosticResult<Option<bool>> {
    match error {
        -25205 | -25212 => Ok(None),
        0 => value.map(Some).ok_or(Diagnostic::PreflightUnavailable),
        _ => Err(Diagnostic::PreflightUnavailable),
    }
}

#[cfg(test)]
mod optional_boolean_tests {
    use super::{optional_boolean_value, Diagnostic};

    #[test]
    fn only_documented_absence_allows_an_unavailable_enabled_attribute() {
        assert_eq!(optional_boolean_value(-25205, None), Ok(None));
        assert_eq!(optional_boolean_value(-25212, None), Ok(None));
        assert_eq!(optional_boolean_value(0, Some(false)), Ok(Some(false)));
        assert_eq!(optional_boolean_value(0, Some(true)), Ok(Some(true)));
        assert_eq!(optional_boolean_value(0, None), Err(Diagnostic::PreflightUnavailable));
        assert_eq!(optional_boolean_value(-25204, None), Err(Diagnostic::PreflightUnavailable));
        assert_eq!(optional_boolean_value(-25204, Some(true)), Err(Diagnostic::PreflightUnavailable));
    }
}
fn attribute_writable(element: Element, name: &'static [u8]) -> DiagnosticResult<()> {
    let name = string(name).map_err(|_| Diagnostic::PreflightUnavailable)?;
    let mut settable = 0;
    if unsafe { AXUIElementIsAttributeSettable(element, name.0, &mut settable) } != 0 || settable == 0 {
        return Err(Diagnostic::PreflightUnavailable);
    }
    Ok(())
}
// Only documented AX absence means a leaf. Wrong types and transport errors
// must not silently truncate a caller's accessibility traversal.
fn children_attribute(element: Element) -> DiagnosticResult<Option<Owned>> {
    let name = string(b"AXChildren\0").map_err(|_| Diagnostic::PreflightUnavailable)?;
    let mut value = ptr::null();
    let error = unsafe { AXUIElementCopyAttributeValue(element, name.0, &mut value) };
    let value = (!value.is_null()).then(|| Owned(value));
    if error == -25205 || error == -25212 {
        return Ok(None);
    }
    let value = value.ok_or(Diagnostic::PreflightUnavailable)?;
    if error != 0 || unsafe { CFGetTypeID(value.0) != CFArrayGetTypeID() } {
        return Err(Diagnostic::PreflightUnavailable);
    }
    Ok(Some(value))
}
pub(crate) fn visible_window_number(pid: i32, position: Point, size: Size) -> Result<u32> {
    let list = owned(unsafe { CGWindowListCopyWindowInfo(1, 0) }, "visible windows")?;
    let count = unsafe { CFArrayGetCount(list.0) };
    ensure!((0..=16384).contains(&count), "Invalid visible window count");
    let mut found = None;
    for i in 0..count {
        let entry = unsafe { CFArrayGetValueAtIndex(list.0, i) };
        if entry.is_null() || unsafe { CFGetTypeID(entry) != CFDictionaryGetTypeID() }
            || number(entry, unsafe { kCGWindowOwnerPID }) != Some(pid as f64)
            || number(entry, unsafe { kCGWindowLayer }) != Some(0.0) { continue; }
        let value = unsafe { CFDictionaryGetValue(entry, kCGWindowBounds) };
        if value.is_null() || unsafe { CFGetTypeID(value) != CFDictionaryGetTypeID() } { continue; }
        let mut rect = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: 0.0, height: 0.0 } };
        if !unsafe { CGRectMakeWithDictionaryRepresentation(value, &mut rect) }
            || bounds(position, size) != bounds(rect.origin, rect.size) { continue; }
        ensure!(number(entry, unsafe { kCGWindowAlpha }) == Some(1.0), "Popup opacity unavailable");
        let id = number(entry, unsafe { kCGWindowNumber }).context("Popup number unavailable")?;
        ensure!(id > 0.0 && id <= u32::MAX as f64 && id.fract() == 0.0 && found.is_none(), "Ambiguous popup identity");
        found = Some(id as u32);
    }
    found.context("No visible CG window matches popup")
}
fn private_directory(path: &Path, private: bool) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("Could not create capture directory"),
    }
    let metadata = std::fs::symlink_metadata(path).context("Could not inspect capture directory")?;
    ensure!(metadata.file_type().is_dir() && metadata.uid() == unsafe { geteuid() },
        "Capture directory must be an owner-controlled directory");
    ensure!(metadata.mode() & if private { 0o077 } else { 0o022 } == 0,
        "Capture directory has unsafe permissions");
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureRecord {
    capture_id: String,
    captured_at_ms: u64,
    pid: i32,
    window_id: u32,
    display_id: u32,
    display_bounds: [f64; 4],
    window_bounds: [f64; 4],
    image_width: u32,
    image_height: u32,
}

fn now_ms() -> Result<u64> {
    Ok(u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)
        .context("System clock is before the Unix epoch")?.as_millis())?)
}

pub(crate) fn bounds(position: Point, size: Size) -> [f64; 4] {
    [position.x, position.y, size.width, size.height]
}

impl CaptureRecord {
    fn matches(&self, target: &CaptureTarget) -> bool {
        self.pid == target.pid && self.window_id == target.number
            && self.display_id == target.display
            && self.window_bounds == bounds(target.position, target.size)
            && self.display_bounds == bounds(target.display_bounds.origin, target.display_bounds.size)
    }
}

fn capture_record_valid(record: &CaptureRecord, now: u64) -> bool {
    record.captured_at_ms <= now && now - record.captured_at_ms <= 300_000
}

#[cfg(test)]
mod capture_record_tests {
    use super::*;

    #[test]
    fn expiry_is_enforced_independently_of_geometry() {
        let record = CaptureRecord {
            capture_id: String::new(), captured_at_ms: 100,
            pid: 1, window_id: 2, display_id: 3, display_bounds: [0.0, 0.0, 100.0, 100.0],
            window_bounds: [0.0, 0.0, 10.0, 10.0], image_width: 10, image_height: 10,
        };
        assert!(capture_record_valid(&record, 100));
        assert!(capture_record_valid(&record, 300_100));
        assert!(!capture_record_valid(&record, 300_101));
        assert!(!capture_record_valid(&record, 99));
    }
}

fn capture_directory() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is required for private captures")?;
    let root = PathBuf::from(home).join(".desk-agent");
    private_directory(&root, false)?;
    let directory = root.join("captures");
    private_directory(&directory, true)?;
    Ok(directory)
}

// O_NOFOLLOW on Darwin prevents opening a symlink even if it is swapped
// between a metadata check and open. The private parent excludes other users.
fn open_private_file(path: &Path) -> Result<std::fs::File> {
    const O_NOFOLLOW: i32 = 0x00000100;
    const O_NONBLOCK: i32 = 0x00000004;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path).context("Could not open private capture file")?;
    validate_private_file(&file)?;
    Ok(file)
}

fn validate_private_file(file: &std::fs::File) -> Result<()> {
    let metadata = file.metadata().context("Could not inspect private capture file")?;
    ensure!(metadata.is_file() && metadata.uid() == unsafe { geteuid() }
        && metadata.mode() & 0o7777 == 0o600 && metadata.nlink() == 1,
        "Capture file is not private and owner-controlled");
    Ok(())
}

pub(crate) fn load_capture(application: DesktopApplication, capture_id: &str) -> Result<WindowCapture> {
    let id = uuid::Uuid::parse_str(capture_id).context("Invalid capture ID")?;
    ensure!(id.to_string() == capture_id, "Capture ID must be a canonical UUID");
    let directory = capture_directory()?;
    let record_path = directory.join(format!("app-window-{capture_id}.json"));
    let file = open_private_file(&record_path)?;
    let mut data = Vec::new();
    file.take(8193).read_to_end(&mut data).context("Could not read capture record")?;
    ensure!(data.len() <= 8192, "Capture record is too large");
    let record: CaptureRecord = serde_json::from_slice(&data).context("Invalid capture record")?;
    ensure!(record.capture_id == capture_id && record.pid > 0 && record.window_id > 0
        && record.display_id > 0 && record.image_width > 0 && record.image_height > 0,
        "Invalid capture record");
    ensure!(record.window_bounds.iter().chain(record.display_bounds.iter()).all(|v| v.is_finite())
        && record.window_bounds[2] > 0.0 && record.window_bounds[3] > 0.0
        && record.display_bounds[2] > 0.0 && record.display_bounds[3] > 0.0,
        "Invalid capture geometry");
    let width = f64::from(record.image_width);
    let height = f64::from(record.image_height);
    ensure!((width - height * record.window_bounds[2] / record.window_bounds[3]).abs() <= 1.0
        || (height - width * record.window_bounds[3] / record.window_bounds[2]).abs() <= 1.0,
        "Capture image dimensions do not match geometry");
    let now = now_ms()?;
    ensure!(capture_record_valid(&record, now),
        "Capture record is expired or has a future timestamp");
    let path = directory.join(format!("app-window-{capture_id}.png"));
    let image = open_private_file(&path)?;
    ensure!(image.metadata()?.len() > 0, "Capture image is empty");
    ensure!(screen_recording_allowed(), "macOS Screen Recording permission is required");
    let current = capture_target(application)?;
    ensure!(record.matches(&current), "application capture target or geometry changed");
    let now = now_ms()?;
    ensure!(capture_record_valid(&record, now),
        "Capture record is expired or has a future timestamp");
    Ok(WindowCapture {
        path,
        capture_id: capture_id.to_owned(),
        image_width: record.image_width,
        image_height: record.image_height,
        window_bounds_points: record.window_bounds,
    })
}

/// Captures the whole selected application window, including child windows,
/// so an anchored transient panel is part of the app-window image.
pub(crate) fn capture(application: DesktopApplication) -> Result<WindowCapture> {
    ensure!(screen_recording_allowed(), "macOS Screen Recording permission is required");
    let target = capture_target(application)?;
    let captured_at_ms = now_ms()?;
    let directory = capture_directory()?;
    let capture_id = uuid::Uuid::new_v4().to_string();
    let (image_width, image_height, png) = {
        super::screenshot::capture_window(target.number, target.pid, target.size.width, target.size.height)?
    };
    // A shadow-free window image must have the window's aspect ratio,
    // allowing one pixel of quantization at each dimension.
    let width_from_height = f64::from(image_height) * target.size.width / target.size.height;
    let height_from_width = f64::from(image_width) * target.size.height / target.size.width;
    ensure!((f64::from(image_width) - width_from_height).abs() <= 1.0
        || (f64::from(image_height) - height_from_width).abs() <= 1.0,
        "application image dimensions do not match window geometry");
    let current = capture_target(application)?;
    ensure!(screen_recording_allowed() && same_capture_target(&current, &target),
        "application capture target changed during capture");
    let path = directory.join(format!("app-window-{capture_id}.png"));
    let record_path = directory.join(format!("app-window-{capture_id}.json"));
    let record = CaptureRecord {
        capture_id: capture_id.clone(),
        captured_at_ms,
        pid: target.pid,
        window_id: target.number,
        display_id: target.display,
        display_bounds: bounds(target.display_bounds.origin, target.display_bounds.size),
        window_bounds: bounds(target.position, target.size),
        image_width,
        image_height,
    };
    let record_bytes = serde_json::to_vec(&record).context("Could not encode capture record")?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
        .open(&path).context("Could not create private capture file")?;
    let mut record_created = false;
    let result = (|| -> Result<()> {
        validate_private_file(&file)?;
        file.write_all(&png)
            .context("Could not write private capture file")?;
        let mut metadata = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
            .open(&record_path).context("Could not create private capture record")?;
        record_created = true;
        validate_private_file(&metadata)?;
        metadata.write_all(&record_bytes).context("Could not write private capture record")?;
        let current = capture_target(application)?;
        ensure!(screen_recording_allowed() && record.matches(&current)
            && unsafe { CFEqual(current.window.0, target.window.0) },
            "application capture target changed during recording");
        let now = now_ms()?;
        ensure!(captured_at_ms <= now && now - captured_at_ms <= 300_000,
            "Capture expired during recording");
        Ok(())
    })();
    drop(file);
    if let Err(error) = result {
        if record_created {
            let _ = std::fs::remove_file(&record_path);
        }
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(WindowCapture {
        path,
        capture_id,
        image_width,
        image_height,
        window_bounds_points: record.window_bounds,
    })
}


pub(crate) fn center(application: DesktopApplication) -> Result<()> {
    let label = application.display_name;
    let target = target(application)?;
    let name = string(b"AXPosition\0")?;
    let value = owned(unsafe { AXValueCreate(1, (&target.position as *const Point).cast()) }, "window position")?;
    let error = unsafe { AXUIElementSetAttributeValue(target.window.0, name.0, value.0) };
    if error != 0 {
        bail!("Could not move {label} window (AX error {error})");
    }
    let (actual, size) = geometry(target.window.0)
        .context("Window move was submitted but its result could not be read back")?;
    ensure!((actual.x - target.position.x).abs() <= 1.0
        && (actual.y - target.position.y).abs() <= 1.0
        && (size.width - target.original_size.width).abs() <= 1.0
        && (size.height - target.original_size.height).abs() <= 1.0,
        "Window move was submitted but position or size did not match on readback");
    Ok(())
}


/// Borrowed opaque native value. The lifetime keeps its owning CF value alive;
/// neither callers nor application adapters can access native pointers.
#[derive(Clone, Copy)]
pub(crate) struct Reference<'a> {
    raw: Object,
    owner: std::marker::PhantomData<&'a Owned>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind { Element, Array, String, Boolean, Other }
impl Owned {
    pub(crate) fn as_ref(&self) -> Reference<'_> {
        Reference { raw: self.0, owner: std::marker::PhantomData }
    }
}
impl Reference<'_> {
    pub(crate) fn is_null(self) -> bool { self.raw.is_null() }
    pub(crate) fn kind(self) -> ValueKind {
        if self.is_null() { return ValueKind::Other; }
        let kind = unsafe { CFGetTypeID(self.raw) };
        unsafe {
            if kind == AXUIElementGetTypeID() { ValueKind::Element }
            else if kind == CFArrayGetTypeID() { ValueKind::Array }
            else if kind == CFStringGetTypeID() { ValueKind::String }
            else if kind == CFBooleanGetTypeID() { ValueKind::Boolean }
            else { ValueKind::Other }
        }
    }
    pub(crate) fn equals(self, other: Reference<'_>) -> bool {
        !self.is_null() && !other.is_null() && unsafe { CFEqual(self.raw, other.raw) }
    }
    pub(crate) fn retained(self) -> Owned {
        assert!(!self.is_null(), "Cannot retain a null native value");
        Owned(unsafe { CFRetain(self.raw) })
    }
    pub(crate) fn array_count(self) -> isize {
        if self.kind() != ValueKind::Array { return -1; }
        unsafe { CFArrayGetCount(self.raw) }
    }
    pub(crate) fn array_at(self, index: isize) -> Self {
        let raw = if index >= 0 && index < self.array_count() {
            unsafe { CFArrayGetValueAtIndex(self.raw, index) }
        } else { ptr::null() };
        Self { raw, owner: std::marker::PhantomData }
    }
    pub(crate) fn pid(self) -> Result<i32> {
        ensure!(self.kind() == ValueKind::Element, "Invalid accessibility element");
        let mut pid = 0;
        let error = unsafe { AXUIElementGetPid(self.raw, &mut pid) };
        ensure!(error == 0, "Could not read accessibility process (AX error {error})");
        Ok(pid)
    }
    pub(crate) fn set_focused(self) -> Result<()> {
        ensure!(self.kind() == ValueKind::Element, "Invalid accessibility element");
        let name = string(b"AXFocused\0")?;
        let error = unsafe { AXUIElementSetAttributeValue(self.raw, name.0, kCFBooleanTrue) };
        ensure!(error == 0, "Accessibility focus request failed");
        Ok(())
    }
}

/// Safe CF/AX queries used by application-owned layout and business selectors.
pub(crate) mod query {
    use super::*;
    pub(crate) type Element<'a> = Reference<'a>;
    pub(crate) fn application(pid: i32) -> Result<Owned> {
        ensure!(pid > 0, "Invalid application process");
        owned(unsafe { AXUIElementCreateApplication(pid) }, "application accessibility element")
    }
    pub(crate) fn attribute(element: Element<'_>, name: &'static [u8]) -> Result<Owned> {
        ensure!(element.kind() == ValueKind::Element, "Invalid accessibility element");
        super::attribute(element.raw, name)
    }
    pub(crate) fn geometry(element: Element<'_>) -> Result<(Point, Size)> {
        ensure!(element.kind() == ValueKind::Element, "Invalid accessibility element");
        super::geometry(element.raw)
    }
    pub(crate) fn matches_string_value(value: Reference<'_>, expected: &str) -> Result<bool> {
        ensure!(!value.is_null(), "Missing accessibility value");
        super::matches_string_value(value.raw, expected)
    }
    pub(crate) fn matches_string_attribute(element: Element<'_>, name: &'static [u8], expected: &str) -> Result<bool> {
        ensure!(element.kind() == ValueKind::Element, "Invalid accessibility element");
        super::matches_string_attribute(element.raw, name, expected)
    }
    pub(crate) fn name_attribute(element: Element<'_>, name: &'static [u8]) -> DiagnosticResult<NameAttribute<Owned>> {
        if element.kind() != ValueKind::Element { return Err(Diagnostic::AccessibilityTitleUnavailable); }
        super::name_attribute(element.raw, name)
    }
    pub(crate) fn boolean_attribute(element: Element<'_>, name: &'static [u8]) -> Option<bool> {
        if element.kind() != ValueKind::Element { return None; }
        super::boolean_attribute(element.raw, name)
    }
    pub(crate) fn optional_boolean_attribute(element: Element<'_>, name: &'static [u8]) -> DiagnosticResult<Option<bool>> {
        if element.kind() != ValueKind::Element { return Err(Diagnostic::PreflightUnavailable); }
        let name = string(name).map_err(|_| Diagnostic::PreflightUnavailable)?;
        let mut raw = ptr::null();
        let error = unsafe { AXUIElementCopyAttributeValue(element.raw, name.0, &mut raw) };
        let owned = (!raw.is_null()).then(|| Owned(raw));
        let value = owned.as_ref().and_then(|value| {
            (unsafe { CFGetTypeID(value.0) } == unsafe { CFBooleanGetTypeID() })
                .then(|| unsafe { CFBooleanGetValue(value.0) })
        });
        optional_boolean_value(error, value)
    }
    pub(crate) fn attribute_writable(element: Element<'_>, name: &'static [u8]) -> DiagnosticResult<()> {
        if element.kind() != ValueKind::Element { return Err(Diagnostic::PreflightUnavailable); }
        super::attribute_writable(element.raw, name)
    }
    pub(crate) fn children_attribute(element: Element<'_>) -> DiagnosticResult<Option<Owned>> {
        if element.kind() != ValueKind::Element { return Err(Diagnostic::PreflightUnavailable); }
        // Only documented absence denotes a leaf; malformed or failed reads do not.
        super::children_attribute(element.raw)
    }
    pub(crate) fn element_window_matches(element: Element<'_>, target: &CaptureTarget) -> Result<bool> {
        ensure!(element.kind() == ValueKind::Element, "Invalid accessibility element");
        super::element_window_matches(element.raw, target)
    }
    pub(crate) fn accessible_name(element: Element<'_>, target: &CaptureTarget) -> DiagnosticResult<Owned> {
        if element.kind() != ValueKind::Element { return Err(Diagnostic::AccessibilityTitleUnavailable); }
        super::accessible_name(element.raw, target)
    }
    pub(crate) fn hit_test(point: [f64; 2]) -> Result<Owned> {
        ensure!(point.iter().all(|v| v.is_finite() && (*v as f32) as f64 == *v), "Invalid hit-test coordinates");
        let system = owned(unsafe { AXUIElementCreateSystemWide() }, "system accessibility element")?;
        let mut hit = ptr::null();
        let error = unsafe { AXUIElementCopyElementAtPosition(system.0, point[0] as f32, point[1] as f32, &mut hit) };
        ensure!(error == 0, "Accessibility hit test unavailable");
        owned(hit, "accessibility hit-test result")
    }
    /// Copy a short accessibility string; longer values are rejected, not truncated.
    pub(crate) fn bounded_string(value: Reference<'_>, max_utf16: isize) -> Result<String> {
        ensure!(value.kind() == ValueKind::String, "Invalid accessibility string value");
        let length = unsafe { CFStringGetLength(value.raw) };
        ensure!((0..=max_utf16).contains(&length), "Accessibility string exceeds limit");
        let mut bytes = vec![0_u8; length as usize * 4];
        let mut used = 0;
        let converted = unsafe {
            CFStringGetBytes(value.raw, CFRange { location: 0, length }, 0x0800_0100, 0, false,
                bytes.as_mut_ptr(), bytes.len() as isize, &mut used)
        };
        ensure!(converted == length && used >= 0 && used as usize <= bytes.len(), "Accessibility string conversion failed");
        bytes.truncate(used as usize);
        String::from_utf8(bytes).context("Invalid accessibility string encoding")
    }
    pub(crate) fn bounded_utf8_matches(value: Reference<'_>, expected: &str) -> Result<bool> {
        ensure!(value.kind() == ValueKind::String, "Invalid accessibility string value");
        let length = unsafe { CFStringGetLength(value.raw) };
        if !(0..=256).contains(&length) { return Ok(false); }
        let mut bytes = [0_u8; 256];
        let mut used = 0;
        let converted = unsafe {
            CFStringGetBytes(value.raw, CFRange { location: 0, length }, 0x0800_0100, 0, false,
                bytes.as_mut_ptr(), bytes.len() as isize, &mut used)
        };
        Ok(converted == length && used >= 0 && used as usize <= bytes.len()
            && &bytes[..used as usize] == expected.as_bytes())
    }
}
