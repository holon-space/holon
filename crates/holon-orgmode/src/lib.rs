//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon "core orchestration" "Rust"
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-filesystem "filesystem ports" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//! @c4 uses holon-org-format "org parse/render" "Rust"
//!
//! Org-mode disk I/O and sync layer.
//!
//! This crate handles file watching, bidirectional sync between org files and
//! the block store, and DI wiring. Format-level concerns (parsing, rendering,
//! diffing) live in `holon-org-format` and are re-exported here so external //
//! ALLOW(compatibility): re-export bridge during the org-format split
//! callers keep working through the crate split.
//!
//! # Type System
//!
//! This crate uses the generic `Block` type from the core holon crate,
//! with org-specific fields stored in the `properties` JSON field. Extension
//! traits (`OrgDocumentExt`, `OrgBlockExt`) provide accessors for these
//! org-specific fields.
//!
//! - `Block` (with `name` set) + `OrgDocumentExt`: Represents an org file
//! - `Block` + `OrgBlockExt`: Represents an org headline

// Format modules — re-exported from holon-org-format for backward compat.
// Internal code can use `crate::models::`, `crate::parser::`, etc. as before.
pub use holon_org_format::dense;
pub use holon_org_format::inline_marks;
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
#[cfg(feature = "di")]
pub mod file_sync_controller;
pub mod file_watcher;
pub mod orgmode_sync_provider;
pub mod writeback_guard;

// Re-export key types
// build_block_params for seeding default layouts (no di feature needed)
pub use block_params::build_block_params;
#[cfg(feature = "di")]
pub use di::FileWatcherReadySignal;
#[cfg(feature = "di")]
pub use di::OrgModeConfig;
#[cfg(feature = "di")]
pub use di::OrgSyncIdleSignal;
// Sync providers and adapters
pub use file_format::OrgFormatAdapter;
// File I/O utilities for org-mode files
pub use file_io::{
    delete_source_block, format_api_source_block, format_block_result, format_header_args,
    format_header_args_from_values, format_org_source_block, insert_source_block,
    update_source_block, value_to_header_arg_string,
};
pub use file_watcher::OrgFileWatcher;
// Core types
// Note: Block is NOT re-exported here to avoid duplicate type issues with flutter_rust_bridge
// Use holon_api::block::Block directly instead
pub use models::BlockResolver;
pub use models::HashMapBlockResolver;
pub use models::OrgBlockExt;
pub use models::OrgDocumentExt;
pub use models::ParsedSectionContent;
pub use models::SourceBlock;
pub use models::ToOrg;
pub use models::find_document_id;
pub use models::get_block_file_path;
// Extension traits for org-specific functionality (forwarded from holon-org-format)
pub use models::org_props;
pub use models::render_document_header;
pub use org_renderer::OrgRenderer;
pub use orgmode_sync_provider::OrgModeSyncProvider;
pub use parser::ParseResult;
pub use parser::parse_org_file;
