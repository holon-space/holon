//! Transition: retitle an existing page through the production
//! `block.set_field("content")` op.
//!
//! @pbt rung dispatch
//!   `set_field("content")` on a Page-tagged block via the op-floor
//!   `SutPageIdentity`. Per `docs/Plans/PageIdentityDeterminism.md` §5.3 a
//!   rename is "an ordinary edit to the existing entity" — the id does NOT
//!   re-mint — so the reference changes the title and nothing else.
//! @pbt covers page-rename — a page's title changes while its id, parent,
//! children and document stay put, FREEING its old path for a later mint
//!
//! ## Why this transition did not exist
//!
//! Page identity is `blake3(page path)` (`PageId::for_path`). A collision can
//! therefore only arise TEMPORALLY: a path must be freed and then re-minted.
//! Nothing in the keystone could free one. `FocusEditableText` (the editor
//! ingress for content edits) excludes page blocks outright, `ApplyMutation`
//! filters them out of both its block and text-block candidate pools, and
//! document names come from a monotonic counter (`CreateDocument`), so a name
//! is never recycled. This transition is the missing first half; its partner
//! `CreatePageAtFreedPath` is the second.

use holon_api::EntityUri;
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

/// Retitle the page `page_id` to `new_title`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I rename page {page_id} to {new_title}")]
pub struct RenamePage {
    pub page_id: EntityUri,
    pub new_title: String,
}

/// Titles the generator draws from. A fixed, tiny pool keeps replay
/// deterministic and shrinking meaningful; the property does not care WHAT the
/// new title is, only that the old one is freed.
///
/// The pool is deliberately DOT-FREE. A dotted title is a real gap in this
/// alphabet (BugFunnel 2026-08-02: `with_extension("org")` truncated dotted
/// titles, and no title the generator could draw exposed it), but a dotted pool
/// entry cannot be shown to be drawn: `RenamePage` only becomes eligible after
/// a `BlockToPage` mints a page under a page, and no green keystone run prints
/// the title it drew. The dotted coverage is therefore DETERMINISTIC — the
/// hand-authored cases `page-renamed-to-a-dotted-title-rehomes` and
/// `dotted-page-title-owns-its-own-file` — not a lucky draw.
///
/// `Café` and `Straße` carry the spellings ASCII cannot: a decomposed accent
/// and a fold that changes length. A page is also renamed to another spelling
/// of its own title (`holon_api::spelling::another_spelling`), and
/// `CreatePageAtFreedPath` links to such spellings of existing pages.
const TITLE_POOL: [&str; 6] = [
    "Renamed",
    "Retitled",
    "Moved",
    "Renamed2",
    "Caf\u{e9}",
    "Stra\u{df}e",
];

/// Pages that may be renamed:
///
/// * non-seed and Page-tagged;
/// * whose parent is itself a page — i.e. NOT a top-level document root. A
///   document page's title is bound to its file name, so renaming one drags in
///   file-move semantics the reference does not model; the pages `BlockToPage`
///   mints all sit under a page ancestor, which is exactly the population the
///   defect concerns.
/// * with NO page children, so the rename cannot change any OTHER page's path
///   (a descendant page's path is prefixed by this one's title);
/// * with a well-formed `/`-joined page path, so the freed path is a string
///   `PageId::for_path` accepts.
fn candidates<R: RefBlockTree + RefPageIdentity>(state: &R) -> Vec<EntityUri> {
    state
        .all_non_seed_block_ids()
        .into_iter()
        .filter(|id| state.is_page_block(id))
        .filter(|id| {
            state
                .parent_of(id)
                .is_some_and(|parent| state.is_page_block(&parent))
        })
        .filter(|id| {
            !state
                .sorted_children(id)
                .iter()
                .any(|child| state.is_page_block(child))
        })
        .filter(|id| state.page_path_of_ref(id).is_some())
        .collect()
}

/// Titles `page_id` may take: those from [`TITLE_POOL`] whose
/// [`holon_api::PageTitleKey`] no page carries, and another spelling of its
/// own title. A title another page holds would make the backend's title-only
/// `resolve_page_name` ambiguous, and the reference would have to guess which
/// page a later link resolves to.
fn titles_for<R: RefBlockTree + RefPageIdentity>(state: &R, page_id: &EntityUri) -> Vec<String> {
    let taken: Vec<holon_api::PageTitleKey> = state
        .page_titles()
        .iter()
        .map(|t| holon_api::PageTitleKey::of(t))
        .collect();
    let own = state
        .block_content(page_id)
        .and_then(|content| content.lines().next())
        .expect("a rename candidate is a titled page");
    TITLE_POOL
        .iter()
        .map(|t| (*t).to_string())
        .filter(|t| !taken.contains(&holon_api::PageTitleKey::of(t)))
        .chain(std::iter::once(holon_api::spelling::another_spelling(own)))
        .collect()
}

impl<R: RefLifecycle + RefBlockTree + RefPageIdentity> TransitionFactory<R> for RenamePage {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let choices: Vec<(EntityUri, Vec<String>)> = candidates(state)
            .into_iter()
            .map(|id| {
                let titles = titles_for(state, &id);
                (id, titles)
            })
            .collect();
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!choices.is_empty(), Reason::PreconditionFailed),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| {
                let strat = prop::sample::select(choices)
                    .prop_flat_map(|(page_id, titles)| {
                        prop::sample::select(titles).prop_map(move |new_title| RenamePage {
                            page_id: page_id.clone(),
                            new_title,
                        })
                    })
                    .boxed();
                // Weight 2: this transition is the ONLY producer of freed page
                // paths, and `CreatePageAtFreedPath` has nothing to draw from
                // until it fires at least once.
                (2, strat)
            })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefPageIdentity> TransitionRef<R> for RenamePage {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                candidates(state).contains(&self.page_id),
                Reason::PreconditionFailed,
            ),
            check(
                !self.new_title.trim().is_empty(),
                Reason::PreconditionFailed,
            ),
            check(
                state.block_content(&self.page_id) != Some(self.new_title.as_str()),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.apply_page_rename(&self.page_id, &self.new_title);
    }
}

crate::cap_transition! {
    RenamePage: holon_pbt_core::capabilities::SutPageIdentity,
    where R: [ RefLifecycle + RefBlockTree + RefPageIdentity ],
    |me, _state, sut| {
        sut.rename_page(&me.page_id, &me.new_title).await;
    }
    sql_budget: |me, state| {
        let blocks = state.block_count();
        // Each block whose link text the rename rewrites adds at most one org
        // write-back, plus its `block_raw` write where SQL holds block CRUD.
        let per_rewrite = if state.content_writes_reach_sql() { 2 } else { 1 };
        ExpectedSql {
            reads: blocks + 8,
            writes: 2 + per_rewrite * state.name_linking_blocks(&me.page_id),
            ddl: 0,
            tolerance: 3,
        }
    }
}
