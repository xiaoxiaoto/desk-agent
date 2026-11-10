/// The installed application selected by an application adapter for launch.
#[derive(Clone, Copy)]
pub(crate) struct LaunchTarget {
    pub(crate) application_name: &'static str,
}

/// Explicit process identity selected by an authorized application adapter.
#[derive(Clone, Copy)]
pub(crate) struct DesktopApplication {
    pub(crate) bundle_identifier: &'static std::ffi::CStr,
    pub(crate) display_name: &'static str,
}

/// Global top-left-origin desktop coordinates in logical points.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Point { pub(crate) x: f64, pub(crate) y: f64 }

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Size { pub(crate) width: f64, pub(crate) height: f64 }

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Rect { pub(crate) origin: Point, pub(crate) size: Size }

/// A private selected-window image and its capture-time geometry.
pub struct WindowCapture {
    pub path: std::path::PathBuf,
    pub capture_id: String,
    pub image_width: u32,
    pub image_height: u32,
    pub window_bounds_points: [f64; 4],
}
