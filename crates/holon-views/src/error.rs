/// More rounds than any real outline needs; the bound SQL's ancestry walk uses
/// too. A recursion still deriving rows past it fails loud.
pub const MAX_DEPTH: u64 = 1000;

use holon_api::EntityUri;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("a recursion still derived rows after {MAX_DEPTH} rounds")]
    DepthBound,
    #[error("block {id} has a NaN or infinite float in property {key:?}")]
    NonFiniteFloat { id: EntityUri, key: String },
}
