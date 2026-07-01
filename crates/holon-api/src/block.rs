use holon_macros::Entity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::entity_uri::EntityUri;
use crate::inline_mark::MarkSpan;
use crate::types::{ContentType, SourceLanguage, Tags};
use crate::{row_id, uri_from_row, Value};

// =============================================================================
// BlockContent - Discriminated union for block content types
// =============================================================================

/// Content of a block - discriminated union for different content types.
///
/// This enables a unified data model across Org Mode, Markdown, and Loro:
/// - Tier 1 (all formats): Text and basic Source blocks
/// - Tier 2 (Org + Loro): Full SourceBlock with name, header_args, results
/// - Tier 3 (Loro only): CRDT history, real-time sync
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum BlockContent {
    /// Plain text content (paragraphs, prose)
    Text {
        /// Raw text content
        raw: String,
    },

    /// Rich text with inline marks (Bold, Italic, Link, etc.).
    ///
    /// `text` is the same flat string that lives in `Block.content` after
    /// flattening; `marks` lives in `Block.marks`. The variant exists as a
    /// type-driven constructor that forces consumers to handle marked
    /// content explicitly. See `crate::inline_mark` for `MarkSpan`.
    RichText { text: String, marks: Vec<MarkSpan> },

    /// Source code block (language-agnostic)
    Source(SourceBlock),
}

impl Default for BlockContent {
    fn default() -> Self {
        BlockContent::Text { raw: String::new() }
    }
}

impl std::fmt::Display for BlockContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockContent::Text { raw } => write!(f, "{}", raw),
            BlockContent::RichText { text, .. } => write!(f, "{}", text),
            BlockContent::Source(sb) => {
                let lang = sb.language.as_deref().unwrap_or("unknown");
                write!(f, "[{}] {}", lang, sb.source)
            }
        }
    }
}

impl BlockContent {
    /// Create a text content block
    pub fn text(raw: impl Into<String>) -> Self {
        BlockContent::Text { raw: raw.into() }
    }

    /// Create a source block with minimal fields (Tier 1)
    pub fn source(language: impl Into<String>, source: impl Into<String>) -> Self {
        BlockContent::Source(SourceBlock::new(language, source))
    }

    /// Get the raw text if this is a Text variant
    /// flutter_rust_bridge:ignore
    pub fn as_text(&self) -> Option<&str> {
        match self {
            BlockContent::Text { raw } => Some(raw),
            _ => None,
        }
    }

    /// Get the source block if this is a Source variant
    /// flutter_rust_bridge:ignore
    pub fn as_source(&self) -> Option<&SourceBlock> {
        match self {
            BlockContent::Source(sb) => Some(sb),
            _ => None,
        }
    }

    /// Get a plain text representation (for search, display, etc.)
    /// flutter_rust_bridge:ignore
    pub fn to_plain_text(&self) -> &str {
        match self {
            BlockContent::Text { raw } => raw,
            BlockContent::RichText { text, .. } => text,
            BlockContent::Source(sb) => &sb.source,
        }
    }
}

/// A source code block with optional metadata.
///
/// Supports three tiers of features:
/// - Tier 1 (all formats): language + source code
/// - Tier 2 (Org + Loro): name, header_args, results
/// - Tier 3 (Loro only): inherited from Block's CRDT features
///
/// In Org Mode: `#+BEGIN_SRC language :arg1 val1 ... #+END_SRC`
/// In Markdown: ` ```language ... ``` `
/// In Loro: Native storage with full fidelity
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceBlock {
    /// Language identifier (e.g., "holon_prql", "holon_sql", "python", "rust")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// The source code itself
    pub source: String,

    /// Optional block name for references (#+NAME: in Org Mode)
    /// Tier 2: Supported in Org Mode and Loro, lost in Markdown
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Header arguments / parameters
    /// Tier 2: Supported in Org Mode (`:var x=1 :results table`) and Loro
    /// Examples for PRQL: { "connection": "main", "results": "table" }
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub header_args: HashMap<String, Value>,
}

impl SourceBlock {
    /// Create a new source block with minimal fields (Tier 1)
    pub fn new(language: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            language: Some(language.into()),
            source: source.into(),
            name: None,
            header_args: HashMap::new(),
        }
    }

    /// Builder: set the block name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Builder: add a header argument
    pub fn with_header_arg(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.header_args.insert(key.into(), value.into());
        self
    }

    /// Check if this is a PRQL source block
    pub fn is_prql(&self) -> bool {
        self.language
            .as_ref()
            .and_then(|l| l.parse::<SourceLanguage>().ok()) // ALLOW(ok): unknown languages → None
            .map(|sl| sl.is_prql())
            .unwrap_or(false)
    }

    /// Get a header argument by key
    /// flutter_rust_bridge:ignore
    pub fn get_header_arg(&self, key: &str) -> Option<&Value> {
        self.header_args.get(key)
    }
}

