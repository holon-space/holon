//! The **shared** γ-composition catalog: every composed invariant, wired once,
//! plus the slice-agnostic test doubles that exercise them.
//!
//! ## Why this is shared (not per-slice)
//! An invariant's `Needs` is a property of its *body's* cap bounds, not of any
//! slice — `inv-no-parent-cycles` needs `SutBackend` whether it runs over a
//! `MemoryBackend`, a Loro store, or a full E2E SUT. So the bridge of a body
//! into a `CapInvariant` (its [`invariants`]`::*::wire()`) lives **here,
//! once**, and *every* composed slice runs the **same**
//! [`composed_invariant_catalog`] through `run_selected` against *its own*
//! component `CapMap`. Selection then lights up exactly the subset whose
//! `Needs` that slice's caps satisfy.
//!
//! A slice therefore contributes only its **components** (the SUT/Ref
//! realizations — genuinely different code) and a thin builder; it does **not**
//! re-list invariants. Adding a slice ≈ a new `components.rs` + builder + a few
//! selection tests, with zero catalog duplication.
//!
//! ## Layout
//! - [`catalog`] — `composed_invariant_catalog()`: one `wire()` line per
//!   invariant.
//! - [`invariants`] — one file per invariant: its `Needs`+bridge `wire()` and a
//!   `#[cfg(test)]` triad (positive / negative-containment / catch) driven by
//!   the hand-crafted doubles below (no real backend needed). *Adding an
//!   invariant = one new file here + one `catalog.rs` line.*
//! - `fixtures` (test-only) — slice-agnostic SUT/Ref doubles + a `fixtures::*`
//!   prelude. These are hand-crafted `Vec<Block>`/editor states, not tied to
//!   any real component, so the catch tests run without
//!   Turso/Loro/MemoryBackend.

pub mod builder;
pub mod catalog;
pub mod correspondences;
pub mod invariants;

/// The composed-slice host for the `inv-sql-budget` span-metrics cap
/// ([`span_metrics::ComposedSpanMetrics`]). `otel-testing`-gated like the
/// budget body itself (the `pbt` feature always enables `otel-testing`);
/// consumed by the `wide_e2e` slice (registers it) and the `harness` (drives
/// its lifecycle).
#[cfg(all(feature = "otel-testing", any(test, feature = "pbt")))]
pub mod span_metrics;

/// Process-global ERROR-event + panic capture surface for
/// `inv-no-observed-errors`.
pub mod observed_errors;

/// Process-global LoroProjection full-reseed attribution surface for
/// `inv-no-steady-reseed-leak` (Inc 0 of the reseed-latency workstream). The
/// tracing layer that feeds it is wired in [`crate::test_tracing`].
pub mod reseed_observer;

/// The generic composed-SUT `StateMachineTest` harness (E3 increment 3).
/// Available under the `pbt` feature (not just `cfg(test)`) so the **macro
/// repoint** can drive a `ComposedSut`-backed StateMachineTest from a `tests/`
/// INTEGRATION test (which links the lib built without `cfg(test)`). Backs the
/// swap slice in [`wide_e2e`].
#[cfg(any(test, feature = "pbt"))]
pub mod harness;

#[cfg(test)]
pub mod fixtures;

/// Shared subsystem-config seeding + the deterministic shrink-causality proofs
/// (lifted out of `subsystem_shrink.rs` so they survive the spike's replacement
/// by the faithful convergence harness). Available under `pbt` (not just
/// `cfg(test)`) so the composed `harness` it backs can be driven from `tests/`
/// integration tests.
#[cfg(any(test, feature = "pbt"))]
pub mod subsystem_seed;

/// Fixed-id seeding primitives shared by the (test-only) `subsystem_seed` spike
/// and the `pbt`-gated windowed slice (`crate::pbt::window_slice`). Available
/// whenever either consumer is compiled.
#[cfg(any(test, feature = "pbt"))]
pub mod seed_primitives;

/// THE SWAP slice (`WideE2E`) + its helpers — the composed `general_e2e`
/// machinery, relocated here from `frontend_slice/structural_pbt.rs`'s
/// `cfg(test)` mod and `pbt`-gated so the `tests/` integration test
/// `general_e2e_composed_pbt` can drive `ComposedSut<WideE2E>`. Single source
/// of truth (the lib slices `use` these).
#[cfg(any(test, feature = "pbt"))]
pub mod wide_e2e;

/// The out-of-process LIVE-MCP rung of the composed keystone:
/// [`live_mcp::LiveMcpE2E`] drives the SAME `WideE2EMachine` transitions +
/// invariant catalog against a REAL Holon app over its embedded MCP server
/// (per-case `reset_vault`). `pbt`-gated like its `wide_e2e` sibling so the
/// `tests/` integration test can drive `ComposedSut<LiveMcpE2E>`.
#[cfg(any(test, feature = "pbt"))]
pub mod live_mcp;
/// Scale-soak vault seeder (env-gated). Inflates the `WideE2E` SUT boot with
/// extra synthetic org doc files so the keystone can be driven against a
/// 5–10k-block vault, quantifying the projection/CDC/consolidator latency
/// cliff. Zero-cost (empty seed, keystone `SETTLE`) when
/// `HOLON_SOAK_SEED_BLOCKS` is unset.
#[cfg(any(test, feature = "pbt"))]
pub mod soak_seed;

/// The single-sourced E4 windowed composition-path check
/// ([`windowed::run_windowed_composed_check`]). `pbt`-gated like the windowed
/// slice it builds; consumed by both the `E2ESut` invariant runner and the gpui
/// TestPlatform sim's per-tick hook (deduplicating what used to be two copies).
#[cfg(any(test, feature = "pbt"))]
pub mod windowed;

pub use catalog::composed_invariant_catalog;
