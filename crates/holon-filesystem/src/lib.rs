//! @c4 component
//! @c4 layer Core
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! FileSystem + FileChangeSource ports with in-memory and notify-based adapters (ADR 0011).
//!
//! This crate provides the filesystem port traits and their adapters used by other Holon crates.

pub mod change_source;
pub mod directory;
pub mod error;
pub mod file;
pub mod file_sync_controller;
pub mod fs_port;
pub mod in_memory;
pub mod sync_base_store;
pub mod sync_conflict;
pub mod sync_ports;

pub use change_source::{FileChange, FileChangeKind, FileChangeSource, NotifyWatcher};
pub use directory::{ChangesWithMetadata, DirectoryChangeProvider, DirectoryDataSource};
pub use directory::{Directory, ROOT_ID};
pub use error::FilesystemError;
pub use file::File;
pub use file_sync_controller::{FileSyncController, RENDERER_VERSION};
pub use fs_port::{FileMeta, FileSystem, RealFileSystem, ScannedEntries};
pub use in_memory::InMemoryFileSystem;
pub use sync_base_store::{BaseKey, BaseStore, SyncBaseStore};
pub use sync_conflict::{
    conflict_artifacts_error, find_sync_conflict_artifacts, is_sync_conflict_artifact,
};
pub use sync_ports::{AliasRegistrar, BlockReader, DocumentManager, ImageDataProvider};

use std::path::Path;

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
