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
//! Loro → `project()` → SQL exactly as production does. The faithful host is the
//! composed `EdgeFieldWriter` (`full_headless`), so the generator gates on a
//! composed config (`cap_set.is_some()`); the monolithic Turso `E2ESut` exposes
//! no Loro authority for this write and is excluded. The catch lives in the
//! composed `general_e2e_composed_pbt` — `full_headless` hosts `SutEdgeFieldWrite`
//! plus the `/matview` invariant that observes the `requires` re-projection.

use holon_api::{EdgeFieldUpdate, EntityUri, Tags};
use holon_pbt_core::capabilities::{
    RefBlockTree, RefLayout, RefLayoutInteract, RefLayoutMutate, RefLifecycle, RefWiring,
    SutEdgeFieldWrite,
};
use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, Union};
use validated::Validated;

use crate::pbt::generators::ADVICE_TAG_POOL;
use holon_pbt_core::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{MutationKind, expected_sql_for_kind};

/// Set one edge field (`tags` or `requires`) on an existing block.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SetEdgeField {
    pub block_id: EntityUri,
    pub update: EdgeFieldUpdate,
}

/// Blocks whose edge fields are safe to rewrite: existing, non-page (so a `tags`
/// rewrite can't flip `is_page`), and not layout/profile/render/query special
/// blocks (rewriting their edge fields would perturb unrelated invariants).
fn eligible_blocks<R: RefBlockTree + RefLayout + RefLayoutInteract>(state: &R) -> Vec<EntityUri> {
    let special: std::collections::HashSet<EntityUri> = state
        .render_source_ids()
        .into_iter()
        .chain(state.query_source_ids())
        .chain(state.profile_block_ids())
        .collect();
    state
        .all_block_ids()
        .into_iter()
        .filter(|id| {
            !state.is_page_block(id)
                && !special.contains(id)
                && !state.is_layout_block(id)
                && !state.is_immutable(id)
        })
        .collect()
}

/// Whether an edge-field write is faithfully hostable here. True only for a
/// **composed** config (`cap_set` is `Some` — set by `with_cap_set`): there the
/// `SutEdgeFieldWrite` cap, when present, is the Loro-backed `EdgeFieldWriter`
/// (the aggregate's `caps_available` then confirms it is actually hosted). The
/// monolithic `E2ESut` leaves `cap_set == None` and, in its default Turso config,
/// exposes no Loro authority for the edge-field write — so it is excluded here.
/// (Wiring can't discriminate: the composed WideE2E reference carries Turso too.)
fn composed_loro_host<R: RefLifecycle + RefWiring>(state: &R) -> bool {
    state.enable_loro() && state.has_cap_set()
}

