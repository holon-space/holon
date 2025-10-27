//! `inv-viewmodel-shows-source-when-no-query` — the degraded ("shows source")
//! render twin.
//!
//! In a no-query-engine wiring (no Turso), the production
//! `loro_ui_watcher::derive_render_expr` degrades a query-source block to the bare
//! `source` view mode (ADR 0004 Phase 9, capability-driven degradation): the root
//! render expression is a `source_editor` `FunctionCall`. This body asserts exactly
//! that, reading the SUT's own root render kind.
//!
//! §5.2 SOUNDNESS: this reads ZERO reference caps. The degradation is decided by
//! the SUT's wiring (query engine present or not), NOT by the reference model —
//! which is cap-blind to the storage backend and would predict the full-mode
//! render. Consulting a ref here would produce a guaranteed false `Fail`, so the
//! check is purely SUT-internal: the root render kind must be `source_editor`.
//!
//! Selection: `Needs { sut_present: [SutRenderer], sut_absent: [SutQueryResults] }`
//! — it fires ONLY where a renderer is wired WITHOUT a query engine (the degraded
//! `block_query_degraded` builder), and is deselected (disclosed) wherever the
//! full-mode `SutQueryResults` is present.

use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvViewmodelShowsSourceWhenNoQuery;

impl InvViewmodelShowsSourceWhenNoQuery {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-shows-source-when-no-query");
    const LABEL: &'static str = "inv-viewmodel-shows-source-when-no-query";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelShowsSourceWhenNoQuery
where
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        match sut.root_render_kind().await {
            Some(kind) if kind == "source_editor" => InvariantResult::Ok,
            Some(kind) => InvariantResult::Fail(format!(
                "[{}] degraded (no query engine) render must be `source_editor`, got `{kind}`",
                Self::LABEL
            )),
            None => InvariantResult::Skipped(format!(
                "[{}] root render not ready (no watch / loading / spacer / non-FunctionCall)",
                Self::LABEL
            )),
        }
    }
}
