//! `inv-value-fn-provider-arg-variance-13`.
//!
//! The value-fn provider checks, asserting on the SUT-computed
//! [`ProviderStabilityReport`]
//! ([`SutViewSelection::provider_stability_report`]) so the `ReactiveEngine`/
//! `interpret_pure`/`ProviderCache` coupling stays on the SUT side. Four
//! sub-checks (inline `[inv_bar]`/`[vfn11/12/13]`):
//! - **inv_bar** — a render_expr mentioning `bottom_dock` must interpret to ≥1
//!   `BottomDock` node.
//! - **arg-variance (vfn11)** — when the active render_expr mentions
//!   `focus_chain`, the reference has a global focus, and providers exist, at
//!   least one provider must produce rows.
//! - **identity (vfn12)** — each `(template, rows)` provider group resolves to
//!   a single `cache_identity` (no `Arc` churn within a pass).
//! - **flicker (vfn13)** — every cache identity from pass 1 reappears in pass
//!   2.
//!
//! `Skipped` when the reference has no blocks or the root is still
//! initializing (the report is `None`).

use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::SutFrontendEmissions;
use holon_pbt_core::capabilities::ViewportHint;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// Narrow viewport that forces the `if_space`-gated mobile action bar
/// (`focus_chain()` + `ops_of(...)`), guaranteeing chain coverage every run.
const PROBE_VIEWPORT: ViewportHint = ViewportHint {
    width_px: 500.0,
    height_px: 800.0,
};

pub struct InvValueFnProviderArgVariance13;

impl InvValueFnProviderArgVariance13 {
    pub const ID: InvariantId = InvariantId("inv-value-fn-provider-arg-variance-13");
    const LABEL: &'static str = "inv-value-fn-provider-arg-variance-13";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvValueFnProviderArgVariance13
where
    R: RefLayout + RefGlobalFocus,
    S: SutFrontendEmissions,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Inline gate: `app_started && !block_state.blocks.is_empty()`
        // (app_started is already enforced by the runner).
        if ref_.all_block_ids().is_empty() {
            return InvariantResult::Skipped(format!("[{}] reference has no blocks", Self::LABEL));
        }

        let Some(report) = sut.provider_stability_report(PROBE_VIEWPORT).await else {
            return InvariantResult::Skipped(format!(
                "[{}] reactive root still initializing (loading/spacer) or no engine",
                Self::LABEL
            ));
        };

        // inv_bar — bottom_dock structural presence.
        if report.mentions_bottom_dock && report.bottom_dock_count < 1 {
            return InvariantResult::Fail(format!(
                "[{}/inv_bar] render_expr mentions bottom_dock but interpreted tree contains 0 \
                 BottomDock nodes",
                Self::LABEL,
            ));
        }

        // vfn11 — provider arg variance: focus_chain present + a focus target +
        // providers surfaced ⇒ at least one provider must produce rows.
        let expects_focus_rows = ref_.global_focused_block().is_some()
            && report.mentions_focus_chain
            && report.total_providers > 0;
        if expects_focus_rows && !report.any_nonempty {
            return InvariantResult::Fail(format!(
                "[{}/vfn11] active render_expr mentions focus_chain and the reference has a \
                 focused block, but no streaming provider produced rows",
                Self::LABEL,
            ));
        }

        // vfn12 — provider identity stability within one pass.
        if let Some(msg) = &report.identity_instability {
            return InvariantResult::Fail(format!(
                "[{}/vfn12] provider identity instability: {msg}",
                Self::LABEL
            ));
        }

        // vfn13 — cache identity flicker across re-interpret.
        if report.flicker_count > 0 {
            return InvariantResult::Fail(format!(
                "[{}/vfn13] provider cache identity flicker: {} id(s) present in pass-1 but \
                 missing in pass-2 — cache wiring regressed",
                Self::LABEL,
                report.flicker_count,
            ));
        }

        InvariantResult::Ok
    }
}
