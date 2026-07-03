//! The CDC "lag vs. real bug" decision, owned in one place.
//!
//! Several invariants compare a *downstream* projection (a CDC-fed Turso IVM
//! matview, a `LiveData` mirror, a `ui_model` row set) against the reference
//! oracle. A downstream that disagrees with the reference is ambiguous: it may
//! just be **lagging** the write side (an eventual-consistency delivery race, no
//! bug), or the divergence may be **real** (the write pipeline produced the wrong
//! value). The two are told apart the same way everywhere — by consulting a more
//! *authoritative upstream* stage (the write-side `block_raw` table, the
//! `focus_roots` matview behind its mirror):
//!
//! - downstream **==** reference           → converged, nothing to see.
//! - downstream ≠ reference, upstream **==** reference → the downstream merely
//!   lagged the upstream → a delivery race, safe to `Skip`.
//! - downstream ≠ reference, upstream **≠** reference → the upstream (source of
//!   truth) is *also* wrong → a real pipeline bug.
//!
//! This module owns *only that decision*. The reads (what "downstream" and
//! "upstream" are, how they're filtered) and the `Fail`/`Skip` messages stay at
//! each call site — exactly like [`super::retry`] owns the timing of an
//! eventual read but not the why. Sibling of the other lag idiom,
//! [`super::retry::retry_until_ok`] (time-boxed re-read until convergence): use
//! *that* when there is no cheaper authoritative stage to consult and you can
//! afford to wait; use *this* when a settled upstream stage already exists and a
//! single consult classifies the divergence without sleeping.
//!
//! The upstream is read **lazily** — the closure runs only when the downstream
//! already disagrees, so the common (converged) path costs no extra query.

use std::future::Future;

/// The classification of a downstream-vs-reference comparison. `T` is the
/// compared value (an id set, a scalar field, a per-region root set).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Staleness<T> {
    /// The downstream projection already equals the reference.
    Converged,
    /// The downstream diverges but the authoritative upstream still equals the
    /// reference — a CDC delivery race. The caller should `Skip` the affected
    /// unit rather than assert against a value that has not settled.
    Lag,
    /// Neither the downstream nor the upstream equals the reference — a real
    /// pipeline bug. Carries the read upstream value so the caller can build a
    /// precise `Fail` message (or attribute blame further down its own chain).
    Divergent { upstream: T },
}

/// Classify a `downstream`-vs-`reference` divergence as CDC lag or a real bug by
/// consulting a more-authoritative `read_upstream` stage. See the module docs
/// for the decision table. `read_upstream` is invoked at most once, only when
/// the downstream already disagrees.
pub async fn classify_staleness<T, Fut>(
    downstream: &T,
    reference: &T,
    read_upstream: impl FnOnce() -> Fut,
) -> Staleness<T>
where
    T: PartialEq,
    Fut: Future<Output = T>,
{
    if downstream == reference {
        return Staleness::Converged;
    }
    let upstream = read_upstream().await;
    if &upstream == reference {
        Staleness::Lag
    } else {
        Staleness::Divergent { upstream }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn classify(down: i32, refr: i32, up: i32) -> Staleness<i32> {
        classify_staleness(&down, &refr, || async move { up }).await
    }

    #[tokio::test]
    async fn converged_when_downstream_matches_reference() {
        // Upstream is never consulted on the converged path.
        let called = std::cell::Cell::new(false);
        let v = classify_staleness(&5, &5, || {
            called.set(true);
            async { 999 }
        })
        .await;
        assert_eq!(v, Staleness::Converged);
        assert!(!called.get(), "upstream must not be read when converged");
    }

    #[tokio::test]
    async fn lag_when_only_downstream_diverges() {
        // downstream 4 ≠ ref 5, but upstream 5 == ref 5 → the mirror lagged.
        assert_eq!(classify(4, 5, 5).await, Staleness::Lag);
    }

    #[tokio::test]
    async fn divergent_when_upstream_also_wrong() {
        // Both downstream 4 and upstream 3 disagree with ref 5 → real bug,
        // and the upstream value rides along for the caller's message.
        assert_eq!(
            classify(4, 5, 3).await,
            Staleness::Divergent { upstream: 3 }
        );
    }
}
