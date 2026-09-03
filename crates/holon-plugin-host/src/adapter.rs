//! A vault file format served by a wasm guest instead of by a Rust crate.
//!
//! The guest returns one JSON Lines stream. Two scope names in it are the
//! contract's own — [`DOCUMENT_SCOPE`] and [`BLOCK_SCOPE`] — and become the
//! parse result's document and blocks; every other scope is a declared-type
//! row set, checked against the sidecar before it leaves this file.
//!
//! Atomicity: everything is validated before anything is returned, so a
//! refused file yields `Err` and leaves NO document block and NO scope behind.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_core::file_format::TypedRowSet;
use holon_core::file_format::WriteTier;
use holon_core::file_format::WritebackDropVerdict;
use holon_rows::checked_local_id;

use crate::PluginHost;
use crate::PluginLimits;
use crate::params::build_block_params;
use crate::sidecar::BLOCK_SCOPE;
use crate::sidecar::DOCUMENT_SCOPE;
use crate::sidecar::PluginFormat;

/// The cell a document row titles the document with; every other cell of that
/// row is a document property.
const TITLE_CELL: &str = "title";
/// The cell a block row carries its text in.
const CONTENT_CELL: &str = "content";
/// The cell every block row identifies itself by, LOCAL to its document.
const ID_CELL: &str = "id";

pub struct PluginFormatAdapter {
    format: PluginFormat,
    /// One instantiated guest, reused across files — instantiation is
    /// milliseconds and buys nothing per call. The mutex serialises parses,
    /// which the sync controller already does per vault scan.
    host: Mutex<PluginHost>,
}

impl PluginFormatAdapter {
    /// Load the sidecar at `sidecar_path` and instantiate the guest it names.
    pub fn load(sidecar_path: &Path, limits: PluginLimits) -> Result<Self> {
        let format = PluginFormat::load(sidecar_path)?;
        let wasm = std::fs::read(&format.guest_path).with_context(|| {
            format!(
                "cannot read the guest {} that format {:?} names",
                format.guest_path.display(),
                format.format_name
            )
        })?;
        let host = PluginHost::from_bytes(&wasm, limits).map_err(|e| {
            anyhow!(
                "guest {} of format {:?} does not load: {e}",
                format.guest_path.display(),
                format.format_name
            )
        })?;
        Ok(Self {
            format,
            host: Mutex::new(host),
        })
    }

    pub fn format(&self) -> &PluginFormat {
        &self.format
    }

    /// Every plugin installed in `dir`, one per `*.yaml` sidecar, sorted by
    /// file name so registration order does not depend on the filesystem.
    ///
    /// This is what replaces a wiring call per format: a `.wasm` and a yaml
    /// dropped in the directory ARE the registration. A missing directory
    /// yields nothing — a vault with no plugins is ordinary — but a sidecar
    /// that will not load is an `Err`, because a format silently absent is
    /// indistinguishable from a format that never existed.
    pub fn load_dir(dir: &Path, limits: PluginLimits) -> Result<Vec<Self>> {
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut sidecars: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("cannot list the plugin directory {}", dir.display()))?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e == "yaml"))
            .collect();
        sidecars.sort();

        sidecars
            .iter()
            .map(|path| Self::load(path, limits))
            .collect()
    }

    /// Run the guest over `content` and return the stream it emitted.
    fn run(&self, source_path: &str, file_stem: &str, content: &str) -> Result<String> {
        let ctx = serde_json::json!({
            "source_path": source_path,
            "file_stem": file_stem,
        })
        .to_string();
        let mut host = self.host.lock().map_err(|_| {
            anyhow!(
                "the {} plugin host is poisoned by an earlier panic inside the guest",
                self.format.format_name
            )
        })?;
        host.parse(content.as_bytes(), ctx.as_bytes()).map_err(|e| {
            anyhow!(
                "the {} plugin refused {source_path}: {e}",
                self.format.format_name
            )
        })
    }
}

