//! Correspondence registry — one logical observable, N store projections.
//!
//! The composed PBT compares one reference model against N SUT stores. The
//! `capability_pair!` macro covers the clean 1:1 two-value edges; this registry
//! covers what the macro structurally cannot (see the macro module doc's scope
//! note): hub reference caps answering several questions, multi-cap extraction,
//! 3-valued (disclosed-Skip) reads, and per-store convergence policy. A
//! correspondence is **declarative test-wiring data**: adding a store to an
//! observable — or a whole new observable — is one table entry in
//! `composed::correspondences`, and [`Correspondence::wire`] emits one
//! [`CapInvariant`] per store for the shared catalog. Registry output coexists
//! with hand-written `wire()`s and macro-derived pair invariants — no flag day.
//!
//! Integrity rule: extraction and comparison strategies are **named `fn`s
//! referenced from the table**, never inline closures buried in SUT impls —
//! the wiring stays greppable, and the SUT cannot influence how it is judged.
//!
//! # Convergence policy — settle-first (user decision 2026-07-03)
//!
//! The harness quiesces projections before the invariant-check pass, so the
//! DEFAULT for every store is [`Converge::None`] (diverged = Fail).
//! [`Converge::Retry`] is a *disclosed exception* for a store the settle
//! provably cannot cover; each use states why in the table. A lag tolerance
//! firing is first a settle GAP to fix, not something to tolerate.
//!
//! The quiescence is the DETERMINISTIC 3-projection convergence settle
//! (`WideE2E::settle_after_apply` → `wide_e2e::converge_projections` →
//! `convergence::converge_signals`, covering CDC + Loro + org): it waits until
//! the projections actually converge, capped at `wide_e2e::SETTLE` (150 ms) so
//! it never over-waits vs the flat post-apply sleep it replaced. Because it
//! waits on real convergence rather than a fixed delay, `Converge::None` is
//! soundly backed — a store that still diverges after the settle is a real bug,
//! not a race.
//!
//! # Growth path (do not add speculatively)
//!
//! Later phases add, WITH their first consumer: `NamedPredicate` +
//! `Compare::Predicate` (SUT-structural checks, e.g. `no_orphan`),
//! `StorePairCorrespondence` (SUT-vs-SUT, e.g. task-state SQL-vs-Loro), and
//! `Converge::UpstreamOf` (consult an authoritative sibling store; its decision
//! kernel already exists as [`crate::pbt::staleness::classify_staleness`]).
//!
//! # Out of scope — the registry must refuse to grow toward
//!
//! Value correspondences over the storage pipeline + editor mirror ONLY. NOT:
//! the viewmodel / renderer / geometry cluster (structural predicates over
//! widget trees against ref *configuration*), self-checks (`no_parent_cycles`,
//! `no_errors`), budget/metrics, fixed-point checks, per-field adaptive lag
//! (`watch_rows`' field classifier), SUT-driven iteration spaces, arg-bearing
//! projections, or `&mut` drains.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::{CapInvariant, CapMap, Needs};
use holon_pbt_core::invariant::{InvariantId, InvariantResult};

use holon_pbt_core::retry::retry_until_ok;

/// A logical observable: one named reference projection with an owned value
/// type. `NAME` is the invariant-id family stem: each store emits
/// `inv-<NAME>/<store>` (enforced in [`Correspondence::wire`]).
pub trait Observable: 'static {
    type Value: fmt::Debug + 'static;
    const NAME: &'static str;
}

/// 3-valued extraction, used by BOTH sides: a side that cannot observe the
/// value right now says so with a disclosed reason (→ `Skipped`, never a faked
/// empty value).
pub enum Extraction<T> {
    Value(T),
    Unobservable(String),
}

/// A named pure comparator — a `fn` path recorded in the table. `Ok(())` =
/// agree; `Err(msg)` = fail-loud divergence message used verbatim as the
/// `Fail` payload (same contract as `capability_pair!`'s `#[compare(with)]`).
pub struct NamedCompare<T: 'static> {
    pub name: &'static str,
    pub f: fn(&T, &T) -> Result<(), String>,
}

/// Convergence / staleness policy for one store projection. Settle-first:
/// default [`Converge::None`]; see the module docs.
pub enum Converge {
    /// The settle hook guarantees convergence at check time: diverged = Fail.
    None,
    /// Disclosed exception: time-boxed re-extract + re-compare until agreement
    /// or `deadline` (then Fail with the last divergence). The table entry
    /// using this must state why the settle hook cannot cover the store.
    Retry {
        deadline: Duration,
        interval: Duration,
    },
}

