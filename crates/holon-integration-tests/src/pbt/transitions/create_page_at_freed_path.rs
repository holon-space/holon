//! Transition: lazily create a page at a path a `RenamePage` has FREED, through
//! the production `block.create_page_from_link` op.
//!
//! @pbt rung dispatch
//!   `create_page_from_link(target)` via the op-floor `SutPageIdentity` — the
//!   path a click on a newly typed, dangling `[[Target]]` takes.
//! @pbt covers page-id-rename-collision — a page name freed by a rename and
//! then re-created must yield a SECOND, distinct page, never overwrite the
//! renamed one @pbt gen target drawn from the reference's freed-path ledger
//! (the temporal axis a per-state generator cannot otherwise reach)
//!
//! ## The property this exists for
//!
//! Page ids are `blake3(normalized path)`. `docs/Plans/PageIdentityDeterminism
//! .md` §5.3 rules that a rename is an ordinary edit — the id does not re-mint
//! — and that "a *new* page created later under the new name gets a new id;
//! that is correct (it is a different logical page)". So after
//! `create A → rename A to B → create A` the store must hold TWO pages.
//!
//! Reaching that state needs a name to be freed and then reused, which is
//! temporal: no single reference snapshot exposes it. `RenamePage` supplies the
//! freeing half and records the vacated path; this transition draws its target
//! from that ledger.
//!
//! The generator only offers a path whose ancestor chain already resolves and
//! whose LEAF does not, so the op mints exactly one page and the reference can
//! predict the effect without mirroring the whole chain-creation search.

use holon_api::link_parser::PageId;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefPageIdentity;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Create the page chain named by the `/`-joined `path` (a path some earlier
/// `RenamePage` vacated).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CreatePageAtFreedPath {
    pub path: String,
}

/// Freed paths the op would mint EXACTLY ONE page for:
///
/// * every segment is non-empty after trimming and `PageId::for_path` accepts
///   the whole path (the op errors otherwise);
/// * every strict prefix still resolves through the backend's title-only
///   `resolve_page_name`, so the op reuses those pages rather than minting
///   them;
/// * the LEAF does not resolve, so the op must mint it — this is the step that
///   recomputes `PageId::for_path(path)` and hits the id the renamed page still
///   holds.
fn candidates<R: RefPageIdentity>(state: &R) -> Vec<String> {
    state
        .freed_page_paths()
        .into_iter()
        .filter(|path| {
            let segments: Vec<&str> = path.split('/').map(str::trim).collect();
            if segments.iter().any(|s| s.is_empty()) || PageId::for_path(path).is_err() {
                return false;
            }
            let mut accumulated = String::new();
            for (i, seg) in segments.iter().enumerate() {
                let hint = if i == 0 {
                    (*seg).to_string()
                } else {
                    format!("{accumulated}/{seg}")
                };
                let is_leaf = i + 1 == segments.len();
                if state.ref_resolve_page_name(&hint).is_some() == is_leaf {
                    return false;
                }
                accumulated = hint;
            }
            true
        })
        .collect()
}

impl<R: RefLifecycle + RefBlockTree + RefPageIdentity> TransitionFactory<R>
    for CreatePageAtFreedPath
{
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = candidates(state);
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!candidates.is_empty(), Reason::PreconditionFailed),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| {
                let strat = prop::sample::select(candidates)
                    .prop_map(|path| CreatePageAtFreedPath { path })
                    .boxed();
                // Weight 3: the pool is only ever non-empty in the narrow window
                // after a rename, and the whole point of the transition is to be
                // drawn inside it.
                (3, strat)
            })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefPageIdentity> TransitionRef<R> for CreatePageAtFreedPath {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                candidates(state).contains(&self.path),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.apply_create_page_at_path(&self.path);
    }
}

crate::cap_transition! {
    CreatePageAtFreedPath: holon_pbt_core::capabilities::SutPageIdentity,
    where R: [ RefLifecycle + RefBlockTree + RefPageIdentity ],
    |me, _state, sut| {
        sut.create_page_from_link(&me.path).await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        ExpectedSql {
            reads: blocks + 12,
            writes: 4,
            ddl: 0,
            tolerance: 4,
        }
    }
}
