//! Org-mode disk I/O and sync layer.
//!
//! This crate handles file watching, bidirectional sync between org files and
//! the block store, and DI wiring. Format-level concerns (parsing, rendering,
//! diffing) live in `holon-org-format` and are re-exported here for backward
//! compatibility.
//!
//! # Type System
//!
//! This crate uses the generic `Block` type from the core holon crate,
//! with org-specific fields stored in the `properties` JSON field. Extension traits
//! (`OrgDocumentExt`, `OrgBlockExt`) provide accessors for these org-specific fields.
//!
//! - `Block` (with `name` set) + `OrgDocumentExt`: Represents an org file
//! - `Block` + `OrgBlockExt`: Represents an org headline

// Format modules — re-exported from holon-org-format for backward compat.
// Internal code can use `crate::models::`, `crate::parser::`, etc. as before.
pub use holon_org_format::block_diff;
pub use holon_org_format::link_parser;
pub use holon_org_format::models;
pub use holon_org_format::org_renderer;
pub use holon_org_format::parser;

// Disk I/O modules (native only)
pub mod block_params;
#[cfg(feature = "di")]
pub mod di;
pub mod file_format;
pub mod file_io;
pub mod file_utils;
pub mod file_watcher;
#[cfg(feature = "di")]
pub mod org_sync_controller;
pub mod orgmode_event_adapter;
pub mod orgmode_sync_provider;
pub mod traits;

// Re-export key types
#[cfg(feature = "di")]
pub use di::{FileWatcherReadySignal, OrgModeConfig, OrgModeModule, OrgSyncIdleSignal};

// Core types
// Note: Block is NOT re-exported here to avoid duplicate type issues with flutter_rust_bridge
// Use holon_api::block::Block directly instead
pub use holon_filesystem::directory::{Directory, ROOT_ID};

// Extension traits for org-specific functionality (forwarded from holon-org-format)
pub use models::org_props;
pub use models::ParsedSectionContent;
pub use models::{
    find_document_id, get_block_file_path, render_document_header, BlockResolver,
    HashMapBlockResolver,
};
pub use models::{OrgBlockExt, OrgDocumentExt, SourceBlock, ToOrg};

// Traits for decoupling from storage backends
pub use traits::{BlockReader, DocumentManager};

// Sync providers and adapters
pub use block_diff::{blocks_to_map, diff_blocks, BlockDiff};
pub use file_format::OrgFormatAdapter;
pub use file_watcher::OrgFileWatcher;
pub use holon_filesystem::directory::DirectoryDataSource;
pub use org_renderer::OrgRenderer;
pub use orgmode_event_adapter::OrgModeEventAdapter;
pub use orgmode_sync_provider::OrgModeSyncProvider;
pub use parser::{parse_org_file, ParseResult};

// build_block_params for seeding default layouts (no di feature needed)
pub use block_params::build_block_params;

// File I/O utilities for org-mode files
pub use file_io::{
    delete_source_block, format_api_source_block, format_block_result, format_header_args,
    format_header_args_from_values, format_org_source_block, insert_source_block,
    update_source_block, value_to_header_arg_string,
};