/// Results from executing a source block.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockResult {
    /// The output content
    pub output: ResultOutput,

    /// Unix timestamp (milliseconds) when the block was executed
    pub executed_at: i64,
}

impl BlockResult {
    /// Create a text result
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            output: ResultOutput::Text {
                content: content.into(),
            },
            executed_at: crate::clock::now_millis(),
        }
    }

    /// Create a table result
    pub fn table(headers: Vec<String>, rows: Vec<Vec<Value>>) -> Self {
        Self {
            output: ResultOutput::Table { headers, rows },
            executed_at: crate::clock::now_millis(),
        }
    }

    /// Create an error result
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            output: ResultOutput::Error {
                message: message.into(),
            },
            executed_at: crate::clock::now_millis(),
        }
    }
}

/// Output types for block execution results.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ResultOutput {
    /// Plain text output
    Text { content: String },

    /// Tabular output (from queries)
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<Value>>,
    },

    /// Error output
    Error { message: String },
}

// =============================================================================
// Block - The main block structure (flattened for database storage)
// =============================================================================

/// A block in the hierarchical document structure.
///
/// This struct is flattened for efficient database storage while maintaining
/// a rich API through helper methods. Complex types (properties, children,
/// source block metadata) are stored as JSON strings.
///
/// Blocks use URI-based IDs to support integration with external systems:
/// - Local blocks: `local://<uuid-v4>` (e.g., `local://550e8400-e29b-41d4-a716-446655440000`)
/// - External systems: `todoist://task/12345`, `logseq://page/abc123`
///
/// # Example
///
/// ```rust
/// use holon_api::{Block, EntityUri};
///
/// // Text block
/// let block = Block::new_text(EntityUri::block("block-1"), EntityUri::no_parent(), "My first block");
///
/// // PRQL source block
/// let query_block = Block::new_source(EntityUri::block("query-1"), EntityUri::no_parent(), "holon_prql", "from tasks");
/// ```
/// `Block` is deliberately **serde-free** (H1 residue closed): the domain type
/// carries `tags`/`requires` as junction-derived edge fields that a naive derive
/// would silently drop, so every on-disk/wire path goes through [`BlockWire`]
/// instead (see [`block_wire_vec`] / [`SnapshotBlock`]). Do not add `Serialize`/
/// `Deserialize` here — add a `BlockWire` boundary conversion.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Entity)]
#[entity(
    name = "block",
    short_name = "block",
    api_crate = "crate",
    graph_label = "block"
)]
pub struct Block {
    /// URI-based unique identifier (e.g. `block:uuid`)
    #[primary_key]
    #[indexed]
    pub id: EntityUri,

    /// Parent reference — document URI, block ID, or root sentinel.
    #[indexed]
    #[reference(Block, edge = "CHILD_OF")]
    pub parent_id: EntityUri,

    /// Tags attached to this block. The literal tag `"Page"` marks the block
    /// as a page (formerly `is_document()`). Other tags are user-defined.
    /// Managed through the block_tags junction table (edge field), not a
    /// direct column. An unordered, duplicate-free set ([`Tags`]).
    #[edge_field]
    pub tags: Tags,

    /// Block IDs this block requires (depends on / is blocked by) before it
    /// can be acted upon. Stored in the `block_requires` junction table
    /// (edge field), not as a direct column. Reads from the `block` matview's
    /// hydrated `requires` JSON array.
    #[edge_field]
    pub requires: Vec<EntityUri>,

    // --- Content fields (flattened from BlockContent) ---
    /// Text content (raw text or source code)
    pub content: String,

    /// Content type: text or source.
    pub content_type: ContentType,

    /// For source blocks: programming language (e.g., prql, python).
    pub source_language: Option<SourceLanguage>,

    /// For source blocks: optional block name for references (#+NAME: in Org Mode)
    /// Tier 2: Supported in Org Mode and Loro, lost in Markdown
    pub source_name: Option<String>,

