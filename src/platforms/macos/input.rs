//! Native input building blocks for already-authorized adapter operations.
//! Preparation allocates both events; posting consumes one pair and never retries.
use anyhow::{ensure, Result};
use std::ffi::c_void;
use std::ptr;
use std::time::Duration;
use super::permissions::input_monitoring_available;

type Object = *const c_void;
#[repr(C)]
#[derive(Clone, Copy)]
struct Point { x: f64, y: f64 }

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Object);
    fn CFRunLoopGetCurrent() -> Object;
    fn CFRunLoopAddSource(run_loop: Object, source: Object, mode: Object);
    fn CFRunLoopRemoveSource(run_loop: Object, source: Object, mode: Object);
    fn CFRunLoopRunInMode(mode: Object, seconds: f64, return_after_source_handled: bool) -> i32;
    fn CFMachPortCreateRunLoopSource(allocator: Object, port: Object, order: isize) -> Object;
    fn CFMachPortInvalidate(port: Object);
    static kCFRunLoopDefaultMode: Object;
}
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventCreateMouseEvent(source: Object, event_type: u32, position: Point, button: u32) -> Object;
    fn CGEventCreateKeyboardEvent(source: Object, key: u16, down: bool) -> Object;
    fn CGEventKeyboardSetUnicodeString(event: Object, length: usize, text: *const u16);
    fn CGEventSetFlags(event: Object, flags: u64);
    fn CGEventSetIntegerValueField(event: Object, field: u32, value: i64);
    fn CGEventGetIntegerValueField(event: Object, field: u32) -> i64;
    fn CGEventGetFlags(event: Object) -> u64;
    fn CGEventGetLocation(event: Object) -> Point;
    fn CGEventGetTimestamp(event: Object) -> u64;
    fn CGEventTapCreateForPid(pid: i32, place: u32, options: u32, mask: u64,
        callback: unsafe extern "C" fn(Object, u32, Object, *mut c_void) -> Object,
        user_info: *mut c_void) -> Object;
    fn CGEventTapIsEnabled(tap: Object) -> bool;
    fn CGEventPost(tap: u32, event: Object);
}

struct Owned(Object);
impl Drop for Owned {
    fn drop(&mut self) { unsafe { CFRelease(self.0) } }
}
fn owned(value: Object, what: &str) -> Result<Owned> {
    ensure!(!value.is_null(), "Could not create {what}");
    Ok(Owned(value))
}

pub(crate) struct PreparedInputPair { down: Owned, up: Owned }
impl PreparedInputPair {
    pub(crate) fn mouse(point: [f64; 2], token: i64) -> Result<Self> {
        let location = Point { x: point[0], y: point[1] };
        let down = owned(unsafe { CGEventCreateMouseEvent(ptr::null(), 1, location, 0) }, "mouse-down event")?;
        let up = owned(unsafe { CGEventCreateMouseEvent(ptr::null(), 2, location, 0) }, "mouse-up event")?;
        unsafe {
            for event in [down.0, up.0] {
                CGEventSetFlags(event, 0);
                CGEventSetIntegerValueField(event, 1, 1);
                CGEventSetIntegerValueField(event, 42, token);
            }
        }
        Ok(Self { down, up })
    }

    pub(crate) fn keyboard(key: u16, flags: u64, text: Option<&[u16]>, token: Option<i64>) -> Result<Self> {
        let down = owned(unsafe { CGEventCreateKeyboardEvent(ptr::null(), key, true) }, "key-down event")?;
        let up = owned(unsafe { CGEventCreateKeyboardEvent(ptr::null(), key, false) }, "key-up event")?;
        unsafe {
            for event in [down.0, up.0] {
                CGEventSetFlags(event, flags);
                if let Some(text) = text {
                    CGEventKeyboardSetUnicodeString(event, text.len(), text.as_ptr());
                }
                if let Some(token) = token {
                    CGEventSetIntegerValueField(event, 42, token);
                }
            }
        }
        Ok(Self { down, up })
    }

    pub(crate) fn post(self) {
        unsafe {
            CGEventPost(0, self.down.0);
            CGEventPost(0, self.up.0);
        }
    }
}

pub(crate) fn observe_for(duration: Duration) {
    unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, duration.as_secs_f64(), false) };
}

#[derive(Clone, Copy)]
struct RoutedEvent {
    kind: u32,
    timestamp: u64,
    token: i64,
    point: Point,
    pid: i64,
    flags: u64,
    click_state: i64,
    button: i64,
}

struct ClickEvidence {
    token: i64,
    point: Point,
    pid: i32,
    stage: u8,
    down_timestamp: u64,
    up_timestamp: u64,
    disabled: bool,
}

