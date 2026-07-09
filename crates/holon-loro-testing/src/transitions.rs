//! Loro subsystem PBT transitions — the peer-sync / CRDT generators, co-located
//! with `holon-loro` (Phase-1a Step 4).
//!
//! Each transition is generic over the reference type `R` (bound on the
//! fine-grained `Ref*` capabilities it drives) and over the SUT cap `S`, so it
//! carries no dependency on the central `ReferenceState` monolith. The central
//! `declare_e2e_transitions!` enum re-exports these structs and assembles them
//! into `E2ETransition` (the strategy stays enum-driven for now).

pub mod add_peer;
pub mod create_stale_loro;
pub mod merge_from_peer;
pub mod peer_char_edit;
pub mod peer_edit;
pub mod sync_with_peer;

pub use add_peer::AddPeer;
pub use create_stale_loro::CreateStaleLoro;
pub use merge_from_peer::MergeFromPeer;
pub use peer_char_edit::PeerCharEdit;
pub use peer_edit::PeerEdit;
pub use sync_with_peer::SyncWithPeer;
