//! Loro CRDT document engine and peer-to-peer synchronization.
//!
//! This crate provides the Loro CRDT backend, P2P sync infrastructure,
//! and snapshot/block query capabilities — extracted from the `holon` god crate.
//!
//! Re-exported into `holon::sync` so existing `holon::sync::*` paths resolve.

pub mod block_cell_registry;
pub mod capability;
pub mod consolidator;
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
pub mod link_event_subscriber;
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
#[cfg(any(test, feature = "test-helpers"))]
pub mod multi_peer;
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
pub mod text_merge_provider;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod ticket;

pub use capability::{CapabilityProfile, Consolidator, SessionCapabilities};
pub use consolidator::BlockConsolidator;
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use degraded_signal_bus::{DegradedSignalBus, ShareDegraded, ShareDegradedReason};
pub use event_bus::*;
pub use event_ring::{DEFAULT_EVENT_RING_CAPACITY, EventRing, deliver_to_subscribers};
pub use holon_api::EntityUri;
pub use holon_core::CanonicalPath;
pub use holon_filesystem::{BaseKey, BaseStore, SyncBaseStore};
#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub use iroh_sync_adapter::IrohSyncAdapter;
pub use live_value::LiveValue;
pub use loro_backend::{
    CONTENT_RAW, CONTENT_TYPE, EXTERNAL_ID, LoroBackend, LoroMapExt, LoroTreeView, SOURCE_CODE,
    SOURCE_LANGUAGE, STABLE_ID, SnapshotBlock, TREE_NAME, configure_text_styles,
    mark_from_loro_value, mark_to_loro_value, read_marks_from_text, snapshot_blocks_from_doc,
    snapshot_blocks_from_doc_settled,
};
pub use loro_block_operations::LoroBlockOperations;
pub use loro_blocks_datasource::LoroBlocksDataSource;
pub use loro_document::*;
pub use loro_document_store::*;
pub use loro_sync_controller::{
    LoroProjection, LoroSyncController, LoroSyncControllerHandle, SinkReader, block_to_params,
};
pub use text_merge_provider::{TextHandle, TextMergeProvider};
