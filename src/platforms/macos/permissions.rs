use std::ffi::c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: *const c_void;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFDictionaryCreate(allocator: *const c_void, keys: *const *const c_void, values: *const *const c_void, count: isize, key_callbacks: *const c_void, value_callbacks: *const c_void) -> *const c_void;
    fn CFRelease(value: *const c_void);
}

pub(crate) fn trusted() -> bool { unsafe { AXIsProcessTrusted() } }
pub(crate) fn input_monitoring_available() -> bool { unsafe { CGPreflightListenEventAccess() } }
pub(crate) fn screen_recording_allowed() -> bool { unsafe { CGPreflightScreenCaptureAccess() } }

/// Live OS permission state after a setup request.
#[derive(Debug, Clone, Copy)]
pub struct PermissionStatus {
    pub accessibility: bool,
    pub screen_recording: bool,
    pub input_monitoring: bool,
}

/// Install-time only: asks macOS once for every permission the runtime needs.
/// macOS never lets a process grant these silently; the user approves each in
/// System Settings (or an MDM PPPC profile pre-approves them). Runtime dispatch
/// only preflights and never calls this.
pub fn request_all() -> PermissionStatus {
    let accessibility = trusted() || unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 1,
            &raw const kCFTypeDictionaryKeyCallBacks, &raw const kCFTypeDictionaryValueCallBacks);
        let result = AXIsProcessTrustedWithOptions(options);
        if !options.is_null() { CFRelease(options); }
        result
    };
    let screen_recording = screen_recording_allowed() || unsafe { CGRequestScreenCaptureAccess() };
    let input_monitoring = input_monitoring_available() || unsafe { CGRequestListenEventAccess() };
    PermissionStatus { accessibility, screen_recording, input_monitoring }
}

pub fn status() -> PermissionStatus {
    PermissionStatus { accessibility: trusted(), screen_recording: screen_recording_allowed(), input_monitoring: input_monitoring_available() }
}
