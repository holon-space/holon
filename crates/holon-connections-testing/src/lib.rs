//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//! @c4 uses holon-connections "remote-list peer port & sync round" "Rust"
//! @c4 uses holon-core "typed row-set projection" "Rust"
//! @c4 uses holon-api "shared value & operation types" "Rust"
//!
//! In-process fixture peers for the generic remote-list reconciler.
//!
//! The production connector owns the round, the reconciler and the intents; the
//! one thing it cannot own is a transport. This crate stands in for that
//! transport so the REAL [`holon_connections::sync_once`] round can be driven
//! in a test, over peers that differ along the axes that distinguish real list
//! APIs.
//!
//! A second list system is covered by declaring its own sidecar block; a second
//! peer behaviour is covered by picking another [`FixtureProfile`]. Nothing
//! here names a product.

pub mod axes;
pub mod fakes;
pub mod peer;

pub use axes::CacheMode;
pub use axes::ChangeDetection;
pub use axes::CommitGranularity;
pub use axes::FixtureProfile;
pub use axes::KeyShape;
pub use fakes::Fake;
pub use fakes::content_keyed_snapshot_cache_bust;
pub use fakes::fakes;
pub use fakes::id_keyed_cursor;
pub use peer::FETCHED_AT;
pub use peer::FixtureList;
pub use peer::FixtureListPeer;
pub use peer::FixtureRows;
pub use peer::ListMutation;
pub use peer::apply_local_intents;
