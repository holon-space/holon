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

/// `block "<id>" is child <n> of block "<parent>"` — position among the
/// parent's children, **1-based** (child 1 is the first). The ordinal the
/// LogSeq corpus asserts as "the Nth bullet".
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} is child {index} of block {parent_id}")]
pub struct BlockIsNthChild {
    pub block_id: EntityUri,
    pub index: usize,
    pub parent_id: EntityUri,
}

/// `block "<id>" comes after block "<other>"` — relative sibling order. Both
/// must share a parent; asserting across parents is a step-authoring error and
/// fails loud rather than comparing incomparable positions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} comes after block {other_id}")]
pub struct BlockComesAfter {
    pub block_id: EntityUri,
    pub other_id: EntityUri,
}

/// `block "<id>" has task state "<state>"` — the org keyword in the block's
/// `properties` bag. The renderer draws a GLYPH rather than the keyword, so no
/// rendered-substring assertion can express this.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} has task state {state}")]
pub struct BlockHasTaskState {
    pub block_id: EntityUri,
    pub state: String,
}

/// `block "<id>" has no task state` — the block is not a task. Both storage
/// encodings of "not a task" satisfy it: the property absent, and the property
/// present but empty (which is what clearing the state leaves behind).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} has no task state")]
pub struct BlockHasNoTaskState {
    pub block_id: EntityUri,
}

/// `block "<id>" is collapsed` — the persisted `block_raw.collapsed` flag,
/// document state that survives restart (not per-device view state). Says
/// nothing about whether the subtree is still painted.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} is collapsed")]
pub struct BlockIsCollapsed {
    pub block_id: EntityUri,
}

/// `block "<id>" is not collapsed` — the negative of [`BlockIsCollapsed`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} is not collapsed")]
pub struct BlockIsNotCollapsed {
    pub block_id: EntityUri,
}

/// `block "<src>" resolves link "<target>" to block "<dst>"` — the
/// `block_links` row for `target` carries `resolved_id = dst`. A resolved and
/// a dangling reference RENDER identically, so this is the only way to tell
/// them apart.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} resolves link {target} to block {resolved_id}")]
pub struct BlockResolvesLink {
    pub block_id: EntityUri,
    pub target: String,
    pub resolved_id: EntityUri,
}

/// `block "<src>" has a dangling link "<target>"` — the row exists but
/// `resolved_id` is NULL. The negative half, so a scenario can pin that a
/// reference to a nonexistent page does NOT silently acquire a target.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("block {block_id} has a dangling link {target}")]
pub struct BlockHasDanglingLink {
    pub block_id: EntityUri,
    pub target: String,
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
        (
            "BlockIsNthChild",
            <BlockIsNthChild as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockComesAfter",
            <BlockComesAfter as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockHasTaskState",
            <BlockHasTaskState as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockHasNoTaskState",
            <BlockHasNoTaskState as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockIsCollapsed",
            <BlockIsCollapsed as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockIsNotCollapsed",
            <BlockIsNotCollapsed as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockResolvesLink",
            <BlockResolvesLink as StepVocabulary>::TEMPLATE,
        ),
        (
            "BlockHasDanglingLink",
            <BlockHasDanglingLink as StepVocabulary>::TEMPLATE,
        ),
    ]
}
