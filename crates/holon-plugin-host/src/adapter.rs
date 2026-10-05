//! A vault file format served by a wasm guest instead of by a Rust crate.
//!
//! The guest returns one [`Stream`] of `holon-plugin-rows` lines: one document,
//! its child blocks, and rows of the scopes the sidecar declares.
//! [`Stream::from_jsonl`] is the one parse of that text; every id in it is a
//! typed [`LocalId`], which this file renders into the [`EntityUri`] it is
//! stored as.
//!
//! Atomicity: everything is validated before anything is returned, so a
//! refused file yields `Err` and leaves NO document block and NO scope behind.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::file_format::DocumentIdentity;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_core::file_format::TypedRowSet;
use holon_core::file_format::WriteTier;
use holon_core::file_format::WritebackDropVerdict;
use holon_plugin_rows::BlockRow;
use holon_plugin_rows::DocumentRow;
use holon_plugin_rows::EntityRef;
use holon_plugin_rows::Fields;
use holon_plugin_rows::Line;
use holon_plugin_rows::LocalId;
use holon_plugin_rows::Owner;
use holon_plugin_rows::Row;
use holon_plugin_rows::Scope;
use holon_plugin_rows::Stream;

use crate::PluginHost;
use crate::PluginLimits;
use crate::params::build_block_params;
use crate::sidecar::DeclaredScope;
use crate::sidecar::GuestSource;
use crate::sidecar::IdSource;
use crate::sidecar::PluginFormat;

/// How many files every plugin in this process has run its guest over.
///
/// The interpreter costs ~20x a native parser, so "how many files did the scan
/// actually parse" is the number the vault-scan budget is spent in. A test
/// reads it across a restart to prove an unchanged file was not re-parsed.
static GUEST_PARSES: AtomicU64 = AtomicU64::new(0);

/// The running total of [`GUEST_PARSES`].
pub fn guest_parses() -> u64 {
    GUEST_PARSES.load(Ordering::Relaxed)
}