    // --- Properties (JSON strings) ---
    /// Key-value properties (TODO, PRIORITY, TAGS, dates, etc.)
    /// Stored as JSON object for native JSON support in Turso.
    /// Tier 2: works fully in Org + Loro
    #[jsonb]
    pub properties: HashMap<String, Value>,

    /// Inline rich-text marks (Bold, Italic, Link, etc.) over `content`.
    /// `None` means the block is plain text (today's behavior); `Some(empty)`
    /// is reserved for "rich block with no active marks". The `marks IS NOT NULL`
    /// projection is the discriminator the renderer uses to decide rich vs plain.
    /// Source/Image blocks always carry `None`.
    #[jsonb]
    pub marks: Option<Vec<MarkSpan>>,

    // --- Timestamps (flattened from BlockMetadata) ---
    /// Unix timestamp (milliseconds) when block was created
    pub created_at: i64,

    /// Unix timestamp (milliseconds) when block was last updated
    pub updated_at: i64,
}

impl Default for Block {
    fn default() -> Self {
        let now = crate::clock::now_millis();
        Self {
            id: EntityUri::block_random(),
            parent_id: EntityUri::no_parent(),
            tags: Tags::default(),
            requires: Vec::new(),
            content: String::new(),
            content_type: ContentType::Text,
            source_language: None,
            source_name: None,
            properties: HashMap::new(),
            marks: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Literal tag value that marks a block as a page (formerly "document").
pub const PAGE_TAG: &str = "Page";

impl Block {
    /// Whether this block is a page. A block is a page iff its `tags` list
    /// contains the literal string [`PAGE_TAG`].
    pub fn is_page(&self) -> bool {
        self.tags.contains(PAGE_TAG)
    }

    /// Mark or unmark this block as a page by toggling [`PAGE_TAG`] in `tags`.
    pub fn set_page(&mut self, is_page: bool) {
        if is_page {
            self.tags.insert(PAGE_TAG);
        } else {
            self.tags.remove(PAGE_TAG);
        }
    }

    /// Create a new text block with sensible defaults.
    pub fn new_text(id: EntityUri, parent_id: EntityUri, text: impl Into<String>) -> Self {
        Self {
            id,
            parent_id,
            content: text.into(),
            ..Self::default()
        }
    }

    /// Create a new source block with sensible defaults.
    ///
    /// `language` is parsed into a `SourceLanguage` via `FromStr`.
    pub fn new_source(
        id: EntityUri,
        parent_id: EntityUri,
        language: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        let lang_str = language.into();
        Self {
            id,
            parent_id,
            content: source.into(),
            content_type: ContentType::Source,
            source_language: Some(lang_str.parse::<SourceLanguage>().unwrap()),
            ..Self::default()
        }
    }

    /// Create a new rich-text block (text + inline marks). The block is
    /// stored as `content_type = Text` with `marks = Some(marks)`; readers
    /// distinguish via `marks IS NOT NULL`. Source/Image kinds are not
    /// constructible via this — use `new_source` / `new_image`.
    pub fn new_rich(
        id: EntityUri,
        parent_id: EntityUri,
        text: impl Into<String>,
        marks: Vec<MarkSpan>,
    ) -> Self {
        Self {
            id,
            parent_id,
            content: text.into(),
            marks: Some(marks),
            ..Self::default()
        }
    }

    /// Create a new image block. `path` is the relative file path (e.g. "attachments/abc.png").
    pub fn new_image(id: EntityUri, parent_id: EntityUri, path: impl Into<String>) -> Self {
        Self {
            id,
            parent_id,
            content: path.into(),
            content_type: ContentType::Image,
            ..Self::default()
        }
    }

    pub fn is_image_block(&self) -> bool {
        self.content_type == ContentType::Image
    }

    /// Derive MIME type from the image file extension.
    pub fn image_mime(&self) -> Option<&'static str> {
        if !self.is_image_block() {
            return None;
        }
        let ext = std::path::Path::new(&self.content)
            .extension()
            .and_then(|e| e.to_str())?;
        Some(match ext {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            "bmp" => "image/bmp",
            "tiff" | "tif" => "image/tiff",
            "ico" => "image/x-icon",
            _ => "application/octet-stream",
        })
    }

    /// Create a Block from a BlockContent.
    pub fn from_block_content(id: EntityUri, parent_id: EntityUri, content: BlockContent) -> Self {
        let (text, content_type, lang, src_name, marks) = match content {
            BlockContent::Text { raw } => (raw, ContentType::Text, None, None, None),
            BlockContent::RichText { text, marks } => {
                (text, ContentType::Text, None, None, Some(marks))
            }
            BlockContent::Source(sb) => (
                sb.source,
                ContentType::Source,
                sb.language.map(|l| l.parse::<SourceLanguage>().unwrap()),
                sb.name,
                None,
            ),
        };

        Self {
            id,
            parent_id,
            content: text,
            content_type,
            source_language: lang,
            source_name: src_name,
            marks,
            ..Self::default()
        }
    }

    /// Get the content as a BlockContent enum (used at the API surface).
    /// `marks IS NOT NULL` reconstitutes `RichText`; `None` flattens to plain `Text`.
    /// flutter_rust_bridge:ignore
    pub fn to_block_content(&self) -> BlockContent {
        match self.content_type {
            ContentType::Source => BlockContent::Source(SourceBlock {
                language: self.source_language.as_ref().map(|l| l.to_string()),
                source: self.content.clone(),
                name: self.source_name.clone(),
                header_args: HashMap::new(),
            }),
            // Image blocks store a file path in `content` — return as Text
            // since BlockContent has no Image variant. The caller should check
            // `content_type` to distinguish.
            ContentType::Text | ContentType::Image => match &self.marks {
                Some(marks) => BlockContent::RichText {
                    text: self.content.clone(),
                    marks: marks.clone(),
                },
                None => BlockContent::Text {
                    raw: self.content.clone(),
                },
            },
        }
    }

    /// Set the content from a BlockContent enum
    /// flutter_rust_bridge:ignore
    pub fn set_block_content(&mut self, content: BlockContent) {
        match content {
            BlockContent::Text { raw } => {
                self.content = raw;
                self.content_type = ContentType::Text;
                self.source_language = None;
                self.source_name = None;
                self.marks = None;
            }
            BlockContent::RichText { text, marks } => {
                self.content = text;
                self.content_type = ContentType::Text;
                self.source_language = None;
                self.source_name = None;
                self.marks = Some(marks);
            }
            BlockContent::Source(sb) => {
                self.content = sb.source;
                self.content_type = ContentType::Source;
                self.source_language = sb.language.map(|l| l.parse::<SourceLanguage>().unwrap());
                self.source_name = sb.name;
                self.marks = None;
            }
        }
        self.updated_at = crate::clock::now_millis();
    }

    /// Get the plain text content of this block.
    /// For text blocks, returns the raw text.
    /// For source blocks, returns the source code.
    /// flutter_rust_bridge:ignore
    pub fn content_text(&self) -> &str {
        &self.content
    }

    /// Get title (first line of content)
    /// flutter_rust_bridge:ignore
    pub fn title(&self) -> String {
        self.content.lines().next().unwrap_or("").to_string()
    }

    /// Check if this block contains a source block
    /// flutter_rust_bridge:ignore
    pub fn is_source_block(&self) -> bool {
        self.content_type == ContentType::Source
    }

    /// Check if this block contains a PRQL source block
    /// flutter_rust_bridge:ignore
    pub fn is_prql_block(&self) -> bool {
        self.is_source_block()
            && self
                .source_language
                .as_ref()
                .map(|l| l.is_prql())
                .unwrap_or(false)
    }

    /// Get properties as a HashMap (returns a clone)
    /// flutter_rust_bridge:ignore
    pub fn properties_map(&self) -> HashMap<String, Value> {
        self.properties.clone()
    }

    /// Set properties from a HashMap
    /// flutter_rust_bridge:ignore
    pub fn set_properties_map(&mut self, props: HashMap<String, Value>) {
        self.properties = props;
        self.updated_at = crate::clock::now_millis();
    }

    /// Get a property value by key
    /// flutter_rust_bridge:ignore
    pub fn get_property(&self, key: &str) -> Option<Value> {
        self.properties.get(key).cloned()
    }

    /// Get a property value as string
    /// flutter_rust_bridge:ignore
    pub fn get_property_str(&self, key: &str) -> Option<String> {
        self.properties
            .get(key)
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    }

    /// Set a property value
    pub fn set_property(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.properties.insert(key.into(), value.into());
        self.updated_at = crate::clock::now_millis();
    }

    /// Get source header arguments from properties (used by the Org Mode round-trip)
    /// flutter_rust_bridge:ignore
    pub fn get_source_header_args(&self) -> HashMap<String, Value> {
        self.properties
            .get("_source_header_args")
            .and_then(|v| {
                if let Value::String(s) = v {
                    serde_json::from_str(s).ok() // ALLOW(ok): properties may not be JSON
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    /// Set source header arguments in properties (used by the Org Mode round-trip)
    /// flutter_rust_bridge:ignore
    pub fn set_source_header_args(&mut self, header_args: HashMap<String, Value>) {
        if !header_args.is_empty() {
            if let Ok(json) = serde_json::to_string(&header_args) {
                self.properties
                    .insert("_source_header_args".to_string(), Value::String(json));
                self.updated_at = crate::clock::now_millis();
            }
        }
    }

    /// Get source results from properties (used by the Org Mode round-trip)
    /// flutter_rust_bridge:ignore
    pub fn get_source_results(&self) -> Option<String> {
        self.properties
            .get("_source_results")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    }

    /// Set source results in properties (used by the Org Mode round-trip)
    /// flutter_rust_bridge:ignore
    pub fn set_source_results(&mut self, results: Option<String>) {
        if let Some(r) = results {
            self.properties
                .insert("_source_results".to_string(), Value::String(r));
            self.updated_at = crate::clock::now_millis();
        }
    }

    /// Get metadata as BlockMetadata
    /// flutter_rust_bridge:ignore
    pub fn metadata(&self) -> BlockMetadata {
        BlockMetadata {
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    /// Set metadata from BlockMetadata
    /// flutter_rust_bridge:ignore
    pub fn set_metadata(&mut self, metadata: BlockMetadata) {
        self.created_at = metadata.created_at;
        self.updated_at = metadata.updated_at;
    }

    /// Get the depth/nesting level of this block by following parent chain.
    ///
    /// This requires a lookup function to resolve parent IDs to blocks.
    /// Returns 0 for root blocks, 1 for children of roots, etc.
    ///
    /// # Arguments
    ///
    /// * `get_block` - Function to look up a block by ID
    ///
    /// flutter_rust_bridge:ignore
    pub fn depth_from<'blk, F>(&self, mut get_block: F) -> usize
    where
        F: for<'a> FnMut(&'a str) -> Option<&'blk Block>,
    {
        let mut depth = 0;
        let mut current_parent_id: Option<&str> = self.parent_id.as_block_id();

        while let Some(pid) = current_parent_id {
            depth += 1;
            match get_block(pid) {
                Some(b) => {
                    current_parent_id = b.parent_id.as_block_id();
                    if current_parent_id.is_none() {
                        break;
                    }
                }
                None => break,
            }
        }

        depth
    }
}

fn require_value<'a>(
    row: &'a crate::StorageEntity,
    col: &str,
    id: &EntityUri,
) -> anyhow::Result<&'a Value> {
    row.get(col)
        .ok_or_else(|| anyhow::anyhow!("block {id}: required column '{col}' absent from row"))
}

fn require_string<'a>(
    row: &'a crate::StorageEntity,
    col: &str,
    id: &EntityUri,
) -> anyhow::Result<&'a str> {
    let v = require_value(row, col, id)?;
    v.as_string()
        .ok_or_else(|| anyhow::anyhow!("block {id}: column '{col}' must be a string, got {v:?}"))
}

fn require_i64(row: &crate::StorageEntity, col: &str, id: &EntityUri) -> anyhow::Result<i64> {
    let v = require_value(row, col, id)?;
    v.as_i64()
        .ok_or_else(|| anyhow::anyhow!("block {id}: column '{col}' must be an integer, got {v:?}"))
}

/// Strict decode of a projection-guaranteed string-array column
/// (`tags`/`requires`): every reader COALESCEs these to `'[]'` (matview) or
/// synthesizes them from the junction tables (block_raw readers), so an
/// absent key, a Null, an empty string, or a non-string element all mean a
/// broken projection — never "no tags".
fn require_string_array(
    row: &crate::StorageEntity,
    col: &str,
    id: &EntityUri,
) -> anyhow::Result<Vec<String>> {
    match require_value(row, col, id)? {
        Value::Array(arr) => arr
            .iter()
            .map(|elem| {
                elem.as_string().map(str::to_string).ok_or_else(|| {
                    anyhow::anyhow!(
                        "block {id}: column '{col}' has a non-string array element: {elem:?}"
                    )
                })
            })
            .collect(),
        Value::Json(s) | Value::String(s) => serde_json::from_str::<Vec<String>>(s).map_err(|e| {
            anyhow::anyhow!("block {id}: column '{col}' holds invalid JSON {s:?}: {e}")
        }),
        other => anyhow::bail!(
            "block {id}: column '{col}' must be a JSON string array, got {other:?} \
             (the reader's projection must COALESCE it to '[]')"
        ),
    }
}

impl TryFrom<crate::StorageEntity> for Block {
    type Error = anyhow::Error;
    fn try_from(row: crate::StorageEntity) -> Result<Self, Self::Error> {
        let id = row_id(&row)?;
        // parent_id must be projected (absent key = reader bug), but NULL is
        // a legal value (root blocks) → the no_parent sentinel.
        let parent_id = match row.get("parent_id") {
            None => anyhow::bail!("block {id}: required column 'parent_id' absent from row"),
            Some(Value::Null) => EntityUri::no_parent(),
            Some(_) => uri_from_row(&row, "parent_id")?,
        };
        let content = require_string(&row, "content", &id)?.to_string();
        let content_type: ContentType = require_string(&row, "content_type", &id)?
            .parse()
            .map_err(|e: anyhow::Error| {
                anyhow::anyhow!("block {id}: invalid 'content_type': {e}")
            })?;
        let source_language: Option<SourceLanguage> = match row.get("source_language") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let s = v.as_string().ok_or_else(|| {
                    anyhow::anyhow!(
                        "block {id}: column 'source_language' must be a string, got {v:?}"
                    )
                })?;
                Some(s.parse().map_err(|e: anyhow::Error| {
                    anyhow::anyhow!("block {id}: invalid 'source_language' {s:?}: {e}")
                })?)
            }
        };
        let source_name = match row.get("source_name") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_string()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "block {id}: column 'source_name' must be a string, got {v:?}"
                        )
                    })?
                    .to_string(),
            ),
        };
        // properties is a nullable column with no schema default: absent/Null
        // → empty map. An empty string is a written-empty value, not missing
        // data. Anything unparseable is a hard error.
        let properties: HashMap<String, Value> = match row.get("properties") {
            None | Some(Value::Null) => HashMap::new(),
            Some(Value::Json(s)) | Some(Value::String(s)) => {
                if s.is_empty() {
                    HashMap::new()
                } else {
                    serde_json::from_str::<HashMap<String, Value>>(s).map_err(|e| {
                        anyhow::anyhow!(
                            "block {id}: column 'properties' holds invalid JSON {s:?}: {e}"
                        )
                    })?
                }
            }
            Some(Value::Object(m)) => m.clone(),
            Some(other) => anyhow::bail!(
                "block {id}: column 'properties' must be a JSON object, got {other:?}"
            ),
        };
        let created_at = require_i64(&row, "created_at", &id)?;
        let updated_at = require_i64(&row, "updated_at", &id)?;
        let tags = Tags::from(require_string_array(&row, "tags", &id)?);
        let requires = require_string_array(&row, "requires", &id)?
            .into_iter()
            .map(|s| {
                EntityUri::parse_owned(s.clone()).map_err(|e| {
                    anyhow::anyhow!("block {id}: 'requires' entry {s:?} is not a valid URI: {e}")
                })
            })
            .collect::<anyhow::Result<Vec<EntityUri>>>()?;
        let marks = match row.get("marks") {
            None | Some(Value::Null) => None,
            Some(Value::Json(s)) | Some(Value::String(s)) => {
                if s.is_empty() {
                    None
                } else {
                    Some(crate::marks_from_json(s).map_err(|e| {
                        anyhow::anyhow!("block {id}: column 'marks' holds invalid JSON {s:?}: {e}")
                    })?)
                }
            }
            Some(other) => {
                anyhow::bail!("block {id}: column 'marks' must be a JSON string, got {other:?}")
            }
        };
        Ok(Block {
            id,
            parent_id,
            tags,
            requires,
            content,
            content_type,
            source_language,
            source_name,
            properties,
            marks,
            created_at,
            updated_at,
        })
    }
}

