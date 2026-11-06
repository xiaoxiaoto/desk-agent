use crate::kernel::capability::{Capability, CapabilityChannel, CapabilityStatus};
use crate::kernel::task::ActivationDiagnostic;
use std::process::Command;

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
        let status = Command::new("open").args(["-a", "企业微信"]).status()?;
        anyhow::ensure!(status.success(), "macOS open command failed");
        Ok(())
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
            "Captures only the selected WeCom window, without other windows that may occlude it.",
            "Protected content may be redacted by macOS and cannot always be detected; availability does not guarantee a subsequent capture will succeed.",
            "Stores sensitive PNG files in a private directory; callers are responsible for their retention and deletion.",
        ],
        reason,
    }
}

/// Request the OS permission only for an explicitly authorized capture task.
pub fn request_screen_recording_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_desktop::request_screen_recording_permission()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// A private window image and its geometry at capture time.
pub struct AppWindowCapture {
    pub path: std::path::PathBuf,
    pub capture_id: String,
    pub image_width: u32,
    pub image_height: u32,
    /// Global macOS top-left-origin screen coordinates, in points: x, y, width, height.
    pub window_bounds_points: [f64; 4],
}

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
            "An exact accessibility role and title at the requested screen point",
        ],
        limitations: vec![
            "Does not launch, activate, or raise WeCom; an obscured point is rejected.",
            "Coordinates must be exactly representable as accessibility hit-test floats; the label is AXTitle.",
            "A click is verified only when the target's focused or selected state changes from false to true.",
        ],
        reason,
    }
}

