//! Transition: create a block through the focused panel's creation slot.
//!
//! The capstone WP-G gate. Drives the "type here to create" gesture a real
//! user performs — focus the live `:__virtual:<parent>` creation slot of the
//! focused page and commit content — through the PRODUCTION creation-slot
//! commit path (`ReactiveEngineDriver::commit_creation_slot` →
//! `ViewEventHandler::handle_text_sync` → `block.create`). The parent the new
//! block lands under is read from the live slot id that WP-E's
//! `resolve_creation_parent` produced (the query's FOCUS ROOT, not the panel
//! container), so this transition makes WP-E's focus-root parenting a
//! cross-backend regression assertion.
//!
//! Oracle: the model predicts a new text block parented under the single Main
//! focus root, appended as its last child (`create_block_under`). The existing
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
    let roots = state.expected_focus_root_ids(CapRegion::Main);
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
            // Short org-safe ASCII words: no trailing whitespace, no org markup,
            // so the created content round-trips through the block store / any
            // org sync unchanged — the ref content matches the SUT verbatim.
            let strat = proptest::string::string_regex("[a-z]{1,8}")
                .expect("valid regex")
                .prop_map(|content| CreateBlockUnderFocus { content })
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
        state.create_block_under(&parent, &self.content);
    }
}

crate::cap_transition! {
    CreateBlockUnderFocus: holon_pbt_core::capabilities::SutBlockCreate,
    where R: [ RefLifecycle + RefLayout + RefFocusRoots + RefLayoutInteract + RefLayoutMutate ],
    |me, _state, sut| {
        sut.apply_create_under_focus(&me.content).await;
    }
    sql_budget: |_me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        let create = expected_sql_for_kind(MutationKind::Create, watches, blocks, docs);
        ExpectedSql {
            reads: create.reads + CACHE_EVENT_READS,
            writes: create.writes,
            ddl: 0,
            tolerance: create.tolerance + REACTIVE_BASE,
        }
    }
}
