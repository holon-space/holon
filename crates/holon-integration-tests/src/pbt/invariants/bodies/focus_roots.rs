//! Phase 7 — `inv-focus-roots` (DEFERRED).
//!
//! Inline body lives at `sut.rs:4716–4838`.
//!
//! # Why deferred
//!
//! The body requires:
//! - `self.live_focus_roots()` — private `LiveData<FocusRoot>` mirror
//! - `self.resolve_uri` — private URI translation
//! - `ref_state.expected_focus_root_ids(region)` — no Ref* cap today
//! - `self.ctx.query_sql(focus_roots truth-check SQL)` — truth-check path
//! - `holon_api::Region::ALL` iteration — integration-tests-internal type
//! - CDC-lag downgrade path (truth-check pattern)
//!
//! Also tagged `RunMode::Warn` in the registry; the WARN/skip classifier
//! depends on `SutCdc::cdc_in_flight` which is itself unimplemented on
//! `E2ESut`. Wire when: (a) `live_focus_roots` is pub or has an accessor,
//! (b) `RefFocus::expected_focus_roots` cap is added, (c) `SutCdc::cdc_in_flight`
//! is wired.

pub struct InvFocusRoots;

impl InvFocusRoots {
    pub const ID: holon_pbt_core::invariant::InvariantId =
        holon_pbt_core::invariant::InvariantId("inv-focus-roots");
}

// `Invariant` impl intentionally omitted — body migration deferred.
