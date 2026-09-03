//! Transition: run a quick-open search and compare the hits against the
//! reference model's own substring match.
//!
//! @pbt rung dispatch
//!   drives `QueryEngine::quick_open_search` — the exact call the cmd-K
//!   overlay makes (`frontends/gpui/src/search_ui.rs:run_search`).
//! @pbt covers quick-open-search — the hit set equals the reference model's
//! literal substring match over block content and page titles, folded by
//! Unicode simple case folding, with pattern metacharacters matching themselves
//! and an empty query returning nothing.

use std::collections::BTreeSet;

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSearch;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Queries that carry a pattern metacharacter or a quote, plus queries written
/// in a script where every letter is cased and non-ASCII. The first class acts
/// as a wildcard when the predicate is built unescaped; the second makes the
/// predicate as large as the folding rule allows, which is where a per-letter
/// nesting blew the stack. Both are always in the alphabet, regardless of what
/// the vault happens to contain.
const ADVERSARIAL_QUERIES: &[&str] = &[
    "%",
    "_",
    "a_b",
    "100%",
    "\\",
    "o_e",
    "'",
    "",
    "*",
    "?",
    "[a-z]",
    "программирование на русском языке",
    "Επεξεργασία κειμένου",
    "абвгдежзийклмнопрстуфхцч αβγδεζηθικλμνξοπρστυφχψω",
];

/// Longest query drawn from block content. Long enough to be selective, short
/// enough that the SUT's `LIMIT` is rarely the reason a block is missing.
const MAX_DRAWN_QUERY: usize = 12;

/// Search the vault and assert the result set against the reference model.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the user searches for {query}")]
pub struct Search {
    pub query: String,
}

/// Substrings of `content` a user could plausibly type, case-perturbed in
/// Unicode (not ASCII) so a drawn `é` also arrives as the `É` a German or
/// French user actually types.
fn query_candidates(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter(|w| w.chars().count() >= 3)
        .flat_map(|w| {
            let trimmed: String = w.chars().take(MAX_DRAWN_QUERY).collect();
            [
                trimmed.chars().map(simple_upper).collect(),
                trimmed.chars().map(simple_fold).collect(),
                trimmed,
            ]
        })
        .collect()
}

/// Unicode *simple* case folding: each character maps to its lowercase form
/// when that is a single character, else to itself.
///
/// Simple, not full, because the fold has to be expressible per character on
/// both sides of the comparison — so `ß` folds to itself and a search for `SS`
/// is not expected to find it.
fn simple_fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// The `simple_fold` counterpart, applied only where the pair round-trips —
/// `ß` uppercases to `SS`, so it (and `ẞ`, which folds onto it) stays put.
fn simple_upper(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) if simple_fold(u) == c => u,
        _ => c,
    }
}

/// Case-insensitive literal substring containment — what the search promises:
/// every character of the query, metacharacters included, matches only itself.
fn folded_contains(haystack: &str, needle: &str) -> bool {
    let fold = |s: &str| s.chars().map(simple_fold).collect::<String>();
    fold(haystack).contains(&fold(needle))
}

impl<R: RefLifecycle + RefBlockTree> TransitionFactory<R> for Search {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // The quick-open predicate reads the `block` matview and the
        // `block_tags` junction — Turso-only surfaces.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Search {
            query: String::new(),
        }
        .preconditions(state)
        .map(|_| {
            let adversarial: Vec<String> =
                ADVERSARIAL_QUERIES.iter().map(|s| s.to_string()).collect();
            let mut drawn: Vec<String> = Vec::new();
            for id in state.all_non_seed_block_ids() {
                if let Some(content) = state.block_content(&id) {
                    drawn.extend(query_candidates(content));
                }
            }
            drawn.sort();
            drawn.dedup();

            // The two classes get equal mass rather than sharing one pool: the
            // drawn candidates grow with the vault and would otherwise crowd
            // out the metacharacter queries, which are the only ones that
            // witness the escaping.
            let strat = if drawn.is_empty() {
                proptest::sample::select(adversarial).boxed()
            } else {
                prop_oneof![
                    1 => proptest::sample::select(adversarial),
                    1 => proptest::sample::select(drawn),
                ]
                .boxed()
            };
            (6, strat.prop_map(|query| Search { query }).boxed())
        })
    }
}

impl<R: RefLifecycle + RefBlockTree> TransitionRef<R> for Search {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, _: &mut R) {
        // Search is a pure read — no reference state changes.
    }
}

crate::cap_transition! {
    Search: SutSearch,
    where R: [ RefLifecycle + RefBlockTree ],
    |me, state, sut| {
        let query = me.query.trim().to_string();
        let hits = sut
            .quick_open_search(&me.query)
            .await
            .unwrap_or_else(|e| panic!("quick_open_search({:?}) must not error: {e:#}", me.query));

        if query.is_empty() {
            assert!(
                hits.is_empty(),
                "an empty query promises no matches, got {} hits",
                hits.len()
            );
            return;
        }

        // Soundness: every hit the reference model knows really does contain the
        // query as a literal, case-folded substring. An unescaped `%` or `_`
        // fails here — it matches blocks that never held the character.
        for hit in &hits {
            let Some(content) = state.block_content(&hit.id) else {
                continue;
            };
            assert!(
                folded_contains(content, &query),
                "quick_open_search({query:?}) returned {} whose content {content:?} does not \
                 contain the query — a pattern metacharacter was treated as a wildcard",
                hit.id
            );
            assert_eq!(
                hit.is_page_section,
                state.is_page_block(&hit.id),
                "quick_open_search({query:?}) filed {} in the wrong section",
                hit.id
            );
        }

        // Completeness, asserted per section and only where that section's
        // `LIMIT` did not truncate: with room to spare, every block the
        // reference model says matches must be in the result set.
        let (page_hits, content_hits): (BTreeSet<EntityUri>, BTreeSet<EntityUri>) = hits
            .iter()
            .fold(Default::default(), |(mut p, mut c), h| {
                if h.is_page_section {
                    p.insert(h.id.clone());
                } else {
                    c.insert(h.id.clone());
                }
                (p, c)
            });
        for id in state.all_non_seed_block_ids() {
            let Some(content) = state.block_content(&id) else {
                continue;
            };
            if !folded_contains(content, &query) {
                continue;
            }
            let (section, found, limit) = if state.is_page_block(&id) {
                ("Pages", page_hits.contains(&id), PAGES_LIMIT)
            } else {
                ("In content", content_hits.contains(&id), CONTENT_LIMIT)
            };
            let truncated = if state.is_page_block(&id) {
                page_hits.len() >= limit
            } else {
                content_hits.len() >= limit
            };
            assert!(
                found || truncated,
                "quick_open_search({query:?}) missed {id} in the {section} section: its content \
                 {content:?} contains the query and the section returned only {} of its {limit} \
                 slots, so nothing was truncated",
                if section == "Pages" { page_hits.len() } else { content_hits.len() }
            );
        }
    }
    sql_budget: |_me, _state| {
        // The two one-shot branch reads (pages + content) plus the three the
        // step's own settle performs. Measured at a stable 5; an empty query
        // short-circuits below it, and the check is an upper bound.
        ExpectedSql { reads: 5, writes: 0, ddl: 0, tolerance: 2 }
    }
}

/// `LIMIT` of the Pages branch in `QueryEngine::quick_open_search`.
const PAGES_LIMIT: usize = 20;
/// `LIMIT` of the In-content branch in `QueryEngine::quick_open_search`.
const CONTENT_LIMIT: usize = 30;
