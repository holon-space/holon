//! Phase 7 smoke test — proves the `Invariant<R, S>` trait + capability
//! bounds compile and dispatch correctly against a toy SUT.
//!
//! Validates the structural claim: an invariant's `where S: SutLoroLog`
//! bound is honoured at the call site — a SUT without `SutLoroLog`
//! simply can't dispatch the invariant (compile-time slice opt-in).
//!
//! Runtime body verification happens in the wide PBT and in the composed
//! keystone (`general_e2e_composed_pbt`). This file is compile-only.

use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// Mirror of the production `InvLoroNoErrors` body in
/// `crates/holon-integration-tests/src/pbt/invariants/bodies/loro_no_errors.
/// rs`.
struct InvLoroNoErrors;

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLoroNoErrors
where
    S: SutLoroLog,
{
    fn id(&self) -> InvariantId {
        InvariantId("inv-loro-no-errors")
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

#[async_trait::async_trait(?Send)]
impl SutLoroLog for ToySut {
    async fn loro_had_errors(&self) -> bool {
        false
    }
    async fn loro_children_of(&self, _: &str) -> Option<Vec<String>> {
        None
    }
    async fn loro_block_snapshot(&self) -> Option<Vec<holon_api::block::Block>> {
        None
    }
    async fn loro_lamport_height(&self) -> Option<u32> {
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
    assert_eq!(id, InvariantId("inv-loro-no-errors"));
}
