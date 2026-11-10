//! Exact-window images for transient windows that screencapture redirects to parents.
use anyhow::{Result, anyhow, ensure};
use block2::RcBlock;
use std::ffi::{CStr, c_char, c_void};
use std::sync::mpsc;
use std::time::{Duration, Instant};

type Object = *const c_void;
type Image = (u32, u32, Vec<u8>);

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Object;
    fn sel_registerName(name: *const c_char) -> Object;
    fn objc_msgSend();
}
#[link(name = "ScreenCaptureKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Object);
    fn CFDataCreateMutable(allocator: Object, capacity: isize) -> Object;
    fn CFDataGetLength(data: Object) -> isize;
    fn CFDataGetBytePtr(data: Object) -> *const u8;
    fn CFStringCreateWithCString(allocator: Object, text: *const c_char, encoding: u32) -> Object;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: Object) -> usize;
    fn CGImageGetHeight(image: Object) -> usize;
}
#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithData(
        data: Object,
        kind: Object,
        count: usize,
        options: Object,
    ) -> Object;
    fn CGImageDestinationAddImage(destination: Object, image: Object, properties: Object);
    fn CGImageDestinationFinalize(destination: Object) -> bool;
}

struct Cf(Object);
impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
struct Objc(Object);
impl Drop for Objc {
    fn drop(&mut self) {
        let release: unsafe extern "C" fn(Object, Object) =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { release(self.0, selector(c"release")) }
    }
}
fn selector(name: &CStr) -> Object {
    unsafe { sel_registerName(name.as_ptr()) }
}
fn require_method(object: Object, name: &CStr) -> Result<()> {
    let responds: unsafe extern "C" fn(Object, Object, Object) -> i8 =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    ensure!(
        unsafe { responds(object, selector(c"respondsToSelector:"), selector(name)) } != 0,
        "Required native screenshot API unavailable"
    );
    Ok(())
}
fn class(name: &CStr) -> Result<Object> {
    let value = unsafe { objc_getClass(name.as_ptr()) };
    ensure!(!value.is_null(), "Native window screenshot API unavailable");
    Ok(value)
}
fn zero(object: Object, name: &CStr) -> Object {
    let send: unsafe extern "C" fn(Object, Object) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { send(object, selector(name)) }
}
fn one(object: Object, name: &CStr, arg: Object) -> Object {
    let send: unsafe extern "C" fn(Object, Object, Object) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { send(object, selector(name), arg) }
}
fn integer(object: Object, name: &CStr) -> usize {
    let send: unsafe extern "C" fn(Object, Object) -> usize =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { send(object, selector(name)) }
}
fn set_size(object: Object, name: &CStr, value: usize) {
    let send: unsafe extern "C" fn(Object, Object, usize) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { send(object, selector(name), value) }
}
fn set_bool(object: Object, name: &CStr, value: bool) {
    let send: unsafe extern "C" fn(Object, Object, i8) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { send(object, selector(name), i8::from(value)) }
}
fn initialized(name: &CStr) -> Result<Objc> {
    let value = zero(zero(class(name)?, c"alloc"), c"init");
    ensure!(
        !value.is_null(),
        "Could not create screenshot configuration"
    );
    Ok(Objc(value))
}
fn encode_png(image: Object) -> Result<Image> {
    ensure!(
        !image.is_null(),
        "Native window screenshot returned no image"
    );
    let width = u32::try_from(unsafe { CGImageGetWidth(image) })?;
    let height = u32::try_from(unsafe { CGImageGetHeight(image) })?;
    ensure!(width > 0 && height > 0, "Native window image is empty");
    let data = unsafe { CFDataCreateMutable(std::ptr::null_mut(), 0) };
    ensure!(!data.is_null(), "Could not allocate encoded window image");
    let data = Cf(data);
    let kind = unsafe {
        CFStringCreateWithCString(std::ptr::null_mut(), c"public.png".as_ptr(), 0x08000100)
    };
    ensure!(!kind.is_null(), "Could not create window image encoding");
    let kind = Cf(kind);
    let destination =
        unsafe { CGImageDestinationCreateWithData(data.0, kind.0, 1, std::ptr::null_mut()) };
    ensure!(
        !destination.is_null(),
        "Could not create window image encoder"
    );
    let destination = Cf(destination);
    unsafe { CGImageDestinationAddImage(destination.0, image, std::ptr::null_mut()) };
    ensure!(
        unsafe { CGImageDestinationFinalize(destination.0) },
        "Window image encoding failed"
    );
    let length = unsafe { CFDataGetLength(data.0) };
    let bytes = unsafe { CFDataGetBytePtr(data.0) };
    ensure!(
        length > 0 && !bytes.is_null(),
        "Encoded window image is empty"
    );
    Ok((
        width,
        height,
        unsafe { std::slice::from_raw_parts(bytes, length as usize) }.to_vec(),
    ))
}

