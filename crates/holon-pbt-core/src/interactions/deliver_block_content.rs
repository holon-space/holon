//! Variant: deliver deferred `live_block` content (async data arrival).

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
#[step_template("block content for {block_id} is delivered")]
pub struct DeliverBlockContent {
    pub block_id: String,
}
