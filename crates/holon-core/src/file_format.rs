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

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use anyhow::bail;
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
    /// Declared-type rows this file owns, beyond its blocks — a `.cook`
    /// recipe's `recipe` + `ingredient_use` rows. A format that projects only
    /// blocks emits none.
    pub typed_rows: Vec<TypedRowSet>,
}

/// Every row of ONE declared type that ONE file owns.
///
/// Re-ingest REPLACES the set: the sink retires each row of `type_name` that
/// `owner_column` scopes to this file and that this parse no longer produces,
/// then writes `rows`. Ownership is declared rather than inferred because a
/// row carries no file-provenance column of its own — the adapter names a
/// column the type already declares and fills it from the file's identity
/// (`recipe.source_path`, `ingredient_use.recipe_id`).
pub struct TypedRowSet {
    /// A type the registry declares, e.g. `recipe`.
    pub type_name: String,
    /// A field of `type_name` whose value ties a row to one source file.
    pub owner_column: String,
    /// The value `owner_column` holds for this file. Every `rows` entry must
    /// carry it, or the row would be written outside the scope its own
    /// replacement sweeps.
    pub owner_value: String,
    /// `create` params per row. `id` is REQUIRED and must be derived from the
    /// file's CONTENT, not from row position: the replacement above keys on it,
    /// and anything holding a row's id must still mean the same row after the
    /// user edits the file somewhere above it.
    pub rows: Vec<StorageEntity>,
}

/// Writes an adapter's [`TypedRowSet`]s through the generic entity-operation
/// path.
///
/// Declared elsewhere than the file-sync controller because that controller
/// sits below the operation dispatcher: the implementation resolves the ONE
/// shared dispatcher and routes `create`/`delete` through it, so vault ingest
/// never becomes a second writer of these tables.
#[async_trait::async_trait]
pub trait TypedRowSink: Send + Sync {
    /// Make each set's owned rows be exactly its `rows`.
    ///
    /// NOT atomic: retire and write are separate operations, so a failure part
    /// way leaves the set partly emptied. It is disclosed — the error names the
    /// type and the caller names the file — and the next ingest of that file
    /// restores the set.
    async fn replace_typed_rows(&self, sets: &[TypedRowSet]) -> Result<()>;
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

/// Property slot holding a document's own declared title — org's `#+TITLE:`,
/// a recipe's `>> title:`, a markdown frontmatter `title:`. Distinct from the
/// document block's NAME (the first line of its content), which stays
/// path-derived; see [`apply_document_metadata`].
pub const DOCUMENT_TITLE_KEY: &str = "title";

/// The ingest contract for document metadata, in one place for every format.
///
/// The persisted document block's property bag becomes EXACTLY what the parse
/// declares: `parsed.properties` verbatim, plus the declared title under
/// [`DOCUMENT_TITLE_KEY`]. Returns whether `persisted` changed, so the caller
/// writes only when there is something to write.
///
/// The file is the authority in both directions — a key the user deleted from
/// it is removed here, or the block would keep serving metadata the file does
/// not have, with nothing to disclose it.
///
/// The block's name is left alone: the sync controller re-resolves a document
/// by its path-derived name chain on every later ingest, so a renamed block is
/// unfindable there and the file gets a second page.
pub fn apply_document_metadata(parsed: &Block, persisted: &mut Block) -> bool {
    let mut declared: HashMap<&str, holon_api::Value> = parsed
        .properties
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    let title = parsed.title();
    if !title.is_empty() {
        declared.insert(DOCUMENT_TITLE_KEY, title.into());
    }

    let mut changed = false;
    let stale: Vec<String> = persisted
        .properties
        .keys()
        .filter(|k| !declared.contains_key(k.as_str()))
        .cloned()
        .collect();
    for key in stale {
        persisted.properties.remove(&key);
        changed = true;
    }
    for (key, value) in declared {
        if persisted.properties.get(key) != Some(&value) {
            persisted.properties.insert(key.to_string(), value);
            changed = true;
        }
    }
    changed
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

    /// Whether this format's files may be written back, or are authoritative
    /// input only.
    ///
    /// The sync controller consults this BEFORE rendering: a read-only
    /// format's `render_*` must never be reached, so the refusal is a
    /// disclosed ERROR rather than a panic in the write-back task. It also
    /// keeps a document homed in a read-only file out of page-file
    /// materialization, which would otherwise mint a SECOND home for it in the
    /// write-capable format's own extension.
    ///
    /// Deliberately without a default: a new adapter that says nothing about
    /// its write half would inherit "writable" by silence, and be discovered
    /// to be otherwise only by overwriting a user's file.
    fn write_tier(&self) -> WriteTier;

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

    /// Reconcile document-header metadata from a freshly-parsed document block
    /// onto the persisted document entity. Mutates `persisted` in place and
    /// returns whether it changed; the controller persists via
    /// `update_metadata` only when `true`.
    ///
    /// The default is the ingest contract — [`apply_document_metadata`]. An
    /// override takes responsibility for that contract too, on top of whatever
    /// format-specific header state it adds (org's `#+TODO:` keyword config,
    /// its file-level drawer).
    fn sync_document_metadata(&self, parsed: &Block, persisted: &mut Block) -> bool {
        apply_document_metadata(parsed, persisted)
    }

    /// The format's name, as a reader of an error or a degraded banner would
    /// recognise it — `"org"`, `"cooklang"`, `"Obsidian markdown"`. Every
    /// message that names the format which refused a file takes it from here.
    fn format_name(&self) -> &'static str;

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

/// Whether a format's files may be written back by the sync controller.
///
/// An enum rather than a `bool` so a call site reads as the question it
/// answers, and so a third tier (write-with-restrictions) grows here instead
/// of as a second flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteTier {
    /// Parse and render both; the controller round-trips these files.
    ReadWrite,
    /// Authoritative input only. The controller refuses write-back and
    /// page-file materialization for documents homed in this format, loudly.
    ReadOnly,
}

/// The vault's registered file formats, routed per file by extension.
///
/// One vault is heterogeneous — an org vault holding `.cook` recipes, a LogSeq
/// vault holding both `.md` and `.org` — so the sync controller resolves the
/// adapter for each path instead of binding one for the whole tree.
///
/// Two adapters claiming one extension is refused at CONSTRUCTION rather than
/// resolved by list order: the two markdown flavors both claim `md` and are
/// separated by a vault-flavor discriminator (`logseq/config.edn` vs
/// `.obsidian/`) that does not exist yet, and an ordered list would let a
/// wiring pick one of them by accident.
pub struct FormatRegistry {
    adapters: Vec<Arc<dyn FileFormatAdapter>>,
    /// Lowercased extension → index into `adapters`.
    by_ext: HashMap<String, usize>,
}

impl FormatRegistry {
    /// Refuses when two adapters claim one extension, naming the extension and
    /// both claimants' full extension sets — the fix is to choose between
    /// them, which a message naming only the extension does not support.
    pub fn new(adapters: Vec<Arc<dyn FileFormatAdapter>>) -> Result<Self> {
        let mut by_ext: HashMap<String, usize> = HashMap::new();
        for (index, adapter) in adapters.iter().enumerate() {
            for ext in adapter.extensions() {
                let ext = ext.to_ascii_lowercase();
                if let Some(&claimed_by) = by_ext.get(&ext) {
                    bail!(
                        "two file-format adapters both claim the '{ext}' extension: one claiming \
                         {:?} and one claiming {:?}. A vault path routes by extension alone, so \
                         the registry cannot choose between them — register only one, or \
                         discriminate them by vault flavor first.",
                        adapters[claimed_by].extensions(),
                        adapter.extensions(),
                    );
                }
                by_ext.insert(ext, index);
            }
        }
        Ok(Self { adapters, by_ext })
    }