/// Retains the exact SCWindow so it outlives the enumeration callback.
///
/// Request the screenshot only after the enumeration callback returns;
/// starting capture inside that callback can prevent image completion.
fn select_window(content: Object, window_id: u32, pid: i32) -> Result<Objc> {
    let windows = zero(content, c"windows");
    ensure!(
        !windows.is_null(),
        "Native screenshot window list unavailable"
    );
    let count = integer(windows, c"count");
    ensure!(
        count <= 16384,
        "Native screenshot window list exceeds limit"
    );
    let get: unsafe extern "C" fn(Object, Object, usize) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let mut selected: Object = std::ptr::null_mut();
    for index in 0..count {
        let window = unsafe { get(windows, selector(c"objectAtIndex:"), index) };
        if integer(window, c"windowID") != window_id as usize {
            continue;
        }
        let app = zero(window, c"owningApplication");
        ensure!(
            !app.is_null() && integer(app, c"processID") == pid as usize,
            "Native screenshot window owner changed"
        );
        ensure!(
            selected.is_null(),
            "Native screenshot window identity is ambiguous"
        );
        selected = window;
    }
    ensure!(
        !selected.is_null(),
        "Selected window unavailable to native screenshot API"
    );
    let retain: unsafe extern "C" fn(Object, Object) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    Ok(Objc(unsafe { retain(selected, selector(c"retain")) }))
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|budget| !budget.is_zero())
        .ok_or_else(|| anyhow!("Native window screenshot timed out"))
}

/// Metadata-only discovery: no screenshot or permission request.
pub(crate) fn available() -> bool {
    let check = || -> Result<()> {
        require_method(
            class(c"SCScreenshotManager")?,
            c"captureImageWithFilter:configuration:completionHandler:",
        )?;
        let instances: unsafe extern "C" fn(Object, Object, Object) -> i8 =
            unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        for (name, method) in [
            (c"SCContentFilter", c"pointPixelScale"),
            (c"SCStreamConfiguration", c"setIncludeChildWindows:"),
        ] {
            ensure!(
                unsafe {
                    instances(
                        class(name)?,
                        selector(c"instancesRespondToSelector:"),
                        selector(method),
                    )
                } != 0,
                "Required whole-window screenshot API unavailable"
            );
        }
        Ok(())
    };
    check().is_ok()
}