impl<R: RefLifecycle + RefBlockTree + RefLayout + RefLayoutInteract + RefWiring + RefLayoutMutate>
    TransitionFactory<R> for SetEdgeField
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        // Single-sourced from the `cap_transition!` below.
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
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

        // `tags` arm (never `Page` → `is_page` unchanged). Two sub-arms, 2:1
        // pool:random. The pool sub-arm draws DISTINCT tags from
        // `ADVICE_TAG_POOL` (the same pool the advice-rule generator anchors on),
        // making `task`/`lesson` tagging and hence advice tag-overlap reachable;
        // the random sub-arm keeps arbitrary `[a-z]{3,6}` coverage.
        {
            let random_tags = (
                proptest::sample::select(eligible.clone()),
                proptest::collection::vec("[a-z]{3,6}", 1..3),
            )
                .prop_map(|(block_id, tags)| SetEdgeField {
                    block_id,
                    update: EdgeFieldUpdate::Tags(Tags::from_csv(&tags.join(","))),
                })
                .boxed();
            let pool_tags = (
                proptest::sample::select(eligible.clone()),
                proptest::sample::subsequence(ADVICE_TAG_POOL.to_vec(), 1..=3),
            )
                .prop_map(|(block_id, tags)| SetEdgeField {
                    block_id,
                    update: EdgeFieldUpdate::Tags(Tags::from_csv(&tags.join(","))),
                })
                .boxed();
            arms.push((1, prop_oneof![2 => pool_tags, 1 => random_tags].boxed()));
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

            // `advice_suppressed` arm: mirrors `requires` — a single dismissal
            // edge to a *distinct* existing block (ADR 0021 exclusion set). The
            // general (unbiased) sub-arm keeps weight 1.
            let elig = eligible.clone();
            let advice_arm = (0..n, 0..n)
                .prop_filter("advice target must differ from subject", |(i, j)| i != j)
                .prop_map(move |(i, j)| SetEdgeField {
                    block_id: elig[i].clone(),
                    update: EdgeFieldUpdate::AdviceSuppressed(vec![elig[j].clone()]),
                })
                .boxed();
            arms.push((1, advice_arm));

            // Biased advice sub-arm (weight 2): when both a `task`-tagged and a
            // `lesson`-tagged eligible block exist, prefer subject = task,
            // target(s) = lesson(s) — the exact shape a `lessons_for_tasks` rule
            // suppresses, so suppression actually removes a woven row. Gated on
            // carrier availability like the `n >= 2` gate. ~1 in 3 draws two
            // distinct lessons: the edge set is REPLACED whole, so multi-element
            // sets exercise the accumulate/replace semantics.
            let carriers = |tag: &str| -> Vec<EntityUri> {
                eligible
                    .iter()
                    .filter(|id| state.block_has_tag(id, tag))
                    .cloned()
                    .collect()
            };
            let tasks = carriers("task");
            let lessons = carriers("lesson");
            // Constructive pairing, NOT a prop_filter: when the only task
            // carrier IS the only lesson carrier, a subject≠target filter can
            // never succeed and its rejects abort the whole run.
            let pairs: Vec<(EntityUri, Vec<EntityUri>)> = tasks
                .iter()
                .map(|t| {
                    let ls: Vec<EntityUri> = lessons.iter().filter(|l| *l != t).cloned().collect();
                    (t.clone(), ls)
                })
                .filter(|(_, ls)| !ls.is_empty())
                .collect();
            if !pairs.is_empty() {
                let biased_advice = proptest::sample::select(pairs)
                    .prop_flat_map(|(subject, ls)| {
                        let targets = if ls.len() >= 2 {
                            prop_oneof![
                                2 => proptest::sample::select(ls.clone()).prop_map(|l| vec![l]),
                                1 => proptest::sample::subsequence(ls.clone(), 2..=2),
                            ]
                            .boxed()
                        } else {
                            proptest::sample::select(ls.clone())
                                .prop_map(|l| vec![l])
                                .boxed()
                        };
                        targets.prop_map(move |targets| SetEdgeField {
                            block_id: subject.clone(),
                            update: EdgeFieldUpdate::AdviceSuppressed(targets),
                        })
                    })
                    .boxed();
                arms.push((2, biased_advice));
            }
        }

        // Weight 4 (cf. ApplyMutation's conflict arm): the sole transition that
        // exercises the edge-field MODIFY → Loro→SQL projection path, so it must
        // fire reliably within the case budget, without dominating the alphabet.
        Validated::Good((4, Union::new_weighted(arms).boxed()))
    }
}

impl<R: RefLifecycle + RefBlockTree + RefLayoutInteract + RefWiring + RefLayoutMutate>
    TransitionRef<R> for SetEdgeField
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(composed_loro_host(state), Reason::PreconditionFailed),
            check(
                state.block_content(&self.block_id).is_some()
                    && !state.is_page_block(&self.block_id),
                Reason::PreconditionFailed,
            ),
            check(
                !state.is_immutable(&self.block_id),
                Reason::FocusedInLayoutBlocks,
            ),
        ];
        // A `requires` / `advice_suppressed` edge must point at an existing block.
        let edge_targets = match &self.update {
            EdgeFieldUpdate::Requires(targets) => Some(targets),
            EdgeFieldUpdate::AdviceSuppressed(targets) => Some(targets),
            EdgeFieldUpdate::Tags(_) => None,
        };
        if let Some(targets) = edge_targets {
            for t in targets {
                checks.push(check(
                    state.block_content(t).is_some(),
                    Reason::PreconditionFailed,
                ));
            }
        }
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.set_edge_field_value(&self.block_id, &self.update);
    }
}

crate::cap_transition! {
    SetEdgeField: SutEdgeFieldWrite,
    where R: [ RefLifecycle + RefBlockTree + RefLayoutInteract + RefWiring + RefLayoutMutate ],
    |me, _state, sut| {
        sut.apply_set_edge_field(&me.block_id, &me.update).await;
    }
    sql_budget: |_me, state| {
        // An edge-field rewrite re-projects one block's junction rows
        // (delete + re-insert): an Update-shaped budget with extra margin.
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        sql.tolerance += 5;
        sql
    }
}