impl FileFormatAdapter for PluginFormatAdapter {
    fn extensions(&self) -> &'static [&'static str] {
        self.format.extensions
    }

    fn write_tier(&self) -> WriteTier {
        self.format.write_tier
    }

    fn format_name(&self) -> &'static str {
        self.format.format_name
    }

    fn parse(
        &self,
        path: &Path,
        content: &str,
        parent_dir_id: &EntityUri,
        root: &Path,
    ) -> Result<FileFormatParseResult> {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let source_path = rel.to_string_lossy().into_owned();
        let file_stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
            anyhow!(
                "path {} has no UTF-8 file name, so the {} plugin has no stem to fall back to",
                path.display(),
                self.format.format_name
            )
        })?;

        let stream = self.run(&source_path, file_stem, content)?;
        let sets = holon_rows::parse_row_sets(&stream).with_context(|| {
            format!(
                "the {} plugin emitted a stream for {source_path} that is not the row contract",
                self.format.format_name
            )
        })?;

        let file_id = EntityUri::file(&source_path);
        let mut document: Option<Block> = None;
        let mut blocks: Vec<Block> = Vec::new();
        let mut typed_rows: Vec<TypedRowSet> = Vec::new();

        for set in sets {
            match set.type_name.as_str() {
                DOCUMENT_SCOPE => {
                    document = Some(self.document_block(set, &file_id, parent_dir_id)?);
                }
                BLOCK_SCOPE => blocks = self.child_blocks(set, &file_id)?,
                _ => {
                    self.check_declared(&set)?;
                    typed_rows.push(set);
                }
            }
        }

        let document = document.with_context(|| {
            format!(
                "the {} plugin emitted no {DOCUMENT_SCOPE} scope for {source_path}, so the file \
                 has no entity to hang its blocks and rows on",
                self.format.format_name
            )
        })?;

        for declared in &self.format.scopes {
            if !typed_rows.iter().any(|s| s.type_name == declared.type_name) {
                bail!(
                    "the {} plugin emitted no {:?} scope for {source_path}; a scope left out is \
                     how the last row of that type would never get swept",
                    self.format.format_name,
                    declared.type_name
                );
            }
        }

        Ok(FileFormatParseResult {
            document,
            blocks,
            // Nothing is written back, so no block needs an id minted for
            // re-rendering.
            blocks_needing_ids: Vec::new(),
            typed_rows,
        })
    }

    fn render_document(&self, _: &Block, _: &[Block], path: &Path, _: &EntityUri) -> String {
        // Unreachability assert, not input handling: a sidecar admits only
        // read-only formats, so no caller has a render path to here.
        unreachable!(
            "the {} plugin is registered read-only; render_document must be unreachable — \
             reaching it for {} is a wiring bug, not bad input.",
            self.format.format_name,
            path.display()
        );
    }

    fn render_blocks(&self, _: &[Block], path: &Path, _: &EntityUri) -> String {
        unreachable!(
            "the {} plugin is registered read-only; render_blocks must be unreachable — reaching \
             it for {} is a wiring bug, not bad input.",
            self.format.format_name,
            path.display()
        );
    }

    fn doc_id_from_content(&self, _: &str) -> Option<String> {
        // A guest is a pure function over bytes; nothing in the contract lets
        // it name a stable document id, so the caller resolves by name chain.
        None
    }

    fn build_block_params(
        &self,
        block: &Block,
        parent_id: &EntityUri,
        document_uri: &EntityUri,
        previous: Option<&Block>,
    ) -> StorageEntity {
        // The trait returns params, not a Result. The parse boundary already
        // refuses a property key that names a storage column, so a block
        // reaching here with one was not built by us.
        build_block_params(block, parent_id, document_uri, previous).expect(
            "this adapter parsed the block, and parse refuses storage-column property keys — a \
             failure here means the block came from elsewhere",
        )
    }

    fn content_differs(&self, a: &Block, b: &Block) -> bool {
        a.content != b.content
    }

    fn writeback_drops(
        &self,
        path: &Path,
        _: &str,
        _: &str,
        _: &[(&Path, &str)],
        _: &HashSet<String>,
        _: &Path,
    ) -> Result<WritebackDropVerdict> {
        bail!(
            "the {} plugin is read-only and refuses write-back to authoritative file {}",
            self.format.format_name,
            path.display()
        )
    }
}

impl PluginFormatAdapter {
    /// The document block: `title` names it, every other cell is a property.
    fn document_block(
        &self,
        set: TypedRowSet,
        file_id: &EntityUri,
        parent_dir_id: &EntityUri,
    ) -> Result<Block> {
        let [row] = <[StorageEntity; 1]>::try_from(set.rows).map_err(|rows| {
            anyhow!(
                "the {} plugin emitted {} {DOCUMENT_SCOPE} rows; a file is exactly one document",
                self.format.format_name,
                rows.len()
            )
        })?;

        let title = match row.get(TITLE_CELL) {
            Some(Value::String(title)) => title.clone(),
            other => bail!(
                "the {} plugin's {DOCUMENT_SCOPE} row carries title {other:?}; a title we invented \
                 instead would look like the file's own and quietly become its identity",
                self.format.format_name
            ),
        };

        let mut document = Block::new_text(file_id.clone(), parent_dir_id.clone(), title);
        document.set_page(true);
        self.apply_properties(&mut document, row, &[TITLE_CELL])?;
        Ok(document)
    }

