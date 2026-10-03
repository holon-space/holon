use holon_api::EntityUri;
use holon_api::commit_clock::CoverAboveHighWater;

/// More rounds than any real outline needs, and the bound the SQL ancestry
/// walk uses too. A recursion still deriving rows past it fails loud.
pub const MAX_DEPTH: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("a recursion still derived rows after {MAX_DEPTH} rounds")]
    DepthBound,
    #[error("block {id} has a NaN or infinite float in property {key:?}")]
    NonFiniteFloat { id: EntityUri, key: String },
    #[error("the parents of {ids:?} form a cycle")]
    ParentCycle { ids: Vec<EntityUri> },
    #[error(transparent)]
    Clock(#[from] CoverAboveHighWater),
    #[error("a payload does not decode to a block: {reason}")]
    UndecodablePayload { reason: String },
    #[error("the engine thread panicked")]
    Stopped,
}
