//! `FileFormatAdapter` — pluggable parse/render seam for vault file formats.
//!
//! Each external file format (org-mode, markdown, …) implements this trait
//! alongside its format crate. The vault-sync controller delegates parse and
//! render through the adapter so the same controller code works across
//! formats — `holon-orgmode` provides `OrgFormatAdapter`, a future
//! `holon-markdown` would provide `MarkdownFormatAdapter`, and so on.
//!
//! Phase 1 of `codev/specs/0006-pre-velocity-refactors.md`. The trait
//! lives here (in `holon-core`) so future format crates can implement it
//! without taking a dependency on `holon-orgmode`.

use anyhow::Result;
use holon_api::block::Block;
use holon_api::EntityUri;
use std::path::Path;

/// Result of parsing a structured-text file. Format-neutral.
///
/// The same shape works for org files (where `blocks_needing_ids` are
/// headlines without `:ID:` properties) and for markdown files (where they
/// could be sections without frontmatter `id:` keys, etc.).
pub struct FileFormatParseResult {
    /// The document-level block (file-as-entity).
    pub document: Block,
    /// All blocks parsed from the file, in tree order.
    pub blocks: Vec<Block>,
    /// Block IDs that need an identity property added back to the source on
    /// the next write — the controller uses this hint to decide whether
    /// re-rendering after parse is required to persist freshly assigned IDs.
    pub blocks_needing_ids: Vec<String>,
}

/// Pluggable parse + render adapter for a single vault file format.
///
/// Implementors are stateless wrappers around the format crate's free
/// functions (`parse_org_file`, `OrgRenderer::render_document`, …). Hold them
/// behind `Arc<dyn FileFormatAdapter>` in the sync controller.
pub trait FileFormatAdapter: Send + Sync {
    /// File extensions this adapter handles, lowercase, without leading dot
    /// (e.g. `&["org"]`, `&["md", "markdown"]`). The vault watcher uses this
    /// to route each on-disk path to the right adapter.
    fn extensions(&self) -> &'static [&'static str];

    /// Parse a file's contents into a document + blocks.
    ///
    /// `path` is the absolute path of the file on disk. `parent_dir_id` is
    /// the EntityUri of the directory entity the file belongs to. `root` is
    /// the vault root used to derive relative paths and stable file IDs.
    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult>;

    /// Render a complete file: document header + all blocks. Returns the
    /// exact bytes that should be written to disk.
    fn render_document(
        &self,
        document: &Block,
        blocks: &[Block],
        file_path: &Path,
        file_id: &EntityUri,
    ) -> String;

    /// Render only the block tree, without document header. Used when the
    /// controller has blocks but no `Block` for the document entity itself
    /// (e.g. during initialization before the document row is loaded).
    fn render_blocks(&self, blocks: &[Block], file_path: &Path, file_id: &EntityUri) -> String;
}
