//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Org-mode format: pure parsing and rendering.
//!
//! This crate contains the format-level concerns for org-mode files:
//! - **Parsing**: `parse_org_file` converts `&str` content into typed `Block`
//!   entities.
//! - **Rendering**: `OrgRenderer` serializes `Block` entities back to org text.
//!
//! No disk I/O, no file watching, no DI wiring. Depends only on `holon-api`
//! types and pure format libraries (`orgize`, `sha2`, `hex`, `chrono`, `uuid`).
//!
//! The disk I/O and sync layer lives in `holon-orgmode`, which re-exports //
//! ALLOW(compatibility): see holon-orgmode crate header everything from this
//! crate so older import paths keep resolving.

pub mod inline_marks;
pub mod link_parser;
pub mod models;
pub mod org_renderer;
pub mod parser;

// Flat re-exports — mirrors what holon-orgmode used to export directly

pub use inline_marks::extract_inline_marks;
pub use inline_marks::render_inline_marks;
pub use models::BlockResolver;
pub use models::HashMapBlockResolver;
pub use models::OrgBlockExt;
pub use models::OrgDocumentExt;
pub use models::ParsedSectionContent;
pub use models::SourceBlock;
pub use models::ToOrg;
pub use models::find_document_id;
pub use models::get_block_file_path;
pub use models::org_props;
pub use models::render_document_header;
pub use org_renderer::OrgRenderer;
pub use parser::ParseResult;
pub use parser::parse_org_file;