    /// The document's child blocks, in emitted order. The scope carries a flat
    /// list: no cell nests one block under another, so a format with a real
    /// tree is not yet expressible here and would need a `parent` cell.
    fn child_blocks(&self, set: TypedRowSet, file_id: &EntityUri) -> Result<Vec<Block>> {
        let mut blocks = Vec::with_capacity(set.rows.len());
        for row in set.rows {
            let local = match row.get(ID_CELL) {
                Some(Value::String(local)) => local.clone(),
                other => bail!(
                    "the {} plugin's {BLOCK_SCOPE} row carries id {other:?}, which is not a local \
                     block id",
                    self.format.format_name
                ),
            };
            let content = match row.get(CONTENT_CELL) {
                Some(Value::String(content)) => content.clone(),
                other => bail!(
                    "the {} plugin's {BLOCK_SCOPE} row {local:?} carries content {other:?}",
                    self.format.format_name
                ),
            };
            // A block's identity is its document's plus the guest's local id,
            // so nothing the guest emits can name a block in another file.
            let id = EntityUri::block(&format!("{}::{local}", file_id.id()));
            let mut block = Block::new_text(id, file_id.clone(), content);
            self.apply_properties(&mut block, row, &[ID_CELL, CONTENT_CELL])?;
            blocks.push(block);
        }
        Ok(blocks)
    }

    /// Every cell but `consumed` becomes a property.
    ///
    /// A key naming a `block_raw` storage column is refused: `partition_params`
    /// routes such a param straight to that column, so emitting one would
    /// overwrite the block's own row state.
    fn apply_properties(
        &self,
        block: &mut Block,
        row: StorageEntity,
        consumed: &[&str],
    ) -> Result<()> {
        for (key, value) in row {
            if consumed.contains(&key.as_ref()) {
                continue;
            }
            if crate::params::names_block_storage_column(key.as_ref()) {
                bail!(
                    "the {} plugin emitted property {key:?}, which names a `block_raw` storage \
                     column; storing it would overwrite the block's own row state",
                    self.format.format_name
                );
            }
            block.set_property(key.to_string(), value);
        }
        Ok(())
    }

    /// A row set the sidecar must have declared, cell for cell.
    fn check_declared(&self, set: &TypedRowSet) -> Result<()> {
        let declared = self.format.scope(&set.type_name).with_context(|| {
            format!(
                "the {} plugin emitted scope {:?}, which its sidecar does not declare",
                self.format.format_name, set.type_name
            )
        })?;

        if set.owner_column != declared.owner_column {
            bail!(
                "the {} plugin scoped {:?} by owner column {:?}, but its sidecar declares {:?} — \
                 re-ingest sweeps by the DECLARED column, so rows would be replaced outside the \
                 scope they were written in",
                self.format.format_name,
                set.type_name,
                set.owner_column,
                declared.owner_column
            );
        }

        for row in &set.rows {
            for (column, _) in row {
                if !declared.columns.contains(column.as_ref()) {
                    bail!(
                        "the {} plugin emitted column {column:?} on a {:?} row, which its sidecar \
                         does not declare",
                        self.format.format_name,
                        set.type_name
                    );
                }
            }
            match row.get(declared.owner_column.as_str()) {
                Some(Value::String(owner)) if *owner == set.owner_value => {}
                other => bail!(
                    "a {:?} row carries owner column {:?} = {other:?} while its scope owns {:?}; \
                     the row would be written outside the scope its own replacement sweeps",
                    set.type_name,
                    declared.owner_column,
                    set.owner_value
                ),
            }
            match row.get(ID_CELL) {
                Some(Value::String(id)) => checked_local_id(&declared.id_entity, id)?,
                other => bail!(
                    "a {:?} row carries id {other:?}; ids are derived from content and every row \
                     must have one, because the replacement on re-ingest keys on it",
                    set.type_name
                ),
            }
        }
        Ok(())
    }
}

impl std::fmt::Debug for PluginFormatAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginFormatAdapter")
            .field("format", &self.format.format_name)
            .finish_non_exhaustive()
    }
}