/// Validate a point-specific activation without dispatching input.
pub fn preflight_app_element(point: [f64; 2], expected_role: &str, expected_label: &str) -> Result<(), ActivationDiagnostic> {
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

/// Revalidate and click the exact authorized element, then require evidence of a state transition.
pub fn activate_app_element(point: [f64; 2], expected_role: &str, expected_label: &str) -> anyhow::Result<()> {
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

#[cfg(target_os = "macos")]
mod macos_desktop {
    use super::AppWindowCapture;
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
    #[derive(Clone, Copy)]
    struct Point { x: f64, y: f64 }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Size { width: f64, height: f64 }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect { origin: Point, size: Size }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(alloc: Object, string: *const c_char, encoding: u32) -> Object;
        fn CFStringCreateWithBytes(alloc: Object, bytes: *const u8, length: isize, encoding: u32, external: bool) -> Object;
        fn CFStringGetTypeID() -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: Object) -> bool;
        fn CFArrayGetCount(array: Object) -> isize;
        fn CFArrayGetValueAtIndex(array: Object, index: isize) -> Object;
        fn CFRetain(value: Object) -> Object;
        fn CFRelease(value: Object);
        fn CFDataCreateMutable(alloc: Object, capacity: isize) -> Object;
        fn CFDataGetLength(data: Object) -> isize;
        fn CFDataGetBytePtr(data: Object) -> *const u8;
        fn CFDictionaryGetValue(dictionary: Object, key: Object) -> Object;
        fn CFGetTypeID(value: Object) -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFNumberGetValue(number: Object, kind: i32, value: *mut c_void) -> bool;
        fn CFEqual(a: Object, b: Object) -> bool;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateApplication(pid: i32) -> Element;
        fn AXUIElementCreateSystemWide() -> Element;
        fn AXUIElementCopyElementAtPosition(element: Element, x: f32, y: f32, result: *mut Element) -> i32;
        fn AXUIElementGetPid(element: Element, pid: *mut i32) -> i32;
        fn AXUIElementCopyAttributeValue(element: Element, attribute: Object, value: *mut Object) -> i32;
        fn AXUIElementIsAttributeSettable(element: Element, attribute: Object, settable: *mut u8) -> i32;
        fn AXUIElementSetAttributeValue(element: Element, attribute: Object, value: Object) -> i32;
        fn AXValueGetValue(value: Object, kind: i32, output: *mut c_void) -> bool;
        fn AXValueCreate(kind: i32, value: *const c_void) -> Object;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayBounds(display: u32) -> Rect;
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn CGWindowListCopyWindowInfo(options: u32, relative_to: u32) -> Object;
        fn CGWindowListCreateImage(bounds: Rect, options: u32, window: u32, image_options: u32) -> Object;
        fn CGRectMakeWithDictionaryRepresentation(dictionary: Object, rect: *mut Rect) -> bool;
        fn CGEventCreateMouseEvent(source: Object, event_type: u32, position: Point, button: u32) -> Object;
        fn CGEventSetFlags(event: Object, flags: u64);
        fn CGEventSetIntegerValueField(event: Object, field: u32, value: i64);
        fn CGEventPost(tap: u32, event: Object);
        static CGRectNull: Rect;
        static kCGWindowOwnerPID: Object;
        static kCGWindowNumber: Object;
        static kCGWindowBounds: Object;
        static kCGWindowLayer: Object;
        static kCGWindowAlpha: Object;
        static kCGWindowSharingState: Object;
        fn CGImageGetWidth(image: Object) -> usize;
        fn CGImageGetHeight(image: Object) -> usize;
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithData(data: Object, image_type: Object, count: usize, options: Object) -> Object;
        fn CGImageDestinationAddImage(destination: Object, image: Object, properties: Object);
        fn CGImageDestinationFinalize(destination: Object) -> bool;
    }
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Object;
        fn sel_registerName(name: *const c_char) -> Object;
        fn objc_msgSend();
    }

    // Owns Create/Copy-rule Core Foundation and Accessibility objects.
    struct Owned(Object);
    impl Drop for Owned {
        fn drop(&mut self) { unsafe { CFRelease(self.0) } }
    }
    fn owned(value: Object, what: &str) -> Result<Owned> {
        ensure!(!value.is_null(), "Could not create {what}");
        Ok(Owned(value))
    }
    fn string(value: &'static [u8]) -> Result<Owned> {
        owned(unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr().cast(), 0x0800_0100) }, "CFString")
    }
    fn attribute(element: Element, name: &'static [u8]) -> Result<Owned> {
        let name = string(name)?;
        let mut value = ptr::null();
        let error = unsafe { AXUIElementCopyAttributeValue(element, name.0, &mut value) };
        ensure!(error == 0, "Could not read WeCom accessibility attribute (AX error {error})");
        owned(value, "accessibility attribute")
    }

    // The Objective-C calls are limited to finding a running process by its
    // bundle identifier. They never launch, activate, or raise an application.
    fn running_pid() -> Result<i32> {
        unsafe {
            let class = objc_getClass(b"NSRunningApplication\0".as_ptr().cast());
            let pool_class = objc_getClass(b"NSAutoreleasePool\0".as_ptr().cast());
            ensure!(!class.is_null() && !pool_class.is_null(), "AppKit is unavailable");
            let selector = |name: &'static [u8]| sel_registerName(name.as_ptr().cast());
            let send0: unsafe extern "C" fn(Object, Object) -> Object = std::mem::transmute(objc_msgSend as *const ());
            let send1: unsafe extern "C" fn(Object, Object, Object) -> Object = std::mem::transmute(objc_msgSend as *const ());
            let send_pid: unsafe extern "C" fn(Object, Object) -> i32 = std::mem::transmute(objc_msgSend as *const ());
            let pool = send0(send0(pool_class, selector(b"alloc\0")), selector(b"init\0"));
            ensure!(!pool.is_null(), "Could not create autorelease pool");
            let result = (|| {
                let bundle = string(b"com.tencent.WeWorkMac\0")?;
                let apps = send1(class, selector(b"runningApplicationsWithBundleIdentifier:\0"), bundle.0);
                ensure!(!apps.is_null(), "Could not enumerate running WeCom applications");
                let count = CFArrayGetCount(apps);
                ensure!(count == 1, "Expected exactly one running WeCom instance, found {count}");
                let app = CFArrayGetValueAtIndex(apps, 0);
                ensure!(!app.is_null(), "Could not identify the running WeCom instance");
                let pid = send_pid(app, selector(b"processIdentifier\0"));
                ensure!(pid > 0, "Invalid WeCom process identifier");
                Ok(pid)
            })();
            let drain: unsafe extern "C" fn(Object, Object) = std::mem::transmute(objc_msgSend as *const ());
            drain(pool, selector(b"drain\0"));
            result
        }
    }

    pub(super) struct Target { window: Owned, position: Point, original_size: Size }

    fn geometry(window: Element) -> Result<(Point, Size)> {
        let pos = attribute(window, b"AXPosition\0")?;
        let size = attribute(window, b"AXSize\0")?;
        let mut point = Point { x: 0.0, y: 0.0 };
        let mut dimensions = Size { width: 0.0, height: 0.0 };
        ensure!(unsafe { AXValueGetValue(pos.0, 1, (&mut point as *mut Point).cast()) }, "Invalid WeCom window position");
        ensure!(unsafe { AXValueGetValue(size.0, 2, (&mut dimensions as *mut Size).cast()) }, "Invalid WeCom window size");
        ensure!(point.x.is_finite() && point.y.is_finite() && dimensions.width.is_finite()
            && dimensions.height.is_finite() && dimensions.width > 0.0 && dimensions.height > 0.0,
            "Invalid WeCom window geometry");
        Ok((point, dimensions))
    }

    pub(super) fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    fn selected_window() -> Result<(i32, Owned)> {
        ensure!(unsafe { AXIsProcessTrusted() }, "macOS Accessibility permission is required for the controlling process");
        let pid = running_pid()?;
        let app = owned(unsafe { AXUIElementCreateApplication(pid) }, "WeCom accessibility element")?;
        let windows = attribute(app.0, b"AXWindows\0")?;
        let count = unsafe { CFArrayGetCount(windows.0) };
        ensure!(count > 0, "Expected a main WeCom window, found none");
        let window = if count == 1 {
            let value = unsafe { CFArrayGetValueAtIndex(windows.0, 0) };
            ensure!(!value.is_null(), "Could not identify the WeCom window");
            Owned(unsafe { CFRetain(value) })
        } else {
            let main = attribute(app.0, b"AXMainWindow\0").context("Expected a main WeCom window")?;
            ensure!((0..count).any(|index| {
                let candidate = unsafe { CFArrayGetValueAtIndex(windows.0, index) };
                !candidate.is_null() && unsafe { CFEqual(candidate, main.0) }
            }), "Main WeCom window is not in the application's window list");
            main
        };
        Ok((pid, window))
    }

    pub(super) fn target() -> Result<Target> {
        let (_, window) = selected_window()?;
        let position_attribute = string(b"AXPosition\0")?;
        let mut settable = 0;
        let error = unsafe { AXUIElementIsAttributeSettable(window.0, position_attribute.0, &mut settable) };
        ensure!(error == 0 && settable != 0, "WeCom window position is not writable (AX error {error})");
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
        let (_, display) = best.context("WeCom window does not overlap an active display")?;
        ensure!(!tied, "WeCom window overlaps multiple displays equally");
        let position = Point {
            x: display.origin.x + (display.size.width - size.width) / 2.0,
            y: display.origin.y + (display.size.height - size.height) / 2.0,
        };
        ensure!(position.x.is_finite() && position.y.is_finite(), "Invalid display geometry");
        Ok(Target { window, position, original_size: size })
    }

    pub(super) fn screen_recording_allowed() -> bool {
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    pub(super) fn request_screen_recording_permission() -> bool {
        screen_recording_allowed() || unsafe { CGRequestScreenCaptureAccess() }
    }

    pub(super) struct CaptureTarget {
        pid: i32,
        window: Owned,
        number: u32,
        position: Point,
        size: Size,
        display: u32,
        display_bounds: Rect,
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

    fn capture_display(position: Point, size: Size) -> Result<(u32, Rect)> {
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
        ensure!(right.is_finite() && bottom.is_finite(), "Invalid WeCom window geometry");
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
                ensure!(selected.is_none(), "WeCom window overlaps multiple active displays");
                ensure!(position.x >= bounds.origin.x && position.y >= bounds.origin.y
                    && right <= display_right && bottom <= display_bottom,
                    "WeCom window is not fully contained in an active display");
                selected = Some((display, bounds));
            }
        }
        selected.context("WeCom window does not overlap an active display")
    }

    pub(super) fn capture_target() -> Result<CaptureTarget> {
        window_target(true)
    }

    fn window_target(require_capture: bool) -> Result<CaptureTarget> {
        let (pid, window) = selected_window()?;
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
                "WeCom AX and CG window geometry do not match exactly");
            ensure!(matched.is_none(), "Multiple visible CG windows match the WeCom window");
            let id = number(entry, unsafe { kCGWindowNumber }).context("WeCom CG window has no number")?;
            ensure!(id > 0.0 && id <= u32::MAX as f64 && id.fract() == 0.0, "Invalid WeCom CG window number");
            let alpha = number(entry, unsafe { kCGWindowAlpha }).context("WeCom CG window has no alpha")?;
            ensure!(alpha > 0.0, "WeCom window is transparent");
            if require_capture {
                let sharing = number(entry, unsafe { kCGWindowSharingState }).context("WeCom CG window has no sharing state")?;
                ensure!(sharing != 0.0, "WeCom window is protected from capture");
            }
            matched = Some(id as u32);
        }
        let number = matched.context("No unique visible CG window matches the WeCom window")?;
        Ok(CaptureTarget { pid, window, number, position, size, display, display_bounds })
    }

    type DiagnosticResult<T> = std::result::Result<T, Diagnostic>;

    fn frontmost(target: &CaptureTarget) -> DiagnosticResult<()> {
        let system = owned(unsafe { AXUIElementCreateSystemWide() }, "system accessibility element")
            .map_err(|_| Diagnostic::PreflightUnavailable)?;
        let app = attribute(system.0, b"AXFocusedApplication\0").map_err(|_| Diagnostic::PreflightUnavailable)?;
        let mut pid = 0;
        if unsafe { AXUIElementGetPid(app.0, &mut pid) } != 0 {
            return Err(Diagnostic::PreflightUnavailable);
        }
        if pid != target.pid {
            return Err(Diagnostic::TargetNotFrontmost);
        }
        let window = attribute(app.0, b"AXFocusedWindow\0").map_err(|_| Diagnostic::PreflightUnavailable)?;
        if !unsafe { CFEqual(window.0, target.window.0) } {
            return Err(Diagnostic::TargetNotFrontmost);
        }
        Ok(())
    }

    pub(super) fn activation_target_available() -> DiagnosticResult<()> {
        if !trusted() {
            return Err(Diagnostic::AccessibilityUnavailable);
        }
        let target = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
        frontmost(&target)
    }

    // The on-screen list is ordered front to back. Treat every visible window
    // above the selected one as an occluder, including other WeCom windows,
    // panels, and transparent-looking windows with nonzero global alpha.
    fn unobstructed(target: &CaptureTarget, point: Point) -> DiagnosticResult<()> {
        let windows = owned(unsafe { CGWindowListCopyWindowInfo(1, 0) }, "on-screen window list")
            .map_err(|_| Diagnostic::PreflightUnavailable)?;
        let count = unsafe { CFArrayGetCount(windows.0) };
        for index in 0..count {
            let entry = unsafe { CFArrayGetValueAtIndex(windows.0, index) };
            if entry.is_null() || unsafe { CFGetTypeID(entry) != CFDictionaryGetTypeID() } {
                return Err(Diagnostic::PreflightUnavailable);
            }
            let id = number(entry, unsafe { kCGWindowNumber }).ok_or(Diagnostic::PreflightUnavailable)?;
            let alpha = number(entry, unsafe { kCGWindowAlpha }).ok_or(Diagnostic::PreflightUnavailable)?;
            if !(0.0..=1.0).contains(&alpha) {
                return Err(Diagnostic::PreflightUnavailable);
            }
            if id == target.number as f64 {
                return if alpha == 1.0 { Ok(()) } else { Err(Diagnostic::PreflightUnavailable) };
            }
            if alpha == 0.0 {
                continue;
            }
            let bounds = unsafe { CFDictionaryGetValue(entry, kCGWindowBounds) };
            if bounds.is_null() || unsafe { CFGetTypeID(bounds) != CFDictionaryGetTypeID() } {
                return Err(Diagnostic::PreflightUnavailable);
            }
            let mut rect = Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { width: 0.0, height: 0.0 } };
            if !unsafe { CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) }
                || !rect.origin.x.is_finite() || !rect.origin.y.is_finite()
                || !rect.size.width.is_finite() || !rect.size.height.is_finite()
                || rect.size.width < 0.0 || rect.size.height < 0.0
                || !(rect.origin.x + rect.size.width).is_finite()
                || !(rect.origin.y + rect.size.height).is_finite() {
                return Err(Diagnostic::PreflightUnavailable);
            }
            if point.x >= rect.origin.x && point.x < rect.origin.x + rect.size.width
                && point.y >= rect.origin.y && point.y < rect.origin.y + rect.size.height {
                return Err(Diagnostic::PointObscured);
            }
        }
        Err(Diagnostic::TargetWindowUnavailable)
    }

    fn matches_string_attribute(element: Element, name: &'static [u8], expected: &str) -> Result<bool> {
        let actual = attribute(element, name)?;
        ensure!(unsafe { CFGetTypeID(actual.0) == CFStringGetTypeID() },
            "Invalid accessibility string attribute");
        let expected = owned(unsafe {
            CFStringCreateWithBytes(ptr::null(), expected.as_ptr(), expected.len() as isize, 0x0800_0100, false)
        }, "expected accessibility string")?;
        Ok(unsafe { CFEqual(actual.0, expected.0) })
    }

    fn element_at_point(point: [f64; 2], role: &str, label: &str) -> DiagnosticResult<Owned> {
        if !trusted() {
            return Err(Diagnostic::AccessibilityUnavailable);
        }
        if !point.iter().all(|value| value.is_finite())
            || role.is_empty() || role.len() > 256 || label.is_empty() || label.len() > 256 {
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
        let location = Point { x: point[0], y: point[1] };
        unobstructed(&target, location)?;
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
            return Err(Diagnostic::TargetElementMismatch);
        }
        if !unsafe { CFEqual(element.0, target.window.0) } {
            let window = attribute(element.0, b"AXWindow\0").map_err(|_| Diagnostic::HitTestUnavailable)?;
            if !unsafe { CFEqual(window.0, target.window.0) } {
                return Err(Diagnostic::TargetElementMismatch);
            }
        }
        if !matches_string_attribute(element.0, b"AXRole\0", role)
            .map_err(|_| Diagnostic::AccessibilityRoleUnavailable)? {
            return Err(Diagnostic::AccessibilityRoleMismatch);
        }
        if !matches_string_attribute(element.0, b"AXTitle\0", label)
            .map_err(|_| Diagnostic::AccessibilityTitleUnavailable)? {
            return Err(Diagnostic::AccessibilityTitleMismatch);
        }
        // Recheck live window identity and geometry after AX reads, then
        // recheck frontmost and occlusion.
        let current = window_target(false).map_err(|_| Diagnostic::TargetWindowUnavailable)?;
        if !(current.pid == target.pid && current.number == target.number
            && unsafe { CFEqual(current.window.0, target.window.0) }
            && bounds(current.position, current.size) == bounds(target.position, target.size)
            && current.display == target.display
            && bounds(current.display_bounds.origin, current.display_bounds.size)
                == bounds(target.display_bounds.origin, target.display_bounds.size)) {
            return Err(Diagnostic::TargetChanged);
        }
        frontmost(&current)?;
        unobstructed(&current, location)?;
        Ok(element)
    }

    pub(super) fn preflight_element(point: [f64; 2], role: &str, label: &str) -> DiagnosticResult<()> {
        element_at_point(point, role, label).map(|_| ())
    }

    fn boolean_attribute(element: Element, name: &'static [u8]) -> Option<bool> {
        let value = attribute(element, name).ok()?;
        if unsafe { CFGetTypeID(value.0) != CFBooleanGetTypeID() } {
            return None;
        }
        Some(unsafe { CFBooleanGetValue(value.0) })
    }

    pub(super) fn activate_element(point: [f64; 2], role: &str, label: &str) -> Result<()> {
        let initial = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
        let location = Point { x: point[0], y: point[1] };
        // Allocate both events before dispatch so allocation failure cannot leave
        // a pressed button behind. There is no retry after either event is posted.
        let down = owned(unsafe { CGEventCreateMouseEvent(ptr::null(), 1, location, 0) }, "mouse-down event")?;
        let up = owned(unsafe { CGEventCreateMouseEvent(ptr::null(), 2, location, 0) }, "mouse-up event")?;
        unsafe {
            // A plain single click must not inherit modifier or multi-click state.
            CGEventSetFlags(down.0, 0);
            CGEventSetFlags(up.0, 0);
            CGEventSetIntegerValueField(down.0, 1, 1);
            CGEventSetIntegerValueField(up.0, 1, 1);
        }
        let element = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
        ensure!(unsafe { CFEqual(initial.0, element.0) }, "The hit-tested element changed during validation");
        let focused = boolean_attribute(element.0, b"AXFocused\0");
        let selected = boolean_attribute(element.0, b"AXSelected\0");
        let dispatch_element = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
        ensure!(unsafe { CFEqual(element.0, dispatch_element.0) }, "The hit-tested element changed before dispatch");
        unsafe {
            CGEventPost(0, down.0);
            CGEventPost(0, up.0);
        }
        let verified = (|| -> Result<bool> {
            let current = element_at_point(point, role, label).map_err(|_| anyhow::anyhow!("Activation validation failed"))?;
            ensure!(unsafe { CFEqual(current.0, element.0) }, "Target changed after dispatch");
            Ok((focused == Some(false) && boolean_attribute(current.0, b"AXFocused\0") == Some(true))
                || (selected == Some(false) && boolean_attribute(current.0, b"AXSelected\0") == Some(true)))
        })();
        ensure!(matches!(verified, Ok(true)),
            "Activation was dispatched but its outcome is unknown");
        Ok(())
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

    fn bounds(position: Point, size: Size) -> [f64; 4] {
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

    pub(super) fn load_capture(capture_id: &str) -> Result<AppWindowCapture> {
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
        ensure!(record.captured_at_ms <= now && now - record.captured_at_ms <= 300_000,
            "Capture record is expired or has a future timestamp");
        let path = directory.join(format!("app-window-{capture_id}.png"));
        let image = open_private_file(&path)?;
        ensure!(image.metadata()?.len() > 0, "Capture image is empty");
        ensure!(screen_recording_allowed(), "macOS Screen Recording permission is required");
        let current = capture_target()?;
        ensure!(record.matches(&current), "WeCom capture target or geometry changed");
        let now = now_ms()?;
        ensure!(record.captured_at_ms <= now && now - record.captured_at_ms <= 300_000,
            "Capture record is expired or has a future timestamp");
        Ok(AppWindowCapture {
            path,
            capture_id: capture_id.to_owned(),
            image_width: record.image_width,
            image_height: record.image_height,
            window_bounds_points: record.window_bounds,
        })
    }

    pub(super) fn capture() -> Result<AppWindowCapture> {
        ensure!(screen_recording_allowed(), "macOS Screen Recording permission is required");
        let target = capture_target()?;
        let captured_at_ms = now_ms()?;
        let image = owned(unsafe { CGWindowListCreateImage(CGRectNull, 8, target.number, 1) }, "WeCom window image")?;
        let width = unsafe { CGImageGetWidth(image.0) };
        let height = unsafe { CGImageGetHeight(image.0) };
        let image_width = u32::try_from(width).context("WeCom capture width is too large")?;
        let image_height = u32::try_from(height).context("WeCom capture height is too large")?;
        ensure!(image_width > 0 && image_height > 0, "WeCom window capture is empty");
        // A shadow-free window image must have the window's aspect ratio,
        // allowing one pixel of quantization at each dimension.
        let width_from_height = f64::from(image_height) * target.size.width / target.size.height;
        let height_from_width = f64::from(image_width) * target.size.height / target.size.width;
        ensure!((f64::from(image_width) - width_from_height).abs() <= 1.0
            || (f64::from(image_height) - height_from_width).abs() <= 1.0,
            "WeCom image dimensions do not match window geometry");
        let current = capture_target()?;
        ensure!(screen_recording_allowed() && current.pid == target.pid && current.number == target.number
            && unsafe { CFEqual(current.window.0, target.window.0) }
            && current.position.x == target.position.x && current.position.y == target.position.y
            && current.size.width == target.size.width && current.size.height == target.size.height
            && current.display == target.display
            && current.display_bounds.origin.x == target.display_bounds.origin.x
            && current.display_bounds.origin.y == target.display_bounds.origin.y
            && current.display_bounds.size.width == target.display_bounds.size.width
            && current.display_bounds.size.height == target.display_bounds.size.height,
            "WeCom capture target changed during capture");
        let data = owned(unsafe { CFDataCreateMutable(ptr::null(), 0) }, "PNG data")?;
        let png = string(b"public.png\0")?;
        let destination = owned(unsafe { CGImageDestinationCreateWithData(data.0, png.0, 1, ptr::null()) }, "PNG encoder")?;
        unsafe { CGImageDestinationAddImage(destination.0, image.0, ptr::null()); }
        ensure!(unsafe { CGImageDestinationFinalize(destination.0) }, "Could not encode window capture");
        let length = unsafe { CFDataGetLength(data.0) };
        let bytes = unsafe { CFDataGetBytePtr(data.0) };
        ensure!(length > 0 && !bytes.is_null(), "PNG capture is empty");
        let directory = capture_directory()?;
        let capture_id = uuid::Uuid::new_v4().to_string();
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
            file.write_all(unsafe { std::slice::from_raw_parts(bytes, length as usize) })
                .context("Could not write private capture file")?;
            let mut metadata = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
                .open(&record_path).context("Could not create private capture record")?;
            record_created = true;
            validate_private_file(&metadata)?;
            metadata.write_all(&record_bytes).context("Could not write private capture record")?;
            let current = capture_target()?;
            ensure!(screen_recording_allowed() && record.matches(&current)
                && unsafe { CFEqual(current.window.0, target.window.0) },
                "WeCom capture target changed during recording");
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
        Ok(AppWindowCapture {
            path,
            capture_id,
            image_width,
            image_height,
            window_bounds_points: record.window_bounds,
        })
    }

    pub(super) fn center() -> Result<()> {
        let target = target()?;
        let name = string(b"AXPosition\0")?;
        let value = owned(unsafe { AXValueCreate(1, (&target.position as *const Point).cast()) }, "window position")?;
        let error = unsafe { AXUIElementSetAttributeValue(target.window.0, name.0, value.0) };
        if error != 0 {
            bail!("Could not move WeCom window (AX error {error})");
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
}
