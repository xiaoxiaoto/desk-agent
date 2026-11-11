//! Private pasteboard primitives for an already-authorized attachment operation.
//! No application layout or message semantics belong here.
use anyhow::{ensure, Context, Result};
use std::ffi::{c_char, c_void};
use std::path::Path;
use std::ptr;

type Object = *const c_void;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(allocator: Object, bytes: *const u8, length: isize, encoding: u32, external: bool) -> Object;
    fn CFDataCreate(allocator: Object, bytes: *const u8, length: isize) -> Object;
    fn CFRelease(value: Object);
}
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Object;
    fn sel_registerName(name: *const c_char) -> Object;
    fn objc_msgSend();
}

struct Owned(Object);
impl Drop for Owned { fn drop(&mut self) { unsafe { CFRelease(self.0) } } }
struct Pool(Object);
impl Drop for Pool {
    fn drop(&mut self) { unsafe { send_void(self.0, c"drain") } }
}
unsafe fn selector(name: &std::ffi::CStr) -> Object { unsafe { sel_registerName(name.as_ptr()) } }
unsafe fn send0(obj: Object, name: &std::ffi::CStr) -> Object {
    let f: unsafe extern "C" fn(Object, Object) -> Object = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { f(obj, selector(name)) }
}
unsafe fn send1(obj: Object, name: &std::ffi::CStr, value: Object) -> Object {
    let f: unsafe extern "C" fn(Object, Object, Object) -> Object = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { f(obj, selector(name), value) }
}
unsafe fn send_void(obj: Object, name: &std::ffi::CStr) {
    let f: unsafe extern "C" fn(Object, Object) = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { f(obj, selector(name)) }
}
unsafe fn send_count(obj: Object, name: &std::ffi::CStr) -> isize {
    let f: unsafe extern "C" fn(Object, Object) -> isize = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { f(obj, selector(name)) }
}
unsafe fn send_bool1(obj: Object, name: &std::ffi::CStr, value: Object) -> bool {
    let f: unsafe extern "C" fn(Object, Object, Object) -> i8 = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { f(obj, selector(name), value) != 0 }
}
unsafe fn send_bool2(obj: Object, name: &std::ffi::CStr, a: Object, b: Object) -> bool {
    let f: unsafe extern "C" fn(Object, Object, Object, Object) -> i8 = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { f(obj, selector(name), a, b) != 0 }
}
fn string(value: &str) -> Result<Owned> {
    let length = isize::try_from(value.len()).context("Pasteboard string too large")?;
    let obj = unsafe { CFStringCreateWithBytes(ptr::null(), value.as_ptr(), length, 0x0800_0100, false) };
    ensure!(!obj.is_null(), "Cannot create pasteboard string");
    Ok(Owned(obj))
}
fn with_pasteboard<T>(f: impl FnOnce(Object) -> Result<T>) -> Result<T> {
    unsafe {
        let pool_class = objc_getClass(c"NSAutoreleasePool".as_ptr());
        let board_class = objc_getClass(c"NSPasteboard".as_ptr());
        ensure!(!pool_class.is_null() && !board_class.is_null(), "Pasteboard unavailable");
        let pool = send0(send0(pool_class, c"alloc"), c"init");
        ensure!(!pool.is_null(), "Cannot create autorelease pool");
        let _pool = Pool(pool);
        let board = send0(board_class, c"generalPasteboard");
        ensure!(!board.is_null(), "Pasteboard unavailable");
        f(board)
    }
}

/// The change count is an ownership marker, not a lock on the global pasteboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClipboardToken(isize);

/// Publish the exact encoded PNG/JPEG bytes under their native UTI.
pub(crate) fn write_image(bytes: &[u8], mime_type: &str) -> Result<ClipboardToken> {
    let uti = match mime_type {
        "image/png" => "public.png",
        "image/jpeg" => "public.jpeg",
        _ => anyhow::bail!("Unsupported image pasteboard type"),
    };
    let length = isize::try_from(bytes.len()).context("Image too large")?;
    let data = unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), length) };
    ensure!(!data.is_null(), "Cannot create image pasteboard data");
    let data = Owned(data);
    let uti = string(uti)?;
    with_pasteboard(|board| unsafe {
        let cleared = send_count(board, c"clearContents");
        ensure!(send_count(board, c"changeCount") == cleared, "Pasteboard changed");
        ensure!(send_bool2(board, c"setData:forType:", data.0, uti.0), "Cannot write image pasteboard data");
        let count = send_count(board, c"changeCount");
        ensure!(count == cleared, "Pasteboard changed while writing");
        Ok(ClipboardToken(count))
    })
}

/// Publish a file URL, not a text path; the staged basename is the original filename.
pub(crate) fn write_file(path: &Path, filename: &str) -> Result<ClipboardToken> {
    ensure!(path.is_absolute() && path.file_name().and_then(|n| n.to_str()) == Some(filename), "Invalid staged filename");
    let path = string(path.to_str().context("Non-UTF-8 staged path")?)?;
    with_pasteboard(|board| unsafe {
        let url_class = objc_getClass(c"NSURL".as_ptr());
        let array_class = objc_getClass(c"NSArray".as_ptr());
        ensure!(!url_class.is_null() && !array_class.is_null(), "File pasteboard unavailable");
        let url = send1(url_class, c"fileURLWithPath:", path.0);
        ensure!(!url.is_null(), "Cannot create staged file URL");
        let array = send1(array_class, c"arrayWithObject:", url);
        ensure!(!array.is_null(), "Cannot create pasteboard item array");
        let cleared = send_count(board, c"clearContents");
        ensure!(send_count(board, c"changeCount") == cleared, "Pasteboard changed");
        ensure!(send_bool1(board, c"writeObjects:", array), "Cannot write file pasteboard data");
        let count = send_count(board, c"changeCount");
        ensure!(count == cleared, "Pasteboard changed while writing");
        Ok(ClipboardToken(count))
    })
}

pub(crate) fn unchanged(token: ClipboardToken) -> Result<bool> {
    with_pasteboard(|board| Ok(unsafe { send_count(board, c"changeCount") == token.0 }))
}

/// Best-effort cleanup only while the global pasteboard still has our change count.
/// NSPasteboard does not provide compare-and-clear, so this is not a lock.
pub(crate) fn clear_if_unchanged(token: ClipboardToken) -> Result<bool> {
    with_pasteboard(|board| unsafe {
        if send_count(board, c"changeCount") != token.0 { return Ok(false); }
        send_count(board, c"clearContents");
        Ok(true)
    })
}
