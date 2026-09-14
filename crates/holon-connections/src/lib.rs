//! @c4 component
//! @c4 layer Adapters
//! Pattern: Anti-Corruption Layer
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-rows "typed row JSON Lines and jaq mappings" "Rust"
//!
//! Low-code connections to remote LISTS: one reconciler, one sync round and one
//! operation, parameterised by what a sidecar declares.
//!
//! A second system costs a UTCP manual, a `holon.list_sync` block and two jaq
//! mappings — no Rust. Identity is the declared `key` expression, so a peer
//! that issues row ids (`.id`) and one that does not (`[.name, .cat]`) reach
//! the same code (ADR 0034).
//!
//! No transport lives here: the peer is a trait, implemented over whatever call
//! surface a connection has. That keeps the reconcile, sync and intent legs in
//! the wasm graph, where an HTTP client is not.

pub mod intent;
pub mod provider;
pub mod reconcile;
pub mod registry;
pub mod snapshot;
pub mod spec;
pub mod sync;

pub use intent::local_intent_operation;
pub use provider::REMOTE_LIST_SYNC;
pub use provider::RemoteListOperations;
pub use provider::RoundClock;
pub use provider::SystemRoundClock;
pub use reconcile::LocalIntent;
pub use reconcile::PushIntent;
pub use reconcile::ReconcileOutcome;
pub use reconcile::RemoteListReconciler;
pub use registry::ConfiguredList;
pub use registry::ConfiguredLists;
pub use registry::ConfiguredRemoteLists;
pub use snapshot::ListSnapshot;
pub use snapshot::LocalRow;
pub use snapshot::LocalRowReader;
pub use snapshot::RemoteRow;
pub use spec::CacheBuster;
pub use spec::CompiledListSync;
pub use spec::ListSyncSpec;
pub use spec::RowKey;
pub use sync::CommandVerb;
pub use sync::CommitAck;
pub use sync::CommitBatch;
pub use sync::CommitCommand;
pub use sync::RemoteListPeer;
pub use sync::SyncOutcome;
pub use sync::sync_once;
