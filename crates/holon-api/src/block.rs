use holon_macros::Entity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::entity_uri::EntityUri;
use crate::inline_mark::MarkSpan;
use crate::types::{ContentType, SourceLanguage};
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
            executed_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create a table result
    pub fn table(headers: Vec<String>, rows: Vec<Vec<Value>>) -> Self {
        Self {
            output: ResultOutput::Table { headers, rows },
            executed_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Create an error result
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            output: ResultOutput::Error {
                message: message.into(),
            },
            executed_at: chrono::Utc::now().timestamp_millis(),
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
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Entity)]
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

    /// For document blocks: the file stem name (e.g. "projects", "todo").
    /// `Some(name)` means this block is a document root; `None` means regular block.
    #[serde(default)]
    pub name: Option<String>,

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
    #[serde(default)]
    #[jsonb]
    pub properties: HashMap<String, Value>,

    /// Inline rich-text marks (Bold, Italic, Link, etc.) over `content`.
    /// `None` means the block is plain text (today's behavior); `Some(empty)`
    /// is reserved for "rich block with no active marks". The `marks IS NOT NULL`
    /// projection is the discriminator the renderer uses to decide rich vs plain.
    /// Source/Image blocks always carry `None`.
    #[serde(default)]
    #[jsonb]
    pub marks: Option<Vec<MarkSpan>>,

    // --- Timestamps (flattened from BlockMetadata) ---
    /// Unix timestamp (milliseconds) when block was created
    pub created_at: i64,

    /// Unix timestamp (milliseconds) when block was last updated
    pub updated_at: i64,

    /// Fractional-index sort key. Hex-encoded bytes consumed by
    /// `gen_key_between` (`loro_fractional_index`). Defaults to
    /// `"a0"` to match the SQL column default. Without this field on
    /// the struct, `BlockEntity::sort_key()` had no actual sort_key
    /// to return — it fell back to `self.id.as_str()` which contains
    /// non-hex `:`/`-` characters and made `gen_key_between` panic
    /// inside any `move_block` / `outdent` / `split_block` flow.
    #[serde(default = "default_sort_key")]
    pub sort_key: String,
}

fn default_sort_key() -> String {
    "a0".to_string()
}

impl Default for Block {
    fn default() -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id: EntityUri::block_random(),
            parent_id: EntityUri::no_parent(),
            name: None,
            content: String::new(),
            content_type: ContentType::Text,
            source_language: None,
            source_name: None,
            properties: HashMap::new(),
            marks: None,
            created_at: now,
            updated_at: now,
            sort_key: default_sort_key(),
        }
    }
}

impl Block {
    /// Whether this block represents a document (org file root).
    /// A block is a document iff it has a `name` (file stem).
    pub fn is_document(&self) -> bool {
        self.name.is_some()
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

    /// Get the content as a BlockContent enum (for API compatibility).
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
        self.updated_at = chrono::Utc::now().timestamp_millis();
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
        self.updated_at = chrono::Utc::now().timestamp_millis();
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
        self.updated_at = chrono::Utc::now().timestamp_millis();
    }

    /// Get source header arguments from properties (for Org Mode compatibility)
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

    /// Set source header arguments in properties (for Org Mode compatibility)
    /// flutter_rust_bridge:ignore
    pub fn set_source_header_args(&mut self, header_args: HashMap<String, Value>) {
        if !header_args.is_empty() {
            if let Ok(json) = serde_json::to_string(&header_args) {
                self.properties
                    .insert("_source_header_args".to_string(), Value::String(json));
                self.updated_at = chrono::Utc::now().timestamp_millis();
            }
        }
    }

    /// Get source results from properties (for Org Mode compatibility)
    /// flutter_rust_bridge:ignore
    pub fn get_source_results(&self) -> Option<String> {
        self.properties
            .get("_source_results")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    }

