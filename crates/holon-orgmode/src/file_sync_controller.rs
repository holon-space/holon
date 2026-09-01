//! Org-default construction for the format-agnostic file-sync engine.
//!
//! The engine itself (`FileSyncController`) now lives in `holon_filesystem` and
//! knows nothing about org-mode. This module is the thin org-side wiring that
//! picks `OrgFormatAdapter` as the file format — the convenience ctor formerly
//! known as `FileSyncController::new`.

use std::path::PathBuf;
use std::sync::Arc;

use holon_core::block_ordering::BlockOrdering;
use holon_core::file_format::FormatRegistry;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_filesystem::FileSystem;

use crate::file_format::OrgFormatAdapter;

/// A registry holding org alone — the single-format vault.
pub fn org_only_format_registry() -> Arc<FormatRegistry> {
    Arc::new(
        FormatRegistry::new(vec![Arc::new(OrgFormatAdapter::new())])
            .expect("one adapter cannot contest its own extensions"),
    )
}

/// Construct a `FileSyncController` over an org-only format registry.
pub fn new_org_sync_controller(
    block_reader: Arc<dyn BlockReader>,
    doc_manager: Arc<dyn DocumentManager>,
    root_dir: PathBuf,
    ordering: Arc<dyn BlockOrdering>,
    fs: Arc<dyn FileSystem>,
) -> FileSyncController {
    FileSyncController::with_formats(
        block_reader,
        doc_manager,
        root_dir,
        org_only_format_registry(),
        ordering,
        fs,
    )
}
