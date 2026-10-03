use holon_api::EntityUri;
use holon_api::commit_clock::CommitSource;
use holon_api::commit_clock::CoverAboveHighWater;
use holon_api::commit_clock::Stamp;

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
    #[error("block {id} encodes to no payload: {reason}")]
    UnencodableBlock { id: EntityUri, reason: String },
    #[error("{store:?} fed {changed} changed rows with no outstanding commit, such as {sample:?}")]
    StamplessChange {
        store: CommitSource,
        changed: usize,
        /// The keys of the changed rows: a block URI or a navigation history
        /// id.
        sample: Vec<String>,
    },
    #[error("{store:?} commit {stamp:?} was stamped, but its feed was never completed")]
    TornCommit { store: CommitSource, stamp: Stamp },
    #[error("{store:?} delivered a commit out of its envelope: {reason}")]
    CommitEnvelope { store: CommitSource, reason: String },
    #[error("{store:?} delivered a row that does not read as its relation's row: {reason}")]
    UnreadableRow { store: CommitSource, reason: String },
    #[error("{store:?} delivered changes to one key that net to no single row: {reason}")]
    ConflictingChanges { store: CommitSource, reason: String },
    #[error("the engine thread panicked: {0}")]
    Panicked(String),
}
