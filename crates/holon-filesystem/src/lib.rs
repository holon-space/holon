//! @c4 component
//! @c4 layer Core
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! FileSystem + FileChangeSource ports with in-memory and notify-based adapters
//! (ADR 0011).
//!
//! This crate provides the filesystem port traits and their adapters used by
//! other Holon crates.

// Native-only half: filesystem/watcher adapters and the file-sync controller
// (tokio::fs / tokio::process / notify are rejected on wasm targets). The wasm
// surface is the port traits + sync base store consumed by holon-loro.
#[cfg(not(target_arch = "wasm32"))]
pub mod change_source;
pub mod error;
pub mod file;
#[cfg(not(target_arch = "wasm32"))]
pub mod file_sync_controller;
#[cfg(not(target_arch = "wasm32"))]
pub mod fs_port;
#[cfg(not(target_arch = "wasm32"))]
pub mod in_memory;
#[cfg(not(target_arch = "wasm32"))]
pub mod ingest_progress;
pub mod sync_base_store;
pub mod sync_conflict;
pub mod sync_ports;
pub mod vault_path;
#[cfg(not(target_arch = "wasm32"))]
pub mod writeback_render;

use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
pub use change_source::FileChange;
#[cfg(not(target_arch = "wasm32"))]
pub use change_source::FileChangeKind;
#[cfg(not(target_arch = "wasm32"))]
pub use change_source::FileChangeSource;
#[cfg(not(target_arch = "wasm32"))]
pub use change_source::NotifyWatcher;
#[cfg(not(target_arch = "wasm32"))]
pub use change_source::RawFsSignal;
#[cfg(not(target_arch = "wasm32"))]
pub use change_source::RenamePairing;
pub use error::FilesystemError;
pub use file::ChangesWithMetadata;
pub use file::File;
#[cfg(not(target_arch = "wasm32"))]
pub use file_sync_controller::BlockDelta;
#[cfg(not(target_arch = "wasm32"))]
pub use file_sync_controller::FileSyncController;
#[cfg(not(target_arch = "wasm32"))]
pub use file_sync_controller::RENDERER_VERSION;
#[cfg(not(target_arch = "wasm32"))]
pub use file_sync_controller::tiered_match;
#[cfg(not(target_arch = "wasm32"))]
pub use fs_port::FileMeta;
#[cfg(not(target_arch = "wasm32"))]
pub use fs_port::FileSystem;
#[cfg(not(target_arch = "wasm32"))]
pub use fs_port::RealFileSystem;
#[cfg(not(target_arch = "wasm32"))]
pub use fs_port::ScannedEntries;
#[cfg(not(target_arch = "wasm32"))]
pub use in_memory::InMemoryFileSystem;
pub use sync_base_store::BaseKey;
pub use sync_base_store::BaseStore;
pub use sync_base_store::SyncBaseStore;
pub use sync_conflict::conflict_artifacts_error;
pub use sync_conflict::find_sync_conflict_artifacts;
pub use sync_conflict::is_sync_conflict_artifact;
pub use sync_ports::AliasRegistrar;
pub use sync_ports::BlockReader;
pub use sync_ports::BlockRowMemo;
pub use sync_ports::DocumentManager;
pub use sync_ports::ExistingChild;
pub use sync_ports::ImageDataProvider;
pub use sync_ports::IncomingIdentity;
pub use sync_ports::MatchBasis;
pub use sync_ports::MatchVerdict;
pub use sync_ports::MemoSeam;
pub use sync_ports::MountRegistry;
pub use sync_ports::ShareWritebackDisclosure;
pub use sync_ports::ThreeWayTextMerge;
pub use sync_ports::WritebackDisclosure;
pub use sync_ports::nearest_page_ancestor;
pub use vault_path::VaultPath;
#[cfg(not(target_arch = "wasm32"))]
pub use writeback_render::WritebackRenderer;

/// Filesystem utilities
pub struct Filesystem;

impl Filesystem {
    /// Check if a path exists
    pub fn exists<P: AsRef<Path>>(path: P) -> bool {
        path.as_ref().exists()
    }

    /// Check if a path is a directory
    pub fn is_dir<P: AsRef<Path>>(path: P) -> bool {
        path.as_ref().is_dir()
    }

    /// Check if a path is a file
    pub fn is_file<P: AsRef<Path>>(path: P) -> bool {
        path.as_ref().is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filesystem_exists() {
        // Test with a path that should exist (current directory)
        assert!(Filesystem::exists("."));
    }
}
