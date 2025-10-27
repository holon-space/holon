//! Org-default construction for the format-agnostic file-sync engine.
//!
//! The engine itself (`FileSyncController`) now lives in `holon_filesystem` and
//! knows nothing about org-mode. This module is the thin org-side wiring that
//! picks `OrgFormatAdapter` as the file format — the convenience ctor formerly
//! known as `FileSyncController::new`.

use std::path::PathBuf;
use std::sync::Arc;

use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::{BlockReader, DocumentManager, FileSyncController, FileSystem};

use crate::file_format::OrgFormatAdapter;

/// Construct a `FileSyncController` wired with the org-mode format adapter.
pub fn new_org_sync_controller(
    block_reader: Arc<dyn BlockReader>,
    doc_manager: Arc<dyn DocumentManager>,
    root_dir: PathBuf,
    ordering: Arc<dyn BlockOrdering>,
    fs: Arc<dyn FileSystem>,
) -> FileSyncController {
    FileSyncController::with_format(
        block_reader,
        doc_manager,
        root_dir,
        Arc::new(OrgFormatAdapter::new()),
        ordering,
        fs,
    )
}
