use anyhow::{ensure, Result};
use std::ffi::{c_char, c_void, CStr};
use std::ptr;

type Object = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(alloc: Object, string: *const c_char, encoding: u32) -> Object;
    fn CFArrayGetCount(array: Object) -> isize;
    fn CFArrayGetValueAtIndex(array: Object, index: isize) -> Object;
    fn CFRelease(value: Object);
}
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Object;
    fn sel_registerName(name: *const c_char) -> Object;
    fn objc_msgSend();
}

/// Find exactly one running instance without launching or activating it.
pub(crate) fn running_pid(bundle: &CStr, application: &str) -> Result<i32> {
    unsafe {
        let class = objc_getClass(c"NSRunningApplication".as_ptr());
        let pool_class = objc_getClass(c"NSAutoreleasePool".as_ptr());
        ensure!(!class.is_null() && !pool_class.is_null(), "AppKit is unavailable");
        let selector = |name: &'static [u8]| sel_registerName(name.as_ptr().cast());
        let send0: unsafe extern "C" fn(Object, Object) -> Object = std::mem::transmute(objc_msgSend as *const ());
        let send1: unsafe extern "C" fn(Object, Object, Object) -> Object = std::mem::transmute(objc_msgSend as *const ());
        let send_pid: unsafe extern "C" fn(Object, Object) -> i32 = std::mem::transmute(objc_msgSend as *const ());
        let pool = send0(send0(pool_class, selector(b"alloc\0")), selector(b"init\0"));
        ensure!(!pool.is_null(), "Could not create autorelease pool");
        let result = (|| {
            let bundle = CFStringCreateWithCString(ptr::null(), bundle.as_ptr(), 0x0800_0100);
            ensure!(!bundle.is_null(), "Could not create CFString");
            let apps = send1(class, selector(b"runningApplicationsWithBundleIdentifier:\0"), bundle);
            CFRelease(bundle);
            ensure!(!apps.is_null(), "Could not enumerate running {application} applications");
            let count = CFArrayGetCount(apps);
            ensure!(count == 1, "Expected exactly one running {application} instance, found {count}");
            let app = CFArrayGetValueAtIndex(apps, 0);
            ensure!(!app.is_null(), "Could not identify the running {application} instance");
            let pid = send_pid(app, selector(b"processIdentifier\0"));
            ensure!(pid > 0, "Invalid {application} process identifier");
            Ok(pid)
        })();
        let drain: unsafe extern "C" fn(Object, Object) = std::mem::transmute(objc_msgSend as *const ());
        drain(pool, selector(b"drain\0"));
        result
    }
}

/// Return the currently frontmost process without activating any application.
pub(crate) fn frontmost_pid() -> Option<i32> {
    unsafe {
        let class = objc_getClass(c"NSWorkspace".as_ptr());
        let pool_class = objc_getClass(c"NSAutoreleasePool".as_ptr());
        if class.is_null() || pool_class.is_null() { return None; }
        let selector = |name: &'static [u8]| sel_registerName(name.as_ptr().cast());
        let send: unsafe extern "C" fn(Object, Object) -> Object = std::mem::transmute(objc_msgSend as *const ());
        let send_pid: unsafe extern "C" fn(Object, Object) -> i32 = std::mem::transmute(objc_msgSend as *const ());
        let pool = send(send(pool_class, selector(b"alloc\0")), selector(b"init\0"));
        if pool.is_null() { return None; }
        let workspace = send(class, selector(b"sharedWorkspace\0"));
        let app = if workspace.is_null() { ptr::null() } else { send(workspace, selector(b"frontmostApplication\0")) };
        let pid = if app.is_null() { 0 } else { send_pid(app, selector(b"processIdentifier\0")) };
        let drain: unsafe extern "C" fn(Object, Object) = std::mem::transmute(objc_msgSend as *const ());
        drain(pool, selector(b"drain\0"));
        (pid > 0).then_some(pid)
    }
}
