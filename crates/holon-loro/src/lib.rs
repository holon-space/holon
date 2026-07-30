//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-filesystem "filesystem ports" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Loro CRDT document engine and peer-to-peer synchronization.
//!
//! This crate provides the Loro CRDT backend, P2P sync infrastructure,
//! and snapshot/block query capabilities — extracted from the `holon` god
//! crate.
//!
//! Re-exported into `holon::sync` so existing `holon::sync::*` paths resolve.

pub mod block_cell_registry;
pub mod capability;
pub mod consolidator;
pub mod container_registry;
pub mod durable_state;

#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod debounced_commit_worker;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod degraded_signal_bus;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod device_key_store;
/// A2 SAS device-pairing ceremony (ADR 0028 A2). Pure state machine, no iroh.
pub mod device_pairing;
pub mod event_bus;
pub mod event_ring;

#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod iroh_advertiser;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod iroh_sync_adapter;
pub mod live_value;
pub mod loro_backend;
pub mod loro_block_operations;
pub mod loro_blocks_datasource;
pub mod loro_document;
pub mod loro_document_store;
pub mod loro_meta_cell_backing;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod loro_share_backend;
pub mod loro_sync_controller;
pub mod loro_text_cell_backing;
pub mod mergeable_child;
#[cfg(any(test, feature = "test-helpers"))]
pub mod multi_peer;
/// Owner-identity key (ADR 0028 D1/OQ4). Ungated: the owner key signs durable
/// authority objects (rosters, the crossing log) even without the iroh
/// transport, so `holon-sharing` can back its `SigningAuthority` with it.
pub mod owner_identity;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod roster_sidecar;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod share_enrollment;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod share_peer_id;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod shared_snapshot_store;
pub mod shared_tree;
/// The transport seam every true-sharing backend implements. Not iroh-gated and
/// pure-Rust: Android builds it.
pub mod sync_transport;
pub mod text_merge_provider;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod ticket;

pub use capability::CapabilityProfile;
pub use capability::Consolidator;
pub use capability::SessionCapabilities;
pub use consolidator::BlockConsolidator;
pub use container_registry::ContainerRegistry;
pub use container_registry::RegisteredContainer;
pub use container_registry::SubtreeIndex;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::DegradedChange;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::DegradedConditionKey;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::DegradedSignalBus;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::DegradedSubscription;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::ShareDegraded;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::ShareDegradedReason;
pub use event_bus::*;
pub use event_ring::DEFAULT_EVENT_RING_CAPACITY;
pub use event_ring::EventRing;
pub use event_ring::deliver_to_subscribers;
pub use holon_api::EntityUri;
pub use holon_core::CanonicalPath;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use iroh_sync_adapter::IrohSyncAdapter;
pub use live_value::LiveValue;
pub use loro_backend::CONTENT_RAW;
pub use loro_backend::CONTENT_TYPE;
pub use loro_backend::EXTERNAL_ID;
pub use loro_backend::LoroBackend;
pub use loro_backend::LoroMapExt;
pub use loro_backend::LoroTreeView;
pub use loro_backend::PendingChange;
pub use loro_backend::SOURCE_CODE;
pub use loro_backend::SOURCE_LANGUAGE;
pub use loro_backend::STABLE_ID;
pub use loro_backend::SnapshotBlock;
pub use loro_backend::TREE_NAME;
pub use loro_backend::build_tid_index;
pub use loro_backend::configure_text_styles;
pub use loro_backend::extract_pending_changes;
pub use loro_backend::incremental_block_changes;
pub use loro_backend::mark_from_loro_value;
pub use loro_backend::mark_to_loro_value;
pub use loro_backend::read_marks_from_text;
pub use loro_backend::snapshot_blocks_from_doc;
pub use loro_backend::snapshot_blocks_from_doc_settled;
pub use loro_block_operations::LoroBlockOperations;
pub use loro_blocks_datasource::LoroBlocksDataSource;
pub use loro_document::*;
pub use loro_document_store::*;
pub use loro_sync_controller::LoroProjection;
pub use loro_sync_controller::LoroSyncController;
pub use loro_sync_controller::LoroSyncControllerHandle;
pub use loro_sync_controller::SinkReader;
pub use loro_sync_controller::block_to_params;
pub use loro_sync_controller::projection_stats;
pub use text_merge_provider::TextHandle;
pub use text_merge_provider::TextMergeProvider;
pub use text_merge_provider::TransientLoroTextMerge;
