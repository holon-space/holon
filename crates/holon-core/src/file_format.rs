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

use std::collections::HashSet;
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

/// The ungrounded-drop verdict a write-back guard produces — DATA, distinct
/// from a real parse/IO failure (which is `Err`). `dropped` is one `id:
/// excerpt` per source block grounded by neither the surviving union nor a
/// sanctioned removal; `source_block_count` is the total non-empty block count
/// of `source`, reported alongside so a veto can say how much of the file the
/// drop covers.
#[derive(Debug, Clone, Default)]
pub struct WritebackDropVerdict {
    /// One `id: excerpt` per ungrounded (dropped) source block; empty =
    /// lossless.
    pub dropped: Vec<String>,
    /// Non-empty block count parsed from `source` (the file about to be
    /// overwritten) — context for how much of the file a drop covers.
    pub source_block_count: usize,
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
    ///
    /// `previous` is the block as the file PREVIOUSLY declared it, and is what
    /// makes the file authoritative for the block's user-visible property set:
    /// a property key `previous` declared and `block` no longer does is emitted
    /// as `Value::REMOVED`, the writer's removal sentinel, so the store-side
    /// merge clears it instead of keeping it alive forever. `None` for a
    /// create, and for any caller that is NOT reconciling a file against
    /// its own prior state — such a write names no authority over peer keys
    /// and must keep the insert-only merge.
    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
        previous: Option<&Block>,
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

    /// Which blocks a write-back would SILENTLY drop from disk (BugFunnel row
    /// 28, P0 data-loss class; ADR 0025 op-grounding) — as DATA, not an error.
    /// Real parse/IO defects are still `Err`, never folded into the verdict.
    ///
    /// `source` is the on-disk file about to be overwritten, `rendered` is the
    /// projection about to be written over it. A block present in `source` but
    /// absent from `rendered` is GROUNDED — and therefore not loss — when it is
    /// present in any `sibling_renders` entry (the file that block now lives
    /// in, folded into the surviving union) OR its id is in
    /// `sanctioned_removals` (the triggering delta's `Remove` set, or an
    /// authority-proven move). A block grounded by neither is loss: the
    /// caller refuses the write and quarantines the file so no write-back
    /// path rewrites the truncated state.
    ///
    /// EVERY write-back boundary — ingest re-project and block-driven alike —
    /// assembles that grounding from the same authority
    /// (`FileSyncController::writeback_drops`), so no boundary can accidentally
    /// ground against the file's own projection alone. A LEGAL canonical
    /// reformat and a 3-way text merge both drop nothing — the anchor is block
    /// preservation, not byte equality. `root` is the vault root used for
    /// stable file-id derivation while parsing.
    fn writeback_drops(
        &self,
        path: &Path,
        source: &str,
        rendered: &str,
        sibling_renders: &[(&Path, &str)],
        sanctioned_removals: &HashSet<String>,
        root: &Path,
    ) -> Result<WritebackDropVerdict>;
}
