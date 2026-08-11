//! `Then`-side step templates, authored in the SAME derive as the action
//! vocabulary (`#[derive(StepVocabulary)]` + `#[step_template]`).
//!
//! The derive is deliberately generic over "annotated struct", not over
//! transitions (`holon-macros/src/step_vocabulary.rs:24`): the transition
//! coupling lives entirely in the `transition_dispatch!` macro, which builds
//! the `E2ETransition` enum. An assertion struct derives the identical
//! renderer/parser without touching that macro, so a Then step gets the same
//! compile-time placeholder checking and the same quoting rules as a When step.
//!
//! `matchers::match_assertion` tries these templates before its remaining
//! regexes; the `within <N> seconds ` prefix is stripped by the matcher, so a
//! template here describes the bare assertion only.

use holon_api::EntityUri;

/// `block "<child>" is a child of block "<parent>"`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {child_id} is a child of block {parent_id}")]
pub struct BlockIsChildOf {
    pub child_id: EntityUri,
    pub parent_id: EntityUri,
}

/// `block "<id>" is a top-level block of "<page>"` — the sibling phrasing for
/// the same relation, read at the page root. Same oracle: a top-level block of
/// a page IS a block whose parent is that page.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} is a top-level block of {page_id}")]
pub struct BlockIsTopLevelOf {
    pub block_id: EntityUri,
    pub page_id: EntityUri,
}

/// `(name, template)` for every assert-side step — the input to the
/// registration-time ambiguity refusal, mirroring
/// `E2ETransition::step_catalog`.
pub fn assert_step_catalog() -> Vec<(&'static str, &'static str)> {
    use holon_pbt_core::step_vocabulary::StepVocabulary;
    vec![
        (
            "BlockIsChildOf",
            <BlockIsChildOf as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockIsTopLevelOf",
            <BlockIsTopLevelOf as StepVocabulary>::TEMPLATE,
        ),
    ]
}