    /// Set source results in properties (for Org Mode compatibility)
    /// flutter_rust_bridge:ignore
    pub fn set_source_results(&mut self, results: Option<String>) {
        if let Some(r) = results {
            self.properties
                .insert("_source_results".to_string(), Value::String(r));
            self.updated_at = chrono::Utc::now().timestamp_millis();
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

impl TryFrom<HashMap<String, Value>> for Block {
    type Error = anyhow::Error;
    fn try_from(row: HashMap<String, Value>) -> Result<Self, Self::Error> {
        let id = row_id(&row).expect("No id found");
        let parent_id = uri_from_row(&row, "parent_id").expect("No parent_id found");
        let content = row
            .get("content")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();
        let content_type: ContentType = row
            .get("content_type")
            .and_then(|v| v.as_string())
            .unwrap_or("text")
            .parse()
            .expect("Invalid content_type in database");
        let source_language: Option<SourceLanguage> = row
            .get("source_language")
            .and_then(|v| v.as_string().map(|s| s.parse().unwrap()));
        let source_name = row
            .get("source_name")
            .and_then(|v| v.as_string().map(|s| s.to_string()));
        let properties = row
            .get("properties")
            .cloned()
            .and_then(|v| match v {
                Value::Json(s) => Some(
                    serde_json::from_str::<HashMap<String, Value>>(&s)
                        .expect("stored properties JSON must be valid"),
                ),
                _ => None,
            })
            .unwrap_or_default();
        let created_at = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let updated_at = row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0);

        let block_name = row
            .get("name")
            .and_then(|v| v.as_string().map(|s| s.to_string()));
        let sort_key = row
            .get("sort_key")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .unwrap_or_else(default_sort_key);
        // marks: stored as JSON string (Value::Json) by the entity framework's
        // #[jsonb] adapter; absent or NULL → None (plain block).
        let marks = row.get("marks").cloned().and_then(|v| match v {
            Value::Json(s) => Some(
                serde_json::from_str::<Vec<MarkSpan>>(&s).expect("stored marks JSON must be valid"),
            ),
            Value::Null => None,
            _ => None,
        });
        Ok(Block {
            id,
            parent_id,
            name: block_name,
            content,
            content_type,
            source_language,
            source_name,
            properties,
            marks,
            created_at,
            updated_at,
            sort_key,
        })
    }
}

/// A block with its tree depth/nesting level.
///
/// Used for tree-ordered iteration and diffing. The depth indicates
/// how deeply nested the block is (0 = root, 1 = child of root, etc.).
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockWithDepth {
    /// The block data
    pub block: Block,
    /// Nesting depth (0 = root level)
    pub depth: usize,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_schema_has_correct_jsonb_fields() {
        let schema = Block::type_definition();

        // These fields should be JSONB
        assert!(
            schema.field_is_jsonb("properties"),
            "properties should be JSONB"
        );

        // These fields should NOT be JSONB
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
}

/// Group blocks by their owning document block.
///
/// Builds a `parent_id → children` index in one pass, then walks from each
/// document block (is_document=true) to collect all descendants. Blocks whose
/// ancestor chain doesn't reach a document block are collected under `None`.
///
/// Returns `(doc_id, Vec<Block>)` pairs. The document block itself is NOT
/// included in its own descendant list.
pub fn blocks_by_document(blocks: &[Block]) -> Vec<(EntityUri, Vec<Block>)> {
    use std::collections::{HashMap, HashSet, VecDeque};

    // 1. Build parent → children index
    let mut children_of: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, block) in blocks.iter().enumerate() {
        children_of
            .entry(block.parent_id.as_str())
            .or_default()
            .push(i);
    }

    // 2. Find document blocks, ordered leaf-first (non-root before root).
    // This ensures nested documents claim their children before a parent
    // document's BFS traverses through them and steals their descendants.
    let mut doc_indices: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_document())
        .map(|(i, _)| i)
        .collect();
    // Documents whose parent_id is another document come first (leaf docs).
    // Root documents (parent_id = sentinel/no_parent) come last.
    doc_indices.sort_by_key(|&i| {
        if blocks[i].parent_id.is_no_parent() || blocks[i].parent_id.is_sentinel() {
            1 // root docs last
        } else {
            0 // nested docs first
        }
    });

    // 3. BFS from each document block to collect descendants
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
                    // Skip other document blocks — they form their own group
                    if blocks[ci].is_document() {
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