/// Metadata associated with a block.
///
/// Note: UI state like `collapsed` is NOT stored here - it's kept locally
/// in the frontend to avoid cross-user UI churn in collaborative sessions.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BlockMetadata {
    /// Unix timestamp (milliseconds) when block was created
    pub created_at: i64,
    /// Unix timestamp (milliseconds) when block was last updated
    pub updated_at: i64,
}

/// Group blocks by their owning page block.
///
/// Builds a `parent_id → children` index in one pass, then walks from each
/// page block (`is_page() == true`) to collect all descendants. Blocks whose
/// ancestor chain doesn't reach a page block are collected under `None`.
///
/// Returns `(page_id, Vec<Block>)` pairs. The page block itself is NOT
/// included in its own descendant list.
pub fn blocks_by_document(blocks: &[Block]) -> Vec<(EntityUri, Vec<Block>)> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let mut children_of: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, block) in blocks.iter().enumerate() {
        children_of
            .entry(block.parent_id.as_str())
            .or_default()
            .push(i);
    }

    let mut doc_indices: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_page())
        .map(|(i, _)| i)
        .collect();
    doc_indices.sort_by_key(|&i| {
        if blocks[i].parent_id.is_no_parent() || blocks[i].parent_id.is_sentinel() {
            1
        } else {
            0
        }
    });

    let mut claimed: HashSet<usize> = HashSet::new();
    let mut result: Vec<(EntityUri, Vec<Block>)> = Vec::new();

    for doc_idx in &doc_indices {
        claimed.insert(*doc_idx);
        let doc_id = blocks[*doc_idx].id.clone();
        let mut descendants = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(blocks[*doc_idx].id.as_str());

        while let Some(parent_key) = queue.pop_front() {
            if let Some(child_indices) = children_of.get(parent_key) {
                for &ci in child_indices {
                    if blocks[ci].is_page() {
                        continue;
                    }
                    if claimed.insert(ci) {
                        descendants.push(blocks[ci].clone());
                        queue.push_back(blocks[ci].id.as_str());
                    }
                }
            }
        }

        result.push((doc_id, descendants));
    }

    // 4. Collect orphans (blocks not reachable from any document)
    let orphans: Vec<Block> = blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| !claimed.contains(i))
        .map(|(_, b)| b.clone())
        .collect();
    if !orphans.is_empty() {
        result.push((EntityUri::no_parent(), orphans));
    }

    result
}

