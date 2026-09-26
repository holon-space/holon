//! Named points where a test makes the file sync die, as a crash or a power
//! loss would, or fail, to prove what survives and what is disclosed.
//! Feature-gated (`crash-injection`, off by default): a release build has no
//! such point.

use std::sync::Mutex;

static ARMED: Mutex<Option<&'static str>> = Mutex::new(None);

/// The next time the file sync reaches `point`, it dies or fails there. One
/// shot.
pub fn arm(point: &'static str) {
    *ARMED.lock().expect("crash-injection lock") = Some(point);
}

/// The point armed and not reached yet.
pub fn armed() -> Option<&'static str> {
    *ARMED.lock().expect("crash-injection lock")
}

/// Whether `point` is armed; reaching it disarms it.
pub fn fires(point: &'static str) -> bool {
    let mut armed = ARMED.lock().expect("crash-injection lock");
    if *armed == Some(point) {
        *armed = None;
        return true;
    }
    false
}

pub(crate) fn reached(point: &'static str) {
    if fires(point) {
        panic!("[crash-injection] the file-sync controller dies at {point}");
    }
}