    /// The adapter claiming `path`'s extension, or `None` when no adapter
    /// does — that path is simply not a vault document (an attachment, a
    /// `.gitignore`, an editor lock file), which is a typed absence and not a
    /// failure.
    /// Returns an owned handle rather than a borrow: the sync controller
    /// resolves the adapter inside `&mut self` methods that go on to mutate
    /// their own state, and a borrow of `self` would outlive that.
    pub fn adapter_for(&self, path: &Path) -> Option<Arc<dyn FileFormatAdapter>> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        self.by_ext.get(&ext).map(|&i| self.adapters[i].clone())
    }

    /// The adapter for a path the scan or watcher has ALREADY admitted as a
    /// vault document. An unclaimed extension here is a routing bug — the
    /// admission filter and this lookup disagreed — so it is loud.
    pub fn require(&self, path: &Path) -> Result<Arc<dyn FileFormatAdapter>> {
        match self.adapter_for(path) {
            Some(adapter) => Ok(adapter),
            None => bail!(
                "no registered file-format adapter claims {}, yet it reached a routing site that \
                 only tracked vault documents reach. Registered extensions: {:?}",
                path.display(),
                self.sorted_extensions(),
            ),
        }
    }

    /// Whether `path` is a vault document of some registered format — the
    /// admission filter the directory scan and the watcher share.
    pub fn handles(&self, path: &Path) -> bool {
        self.adapter_for(path).is_some()
    }

    /// The union of every registered adapter's claimed extensions, lowercase
    /// and without leading dots, in no particular order.
    pub fn extensions(&self) -> impl Iterator<Item = &str> {
        self.by_ext.keys().map(String::as_str)
    }

    /// Every registered extension, lowercase, sorted — a stable order for
    /// error messages and for extension-driven candidate probes.
    pub fn sorted_extensions(&self) -> Vec<&str> {
        let mut exts: Vec<&str> = self.extensions().collect();
        exts.sort_unstable();
        exts
    }
}

impl std::fmt::Debug for FormatRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormatRegistry")
            .field("extensions", &self.sorted_extensions())
            .finish()
    }
}
