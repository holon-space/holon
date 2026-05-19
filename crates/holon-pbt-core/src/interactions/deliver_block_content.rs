//! Variant: deliver deferred `live_block` content (async data arrival).

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DeliverBlockContent {
    pub block_id: String,
}
