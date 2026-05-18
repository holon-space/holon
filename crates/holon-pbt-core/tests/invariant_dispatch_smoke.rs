//! Phase 7 smoke test — proves the `Invariant<R, S>` trait + capability
//! bounds compile and dispatch correctly against a toy SUT.
//!
//! Validates the structural claim: an invariant's `where S: SutLoroLog`
//! bound is honoured at the call site — a SUT without `SutLoroLog`
//! simply can't dispatch the invariant (compile-time slice opt-in).
//!
//! Runtime body verification happens in the wide PBT and in the
//! future `storage_consistency_pbt` slice (Phase 8). This file is
//! compile-only.

use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

/// Mirror of the production `InvLoroNoErrors` body in
/// `crates/holon-integration-tests/src/pbt/invariants/bodies/loro_no_errors.rs`.
struct InvLoroNoErrors;

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLoroNoErrors
where
    S: SutLoroLog,
{
    fn id(&self) -> InvariantId {
        InvariantId("inv-loro-no-errors")
    }
    fn mode(&self) -> RunMode {
        RunMode::Strict
    }
    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        if sut.loro_had_errors().await {
            InvariantResult::Fail("loro had errors".into())
        } else {
            InvariantResult::Ok
        }
    }
}

struct ToyRef;
struct ToySut;

#[allow(async_fn_in_trait)]
impl SutLoroLog for ToySut {
    async fn loro_had_errors(&self) -> bool {
        false
    }
    async fn loro_children_of(&self, _: &str) -> Option<Vec<String>> {
        None
    }
}

/// Type-level smoke: this compiling means the where-clause filter works.
/// The fn never runs; it just forces the compiler to verify that
/// `InvLoroNoErrors` implements `Invariant<ToyRef, ToySut>`.
#[allow(dead_code)]
fn assert_invariant_resolves() {
    fn takes_invariant<I: Invariant<ToyRef, ToySut>>(_: &I) {}
    takes_invariant(&InvLoroNoErrors);
}

#[test]
fn invariant_metadata_is_addressable() {
    // Disambiguate via UFCS — same R, S as the assert_resolves fn above.
    let id = <InvLoroNoErrors as Invariant<ToyRef, ToySut>>::id(&InvLoroNoErrors);
    let mode = <InvLoroNoErrors as Invariant<ToyRef, ToySut>>::mode(&InvLoroNoErrors);
    assert_eq!(id, InvariantId("inv-loro-no-errors"));
    assert_eq!(mode, RunMode::Strict);
}
