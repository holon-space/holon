//! Org-mode format: pure parsing, rendering, and diffing.
//!
//! This crate contains the format-level concerns for org-mode files:
//! - **Parsing**: `parse_org_file` converts `&str` content into typed `Block` entities.
//! - **Rendering**: `OrgRenderer` serializes `Block` entities back to org text.
//! - **Diffing**: `diff_blocks` computes the delta between two block sets.
//!
//! No disk I/O, no file watching, no DI wiring. Depends only on `holon-api`
//! types and pure format libraries (`orgize`, `sha2`, `hex`, `chrono`, `uuid`).
//!
//! The disk I/O and sync layer lives in `holon-orgmode`, which re-exports
//! everything from this crate for backward compatibility.

pub mod block_diff;
pub mod inline_marks;
pub mod link_parser;
pub mod models;
pub mod org_renderer;
pub mod parser;

// Flat re-exports — mirrors what holon-orgmode used to export directly

pub use block_diff::{blocks_to_map, diff_blocks, BlockDiff};
pub use inline_marks::{extract_inline_marks, render_inline_marks};
pub use models::org_props;
pub use models::ParsedSectionContent;
pub use models::{
    find_document_id, get_block_file_path, render_document_header, BlockResolver,
    HashMapBlockResolver,
};
pub use models::{OrgBlockExt, OrgDocumentExt, SourceBlock, ToOrg};
pub use org_renderer::OrgRenderer;
pub use parser::{parse_org_file, ParseResult};