/// On-disk / wire representation of a [`Block`]. `Block` is serde-free because
/// its `tags`/`requires` are junction-derived edge fields; a naive derive would
/// silently drop them on every serialize path (BUG H1 — PBT fixtures lost edge
/// fields on replay). `BlockWire` carries them as **real fields** so the round-
/// trip is lossless. Field layout is byte-compatible with the pre-H1 `Block`
/// serde output: `tags`/`requires` are `#[serde(default)]` (older fixtures /
/// sidecars written before this milestone simply omit them) and empty sets are
/// elided so an edge-field-free block serializes exactly as before.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockWire {
    pub id: EntityUri,
    pub parent_id: EntityUri,
    pub content: String,
    pub content_type: ContentType,
    pub source_language: Option<SourceLanguage>,
    pub source_name: Option<String>,
    #[serde(default)]
    pub properties: HashMap<String, Value>,
    #[serde(default)]
    pub marks: Option<Vec<MarkSpan>>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Junction-derived edge field, carried explicitly (disclosed legacy default
    /// so pre-milestone files parse). See type-level doc.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Junction-derived edge field, carried explicitly. See `tags`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<EntityUri>,
}

impl From<&Block> for BlockWire {
    fn from(b: &Block) -> Self {
        BlockWire {
            id: b.id.clone(),
            parent_id: b.parent_id.clone(),
            content: b.content.clone(),
            content_type: b.content_type.clone(),
            source_language: b.source_language.clone(),
            source_name: b.source_name.clone(),
            properties: b.properties.clone(),
            marks: b.marks.clone(),
            created_at: b.created_at,
            updated_at: b.updated_at,
            tags: b.tags.to_vec(),
            requires: b.requires.clone(),
        }
    }
}

