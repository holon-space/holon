//! Transition: turn an existing non-page block into a page through the
//! production `block.convert_block_to_page` compound (BlockToPageTransform,
//! Option B). The SUT-side op-floor driver dispatches `convert_block_to_page`
//! with an EMPTY `destination_path` (omitted), so the backend takes its
//! `destination_path`-absent branch and defaults the destination to the
//! origin's nearest page ancestor. The ref side mints the SAME deterministic
//! page id (`PageId::for_path` over the same title path the backend planner
//! walks) under that ancestor and re-homes the origin's children onto it —
//! leaving the origin a non-page `[[P]]` link — so the block-comparison and
//! `inv-no-page-under-non-page` invariants verify the transform.
//!
//! @pbt rung dispatch
//!   `convert_block_to_page` mints a NEW page P under the origin's nearest page
//!   ancestor and re-homes the origin's children under P via the op-floor
//!   `SutBlockToPage`. The origin stays a non-page — Option B, not an in-place
//!   page-hood flip.
//! @pbt covers block-to-page — non-page block + children -> NEW page under the
//! nearest page ancestor (origin left as a `[[P]]` link)

use std::collections::BTreeSet;

use holon_api::EntityUri;
use holon_api::link_parser::PageId;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Convert `origin_id` (a non-page block with children, under a page ancestor)
/// into a page: a NEW page P is minted under the origin's nearest page ancestor
/// and the origin's children move under P.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BlockToPage {
    pub origin_id: EntityUri,
}

/// The origin's nearest PAGE ancestor, found by walking `parent_of` upward
/// until a page block is reached. `None` if no page ancestor exists (or a
/// parent cycle is hit — the ref's `inv-no-parent-cycles` owns that; here we
/// bail).
fn nearest_page_ancestor<R: RefBlockTree>(state: &R, origin: &EntityUri) -> Option<EntityUri> {
    let mut seen: BTreeSet<EntityUri> = BTreeSet::new();
    let mut cursor = state.parent_of(origin);
    while let Some(anc) = cursor {
        if !seen.insert(anc.clone()) {
            return None;
        }
        if state.is_page_block(&anc) {
            return Some(anc);
        }
        cursor = state.parent_of(&anc);
    }
    None
}

/// The `/`-joined page path (root→leaf) of an existing page, reconstructed by
/// walking its page-ancestor chain and collecting each page's trimmed `content`
/// title. Mirrors the backend planner's `page_path_of` so `PageId::for_path`
/// hashes the SAME string on both sides. Returns `None` if any page in the
/// chain has empty content (the backend would then produce an empty path
/// segment and fail `PageId::for_path` — such candidates are gated out).
fn page_path_of<R: RefBlockTree>(state: &R, page: &EntityUri) -> Option<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut seen: BTreeSet<EntityUri> = BTreeSet::new();
    let mut cursor = Some(page.clone());
    while let Some(id) = cursor {
        if !seen.insert(id.clone()) {
            break;
        }
        if !state.is_page_block(&id) {
            break;
        }
        let title = state.block_content(&id)?.trim().to_string();
        if title.is_empty() {
            return None;
        }
        segments.push(title);
        cursor = state.parent_of(&id);
    }
    segments.reverse();
    Some(segments.join("/"))
}

/// Compute `(nearest_page_ancestor, born-equal page id)` for the origin, or
/// `None` if the origin is not a legal block→page candidate (no page ancestor,
/// empty content anywhere in the resulting path, or a path `PageId::for_path`
/// rejects). Single source shared by the generator, the precondition, and the
/// ref effect so all three agree on candidacy.
fn plan_new_page<R: RefBlockTree>(state: &R, origin: &EntityUri) -> Option<(EntityUri, EntityUri)> {
    let ancestor = nearest_page_ancestor(state, origin)?;
    let ancestor_path = page_path_of(state, &ancestor)?;
    let origin_content = state.block_content(origin)?.trim().to_string();
    if origin_content.is_empty() {
        return None;
    }
    let full_path = if ancestor_path.is_empty() {
        origin_content
    } else {
        format!("{ancestor_path}/{origin_content}")
    };
    let page_id = PageId::for_path(&full_path).ok()?.into_entity_uri();
    Some((ancestor, page_id))
}

/// Non-seed, NON-page blocks that (i) have a nearest page ancestor, (ii) have
/// at least one child, and (iii) yield a well-formed page path.
fn candidates<R: RefBlockTree>(state: &R) -> Vec<EntityUri> {
    state
        .all_non_seed_block_ids()
        .into_iter()
        .filter(|id| !state.is_page_block(id))
        .filter(|id| !state.sorted_children(id).is_empty())
        .filter(|id| plan_new_page(state, id).is_some())
        .collect()
}

impl<R: RefLifecycle + RefBlockTree + RefLayoutMutate> TransitionFactory<R> for BlockToPage {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = candidates(state);
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|origin_id| BlockToPage { origin_id })
                .boxed();
            (2, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefLayoutMutate> TransitionRef<R> for BlockToPage {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.block_content(&self.origin_id).is_some(),
                Reason::PreconditionFailed,
            ),
            check(
                !state.is_page_block(&self.origin_id),
                Reason::PreconditionFailed,
            ),
            check(
                !state.sorted_children(&self.origin_id).is_empty(),
                Reason::PreconditionFailed,
            ),
            check(
                plan_new_page(state, &self.origin_id).is_some(),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        let (ancestor, page_id) = plan_new_page(state, &self.origin_id)
            .expect("BlockToPage precondition guarantees a legal page-conversion candidate");
        state.apply_block_to_page(&self.origin_id, page_id, &ancestor);
    }
}

crate::cap_transition! {
    BlockToPage: holon_pbt_core::capabilities::SutBlockToPage,
    where R: [ RefLifecycle + RefBlockTree ],
    |me, _state, sut| {
        // Empty destination_path ⇒ the backend defaults to the origin's nearest
        // page ancestor, matching the ref effect's destination.
        sut.convert_block_to_page(&me.origin_id, "").await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        ExpectedSql {
            reads: blocks + 12,
            writes: 6,
            ddl: 0,
            tolerance: 3,
        }
    }
}
