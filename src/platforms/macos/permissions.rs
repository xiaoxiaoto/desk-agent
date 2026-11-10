#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
}

pub(crate) fn trusted() -> bool { unsafe { AXIsProcessTrusted() } }
pub(crate) fn input_monitoring_available() -> bool { unsafe { CGPreflightListenEventAccess() } }
pub(crate) fn screen_recording_allowed() -> bool { unsafe { CGPreflightScreenCaptureAccess() } }
pub(crate) fn request_screen_recording_permission() -> bool {
    screen_recording_allowed() || unsafe { CGRequestScreenCaptureAccess() }
}
