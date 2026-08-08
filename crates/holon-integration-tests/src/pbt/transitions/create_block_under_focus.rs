//! Transition: create a block through the focused panel's creation slot.
//!
//! @pbt rung input-pipeline
//!   no-id arm drives the production creation-slot gesture
//! (`commit_creation_slot`,   re-resolves parent from the live rowset).
//! Explicit-id arm dispatches   `block.create{id,..}` directly (no
//! born-equal-id slot gesture exists). @pbt covers creation-slot —
//! creation-slot gesture mint under focus root @pbt gen both id-present (direct
//! create) and id-absent (slot gesture) arms
//!
//! The capstone WP-G gate. Drives the "type here to create" gesture a real user
//! performs. Under ruling C (2026-08-08) + sub-ruling B (2026-08-09) that
//! gesture is: focus reaches the creation AFFORDANCE — a rendered row that
//! mounts no editor and is not a block — which the engine intercepts into a
//! BIRTH (`ReactiveEngine::birth_creation_affordance`): a frontend-minted id,
//! an authority-only `block.create` with empty content under the resolved
//! parent (`OpOrigin::Rule`, so no undo entry), and the caret seated in the
//! newborn. The typed text then lands as an ordinary undo-visible content
//! write. The headless driver (`ReactiveEngineDriver::commit_creation_slot`)
//! drives exactly that sequence through `BuilderServices::set_focus`. The
//! parent is read from the live affordance id that WP-E's
//! `resolve_creation_parent` produced (the query's FOCUS ROOT, not the panel
//! container), so this transition makes WP-E's focus-root parenting a
//! cross-backend regression assertion.
//!
//! Oracle: for the born-equal arm the model predicts one user create
//! (`create_block_under_with_id`); for the gesture arm it predicts the two-op
//! shape plus the caret move (`birth_block_under_slot`). The existing
//! `inv-live-children-match-ref` then asserts — across BOTH the SQL projection
//! and the Loro tree — that the new block exists under exactly that parent; a
//! backend that dropped it, or parented it to the panel/container instead of
//! the focus root, diverges there. No bespoke oracle is added: the existing
//! tree-comparison invariant already encodes "new block exists AND parent ==
//! focus root".

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::CACHE_EVENT_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

/// Create a text block under the currently focused page via its creation slot.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateBlockUnderFocus {
    pub content: String,
    /// When `Some(uri)`, create the block with that exact born-equal id (the
    /// op-floor `block.create` receives it verbatim, so oracle and SUT share
    /// the id — no synthetic→real reconcile). When `None`, the oracle mints
    /// a synthetic `block::create-N` and the SUT's `block.create` MINTS a
    /// fresh uuid (the mint-when-absent provider path); the harness
    /// reconcile pairs the two 1:1. `id` is a fresh normal id
    /// (`block:gen-N`), deliberately NOT a `block::create-` synthetic, so
    /// the born-equal partition holds.
    pub id: Option<EntityUri>,
}

/// The single Main focus root the creation slot parents a new block under, iff
/// the default layout is active (it renders the `creation_slot: true`
/// collection) and exactly one page is focused AND rendered. `None` when a user
/// `index.org` custom layout is active (its slot semantics differ — WP-E's
/// forest-root resolution is only guaranteed 1:1 with the focus root under the
/// default tree), when focus is home / ambiguous, or when the focused page is
/// not currently in the rendered set (the slot wouldn't appear).
fn create_under_focus_parent<R: RefLayout + RefFocusRoots + RefLayoutInteract>(
    state: &R,
) -> Option<EntityUri> {
    if state.has_user_index_org() {
        return None;
    }
    let roots = state.rendered_focus_root_ids(CapRegion::Main);
    if roots.len() != 1 {
        return None;
    }
    let root = roots.into_iter().next().expect("len checked == 1");
    if !state.main_rendered_block_ids().contains(&root) {
        return None;
    }
    Some(root)
}

