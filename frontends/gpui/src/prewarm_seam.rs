//! Test seam: open the next window as if its pre-warm timed out, so it opens
//! on the loading state whatever the load of the machine.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

static TIME_OUT_NEXT: AtomicBool = AtomicBool::new(false);

/// The next window that launches skips its pre-warm wait.
pub fn time_out_next_prewarm() {
    TIME_OUT_NEXT.store(true, Ordering::SeqCst);
}

pub(crate) fn take_prewarm_timeout() -> bool {
    TIME_OUT_NEXT.swap(false, Ordering::SeqCst)
}
