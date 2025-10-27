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

use std::path::Path;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::block::Block;

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
///
/// @c4 code
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

    /// Extract the document's stable bare id from raw file content, if the
    /// format embeds one (e.g. org's `#+ID:` header). Returns `None` when the
    /// file carries no explicit identity — the controller then falls back to
    /// name-chain resolution. Cheaper than a full `parse` because the
    /// controller only needs the id to resolve the document entity before
    /// committing to a full parse.
    fn doc_id_from_content(&self, content: &str) -> Option<String>;

    /// Build the operation-params `StorageEntity` for a create/update of
    /// `block`, as handed to `OperationProvider::execute_operation`. The
    /// `document_uri` is recorded under `ROUTING_DOC_URI_KEY` so the consumer
    /// can route the op to the owning document regardless of where `parent_id`
    /// points. Format-specific because the param shape encodes the format's
    /// structured fields (org drawer properties, task state, scheduling, …).
    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
    ) -> StorageEntity;

    /// Decide whether two blocks differ in a way that warrants re-emitting an
    /// update op. Format-specific because "content-equivalent" depends on the
    /// format's structured fields (e.g. org task state / priority / scheduling
    /// read from the properties drawer). Sibling order is intentionally
    /// excluded — it is derived from document position, not a per-block field.
    fn content_differs(&self, a: &Block, b: &Block) -> bool;

    /// Reconcile format-specific document-header metadata from a freshly-parsed
    /// document block onto the persisted document entity — e.g. org's `#+TODO:`
    /// keyword config, which the parser reads from the file header but the
    /// stored entity (created via `DocumentManager`) doesn't carry, so a
    /// re-render would otherwise drop the header. Mutates `persisted` in place
    /// and returns whether it changed; the controller persists via
    /// `update_metadata` only when `true`.
    fn sync_document_metadata(&self, parsed: &Block, persisted: &mut Block) -> bool;
}
