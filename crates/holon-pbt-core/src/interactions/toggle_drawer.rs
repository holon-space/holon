//! Variant: toggle a drawer's open/closed state.

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    holon_macros::StepVocabulary,
)]
#[step_template("I toggle drawer {block_id}")]
pub struct ToggleDrawer {
    /// The drawer's block_id (e.g. `"block:default-left-sidebar"`).
    pub block_id: String,
}
