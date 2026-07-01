//! `declare_pbt_slice!` — declarative shortcut for new PBT slices.
//!
//! See [`docs/Testing/PbtSlicing.md`](../../../../docs/Testing/PbtSlicing.md).
//! Replaces the speculative `PbtSlice` trait from §5 (typed-tuple
//! `TransitionSet`/`InvariantSet`) with a macro that accepts arbitrary
//! expressions per item — required to express slice-specific quirks like
//! `prop_filter` on `WriteOrgFile` and `PhantomData<R>`-carrying invariants.
//!
//! ## Capture-on-panic + fixture-dir replay
//!
//! Every slice declared via `declare_pbt_slice!` automatically captures
//! the transition sequence it just applied; if the proptest assertion
//! panic unwinds through the slice's SUT wrapper, the captured sequence
//! is written to `tests/.captures/<test_fn>.captured.json`. Hand-edit /
//! rename it into `tests/fixtures/<test_fn>/<name>.json` (or wherever
//! the slice's `fixtures_dir` points) to lock it as a literal-value
//! regression.
//!
//! When `fixtures_dir: "..."` is supplied to the macro, an additional
//! test function (`fixtures_test_fn: <ident>`) scans that directory and
//! replays every `*.json` fixture before the proptest sweep runs.

// ── Capture-on-panic and fixture-dir replay helpers ───────────────

use crate::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use std::cell::RefCell;
use std::path::PathBuf;

thread_local! {
    /// Per-test-thread buffer of transitions applied by the active slice.
    /// Reset in `init_test`; appended to in `apply`; serialized to a
    /// `.captured.json` file by the SUT wrapper's `Drop` when the thread
    /// is panicking.
    static CAPTURE: RefCell<Option<CaptureState>> = RefCell::default();
}

struct CaptureState {
    transitions: Vec<E2ETransition>,
    wiring: Option<holon_pbt_core::Wiring>,
    already_written: bool,
}

pub fn reset_capture(_: &'static str) {
    CAPTURE.with(|c| {
        *c.borrow_mut() = Some(CaptureState {
            transitions: Vec::new(),
            wiring: None,
            already_written: false,
        });
    });
}

/// Record the wiring the capture is being generated under (called once the
/// run's `Wiring` is known — it may not be at `reset_capture` time).
pub fn record_capture_wiring(wiring: &holon_pbt_core::Wiring) {
    CAPTURE.with(|c| {
        if let Some(state) = c.borrow_mut().as_mut() {
            state.wiring = Some(wiring.clone());
        }
    });
}

pub fn record_transition(t: &E2ETransition) {
    CAPTURE.with(|c| {
        if let Some(state) = c.borrow_mut().as_mut() {
            state.transitions.push(t.clone());
        }
    });
}

/// Number of transitions applied (recorded) so far in the active case. The
/// convergence harness uses this in `teardown` to tell a non-trivial case from
/// a shrunk near-empty one when checking that `StartApp` actually fired.
pub fn captured_transition_count() -> usize {
    CAPTURE.with(|c| {
        c.borrow()
            .as_ref()
            .map(|state| state.transitions.len())
            .unwrap_or(0)
    })
}

/// Write the captured transition sequence to
/// `$CARGO_MANIFEST_DIR/tests/.captures/<slice>.captured.json`. Idempotent
/// per panic (proptest may unwind through multiple Drop sites during
/// shrink — first writer wins).
pub fn write_captured_fixture(slice_name: &str) {
    CAPTURE.with(|c| {
        let mut borrow = c.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        if state.already_written || state.transitions.is_empty() {
            return;
        }
        state.already_written = true;
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join(".captures");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("[slice capture] failed to mkdir {dir:?}: {e}");
            return;
        }
        let path = dir.join(format!("{slice_name}.captured.json"));
        let fixture: Fixture<E2ETransition, ()> = Fixture {
            name: format!("{slice_name}_captured"),
            description: "Auto-captured from a panicking proptest case. \
                          Rename + move into the slice's fixtures_dir to lock it."
                .to_string(),
            environment: holon_pbt_core::fixture::CaptureEnvironment {
                wiring: state.wiring.clone(),
                env_flags: holon_pbt_core::fixture::CaptureEnvironment::current_env_flags(),
            },
            initial_state: (),
            transitions: state.transitions.clone(),
        };
        if let Err(e) = fixture.save(&path) {
            eprintln!("[slice capture] failed to write {path:?}: {e}");
        } else {
            eprintln!(
                "[slice capture] wrote {} transitions to {}",
                fixture.transitions.len(),
                path.display()
            );
        }
    });
}