impl<R: RefLifecycle + RefLayout + RefFocusRoots + RefLayoutInteract + RefLayoutMutate>
    TransitionFactory<R> for CreateBlockUnderFocus
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        check(
            create_under_focus_parent(state).is_some(),
            Reason::PreconditionFailed,
        )
        .map(|_| {
            // A fresh, deterministic, non-`create-` id for the born-equal (Some)
            // arm. `next_block_id()` advances after every mutating tick
            // (`recanon_and_rebuild` bumps it), so `block:gen-N` is unique per
            // creation and reproducible under proptest replay (unlike a random
            // uuid). Distinct `gen-` prefix ⇒ no collision with `create-N` /
            // `split-N` / `bulk-N`.
            let next = state.next_block_id();
            // Short org-safe ASCII words: no trailing whitespace, no org markup,
            // so the created content round-trips through the block store / any
            // org sync unchanged — the ref content matches the SUT verbatim.
            let content = proptest::string::string_regex("[a-z]{1,8}").expect("valid regex");
            // Mix both create paths: `Some` exercises the explicit-id (born-equal)
            // op dispatch; `None` exercises the provider's mint-when-absent fix.
            let strat = (content, proptest::bool::ANY)
                .prop_map(move |(content, with_id)| CreateBlockUnderFocus {
                    content,
                    id: with_id.then(|| EntityUri::block(&format!("gen-{next}"))),
                })
                .boxed();
            // Moderate weight: valuable structural growth of the focused page,
            // but not so high it starves the editing/navigation alphabet.
            (25, strat)
        })
    }
}

impl<R: RefLifecycle + RefLayout + RefFocusRoots + RefLayoutInteract + RefLayoutMutate>
    TransitionRef<R> for CreateBlockUnderFocus
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            check(!self.content.is_empty(), Reason::PreconditionFailed),
            check(
                create_under_focus_parent(state).is_some(),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        let parent = create_under_focus_parent(state).expect(
            "CreateBlockUnderFocus precondition guarantees a single rendered Main focus root \
             under the default layout",
        );
        match &self.id {
            // Born-equal: the oracle holds the exact id the SUT will dispatch.
            Some(uri) => state.create_block_under_with_id(&parent, &self.content, uri.clone()),
            // The creation-slot GESTURE. Two ops, not one: an undo-invisible
            // empty birth then the user's undo-visible content write (ruling C
            // + sub-ruling B; ADR 0030 D1 authority-only birth, `OpOrigin::Rule`
            // pushes no undo entry). The oracle still allocates a synthetic
            // `create-N` for the harness reconcile to pair with the SUT's
            // minted uuid.
            None => state.birth_block_via_creation_slot(&parent, &self.content),
        }
    }
}

crate::cap_transition! {
    CreateBlockUnderFocus: holon_pbt_core::capabilities::SutBlockCreate,
    where R: [ RefLifecycle + RefLayout + RefFocusRoots + RefLayoutInteract + RefLayoutMutate ],
    |me, state, sut| {
        // Resolve the creation-slot focus-root parent from the SAME ref predicate
        // the precondition/generator gate on. The headless UI driver honors the
        // production slot gesture for the `None` case (re-resolving the parent
        // from its own live rowset — WP-E cross-check preserved); the op-floor
        // driver dispatches `block.create` directly under this parent.
        let parent = create_under_focus_parent(state).expect(
            "CreateBlockUnderFocus precondition guarantees a single rendered Main focus root \
             under the default layout",
        );
        sut.apply_create_under_focus(&parent, &me.content, me.id.as_ref()).await;
    }
    sql_budget: |me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        let create = expected_sql_for_kind(MutationKind::Create, watches, blocks, docs);
        // The born-equal arm is ONE direct `block.create`. The slot-gesture arm
        // (`id: None`) is TWO ops — the authority-only empty birth plus the
        // user's content write — so it must declare both. Declaring only the
        // create would under-state the gesture and leave the difference to be
        // absorbed by tolerance, which is exactly the cap that stops being able
        // to catch a regression.
        let content_write = me
            .id
            .is_none()
            .then(|| expected_sql_for_kind(MutationKind::Update, watches, blocks, docs));
        ExpectedSql {
            reads: create.reads
                + CACHE_EVENT_READS
                + content_write.as_ref().map_or(0, |w| w.reads),
            writes: create.writes + content_write.as_ref().map_or(0, |w| w.writes),
            ddl: 0,
            tolerance: create.tolerance
                + REACTIVE_BASE
                + content_write.as_ref().map_or(0, |w| w.tolerance),
        }
    }
}
