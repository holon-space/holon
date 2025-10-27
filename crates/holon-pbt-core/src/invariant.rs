//! Invariant trait surface for Phase 7.
//!
//! Each wide-PBT invariant migrates to a unit-struct (or small struct)
//! that implements [`Invariant<R, S>`]. The struct carries its capability
//! bounds at the *impl* site, so `PbtSuiteSpec` (Phase 10) can
//! statically filter which invariants a slice provides.

/// Stable identifier for an invariant. Used by the anti-rubber-stamp
/// registry assertions (plan H11) and by slice opt-in self-tests
/// (Phase 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvariantId(pub &'static str);

/// How strictly an invariant violation is enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Failure panics immediately. Use for invariants whose violation is
    /// unambiguously a bug.
    Strict,
    /// Failure is logged but does not abort the test run. Use for
    /// invariants that have known transient false-positive windows.
    Warn,
    /// When the truth-check classifier detects CDC hasn't caught up yet,
    /// the check is suppressed rather than producing a spurious failure.
    /// See plan H6 (CDC-lag classifier).
    SkipOnCdcLag,
}

/// The outcome of a single invariant evaluation.
#[derive(Debug, Clone)]
pub enum InvariantResult {
    Ok,
    /// The invariant was violated. In `Strict` mode the runner panics;
    /// in `Warn` mode it logs and continues.
    Fail(String),
    /// The check was suppressed. Carries the reason for telemetry.
    Skipped(String),
}

/// An invariant parameterised over reference-state `R` and SUT `S`.
///
/// Capability bounds (`R: RefBlockTree`, `S: SutSqlProjection`, etc.) are
/// placed on the *impl* block, not on the trait or the struct, so a single
/// `InvariantId` registry can hold `Box<dyn Invariant<R, S>>` values for
/// heterogeneous capability requirements without fighting Rust's object
/// safety rules.
///
/// # Example skeleton
///
/// ```ignore
/// struct InvBlockIdsMatchRef;
///
/// impl<R: RefBlockTree, S: SutSqlProjection> Invariant<R, S> for InvBlockIdsMatchRef {
///     fn id(&self) -> InvariantId { InvariantId("inv-block-ids-match-ref") }
///     async fn check(&self, ref_: &R, sut: &S) -> InvariantResult { … }
/// }
/// ```
///
/// Run mode (Strict/Warn) lives on the registry's `InvariantSpec` — the
/// single source of truth — not on the body.
#[allow(async_fn_in_trait)]
pub trait Invariant<R, S> {
    fn id(&self) -> InvariantId;
    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult;
}
