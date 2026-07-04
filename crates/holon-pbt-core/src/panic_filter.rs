//! The panic hook a class-1 invariant sweep runs under.
//!
//! A sweep uses panics as DATA: [`NullRef`](crate::null_ref::NullRef) mints
//! `"class-2: …"` and a live snapshot backend mints `"no live source: …"`, and
//! the caller's `catch_unwind` classifies them. Printing those would bury the
//! report in dozens of backtraces.
//!
//! It must not cost anything else its voice. The installed hook FILTERS rather
//! than silences: only the two sweep payloads are dropped, and only while a
//! sweep is actually running — every other panic, on any thread, is forwarded
//! to the hook that was installed before.
//!
//! The hook is installed ONCE and never uninstalled; [`SweepPanicHook`] only
//! raises and lowers a flag. That is what makes the guard correct on an
//! unwinding exit: `std::panic::set_hook` itself panics when called from a
//! panicking thread, so a guard that tried to restore the previous hook in
//! `Drop` would abort the process the moment a sweep unwound through it.
//! Outside a sweep the hook is a pure forwarder, so leaving it installed
//! changes nothing an observer can see.

use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::Once;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Payload prefix a reference-model read mints. See
/// [`NullRef`](crate::null_ref::NullRef).
pub const CLASS_TWO_PREFIX: &str = "class-2:";

/// Payload prefix a capability with no live source mints.
pub const NO_LIVE_SOURCE_PREFIX: &str = "no live source:";

static SWEEPING: AtomicBool = AtomicBool::new(false);

fn sweep_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// True when `payload` is one a sweep mints and classifies itself.
pub fn is_sweep_payload(payload: &(dyn std::any::Any + Send)) -> bool {
    let text = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied());
    text.is_some_and(|t| t.starts_with(CLASS_TWO_PREFIX) || t.starts_with(NO_LIVE_SOURCE_PREFIX))
}

/// Install the forwarding hook, once per process. Called from
/// [`SweepPanicHook::install`], never while unwinding.
///
/// ACCEPTED RESIDUAL — once per process: any later `std::panic::set_hook`
/// replaces this forwarder permanently. The failure direction is NOISE
/// (class-2 backtraces printed during a sweep), never SILENCE, which is why it
/// is accepted. Re-install hardening is tracked in task #87 — it grows the hook
/// chain unboundedly and needs its own design.
fn ensure_hook_installed() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if SWEEPING.load(Ordering::Acquire) && is_sweep_payload(info.payload()) {
                return;
            }
            previous(info);
        }));
    });
}

/// Marks a sweep in progress. Holds the process-wide sweep lock, so two sweeps
/// cannot overlap, and lowers the flag on every exit — including an unwind,
/// because lowering it cannot fail.
pub struct SweepPanicHook {
    _lock: MutexGuard<'static, ()>,
}

impl SweepPanicHook {
    pub fn install() -> Self {
        // A poisoned lock means a previous sweep unwound; its guard still
        // lowered the flag on the way out, so the lock is safe to reclaim.
        let lock = sweep_lock().lock().unwrap_or_else(|e| e.into_inner());
        ensure_hook_installed();
        SWEEPING.store(true, Ordering::Release);
        Self { _lock: lock }
    }
}

impl Drop for SweepPanicHook {
    fn drop(&mut self) {
        SWEEPING.store(false, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn sweep_payloads_are_recognised_and_others_are_not() {
        assert!(is_sweep_payload(&String::from(
            "class-2: invariant read RefBlockTree::block_content"
        )));
        assert!(is_sweep_payload(&"no live source: SutBackend::x"));
        assert!(!is_sweep_payload(&String::from("index out of bounds")));
        assert!(!is_sweep_payload(&7u32));
    }

    /// Every hook assertion lives in ONE test: installing a recording hook is a
    /// process-global act, so two of them running concurrently would trade
    /// hooks mid-flight. Foreign payloads (this crate's `should_panic` tests
    /// run in parallel and reach any installed hook) are filtered out by
    /// marker rather than serialized against.
    #[test]
    fn the_guard_filters_sweep_payloads_forwards_everything_else_and_always_restores() {
        const MARKER: &str = "panic-filter-probe:";
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);

        // Installed BEFORE the forwarder, so `ensure_hook_installed` captures
        // this recorder as its `previous` and the chain becomes
        // forwarder → recorder → outer. It must not be able to panic: a panic
        // inside a panic hook is non-unwinding and aborts the process. It also
        // FORWARDS everything it does not record — this hook outlives the test
        // (see the teardown note below), so swallowing would mute the rest of
        // the binary.
        let outer = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let text = info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| info.payload().downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            if text.starts_with(MARKER) {
                recorder
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(text);
            } else {
                outer(info);
            }
        }));

        // (1) sweep payloads suppressed, same-thread foreign panic forwarded.
        {
            let _sweep = SweepPanicHook::install();
            let _ = std::panic::catch_unwind(|| panic!("class-2: invariant read RefFocus::x"));
            let _ = std::panic::catch_unwind(|| panic!("no live source: SutBackend::y"));
            let _ = std::panic::catch_unwind(|| panic!("{MARKER} same-thread"));

            // (2) the reported defect's shape: a SIBLING thread's panic raised
            // mid-sweep must still reach the hook.
            std::thread::spawn(|| {
                let _ = std::panic::catch_unwind(|| panic!("{MARKER} sibling-thread"));
            })
            .join()
            .expect("probe thread");
        }

        // (3) the guard restored on a clean exit — this reaches the recorder.
        let _ = std::panic::catch_unwind(|| panic!("{MARKER} after-clean-exit"));

        // (4) and it restores while UNWINDING through the guard, too.
        let _ = std::panic::catch_unwind(|| {
            let _sweep = SweepPanicHook::install();
            panic!("{MARKER} through-the-guard");
        });
        let _ = std::panic::catch_unwind(|| panic!("{MARKER} after-unwinding-exit"));

        // TEARDOWN: deliberately does NOT restore a hook. `set_hook` here would
        // replace the forwarder for the rest of the binary, and
        // `ensure_hook_installed`'s `Once` never reinstalls it — a later test's
        // sweep would then print the payloads this module exists to suppress.
        // The chain left behind forwards everything non-marker, so nothing is
        // muted.
        let got = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();

        let expected: Vec<String> = [
            // forwarded from INSIDE an active sweep — the property a blanket
            // silencing hook lost
            "same-thread",
            "sibling-thread",
            "after-clean-exit",
            // raised inside the sweep and unwound through the guard
            "through-the-guard",
            "after-unwinding-exit",
        ]
        .iter()
        .map(|s| format!("{MARKER} {s}"))
        .collect();
        assert_eq!(
            got, expected,
            "sweep payloads must be suppressed, everything else forwarded, and \
             the previous hook restored on both a clean and an unwinding exit",
        );
    }
}
