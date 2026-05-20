//! GPUI UI PBT (Full wiring — Loro enabled) — random geometry-based PBT
//! against a REAL GPUI window with xcap screenshots, driven by a real
//! `proptest-state-machine` runner so a failing sequence **shrinks**
//! in-process. The window is opened once and re-pointed at a fresh SUT per
//! case/shrink-candidate via the shared [`pbt_harness::windowed_replay`]
//! service (the same plumbing the ddmin `gpui_windowed_minimize` uses).
//!
//! `harness = false` (GPUI needs the main thread). Run with:
//!   cargo test -p holon-gpui --test gpui_ui_pbt --features pbt
//! Tune the per-case sequence length with `PBT_NUM_STEPS` (default 50).
//!
//! The no-Loro / SqlOnly variant is its own test target (so it runs
//! automatically): `gpui_ui_pbt_no_loro`. The shared body lives in
//! [`pbt_harness::random_pbt`].
//!
//! ## Generation + shrinking
//!
//! Each case is `WindowedRefMachine::sequential_strategy(num_steps)` — the exact
//! same `ReferenceMachine` alphabet/preconditions/apply the headless slices use,
//! with one override: `init_state` honors the slice's wiring (the headless
//! `ReferenceMachine::init_state` hardcodes `Wiring::full()`). On a failing
//! case proptest shrinks the `Vec<E2ETransition>` (precondition-aware deletion
//! + value-level payload shrinking) and replays each candidate through the
//! reused window. Defaults: `cases = 1` (windowed replays are expensive;
//! shrinking only triggers on failure anyway), overridable via `PROPTEST_CASES`;
//! shrink budget via `PROPTEST_MAX_SHRINK_ITERS` (default 50).
//!
//! ## Signature pinning
//!
//! The FIRST failure of a case always fails the run. During shrinking, a
//! candidate only counts as a reproduction if its panic message contains the
//! pinned signature — the `format_layer_report` marker `"trouble begins at:"`
//! by default, overridable via `HOLON_MINIMIZE_SIGNATURE` (e.g.
//! `inv-blocks-match-ref/loro`). A non-matching panic returns `Ok(())` so
//! proptest backs off that shrink branch.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

fn main() {
    pbt_harness::random_pbt::run(holon_pbt_core::Wiring::full(), "gpui_ui_pbt");
}