impl ClickEvidence {
    fn observe(&mut self, event: RoutedEvent) {
        if self.disabled || event.token != self.token {
            return;
        }
        if event.point.x != self.point.x || event.point.y != self.point.y
            || event.pid != i64::from(self.pid) || event.flags != 0
            || event.click_state != 1 || event.button != 0 {
            self.disabled = true;
            return;
        }
        self.stage = match (self.stage, event.kind) {
            (0, 1) => { self.down_timestamp = event.timestamp; 1 },
            // A process tap can observe the same routed event more than once.
            (1, 1) if event.timestamp == self.down_timestamp => 1,
            (1, 2) => { self.up_timestamp = event.timestamp; 2 },
            (2, 1) if event.timestamp == self.down_timestamp => 2,
            (2, 2) if event.timestamp == self.up_timestamp => 2,
            _ => { self.disabled = true; self.stage }
        };
    }

    fn complete(&self) -> bool { !self.disabled && self.stage == 2 }
}

unsafe extern "C" fn click_tap_callback(
    _proxy: Object, kind: u32, event: Object, user_info: *mut c_void,
) -> Object {
    let evidence = unsafe { &mut *user_info.cast::<ClickEvidence>() };
    if kind == 0xffff_fffe || kind == 0xffff_ffff {
        evidence.disabled = true;
    } else if !event.is_null() {
        evidence.observe(RoutedEvent {
            kind,
            timestamp: unsafe { CGEventGetTimestamp(event) },
            token: unsafe { CGEventGetIntegerValueField(event, 42) },
            point: unsafe { CGEventGetLocation(event) },
            pid: unsafe { CGEventGetIntegerValueField(event, 40) },
            flags: unsafe { CGEventGetFlags(event) },
            click_state: unsafe { CGEventGetIntegerValueField(event, 1) },
            button: unsafe { CGEventGetIntegerValueField(event, 3) },
        });
    }
    event
}

// The callback state stays at a stable address until the source is removed
// and the tap is invalidated. This tap only observes the target process.
pub(crate) struct MouseMonitor {
    port: Owned,
    source: Owned,
    run_loop: Object,
    evidence: Box<ClickEvidence>,
}

impl MouseMonitor {
    pub(crate) fn complete(&self) -> bool { self.evidence.complete() }
    pub(crate) fn new(pid: i32, point: [f64; 2], token: i64) -> Option<Self> {
        let point = Point { x: point[0], y: point[1] };
        if !input_monitoring_available() { return None; }
        let mut evidence = Box::new(ClickEvidence { token, point, pid, stage: 0,
            down_timestamp: 0, up_timestamp: 0, disabled: false });
        let port = unsafe {
            CGEventTapCreateForPid(pid, 1, 1, (1 << 1) | (1 << 2),
                click_tap_callback, (&mut *evidence as *mut ClickEvidence).cast())
        };
        if port.is_null() { return None; }
        let port = Owned(port);
        let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), port.0, 0) };
        if source.is_null() {
            unsafe { CFMachPortInvalidate(port.0) };
            return None;
        }
        let source = Owned(source);
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        unsafe { CFRunLoopAddSource(run_loop, source.0, kCFRunLoopDefaultMode) };
        Some(Self { port, source, run_loop, evidence })
    }

    pub(crate) fn enabled(&self) -> bool {
        !self.evidence.disabled && unsafe { CGEventTapIsEnabled(self.port.0) }
    }
}

impl Drop for MouseMonitor {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopRemoveSource(self.run_loop, self.source.0, kCFRunLoopDefaultMode);
            CFMachPortInvalidate(self.port.0);
        }
    }
}

