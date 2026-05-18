//! Phase 7 — `inv-viewmodel-snapshot` (DEFERRED).
//!
//! Inline body lives at `sut.rs:4892–4991` (the large ReactiveEngine snapshot
//! setup block — root stream drain, loading/spacer gate, interpret_pure call).
//!
//! # Why deferred
//!
//! The entire ReactiveEngine snapshot lifecycle (watch drain, first-emission
//! wait, loading/spacer guard, `interpret_pure` call) lives in
//! `check_invariants_async` and uses private fields:
//! - `self.reactive_engine` (private `Rc<RefCell<Option<Arc<ReactiveEngine>>>>`)
//! - `self.ensure_reactive_engine` (private method)
//! - `self.engine()` (private accessor)
//! - `holon_frontend::reactive::HeadlessBuilderServices` — frontend crate type
//! - `holon_frontend::interpret_pure` — frontend crate function
//!
//! None of these are reachable via `SutViewModel` today. The entire snapshot
//! block is the pre-requisite machinery that all 10a–10j sub-checks depend on.
//! Wire all ReactiveEngine-snapshot-gated invariants together in the phase that
//! exposes the headless interpretation pipeline via a `SutViewModel` capability.

pub struct InvViewmodelSnapshot;

impl InvViewmodelSnapshot {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-viewmodel-snapshot");
}

// `Invariant` impl intentionally omitted — body migration deferred.