/// Async extraction of the observable from one SUT store. Takes BOTH maps:
/// caps are read off the SUT map exactly like the `#[capmap_adapter]` forwards
/// do; the ref map supplies *comparability context only* (e.g. the seed-id
/// filter) — the expected value itself only ever comes from
/// [`Correspondence::ref_project`].
pub type ExtractFn<T> =
    for<'a> fn(&'a CapMap, &'a CapMap) -> Pin<Box<dyn Future<Output = Extraction<T>> + 'a>>;

/// One SUT store's view of the observable. Declared in the correspondence
/// table, NOT returned by a SUT method.
pub struct StoreProjection<O: Observable> {
    /// Full invariant id, `inv-<O::NAME>/<store>` — written out (not derived)
    /// so it is greppable from a failure report straight to the table entry.
    pub id: &'static str,
    /// Store name — the id suffix ("block_raw", "matview", "loro", …).
    pub store: &'static str,
    /// Cap selection — same `Needs` data hand-written wires declare.
    pub needs: Needs,
    pub extract: ExtractFn<O::Value>,
    pub compare: NamedCompare<O::Value>,
    pub converge: Converge,
}

/// One logical observable: exactly one reference projection + N SUT store
/// projections.
pub struct Correspondence<O: Observable> {
    /// The reference projection — a named `fn` reading the ref `CapMap`.
    pub ref_project: fn(&CapMap) -> Extraction<O::Value>,
    pub stores: Vec<StoreProjection<O>>,
}

impl<O: Observable> Correspondence<O> {
    /// Emit one `CapInvariant` per store for the shared catalog. Panics (at
    /// catalog-build time, fail-loud) if a store's written-out `id` violates
    /// the `inv-<NAME>/<store>` convention.
    pub fn wire(self) -> Vec<Box<dyn CapInvariant>> {
        let ref_project = self.ref_project;
        self.stores
            .into_iter()
            .map(|store| {
                let expected = format!("inv-{}/{}", O::NAME, store.store);
                assert_eq!(
                    store.id, expected,
                    "correspondence id convention violated for store '{}'",
                    store.store
                );
                Box::new(StoreInvariant::<O> { ref_project, store }) as Box<dyn CapInvariant>
            })
            .collect()
    }
}

/// The one generic invariant body behind every registry entry: select on
/// `needs`; project the reference; extract the store (honouring `converge`);
/// `Unobservable` on either side → `Skipped` (disclosed); compare → Ok/Fail.
struct StoreInvariant<O: Observable> {
    ref_project: fn(&CapMap) -> Extraction<O::Value>,
    store: StoreProjection<O>,
}

impl<O: Observable> CapInvariant for StoreInvariant<O> {
    fn id(&self) -> InvariantId {
        InvariantId(self.store.id)
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    fn needs(&self) -> Needs {
        self.store.needs.clone()
    }

    fn check_boxed<'a>(
        &'a self,
        sut: &'a CapMap,
        ref_: &'a CapMap,
    ) -> Pin<Box<dyn Future<Output = InvariantResult> + 'a>> {
        Box::pin(async move {
            let id = self.store.id;
            let ref_val = match (self.ref_project)(ref_) {
                Extraction::Value(v) => v,
                Extraction::Unobservable(reason) => {
                    return InvariantResult::Skipped(format!(
                        "[{id}] reference unobservable: {reason}"
                    ));
                }
            };

            // One attempt = extract + compare. `Ok(result)` is final
            // (success or a disclosed Skip — unobservability is not lag);
            // `Err(msg)` is a divergence, retryable under `Converge::Retry`.
            let attempt = async || -> Result<InvariantResult, String> {
                match (self.store.extract)(sut, ref_).await {
                    Extraction::Unobservable(reason) => Ok(InvariantResult::Skipped(format!(
                        "[{id}] store '{}' unobservable: {reason}",
                        self.store.store
                    ))),
                    Extraction::Value(sut_val) => {
                        match (self.store.compare.f)(&sut_val, &ref_val) {
                            Ok(()) => Ok(InvariantResult::Ok),
                            Err(msg) => Err(msg),
                        }
                    }
                }
            };

            match self.store.converge {
                Converge::None => match attempt().await {
                    Ok(result) => result,
                    Err(msg) => InvariantResult::Fail(msg),
                },
                Converge::Retry { deadline, interval } => {
                    match retry_until_ok(deadline, interval, attempt).await {
                        Ok(result) => result,
                        Err(msg) => InvariantResult::Fail(format!(
                            "[{id}] still diverged after the {deadline:?} retry window \
                             (disclosed Converge::Retry exception): {msg}"
                        )),
                    }
                }
            }
        })
    }
}
