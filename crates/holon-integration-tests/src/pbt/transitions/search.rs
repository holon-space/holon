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

use std::collections::BTreeMap;
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
    // Many-to-one folds: a two-element [lower upper] class hid every OTHER
    // character sharing the same fold, e.g. capital ẞ was unreachable from a
    // "ß" query. See fold_class_tests in query_engine.rs for the BMP-wide proof;
    // these keep the SAME family visible to the keystone's own generated draws.
    "ß",
    "ẞ",
    "ǅ",
    "ǆ",
    "Ǆ",
    "\u{212A}",
    "\u{2126}",
    "\u{212B}",
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

        // Every oracle block paired with the id the SUT minted for it. The two
        // spaces part company on the blocks a transition minted (a split tail is
        // `block::split-N` to the model, a created page `block:ref-doc-N`), so
        // both loops below compare through this pairing, never raw ids.
        let model: Vec<(EntityUri, EntityUri)> = state
            .all_non_seed_block_ids()
            .into_iter()
            .map(|id| {
                let sut_id = sut.resolve_block_id(&id);
                (id, sut_id)
            })
            .collect();
        let oracle_of = reverse_pairing(&model);

        // Soundness: every hit the reference model knows really does contain the
        // query as a literal, case-folded substring. An unescaped `%` or `_`
        // fails here — it matches blocks that never held the character.
        for hit in &hits {
            let oracle = oracle_of.get(&hit.id).unwrap_or(&hit.id);
            let Some(content) = state.block_content(oracle) else {
                continue;
            };
            assert!(
                folded_contains(content, &query),
                "quick_open_search({query:?}) returned {} (model id {oracle}) whose content \
                 {content:?} does not contain the query — a pattern metacharacter was treated as \
                 a wildcard",
                hit.id
            );
            assert_eq!(
                hit.is_page_section,
                state.is_page_block(oracle),
                "quick_open_search({query:?}) filed {} (model id {oracle}) in the wrong section",
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
        for (id, sut_id) in &model {
            let Some(content) = state.block_content(id) else {
                continue;
            };
            if !folded_contains(content, &query) {
                continue;
            }
            let (section, found, limit) = if state.is_page_block(id) {
                ("Pages", page_hits.contains(sut_id), PAGES_LIMIT)
            } else {
                ("In content", content_hits.contains(sut_id), CONTENT_LIMIT)
            };
            let truncated = if state.is_page_block(id) {
                page_hits.len() >= limit
            } else {
                content_hits.len() >= limit
            };
            let sut_note = if sut_id == id {
                String::new()
            } else {
                format!(" (SUT id {sut_id})")
            };
            assert!(
                found || truncated,
                "quick_open_search({query:?}) missed {id}{sut_note} in the {section} section: its \
                 content {content:?} contains the query and the section returned only {} of its \
                 {limit} slots, so nothing was truncated",
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

/// Reverse the model's id pairing into SUT id -> oracle id, so a returned hit's
/// content can be read back through the model.
///
/// The side map is synthetic->real and injective at reconcile time: a
/// self-mapped id is never a pair's value, and pair values are minted once. A
/// violation would silently read the WRONG block's content, so it panics naming
/// both model ids.
fn reverse_pairing(model: &[(EntityUri, EntityUri)]) -> BTreeMap<EntityUri, EntityUri> {
    let mut oracle_of = BTreeMap::new();
    for (id, sut_id) in model {
        if let Some(prev) = oracle_of.insert(sut_id.clone(), id.clone()) {
            panic!(
                "two model ids resolve to one SUT id {sut_id}: {prev} and {id} — the reconcile's \
                 synthetic->real pairing is not injective, so a hit's content cannot be read back"
            );
        }
    }
    oracle_of
}

/// `LIMIT` of the Pages branch in `QueryEngine::quick_open_search`.
const PAGES_LIMIT: usize = 20;
/// `LIMIT` of the In-content branch in `QueryEngine::quick_open_search`.
const CONTENT_LIMIT: usize = 30;

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(model: &str, sut: &str) -> (EntityUri, EntityUri) {
        (EntityUri::block(model), EntityUri::block(sut))
    }

    /// A collision must name BOTH model ids. The message is formatted AFTER the
    /// insert, so reading the displaced value back out of the map would print
    /// the second id twice and leave the panic undiagnosable.
    #[test]
    fn reverse_pairing_names_both_model_ids_on_a_collision() {
        let model = vec![pair("ref-doc-0", "real-1"), pair("ref-doc-1", "real-1")];
        let msg = std::panic::catch_unwind(|| reverse_pairing(&model))
            .expect_err("a non-injective pairing must not resolve silently")
            .downcast_ref::<String>()
            .expect("panic payload is a String")
            .clone();
        assert!(
            msg.contains("block:ref-doc-0"),
            "names the displaced model id: {msg}"
        );
        assert!(
            msg.contains("block:ref-doc-1"),
            "names the colliding model id: {msg}"
        );
        assert!(
            msg.contains("block:real-1"),
            "names the shared SUT id: {msg}"
        );
    }

    /// A born-equal id self-maps to itself, which is the pass-through case the
    /// soundness loop's `unwrap_or` relies on.
    #[test]
    fn reverse_pairing_maps_a_minted_id_back_and_self_maps_a_born_equal_one() {
        let model = vec![pair("ref-doc-0", "real-1"), pair("renhost", "renhost")];
        let oracle_of = reverse_pairing(&model);
        assert_eq!(
            oracle_of[&EntityUri::block("real-1")],
            EntityUri::block("ref-doc-0")
        );
        assert_eq!(
            oracle_of[&EntityUri::block("renhost")],
            EntityUri::block("renhost")
        );
    }
}