/// The column a row's [`Row::id`] is stored in, so no cell or ref may name it.
const ID_COLUMN: &str = "id";

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
        Self::instantiate(PluginFormat::load(sidecar_path)?, limits)
    }

    /// Every format Holon ships, instantiated from the bytes compiled in.
    ///
    /// This is what a wiring call per format used to be: a format joins the
    /// vault by its sidecar, whether that sidecar is bundled or installed.
    pub fn bundled(limits: PluginLimits) -> Result<Vec<Self>> {
        crate::sidecar::BUNDLED_PLUGINS
            .iter()
            .map(|plugin| Self::instantiate(PluginFormat::bundled(plugin)?, limits))
            .collect()
    }

    fn instantiate(format: PluginFormat, limits: PluginLimits) -> Result<Self> {
        let wasm = match &format.guest {
            GuestSource::File(path) => std::fs::read(path).with_context(|| {
                format!(
                    "cannot read the guest {} that format {:?} names",
                    path.display(),
                    format.format_name
                )
            })?,
            GuestSource::Bundled(bytes) => bytes.to_vec(),
        };
        let host = PluginHost::from_bytes(&wasm, limits).map_err(|e| {
            anyhow!(
                "guest {} of format {:?} does not load: {e}",
                format.guest.label(),
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
        GUEST_PARSES.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(source_path, format = self.format.format_name, "guest parse");
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

        let file_path = LocalId::from_path(&source_path).map_err(|e| {
            anyhow!("vault path {source_path:?} names no document to hang blocks on: {e}")
        })?;

        let text = self.run(&source_path, file_stem, content)?;
        let stream = Stream::from_jsonl(&text).map_err(|e| {
            anyhow!(
                "the {} plugin emitted a stream for {source_path} that is not the row contract: \
                 {e}",
                self.format.format_name
            )
        })?;

        let file_id = EntityUri::file(&source_path);

        let mut scopes = stream
            .scopes
            .into_iter()
            .map(|scope| self.open_scope(scope))
            .collect::<Result<Vec<_>>>()?;
        for (i, scope) in scopes.iter().enumerate() {
            if scopes[..i]
                .iter()
                .any(|s| s.set.type_name == scope.set.type_name)
            {
                bail!(
                    "the {} plugin opened scope {:?} twice for {source_path}, so its rows could \
                     not be routed to either",
                    self.format.format_name,
                    scope.set.type_name
                );
            }
        }
        for declared in &self.format.scopes {
            if !scopes
                .iter()
                .any(|s| s.declared.type_name == declared.type_name)
            {
                bail!(
                    "the {} plugin emitted no {:?} scope for {source_path}; a scope left out is \
                     how the last row of that type would never get swept",
                    self.format.format_name,
                    declared.type_name
                );
            }
        }

        let mut document: Option<Block> = None;
        let mut blocks: Vec<Block> = Vec::new();
        for line in stream.lines {
            match line {
                Line::Document(row) => {
                    if document.is_some() {
                        bail!(
                            "the {} plugin emitted a second document for {source_path}; a file is \
                             exactly one document",
                            self.format.format_name
                        );
                    }
                    document = Some(self.document_block(row, &file_id, parent_dir_id)?);
                }
                Line::Block(row) => blocks.push(self.child_block(row, &file_path, &file_id)?),
                Line::Row(row) => self.add_row(&mut scopes, row, &file_path)?,
            }
        }

        let document = document.with_context(|| {
            format!(
                "the {} plugin emitted no document for {source_path}, so the file has no entity \
                 to hang its blocks and rows on",
                self.format.format_name
            )
        })?;

        Ok(FileFormatParseResult {
            document,
            blocks,
            // Nothing is written back, so no block needs an id minted for
            // re-rendering.
            blocks_needing_ids: Vec::new(),
            typed_rows: scopes.into_iter().map(|s| s.set).collect(),
        })
    }

    fn render_document(
        &self,
        _: &Block,
        _: &[Block],
        path: &Path,
        _: &EntityUri,
    ) -> anyhow::Result<holon_api::Rendered> {
        // Unreachability assert, not input handling: a sidecar admits only
        // read-only formats, so no caller has a render path to here.
        unreachable!(
            "the {} plugin is registered read-only; render_document must be unreachable — \
             reaching it for {} is a wiring bug, not bad input.",
            self.format.format_name,
            path.display()
        );
    }

    fn render_blocks(
        &self,
        _: &[Block],
        path: &Path,
        _: &EntityUri,
    ) -> anyhow::Result<holon_api::Rendered> {
        unreachable!(
            "the {} plugin is registered read-only; render_blocks must be unreachable — reaching \
             it for {} is a wiring bug, not bad input.",
            self.format.format_name,
            path.display()
        );
    }

    fn doc_id_from_content(&self, _: &str) -> anyhow::Result<Option<EntityUri>> {
        // A guest is a pure function over bytes; nothing in the contract lets
        // it name a stable document id, so the caller resolves by name chain.
        Ok(None)
    }

    fn document_identity(&self) -> DocumentIdentity {
        DocumentIdentity::ByRecordedHome
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

/// One scope the stream replaces, checked against its declaration, gathering
/// the rows that follow it.
struct OpenScope<'a> {
    declared: &'a DeclaredScope,
    owner: Owner,
    set: TypedRowSet,
}

impl PluginFormatAdapter {
    fn document_block(
        &self,
        row: DocumentRow,
        file_id: &EntityUri,
        parent_dir_id: &EntityUri,
    ) -> Result<Block> {
        let mut document = Block::new_text(file_id.clone(), parent_dir_id.clone(), row.title);
        document.set_page(true);
        self.apply_properties(&mut document, row.properties)?;
        Ok(document)
    }

    /// One child block, in emitted order. The stream carries a flat list: no
    /// line nests one block under another, so a format with a real tree is not
    /// yet expressible here and would need a parent key.
    ///
    /// A block's identity is its document's path plus the guest's key, so
    /// nothing the guest emits can name a block in another file.
    fn child_block(
        &self,
        row: BlockRow,
        file_path: &LocalId,
        file_id: &EntityUri,
    ) -> Result<Block> {
        let id = EntityUri::from_segments("block", file_path.path_segments(), row.key.parts());
        let mut block = Block::new_text(id, file_id.clone(), row.content);
        self.apply_properties(&mut block, row.properties)?;
        Ok(block)
    }

    /// A key naming a `block_raw` storage column is refused: `partition_params`
    /// routes such a param straight to that column, so emitting one would
    /// overwrite the block's own row state.
    fn apply_properties(
        &self,
        block: &mut Block,
        properties: Fields<serde_json::Value>,
    ) -> Result<()> {
        for (key, value) in properties {
            if holon_api::schema::is_block_column(&key) {
                bail!(
                    "the {} plugin emitted property {key:?}, which names a `block_raw` storage \
                     column; storing it would overwrite the block's own row state",
                    self.format.format_name
                );
            }
            block.set_property(key, Value::from_json_value(value));
        }
        Ok(())
    }

    fn open_scope(&self, scope: Scope) -> Result<OpenScope<'_>> {
        let declared = self.declared(&scope.type_name)?;
        if scope.owner_column != declared.owner_column {
            bail!(
                "the {} plugin scoped {:?} by owner column {:?}, but its sidecar declares {:?} — \
                 re-ingest sweeps by the DECLARED column, so rows would be replaced outside the \
                 scope they were written in",
                self.format.format_name,
                scope.type_name,
                scope.owner_column,
                declared.owner_column
            );
        }
        let owner_value = match &scope.owner {
            Owner::Text(text) => text.clone(),
            Owner::Ref(entity) => self.entity_uri(entity)?.to_string(),
        };
        Ok(OpenScope {
            declared,
            owner: scope.owner,
            set: TypedRowSet {
                type_name: scope.type_name,
                owner_column: scope.owner_column,
                owner_value,
                rows: Vec::new(),
            },
        })
    }

    /// File `row` under the scope the envelope opened for its type, as the
    /// create params it is stored with.
    fn add_row(&self, scopes: &mut [OpenScope<'_>], row: Row, file_path: &LocalId) -> Result<()> {
        let scope = scopes
            .iter_mut()
            .find(|s| s.set.type_name == row.type_name)
            .with_context(|| {
                format!(
                    "the {} plugin emitted a {:?} row, but its envelope opens no scope of that \
                     type to sweep it with",
                    self.format.format_name, row.type_name
                )
            })?;
        let declared = scope.declared;

        let owned = match &scope.owner {
            Owner::Text(text) => {
                row.cells.get(&declared.owner_column) == Some(&serde_json::json!(text))
            }
            Owner::Ref(entity) => row.refs.get(&declared.owner_column) == Some(entity),
        };
        if !owned {
            bail!(
                "a {:?} row carries owner column {:?} = {:?} while its scope owns {:?}; the row \
                 would be written outside the scope its own replacement sweeps",
                row.type_name,
                declared.owner_column,
                row.refs
                    .get(&declared.owner_column)
                    .map(|r| format!("{r:?}"))
                    .or_else(|| row.cells.get(&declared.owner_column).map(|c| c.to_string())),
                scope.set.owner_value
            );
        }

        if declared.id_from == Some(IdSource::SourcePath) && row.id != *file_path {
            bail!(
                "the {} plugin emitted a {:?} row with id {:?}, but its sidecar declares that id \
                 to be the source path {:?}",
                self.format.format_name,
                row.type_name,
                row.id,
                file_path
            );
        }

        let mut entity = StorageEntity::new();
        entity.insert(
            ID_COLUMN.into(),
            Value::String(
                EntityUri::from_segments(
                    &declared.id_entity,
                    row.id.path_segments(),
                    row.id.parts(),
                )
                .to_string(),
            ),
        );
        let refs = row
            .refs
            .into_iter()
            .map(|(column, target)| {
                Ok((column, Value::String(self.entity_uri(&target)?.to_string())))
            })
            .collect::<Result<Vec<_>>>()?;
        let cells = row
            .cells
            .into_iter()
            .map(|(column, value)| (column, Value::from_json_value(value)));
        for (column, value) in refs.into_iter().chain(cells) {
            if !declared.columns.contains(&column) {
                bail!(
                    "the {} plugin emitted column {column:?} on a {:?} row, which its sidecar \
                     does not declare",
                    self.format.format_name,
                    declared.type_name
                );
            }
            if entity.contains_key(column.as_str()) {
                bail!(
                    "the {} plugin stated column {column:?} of a {:?} row twice — as the row's id, \
                     or as both a ref and a cell",
                    self.format.format_name,
                    declared.type_name
                );
            }
            entity.insert(column.into(), value);
        }
        scope.set.rows.push(entity);
        Ok(())
    }

    /// The stored reference for a row of a declared type, its scheme the type's
    /// `id_entity`.
    fn entity_uri(&self, entity: &EntityRef) -> Result<EntityUri> {
        let declared = self.declared(&entity.type_name)?;
        Ok(EntityUri::from_segments(
            &declared.id_entity,
            entity.id.path_segments(),
            entity.id.parts(),
        ))
    }

    fn declared(&self, type_name: &str) -> Result<&DeclaredScope> {
        self.format.scope(type_name).with_context(|| {
            format!(
                "the {} plugin emitted type {type_name:?}, which its sidecar does not declare",
                self.format.format_name
            )
        })
    }
}
impl std::fmt::Debug for PluginFormatAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginFormatAdapter")
            .field("format", &self.format.format_name)
            .finish_non_exhaustive()
    }
}
