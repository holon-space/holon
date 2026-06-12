//! Faithful generic subsystem-selectable PBT — **the convergence harness**.
//!
//! Unlike the blessed slices (`general_e2e_pbt`, …), which each pin one fixed
//! [`Wiring`](holon_pbt_core::Wiring), this harness **generates** which
//! subsystems are active as the `init_state` of the state machine, drives the
//! real booted [`E2ESut`](holon_integration_tests::pbt::E2ESut) through the full
//! production transition alphabet (`aggregate_transitions`), runs the
//! wiring-gated invariant registry (`run_invariant_registry`), and on failure
//! **shrinks the active subsystem set toward the minimal combination that still
//! reproduces** — an architectural delta-debugger over a *faithful* app.
//!
//! It boots via the production `StartApp` path (pre-startup `CreateDirectory` /
//! `WriteOrgFile` satisfy its precondition), so unlike the retired in-process
//! `subsystem_shrink` spike it exercises startup and can subsume the wide PBT.
//!
//! ## Runtime cost & bounding
//!
//! Each case boots a real `E2ESut` (a Turso draw boots the full `BackendEngine`)
//! and each shrink iteration reboots + replays — heavy. Rather than hide the test
//! behind `#[ignore]` (which rots undetected) or an env-gate (vacuously green when
//! unset), [`declare_pbt_convergence!`] runs it under a plain `cargo test`, kept
//! fast enough by hard bounds: `cases: 4` and `max_shrink_iters: 8` (see
//! [`convergence_config`]), with Turso down-weighted so most cases hit the cheap
//! LoroMemory backend. `PROPTEST_CASES` / `PROPTEST_MAX_SHRINK_ITERS` widen a run.
//!
//! ## Generation scope
//!
//! The active-subsystem universe is [`wiring_axes`](holon_pbt_core::wiring_axes):
//! default storage `{Loro, Org, Turso}`, no sync, actors `{MCPServer,
//! ActionEngine}` — **no `UI`** (headless editor faithfulness is unverified, so
//! editor transitions must not generate here yet). Turso (the only expensive
//! backend) is down-weighted so most cases run on the cheap LoroMemory backend.
//! Scope a run with `HOLON_PBT_WIRING_AXES="storage;sync;actors"`, e.g.
//! `"Loro,Turso;;"` for a two-storage-axis sweep with no sync/actors.
//!
//! ## Smoke
//!
//! ```sh
//! HOLON_PBT_WIRING_AXES="Loro;;" PROPTEST_CASES=1 \
//!   cargo test -p holon-integration-tests --features pbt \
//!   --test subsystem_convergence_pbt
//! ```
//! — one LoroMemory case boots + runs green.

use holon_integration_tests::declare_pbt_convergence;
use holon_integration_tests::pbt::standard_pbt_config;

/// The convergence harness config. Reuses [`standard_pbt_config`] (rejection
/// histogram hook + dedicated `*.proptest-regressions` persistence) but:
///
/// - **`cases: 4`** — each case is a full booted run; the sweep stays bounded.
///   `PROPTEST_CASES` overrides (the smoke recipe sets `1`).
/// - **`max_shrink_iters: 8`** — a hard low cap. The faithful harness produces a
///   *reproducing* fixture, not an exhaustively-minimized one (each shrink iter
///   reboots `E2ESut`); the cheap deterministic planted-model proofs own
///   architectural delta-debug. `PROPTEST_MAX_SHRINK_ITERS` overrides.
fn convergence_config() -> proptest::test_runner::Config {
    let mut cfg = standard_pbt_config("subsystem_convergence_pbt");
    cfg.cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    cfg.max_shrink_iters = std::env::var("PROPTEST_MAX_SHRINK_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    cfg
}

declare_pbt_convergence! {
    test_fn: subsystem_convergence_pbt,
    proptest_config: convergence_config(),
    steps: 3..12,
}
