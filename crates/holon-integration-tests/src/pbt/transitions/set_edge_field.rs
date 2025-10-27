//! Transition: set a block **edge field** (`tags` / `requires`) on an existing
//! block — the MODIFY path of the junction-backed, set-valued attributes.
//!
//! This is the only transition that mutates an edge field on an *already
//! created* block, so it is the one that exercises the Loro→SQL change gate
//! (`loro_sync_controller::blocks_differ`) for edge fields. It is parameterized
//! over *which* edge field via [`EdgeFieldUpdate`] so neither `tags` nor
//! `requires` is special-cased: a `tags` modify must re-project (the gate
//! compares `tags`), a `requires` modify must too (H12 = the gate dropping it).
//!
//! **Composed-host only.** The write routes through the production edge-field
//! writers (`set_block_tags` / `set_block_requires` on the Loro backend — the
//! same functions the org re-scan reconciliation calls), so it flows
//! Loro → `project()` → SQL exactly as production does. The faithful host is
//! the composed `EdgeFieldWriter` (`full_headless`), so the generator gates on
//! a composed config (`cap_set.is_some()`); the monolithic Turso `E2ESut`
//! exposes no Loro authority for this write and is excluded. The catch lives in
//! the composed `general_e2e_composed_pbt` — `full_headless` hosts
//! `SutEdgeFieldWrite` plus the `/matview` invariant that observes the
//! `requires` re-projection.

use holon_api::EdgeFieldUpdate;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutEdgeFieldWrite;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Union;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;
use crate::pbt::validation::Reason;
use crate::pbt::validation::check;

/// Set one edge field (`tags` or `requires`) on an existing block.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SetEdgeField {
    pub block_id: EntityUri,
    pub update: EdgeFieldUpdate,
}

/// Blocks whose edge fields are safe to rewrite: existing, non-page (so a
/// `tags` rewrite can't flip `is_page`), and not layout/profile/render/query
/// special blocks (rewriting their edge fields would perturb unrelated
/// invariants).
fn eligible_blocks(state: &ReferenceState) -> Vec<EntityUri> {
    let special: std::collections::HashSet<EntityUri> = state
        .domain
        .layout_blocks
        .render_source_ids
        .iter()
        .chain(state.domain.layout_blocks.query_source_ids.iter())
        .chain(state.domain.profile_block_ids.iter())
        .cloned()
        .collect();
    state
        .domain
        .block_state
        .blocks
        .iter()
        .filter(|(id, b)| {
            !b.is_page()
                && !special.contains(*id)
                && !state.domain.layout_blocks.contains(id)
                && !state.domain.layout_blocks.is_immutable(id)
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// Whether an edge-field write is faithfully hostable here. True only for a
/// **composed** config (`cap_set` is `Some` — set by `with_cap_set`): there the
/// `SutEdgeFieldWrite` cap, when present, is the Loro-backed `EdgeFieldWriter`
/// (the aggregate's `caps_available` then confirms it is actually hosted). The
/// monolithic `E2ESut` leaves `cap_set == None` and, in its default Turso
/// config, exposes no Loro authority for the edge-field write — so it is
/// excluded here. (Wiring can't discriminate: the composed WideE2E reference
/// carries Turso too.)
fn composed_loro_host(state: &ReferenceState) -> bool {
    state.enable_loro() && state.cap_set.is_some()
}

impl TransitionFactory<ReferenceState> for SetEdgeField {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        // Single-sourced from the `cap_transition!` below.
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(composed_loro_host(state), Reason::PreconditionFailed),
        ];
        let merged: Validated<Vec<()>, Reason> = checks.into_iter().collect();
        if let Validated::Fail(reasons) = merged {
            return Validated::Fail(reasons);
        }

        let eligible = eligible_blocks(state);
        if eligible.is_empty() {
            return Validated::fail(Reason::PreconditionFailed);
        }

        let mut arms: Vec<(u32, BoxedStrategy<SetEdgeField>)> = Vec::new();

        // `tags` arm: 1–2 lowercase tags (never `Page` → `is_page` unchanged).
        {
            let elig = eligible.clone();
            let tags_arm = (
                proptest::sample::select(elig),
                proptest::collection::vec("[a-z]{3,6}", 1..3),
            )
                .prop_map(|(block_id, tags)| SetEdgeField {
                    block_id,
                    update: EdgeFieldUpdate::Tags(Tags::from_csv(&tags.join(","))),
                })
                .boxed();
            arms.push((1, tags_arm));
        }

        // `requires` arm: a single dependency edge to a *distinct* existing block.
        let n = eligible.len();
        if n >= 2 {
            let elig = eligible.clone();
            let requires_arm = (0..n, 0..n)
                .prop_filter("requires target must differ from subject", |(i, j)| i != j)
                .prop_map(move |(i, j)| SetEdgeField {
                    block_id: elig[i].clone(),
                    update: EdgeFieldUpdate::Requires(vec![elig[j].clone()]),
                })
                .boxed();
            arms.push((2, requires_arm));
        }

        // Weight 4 (cf. ApplyMutation's conflict arm): the sole transition that
        // exercises the edge-field MODIFY → Loro→SQL projection path, so it must
        // fire reliably within the case budget, without dominating the alphabet.
        Validated::Good((4, Union::new_weighted(arms).boxed()))
    }
}

impl TransitionRef<ReferenceState> for SetEdgeField {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(composed_loro_host(state), Reason::PreconditionFailed),
            check(
                state
                    .domain
                    .block_state
                    .blocks
                    .get(&self.block_id)
                    .is_some_and(|b| !b.is_page()),
                Reason::PreconditionFailed,
            ),
            check(
                !state.domain.layout_blocks.is_immutable(&self.block_id),
                Reason::FocusedInLayoutBlocks,
            ),
        ];
        // A `requires` dependency must point at an existing block.
        if let EdgeFieldUpdate::Requires(targets) = &self.update {
            for t in targets {
                checks.push(check(
                    state.domain.block_state.blocks.contains_key(t),
                    Reason::PreconditionFailed,
                ));
            }
        }
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let block = state
            .domain
            .block_state
            .blocks
            .get_mut(&self.block_id)
            .expect("SetEdgeField: subject block must exist (precondition)");
        // Direct field assignment (both are public edge-field columns):
        // `is_page` is computed from `tags` on read, so there is no cached
        // state to keep in sync.
        match &self.update {
            EdgeFieldUpdate::Tags(tags) => block.tags = tags.clone(),
            EdgeFieldUpdate::Requires(reqs) => block.requires = reqs.clone(),
        }
    }
}

crate::cap_transition! {
    SetEdgeField: SutEdgeFieldWrite,
    |me, _state, sut| { sut.apply_set_edge_field(&me.block_id, &me.update).await; }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for SetEdgeField {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        // An edge-field rewrite re-projects one block's junction rows
        // (delete + re-insert): an Update-shaped budget with extra margin.
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.mcp.active_watches.len(),
            state.domain.block_state.blocks.len(),
            state.files.documents.len(),
        );
        sql.tolerance += 5;
        sql
    }
}