struct KeyEvidence {
    token: i64, pid: i32, key: u16, flags: u64, stage: u8, down: u64, up: u64, disabled: bool,
}
impl KeyEvidence {
    fn observe(&mut self, kind: u32, timestamp: u64, token: i64, pid: i64, key: i64, flags: u64) {
        if self.disabled || token != self.token { return; }
        if pid != i64::from(self.pid) || key != i64::from(self.key) || flags != self.flags {
            self.disabled = true;
            return;
        }
        self.stage = match (self.stage, kind) {
            (0, 10) => { self.down = timestamp; 1 },
            (1, 10) if timestamp == self.down => 1,
            (1, 11) => { self.up = timestamp; 2 },
            (2, 10) if timestamp == self.down => 2,
            (2, 11) if timestamp == self.up => 2,
            _ => { self.disabled = true; self.stage },
        };
    }
    fn complete(&self) -> bool { !self.disabled && self.stage == 2 }
}
unsafe extern "C" fn key_tap_callback(_: Object, kind: u32, event: Object, info: *mut c_void) -> Object {
    let evidence = unsafe { &mut *info.cast::<KeyEvidence>() };
    if kind == 0xffff_fffe || kind == 0xffff_ffff {
        evidence.disabled = true;
    } else if !event.is_null() {
        evidence.observe(kind, unsafe { CGEventGetTimestamp(event) },
            unsafe { CGEventGetIntegerValueField(event, 42) },
            unsafe { CGEventGetIntegerValueField(event, 40) },
            unsafe { CGEventGetIntegerValueField(event, 9) }, unsafe { CGEventGetFlags(event) });
    }
    event
}
pub(crate) struct KeyboardMonitor { port: Owned, source: Owned, run_loop: Object, evidence: Box<KeyEvidence> }
impl KeyboardMonitor {
    pub(crate) fn complete(&self) -> bool { self.evidence.complete() }
    pub(crate) fn new(pid: i32, token: i64) -> Option<Self> {
        Self::for_key(pid, token, 36)
    }
    pub(crate) fn for_key(pid: i32, token: i64, key: u16) -> Option<Self> {
        Self::for_chord(pid, token, key, 0)
    }
    /// Correlated evidence for one key with an exact modifier mask.
    pub(crate) fn for_chord(pid: i32, token: i64, key: u16, flags: u64) -> Option<Self> {
        if !input_monitoring_available() { return None; }
        let mut evidence = Box::new(KeyEvidence { token, pid, key, flags, stage: 0, down: 0, up: 0, disabled: false });
        let port = unsafe { CGEventTapCreateForPid(pid, 1, 1, (1 << 10) | (1 << 11),
            key_tap_callback, (&mut *evidence as *mut KeyEvidence).cast()) };
        if port.is_null() { return None; }
        let port = Owned(port);
        let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), port.0, 0) };
        if source.is_null() { unsafe { CFMachPortInvalidate(port.0) }; return None; }
        let source = Owned(source);
        let run_loop = unsafe { CFRunLoopGetCurrent() };
        unsafe { CFRunLoopAddSource(run_loop, source.0, kCFRunLoopDefaultMode) };
        Some(Self { port, source, run_loop, evidence })
    }
    pub(crate) fn enabled(&self) -> bool { !self.evidence.disabled && unsafe { CGEventTapIsEnabled(self.port.0) } }
}
impl Drop for KeyboardMonitor {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopRemoveSource(self.run_loop, self.source.0, kCFRunLoopDefaultMode);
            CFMachPortInvalidate(self.port.0);
        }
    }
}

#[cfg(test)]
mod key_evidence_tests {
    use super::KeyEvidence;

    fn evidence() -> KeyEvidence {
        KeyEvidence { token: 123, pid: 42, key: 36, flags: 0, stage: 0, down: 0, up: 0, disabled: false }
    }

    #[test]
    fn requires_correlated_ordered_pair_and_deduplicates() {
        let mut e = evidence();
        e.observe(10, 1, 999, 42, 36, 0);
        assert!(!e.complete());
        e.observe(10, 1, 123, 42, 36, 0);
        e.observe(10, 1, 123, 42, 36, 0);
        assert!(!e.complete());
        e.observe(11, 2, 123, 42, 36, 0);
        e.observe(10, 1, 123, 42, 36, 0);
        e.observe(11, 2, 123, 42, 36, 0);
        assert!(e.complete());
        e.observe(11, 3, 123, 42, 36, 0);
        assert!(!e.complete());
    }

    #[test]
    fn chord_requires_exact_modifier_mask() {
        let mut e = KeyEvidence { token: 1, pid: 42, key: 9, flags: 0x10_0000, stage: 0, down: 0, up: 0, disabled: false };
        e.observe(10, 1, 1, 42, 9, 0x10_0000);
        e.observe(11, 2, 1, 42, 9, 0x10_0000);
        assert!(e.complete());
        for flags in [0, 0x12_0000] {
            let mut e = KeyEvidence { token: 1, pid: 42, key: 9, flags: 0x10_0000, stage: 0, down: 0, up: 0, disabled: false };
            e.observe(10, 1, 1, 42, 9, flags);
            e.observe(11, 2, 1, 42, 9, 0x10_0000);
            assert!(!e.complete());
        }
    }

    #[test]
    fn rejects_wrong_order_and_correlated_event_fields() {
        for (kind, pid, key, flags) in [(11, 42, 36, 0), (10, 41, 36, 0),
            (10, 42, 35, 0), (10, 42, 36, 1)] {
            let mut e = evidence();
            e.observe(kind, 1, 123, pid, key, flags);
            e.observe(10, 2, 123, 42, 36, 0);
            e.observe(11, 3, 123, 42, 36, 0);
            assert!(!e.complete());
        }
    }
}