impl From<BlockWire> for Block {
    fn from(w: BlockWire) -> Self {
        Block {
            id: w.id,
            parent_id: w.parent_id,
            tags: w.tags.into(),
            requires: w.requires,
            content: w.content,
            content_type: w.content_type,
            source_language: w.source_language,
            source_name: w.source_name,
            properties: w.properties,
            marks: w.marks,
            created_at: w.created_at,
            updated_at: w.updated_at,
        }
    }
}

/// `#[serde(with = "...")]` adapter serializing a `Vec<Block>` through
/// [`BlockWire`]. Lets a serde-deriving container (PBT transitions, fixtures)
/// keep an in-memory `Vec<Block>` while persisting the lossless wire form.
pub mod block_wire_vec {
    use super::{Block, BlockWire};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(blocks: &[Block], s: S) -> Result<S::Ok, S::Error> {
        let wires: Vec<BlockWire> = blocks.iter().map(BlockWire::from).collect();
        wires.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Block>, D::Error> {
        let wires = Vec::<BlockWire>::deserialize(d)?;
        Ok(wires.into_iter().map(Block::from).collect())
    }
}

/// A block paired with its **internal fractional index** `sort_key` — the
/// ordering encoding a storage adapter keeps alongside the block (e.g. the
/// Loro tree's `fractional_index`, hex-formatted). The domain `Block` no
/// longer carries `sort_key` (ADR 0005); the file-sync diff base and the SQL
/// projector read the ordering from here and `block` for everything else.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(into = "SnapshotBlockWire", from = "SnapshotBlockWire")]
pub struct SnapshotBlock {
    pub block: Block,
    pub sort_key: String,
}

/// On-disk serde representation for [`SnapshotBlock`]. Embeds a [`BlockWire`],
/// which carries the junction-derived `tags`/`requires` edge fields as real
/// fields — a plain `Block` derive would round-trip them as empty and make every
/// cold boot re-emit a spurious edge-field write (BUG H1).
///
/// `legacy_tags`/`legacy_requires` are a **read-only backward-compat allowance**:
/// sidecars written before this milestone stored the edge fields as siblings of
/// `block` (which then lacked them). They are never emitted (empty ones elide);
/// on read they seed the block only when the wire itself carried none. Back-
/// conversion is infallible, so `from` (not `try_from`).
#[derive(Serialize, Deserialize)]
struct SnapshotBlockWire {
    block: BlockWire,
    #[serde(rename = "tags", default, skip_serializing_if = "Vec::is_empty")]
    legacy_tags: Vec<String>,
    #[serde(rename = "requires", default, skip_serializing_if = "Vec::is_empty")]
    legacy_requires: Vec<EntityUri>,
    sort_key: String,
}

impl From<SnapshotBlock> for SnapshotBlockWire {
    fn from(s: SnapshotBlock) -> Self {
        SnapshotBlockWire {
            block: BlockWire::from(&s.block),
            legacy_tags: Vec::new(),
            legacy_requires: Vec::new(),
            sort_key: s.sort_key,
        }
    }
}

impl From<SnapshotBlockWire> for SnapshotBlock {
    fn from(w: SnapshotBlockWire) -> Self {
        let mut block = Block::from(w.block);
        if block.tags.is_empty() && !w.legacy_tags.is_empty() {
            block.tags = w.legacy_tags.into();
        }
        if block.requires.is_empty() && !w.legacy_requires.is_empty() {
            block.requires = w.legacy_requires;
        }
        SnapshotBlock {
            block,
            sort_key: w.sort_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_schema_has_correct_jsonb_fields() {
        let schema = Block::type_definition();

        assert!(
            schema.field_is_jsonb("properties"),
            "properties should be JSONB"
        );

        assert!(!schema.field_is_jsonb("id"), "id should NOT be JSONB");
        assert!(
            !schema.field_is_jsonb("content"),
            "content should NOT be JSONB"
        );
        assert!(
            !schema.field_is_jsonb("parent_id"),
            "parent_id should NOT be JSONB"
        );
    }

    /// BUG H1 regression: the file-sync diff base / projection sidecar serde-
    /// persists `SnapshotBlock`, which embeds a `Block` whose `tags`/`requires`
    /// are `#[serde(skip)]`. Without the `SnapshotBlockWire` DTO this round-trip
    /// drops both edge fields, so a cold boot re-emits spurious edge-field writes
    /// for every tagged block. Must carry NON-empty `tags` AND `requires`.
    #[test]
    fn snapshot_block_serde_round_trip_preserves_edge_fields() {
        let expected_tags: Tags = vec!["Inbox".to_string(), "Page".to_string()].into();
        let expected_requires = vec![EntityUri::block("dep1"), EntityUri::block("dep2")];

        let mut block = Block::new_text(
            EntityUri::block("h1"),
            EntityUri::no_parent(),
            "x".to_string(),
        );
        block.tags = expected_tags.clone();
        block.requires = expected_requires.clone();
        let snap = SnapshotBlock {
            block,
            sort_key: "a0".to_string(),
        };

        let bytes = serde_json::to_vec(&snap).expect("serialize SnapshotBlock");
        let back: SnapshotBlock =
            serde_json::from_slice(&bytes).expect("deserialize SnapshotBlock");

        assert_eq!(back.block.tags, expected_tags);
        assert_eq!(back.block.requires, expected_requires);
        assert_eq!(back.sort_key, "a0");
    }
}