/// Captures the whole selected window together with its child windows, so an
/// anchored transient panel remains part of the app-window image.
pub(super) fn capture_window(window_id: u32, pid: i32, width: f64, height: f64) -> Result<Image> {
    require_method(
        class(c"SCScreenshotManager")?,
        c"captureImageWithFilter:configuration:completionHandler:",
    )?;
    ensure!(
        window_id > 0 && pid > 0 && width > 0.0 && height > 0.0,
        "Invalid selected screenshot window"
    );
    // One bounded budget covers enumeration and capture together.
    let deadline = Instant::now() + Duration::from_secs(15);
    let (tx, rx) = mpsc::channel();
    let completion = RcBlock::new(move |content: Object, error: Object| {
        let result = if error.is_null() && !content.is_null() {
            select_window(content, window_id, pid)
        } else {
            Err(anyhow!("Could not enumerate native screenshot windows"))
        };
        let _ = tx.send(result);
    });
    let send: unsafe extern "C" fn(Object, Object, i8, i8, *const c_void) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe {
        send(class(c"SCShareableContent")?, selector(c"getShareableContentExcludingDesktopWindows:onScreenWindowsOnly:completionHandler:"), 1, 1, &*completion as *const block2::Block<dyn Fn(Object, Object)> as *const c_void)
    };
    // Runs only after the enumeration callback returned; requesting the image
    // from inside that callback never delivers a completion.
    // Transfer the retained immutable SCWindow as an owned value. Late sends
    // and queued results are released even when the receiver has timed out.
    let window = rx.recv_timeout(remaining(deadline)?)
        .map_err(|_| anyhow!("Native window screenshot timed out"))??;
    let filter = one(
        zero(class(c"SCContentFilter")?, c"alloc"),
        c"initWithDesktopIndependentWindow:",
        window.0,
    );
    ensure!(
        !filter.is_null(),
        "Could not select independent screenshot window"
    );
    let filter = Objc(filter);
    require_method(filter.0, c"pointPixelScale")?;
    let scale: unsafe extern "C" fn(Object, Object) -> f32 =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    let scale = f64::from(unsafe { scale(filter.0, selector(c"pointPixelScale")) });
    ensure!(
        scale.is_finite() && scale > 0.0,
        "Invalid native screenshot pixel scale"
    );
    let pixels = |points: f64| -> Result<usize> {
        let value = (points * scale).round();
        ensure!(
            value.is_finite() && (1.0..=32768.0).contains(&value),
            "Invalid native screenshot dimensions"
        );
        Ok(value as usize)
    };
    let config = initialized(c"SCStreamConfiguration")?;
    require_method(config.0, c"setIncludeChildWindows:")?;
    set_size(config.0, c"setWidth:", pixels(width)?);
    set_size(config.0, c"setHeight:", pixels(height)?);
    set_bool(config.0, c"setShowsCursor:", false);
    set_bool(config.0, c"setIgnoreShadowsSingleWindow:", true);
    set_bool(config.0, c"setIncludeChildWindows:", true);
    set_bool(config.0, c"setScalesToFit:", true);
    // ScreenCaptureKit copies the block. Its owned sender remains valid even if
    // the waiter times out; image encoding finishes inside the image's lifetime.
    let (tx, rx) = mpsc::channel();
    let completion = RcBlock::new(move |image: Object, error: Object| {
        let result = if error.is_null() {
            encode_png(image)
        } else {
            Err(anyhow!("Native window screenshot failed"))
        };
        let _ = tx.send(result);
    });
    let send: unsafe extern "C" fn(Object, Object, Object, Object, *const c_void) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe {
        send(
            class(c"SCScreenshotManager")?,
            selector(c"captureImageWithFilter:configuration:completionHandler:"),
            filter.0,
            config.0,
            &*completion as *const block2::Block<dyn Fn(Object, Object)> as *const c_void,
        )
    };
    rx.recv_timeout(remaining(deadline)?)
        .map_err(|_| anyhow!("Native window screenshot timed out"))?
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn abandoned_enumeration_results_release_retained_objects() {
        // NSObject allocation/ARC only: no window enumeration, permission
        // request, screenshot or desktop input.
        for queued in [false, true] {
            let owner = initialized(c"NSObject").unwrap();
            let baseline = integer(owner.0, c"retainCount");
            let retained = Objc(zero(owner.0, c"retain"));
            let (tx, rx) = mpsc::channel::<Result<Objc>>();
            if queued {
                assert!(tx.send(Ok(retained)).is_ok());
                assert_eq!(integer(owner.0, c"retainCount"), baseline + 1);
                drop(rx);
            } else {
                drop(rx);
                drop(tx.send(Ok(retained)));
            }
            assert_eq!(integer(owner.0, c"retainCount"), baseline);
        }
    }
}