pub(crate) fn click_verified(focused_before: Option<bool>, focused_after: Option<bool>,
    selected_before: Option<bool>, selected_after: Option<bool>, routed: bool) -> bool {
    routed || (focused_before == Some(false) && focused_after == Some(true))
        || (selected_before == Some(false) && selected_after == Some(true))
}

#[cfg(test)]
mod click_evidence_tests {
    use super::*;

    fn evidence() -> ClickEvidence {
        ClickEvidence { token: 123, point: Point { x: 20.0, y: 30.0 }, pid: 42,
            stage: 0, down_timestamp: 0, up_timestamp: 0, disabled: false }
    }

    fn event(kind: u32) -> RoutedEvent {
        RoutedEvent { kind, timestamp: u64::from(kind), token: 123,
            point: Point { x: 20.0, y: 30.0 }, pid: 42, flags: 0, click_state: 1, button: 0 }
    }

    #[test]
    fn exact_ordered_pair_verifies_already_focused_target() {
        let mut evidence = evidence();
        assert!(!click_verified(Some(true), Some(true), None, None, evidence.complete()));
        evidence.observe(event(1));
        assert!(!evidence.complete());
        evidence.observe(event(2));
        assert!(click_verified(Some(true), Some(true), None, None, evidence.complete()));
    }

    #[test]
    fn duplicate_routing_observations_require_the_same_event_timestamp() {
        let mut pair = evidence();
        pair.observe(event(1));
        pair.observe(event(1));
        assert!(!pair.complete());
        pair.observe(event(2));
        pair.observe(event(2));
        assert!(pair.complete());

        let mut pair = evidence();
        pair.observe(event(1));
        pair.observe(event(2));
        pair.observe(event(1));
        pair.observe(event(2));
        assert!(pair.complete());

        let mut pair = evidence();
        pair.observe(event(1));
        pair.observe(event(2));
        let mut different = event(1);
        different.timestamp += 1;
        pair.observe(different);
        assert!(!pair.complete());

        let mut pair = evidence();
        pair.observe(event(1));
        let mut different = event(1);
        different.timestamp += 1;
        pair.observe(different);
        pair.observe(event(2));
        assert!(!pair.complete());
    }

    #[test]
    fn unrelated_token_and_missing_or_reversed_pair_do_not_verify() {
        let mut evidence = evidence();
        let mut other = event(1);
        other.token += 1;
        evidence.observe(other);
        evidence.observe(event(2));
        assert!(!evidence.complete());
        evidence.observe(event(1));
        assert!(!evidence.complete());
    }

    #[test]
    fn mismatched_event_fields_cannot_complete_pair() {
        for mismatch in 0..5 {
            let mut evidence = evidence();
            let mut down = event(1);
            match mismatch {
                0 => down.point.x += 1.0,
                1 => down.pid += 1,
                2 => down.flags = 1,
                3 => down.click_state = 2,
                _ => down.button = 1,
            }
            evidence.observe(down);
            evidence.observe(event(1));
            evidence.observe(event(2));
            assert!(!evidence.complete());
        }
    }

    #[test]
    fn disabled_tap_cannot_complete_and_transition_remains_valid() {
        let mut evidence = evidence();
        evidence.observe(event(1));
        evidence.disabled = true;
        evidence.observe(event(2));
        assert!(!evidence.complete());
        assert!(click_verified(Some(false), Some(true), None, None, false));
        assert!(click_verified(None, None, Some(false), Some(true), false));
    }
}


pub(crate) fn unicode_chunks(text: &str) -> Vec<Vec<u16>> {
    let mut chunks: Vec<Vec<u16>> = Vec::new();
    for ch in text.chars() {
        let mut encoded = [0; 2];
        let units = ch.encode_utf16(&mut encoded);
        if chunks.last().map_or(true, |chunk| chunk.len() + units.len() > 20) {
            chunks.push(Vec::with_capacity(20));
        }
        chunks.last_mut().unwrap().extend_from_slice(units);
    }
    chunks
}

#[cfg(test)]
mod unicode_input_tests {
    use super::unicode_chunks;

    #[test]
    fn unicode_chunks_preserve_exact_text_and_surrogates() {
        for text in [
            String::new(),
            "a".repeat(19) + "😀" + " b",
            "a".repeat(18) + "😀" + "🦊" + " 小希-7223-5237\n",
            "😀".repeat(21),
        ] {
            let chunks = unicode_chunks(&text);
            assert!(chunks.iter().all(|chunk| !chunk.is_empty() && chunk.len() <= 20
                && String::from_utf16(chunk).is_ok()));
            let joined: Vec<u16> = chunks.into_iter().flatten().collect();
            assert_eq!(String::from_utf16(&joined).unwrap(), text);
        }
    }
}
