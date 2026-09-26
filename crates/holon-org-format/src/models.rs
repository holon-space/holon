//! Org-mode specific extensions for Block types.
//!
//! This module provides extension traits that add org-mode specific
//! functionality to the generic Block type. Org-specific fields are stored in
//! the `properties` JSON field.

use std::collections::HashMap;
use std::collections::HashSet;

// Import Block for use in extension traits (not re-exported to avoid FRB issues)
use holon_api::MarkClass;
use holon_api::MarkSpan;
use holon_api::RenderLoss;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::ContentType;
use holon_api::types::Priority;
use holon_api::types::StateCategory;
use holon_api::types::Tags;
use holon_api::types::TaskState;
use holon_api::types::Timestamp;
use serde::Deserialize;
use serde::Serialize;

use crate::comma_escape::CommaEscape;
use crate::drawer::DrawerId;
use crate::drawer::TypedDrawerKey;
use crate::drawer::UnrepresentableId;
use crate::drawer::ValueCarrier;
use crate::parser::BlockReading;
use crate::parser::HeadlineReading;
use crate::task_keyword::TaskKeywordVocabulary;

/// Property keys for org-specific fields stored in properties JSON.
pub mod org_props {
    pub const TITLE: &str = "title";
    pub const TODO_KEYWORDS: &str = "todo_keywords";
    pub const TASK_STATE: &str = "task_state";
    /// Sidecar for TASK_STATE: "active" | "done". TASK_STATE stays a bare
    /// keyword (many consumers read it as such); the category — derived at
    /// the parse boundary from `#+TODO:` config — would otherwise be lost.
    pub const TASK_STATE_CATEGORY: &str = "task_state_category";
    pub const PRIORITY: &str = "priority";
    pub const TAGS: &str = "tags";
    pub const LEVEL: &str = "level";
    pub const SEQUENCE: &str = "sequence";
    pub const SCHEDULED: &str = "scheduled";
    pub const DEADLINE: &str = "deadline";
    pub const ORG_PROPERTIES: &str = "org_properties";
    /// JSON array of the `:PROPERTIES:` drawer keys in the order the author
    /// wrote them, recorded at parse and replayed by the renderer. Underscore
    /// prefix keeps it out of the drawer it describes.
    pub const DRAWER_ORDER: &str = "_drawer_order";
    /// Set when the `:PROPERTIES:` drawer is the SOLE carrier of the block's
    /// priority — the author wrote `:priority: A` and no `[#A]` cookie. The
    /// cookie is the default carrier, so only this exceptional case needs
    /// recording; without it write-back would invent a cookie the file never
    /// had. Underscore prefix keeps it out of the drawer.
    pub const PRIORITY_DRAWER_ONLY: &str = "_priority_drawer_only";
    /// The block's [`super::BlankLines`] as JSON, present when the file had
    /// any. Underscore prefix keeps it out of the drawer.
    pub const BLANK_LINES: &str = "_blank_lines";
    /// A headline block's drawer values as authored, by key, as a JSON object:
    /// only those the renderer would write with other bytes (spacing, an
    /// empty value). The renderer writes one back while it still reads as the
    /// block's value.
    pub const DRAWER_RAW: &str = "_drawer_raw";
    /// A block's text as the file wrote it (a JSON string), present when the
    /// renderer would write the same text with other bytes (a raw `#+` line in
    /// an example or source block). Written back while the text is unchanged.
    pub const AUTHORED_TEXT: &str = "_authored_text";
    /// A headline's `:PROPERTIES:` drawer as the file wrote it (a JSON
    /// string), present when the renderer would write it with other bytes.
    /// Written back while the block's id and drawer values are unchanged.
    pub const DRAWER_TEXT: &str = "_drawer_text";
    /// `"t"` on a block whose section had text after a source block child:
    /// the renderer writes that text before its source blocks.
    pub const TEXT_AFTER_SOURCE: &str = "_text_after_source";
    /// A headline block's [`super::KeywordLine`]s as a JSON array, present
    /// when its text in the file had any.
    pub const KEYWORD_LINES: &str = "_keyword_lines";
    /// A doc-root's keyword lines before its first headline, as a JSON array
    /// of their authored bytes in file order (each with its line break and
    /// the blank lines after it). Present only when the file had any.
    pub const HEADER_LINES: &str = "_header_lines";
    /// Where each of [`HEADER_LINES`] stands among the text before the first
    /// headline, as a JSON array of `{before_line, after_source}` in the same
    /// order. Absent: every line stands before that text.
    pub const HEADER_PLACES: &str = "_header_places";
    /// A doc-root's [`super::LineBreaks`] when its file does not use LF.
    pub const LINE_BREAKS: &str = "_line_breaks";
    /// A doc-root's FILE-LEVEL `:PROPERTIES:` drawer (org 9.0+, org-roam's
    /// identity carrier) as a JSON object in the order the author wrote it,
    /// `ID` included. Present only on a doc-root whose file had one, and its
    /// presence is what makes the renderer emit the drawer back. The one
    /// carrier an engine write may set, as its own field: it is the page's
    /// drawer, and the engine checks each key and id in it.
    pub const FILE_PROPERTIES: &str = "_file_properties";
    /// `"t"` on a doc-root whose file carried a `#+ID:` keyword. It is what
    /// keeps a file that declares its identity BOTH ways (drawer `:ID:` and
    /// `#+ID:`, in agreement) from losing one of the two on write-back.
    pub const FILE_ID_KEYWORD: &str = "_file_id_keyword";
    /// A source block's [`super::SourceLines`] as JSON, present when the
    /// renderer would write its delimiter lines with other bytes.
    pub const SOURCE_LINES: &str = "_source_lines";
    /// A headline's star count as the file wrote it, present when that is
    /// not one more than its parent's.
    pub const STARS: &str = "_stars";
    /// The spaces and tabs that end a headline line in the file, present when
    /// there are any. Org reads nothing from them.
    pub const HEADLINE_END: &str = "_headline_end";
    /// The carriers only the org parser writes: what a file held that is not
    /// block data (layout, authored bytes). An engine write from any other
    /// origin may not name them.
    pub const PARSER_CARRIERS: &[&str] = &[
        DRAWER_ORDER,
        PRIORITY_DRAWER_ONLY,
        BLANK_LINES,
        DRAWER_RAW,
        AUTHORED_TEXT,
        DRAWER_TEXT,
        TEXT_AFTER_SOURCE,
        KEYWORD_LINES,
        HEADER_LINES,
        HEADER_PLACES,
        LINE_BREAKS,
        FILE_ID_KEYWORD,
        SOURCE_LINES,
        STARS,
        HEADLINE_END,
    ];
}

/// Blank lines around a block's own text in its file: between the headline's
/// drawer and its body (before a source block's first line), and after its
/// section. Org reads nothing from them,
/// so they are kept beside the block and written back where they were. Each
/// entry is one line's bytes without its line break: nothing but whitespace.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlankLines {
    pub before_body: Vec<String>,
    pub after: Vec<String>,
}

/// A source block's lines around its text as the file wrote them. The
/// renderer writes them back while the block's head reads as it did.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceLines {
    /// Its `#+NAME:` line and `#+BEGIN_SRC` line, each with its line break.
    pub head: String,
    /// Its `#+END_SRC` line, with its line break if it had one.
    pub end: String,
    /// The head the renderer writes for the block as the file declared it.
    pub written: String,
    /// The head names no `:id`: the block's id is the one the parser mints
    /// from its place (`<parent>::src::<index>`).
    pub minted: bool,
}

/// A keyword line in a block's text as the file has it (`#+STARTUP: fold`,
/// `#+CAPTION: x`): org reads it as a keyword, so it is not block text, and it
/// is written back raw in its place.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeywordLine {
    /// The line of the block's body text it stands before; the body's line
    /// count when it comes after the last one.
    pub before_line: usize,
    /// Its bytes, with the blank lines before it, its line break and the
    /// blank lines after it.
    pub raw: String,
    /// It stood after a source block child in the file, where the renderer
    /// cannot write it.
    #[serde(default)]
    pub after_source: bool,
}

/// How a file ends its lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LineBreaks {
    #[default]
    Lf,
    /// Every line ends with CR LF.
    Crlf,
    /// Some lines end with CR LF and some with LF alone.
    Mixed,
}

impl LineBreaks {
    pub fn of(text: &str) -> Self {
        let lines = text.matches('\n').count();
        match text.matches("\r\n").count() {
            0 => Self::Lf,
            crlf if crlf == lines => Self::Crlf,
            _ => Self::Mixed,
        }
    }
}

// =============================================================================
// Path derivation utilities for org-mode
// =============================================================================

/// Trait for resolving blocks by ID (used for parent chain walking)
pub trait BlockResolver {
    /// Get a block by its ID
    fn get_block(&self, id: &str) -> Option<Block>;
}

/// Find the page ID for a block by walking up the parent chain.
///
/// Walks up the parent chain until it finds a page block (one tagged
/// with `"Page"`).
pub fn find_document_id<R: BlockResolver>(block: &Block, resolver: &R) -> Option<EntityUri> {
    if block.is_page() {
        return Some(block.id.clone());
    }

    let mut current_parent_id = block.parent_id.to_string();
    let mut visited = std::collections::HashSet::new();

    loop {
        if visited.contains(&current_parent_id) {
            return None;
        }
        visited.insert(current_parent_id.clone());

        let parent = resolver.get_block(&current_parent_id)?;
        if parent.is_page() {
            return Some(parent.id.clone());
        }
        if parent.parent_id.is_no_parent() || parent.parent_id.is_sentinel() {
            return None;
        }
        current_parent_id = parent.parent_id.to_string();
    }
}

/// Get the title (first content line) of a block's owning page.
///
/// Walks up to the nearest page ancestor and returns its title.
pub fn get_block_file_path<R: BlockResolver>(block: &Block, resolver: &R) -> Option<String> {
    let doc_id = find_document_id(block, resolver)?;
    let doc_block = resolver.get_block(doc_id.as_str())?;
    Some(doc_block.title())
}

/// Simple in-memory block resolver using a HashMap
pub struct HashMapBlockResolver {
    blocks: HashMap<String, Block>,
}

impl HashMapBlockResolver {
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
        }
    }

    pub fn insert(&mut self, block: Block) {
        self.blocks.insert(block.id.to_string(), block);
    }

    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        let mut resolver = Self::new();
        for block in blocks {
            resolver.insert(block);
        }
        resolver
    }
}

impl Default for HashMapBlockResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockResolver for HashMapBlockResolver {
    fn get_block(&self, id: &str) -> Option<Block> {
        self.blocks.get(id).cloned()
    }
}

pub use holon_api::types::DEFAULT_ACTIVE_KEYWORDS;
pub use holon_api::types::DEFAULT_DONE_KEYWORDS;

/// Check if a keyword is considered "done" using default keywords
pub fn is_done_keyword(keyword: &str) -> bool {
    DEFAULT_DONE_KEYWORDS.contains(&keyword)
}

/// True for a flat property key a headline's `:PROPERTIES:` drawer never
/// shows: Holon's own block fields and `_`-prefixed carriers.
pub fn is_hidden_drawer_key(key: &str) -> bool {
    const INTERNAL_KEYS: &[&str] = &[
        "level",
        "sequence",
        "task_state",
        "task_state_category",
        "priority",
        "tags",
        "requires",
        "advice_suppressed",
        "contributes_to",
        "scheduled",
        "deadline",
        "org_properties",
        "TODO",
        "PRIORITY",
        "TAGS",
        "SCHEDULED",
        "DEADLINE",
        "ID",
        "COLLAPSED",
        "WIDGET_ONLY",
        "_source_header_args",
        "_source_results",
    ];
    INTERNAL_KEYS.contains(&key) || key.starts_with('_')
}

/// Trait for converting entities to org-mode formatted strings
pub trait ToOrg {
    fn to_org(&self) -> String;
}

fn drawer_map(properties_json: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_str(properties_json).unwrap_or_else(|e| {
        panic!(
            "malformed org_properties JSON {properties_json:?}: {e} — silently dropping the \
             :PROPERTIES: drawer would lose :ID: and churn block identity"
        )
    })
}

/// The id a block's `:ID:` line carries: the `ID` of its drawer carrier, else
/// the block's own id.
fn headline_drawer_id(block: &Block) -> Result<DrawerId, UnrepresentableId> {
    match block
        .org_properties()
        .and_then(|json| drawer_map(&json).get("ID").map(json_text))
    {
        Some(carried) => DrawerId::parse(&carried),
        None => own_drawer_id(&block.id),
    }
}

/// `id` as an org id line writes it. The whole URI text is checked, so a
/// scheme, fragment or query the line cannot carry is refused, not dropped.
fn own_drawer_id(id: &EntityUri) -> Result<DrawerId, UnrepresentableId> {
    DrawerId::parse(id.as_str().strip_prefix("block:").unwrap_or(id.as_str()))
}

/// Format properties drawer from JSON, the `:ID:` line first.
/// Input: JSON string -> Output: ":PROPERTIES:\n:KEY: VALUE\n:END:"
fn format_properties_drawer(
    properties_json: &str,
    id: &DrawerId,
    owner: &EntityUri,
    authored: &HashMap<String, String>,
    losses: &mut Vec<RenderLoss>,
) -> String {
    let props = drawer_map(properties_json);
    let mut result = format!(":PROPERTIES:\n:ID: {}\n", id.as_str());

    // Render other properties (excluding ID which we already rendered) in the
    // JSON's own key order — serde_json::Map is an IndexMap (preserve_order
    // enabled by a transitive dependency), and the renderer built that order
    // from the author's drawer.
    for (key, value) in props.iter().filter(|(k, _)| k.as_str() != "ID") {
        let raw = authored.get(key).filter(|raw| {
            !raw.contains(['\n', '\r'])
                && ValueCarrier::HeadlineDrawer.decode(raw.trim()) == json_text(value)
        });
        match raw {
            Some(raw) => result.push_str(&format!(":{key}:{raw}\n")),
            None => result.push_str(&drawer_line(
                key,
                value,
                ValueCarrier::HeadlineDrawer,
                owner,
                losses,
            )),
        }
    }
    result.push_str(":END:");
    result
}

/// The `:PROPERTIES:` drawer the renderer writes for `block`, prepared for
/// org; none for a block with no drawer properties.
pub(crate) fn canonical_drawer(block: &Block) -> anyhow::Result<Option<String>> {
    let Some(props_json) = block.org_properties() else {
        return Ok(None);
    };
    let Ok(id) = headline_drawer_id(block) else {
        // ALLOW(ok): an id no :ID: line can carry has no canonical drawer
        return Ok(None);
    };
    let authored = read_carrier(block, org_props::DRAWER_RAW)?.unwrap_or_default();
    Ok(Some(format_properties_drawer(
        &props_json,
        &id,
        &block.id,
        &authored,
        &mut Vec::new(),
    )))
}

/// The drawer to write for `block`: the authored drawer while the block's id
/// and drawer values are what the parser read from it, else `canonical` with
/// the losses writing it records. Every authored line the file then loses,
/// and every authored drawer org reads differently from Holon, is recorded in
/// `losses`.
fn with_authored_drawer(
    block: &Block,
    props_json: &str,
    id: &DrawerId,
    (canonical, canonical_losses): (String, Vec<RenderLoss>),
    losses: &mut Vec<RenderLoss>,
) -> String {
    let Some(authored) = carrier_or_loss(
        read_carrier::<String>(block, org_props::DRAWER_TEXT),
        &block.id,
        losses,
    ) else {
        losses.extend(canonical_losses);
        return canonical;
    };
    if authored.is_empty() {
        return drawer_from_body(block, props_json, id, (canonical, canonical_losses), losses);
    }
    let reading = crate::parser::read_property_drawer(&authored);
    let values: std::collections::BTreeMap<String, String> = drawer_map(props_json)
        .iter()
        .filter(|(k, _)| TypedDrawerKey::parse(k) != Some(TypedDrawerKey::Id))
        .map(|(k, v)| (k.clone(), json_text(v)))
        .collect();
    let unedited = reading.id.as_deref() == Some(id.as_str()) && reading.values() == values;
    let id_lines = reading.id_lines();
    let (text, details) = match (&reading.unread, unedited) {
        (Some(reason), true) => (
            authored.clone(),
            vec![format!(
                "org reads no property drawer under this headline ({reason}); the drawer is \
                 written as authored, and org finds no id {} there",
                id.as_str()
            )],
        ),
        (None, true) if id_lines.len() > 1 => (
            authored.clone(),
            vec![format!(
                "the property drawer has the id lines {id_lines:?}; Holon and org-entry-get \
                 read the last, {}",
                id.as_str()
            )],
        ),
        (None, true) => (authored.clone(), Vec::new()),
        (Some(reason), false) => {
            losses.extend(canonical_losses);
            (
                format!("{canonical}\n{authored}"),
                vec![format!(
                    "org reads no property drawer in {authored:?} ({reason}); a new property \
                     drawer is written above it, and it stays as text"
                )],
            )
        }
        (None, false) => {
            losses.extend(canonical_losses);
            (
                canonical,
                id_lines
                    .iter()
                    .rev()
                    .skip(1)
                    .map(|line| format!("the property drawer line {line:?} is removed"))
                    .collect(),
            )
        }
    };
    for detail in details {
        tracing::warn!(block = %block.id, "org render: {detail}");
        losses.push(RenderLoss {
            block: block.id.clone(),
            detail,
        });
    }
    text
}

/// The drawer to write for a headline whose file wrote none, its id read from
/// a `:PROPERTIES:` drawer in its body: none while the body still names `id`
/// and the block has no other drawer value, else `canonical`. Either is a loss,
/// since org finds no id under the headline.
fn drawer_from_body(
    block: &Block,
    props_json: &str,
    id: &DrawerId,
    (canonical, canonical_losses): (String, Vec<RenderLoss>),
    losses: &mut Vec<RenderLoss>,
) -> String {
    let only_id = drawer_map(props_json)
        .keys()
        .all(|k| TypedDrawerKey::parse(k) == Some(TypedDrawerKey::Id));
    let body_names_id = block
        .body()
        .and_then(|body| crate::parser::id_in_body(&body))
        .is_some_and(|body_id| body_id == id.as_str());
    let (text, detail) = if only_id && body_names_id {
        (
            String::new(),
            format!(
                "org reads no property drawer under this headline: its :PROPERTIES: drawer \
                 follows text. Holon keeps the id {} from it, org finds none",
                id.as_str()
            ),
        )
    } else {
        losses.extend(canonical_losses);
        (
            canonical,
            format!(
                "org reads no property drawer under this headline; a property drawer with the \
                 id {} is written, and the :PROPERTIES: drawer in the body stays as text",
                id.as_str()
            ),
        )
    };
    tracing::warn!(block = %block.id, "org render: {detail}");
    losses.push(RenderLoss {
        block: block.id.clone(),
        detail,
    });
    text
}

/// A parser-owned carrier's stored JSON read as `T`; `None` when the block has
/// none. The carriers reach the store from the parser, a peer merge or an older
/// build, so an unreadable one is an error for the caller to disclose.
pub(crate) fn read_carrier<T: serde::de::DeserializeOwned>(
    block: &Block,
    key: &str,
) -> anyhow::Result<Option<T>> {
    let Some(value) = block.get_property(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let json = value
        .as_string()
        .ok_or_else(|| anyhow::anyhow!("{key} on block {} is not text: {value:?}", block.id))?;
    serde_json::from_str(json)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("{key} {json:?} on block {} is unreadable: {e}", block.id))
}

/// `carrier`'s value, or its default after a loss that names the unreadable
/// carrier: the block is then written as if it had none.
pub(crate) fn carrier_or_loss<T: Default>(
    carrier: anyhow::Result<T>,
    owner: &EntityUri,
    losses: &mut Vec<RenderLoss>,
) -> T {
    carrier.unwrap_or_else(|e| {
        let detail = format!("{e:#}; the block is written without it");
        tracing::warn!(block = %owner, "org render: {detail}");
        losses.push(RenderLoss {
            block: owner.clone(),
            detail,
        });
        T::default()
    })
}

/// One `:key: value` drawer line, the value encoded by the drawer codec.
/// Empty for a key the carrier cannot hold, which is left out of the file and
/// recorded in `losses`.
fn drawer_line(
    key: &str,
    value: &serde_json::Value,
    carrier: ValueCarrier,
    owner: &EntityUri,
    losses: &mut Vec<RenderLoss>,
) -> String {
    let key = match carrier.key(key) {
        Ok(key) => key,
        Err(e) => {
            left_out_key(owner, &e, losses);
            return String::new();
        }
    };
    format!(":{}: {}\n", key.as_str(), key.encode(&json_text(value)))
}

fn left_out_key(
    owner: &EntityUri,
    e: &crate::drawer::UnrepresentableKey,
    losses: &mut Vec<RenderLoss>,
) {
    tracing::warn!(block = %owner, "org render: {e}; the property is left out of the file");
    losses.push(RenderLoss {
        block: owner.clone(),
        detail: format!("{e}; the property is left out of the file"),
    });
}

/// The text a drawer line holds for a carrier value: a string as is, any
/// other JSON value as its JSON text.
fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

/// Format a properties drawer with the `:ID:` line omitted (dense projection).
/// Returns an empty string when no non-ID properties remain, so a block whose
/// only drawer content was its `:ID:` renders with no drawer at all.
fn format_properties_drawer_without_id(
    properties_json: &str,
    owner: &EntityUri,
    losses: &mut Vec<RenderLoss>,
) -> String {
    let lines: String = drawer_map(properties_json)
        .iter()
        .filter(|(k, _)| k.as_str() != "ID")
        .map(|(key, value)| drawer_line(key, value, ValueCarrier::HeadlineDrawer, owner, losses))
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(":PROPERTIES:\n{lines}:END:")
}

/// Format the planning line (SCHEDULED/DEADLINE).
///
/// Both keywords MUST share one line: orgize's `planning_node` parser reads
/// `(keyword, timestamp)` pairs back to back with no intervening newline,
/// then consumes a single end-of-line — a second keyword on its OWN line
/// isn't part of the same `PLANNING` node, so it (and everything meant to
/// follow it, e.g. the `:PROPERTIES:` drawer) gets swallowed into the
/// section body as plain text instead of being parsed structurally.
fn format_planning(scheduled: Option<&str>, deadline: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(sched) = scheduled {
        parts.push(format!("SCHEDULED: {}", sched.trim()));
    }
    if let Some(dead) = deadline {
        parts.push(format!("DEADLINE: {}", dead.trim()));
    }
    if parts.is_empty() {
        return String::new();
    }
    parts.join(" ") + "\n"
}

/// Format header arguments with Value types as Org Mode inline parameters.
/// Input: `{ "connection": String("main"), "results": String("table") }`
/// Output: `:connection main :results table`
fn format_header_args_value(args: &HashMap<String, holon_api::Value>) -> String {
    if args.is_empty() {
        return String::new();
    }

    let mut parts: Vec<String> = args
        .iter()
        .map(|(k, v)| {
            let v_str = match v {
                holon_api::Value::String(s) => s.clone(),
                holon_api::Value::Integer(i) => i.to_string(),
                holon_api::Value::Float(f) => f.to_string(),
                holon_api::Value::Boolean(b) => b.to_string(),
                holon_api::Value::Null => String::new(),
                holon_api::Value::Json(j) => j.to_string(),
                holon_api::Value::DateTime(dt) => dt.to_string(),
                holon_api::Value::Array(_) => "[array]".to_string(),
                holon_api::Value::Object(_) => "[object]".to_string(),
                holon_api::Value::Removed(_) => panic!(
                    "source header args cannot carry Value::REMOVED — it is a write-leg sentinel"
                ),
            };
            if v_str.is_empty() {
                format!(":{}", k)
            } else {
                format!(":{} {}", k, v_str)
            }
        })
        .collect();

    parts.sort();
    parts.join(" ")
}

// =============================================================================
// OrgDocumentExt - Extension trait for Document with org-specific functionality
// =============================================================================

/// Extension trait for document blocks (those with a `name`) with org-mode
/// specific functionality.
///
/// Provides accessors for org-specific fields stored in the properties JSON:
/// - title: #+TITLE value
/// - todo_keywords: Custom TODO keyword configuration
pub trait OrgDocumentExt {
    /// Get the org file title (#+TITLE value) from the document block's
    /// properties.
    fn file_title(&self) -> Option<String>;

    /// Set the org file title (#+TITLE value)
    fn set_file_title(&mut self, title: Option<String>);

    /// The file-level `:PROPERTIES:` drawer, keys in the order the author
    /// wrote them and `ID` included. `None` when the file had no such drawer;
    /// an error when the carrier is not a JSON object.
    fn file_drawer(&self) -> anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>>;

    /// Set (or clear) the file-level `:PROPERTIES:` drawer.
    fn set_file_drawer(&mut self, drawer: Option<serde_json::Map<String, serde_json::Value>>);

    /// Get the TODO keywords as TaskState objects.
    fn todo_keywords(&self) -> Option<Vec<TaskState>>;

    /// Set the TODO keywords from TaskState objects.
    fn set_todo_keywords(&mut self, keywords: Option<Vec<TaskState>>);

    /// Parse TODO keywords configuration into (active, done) keyword lists.
    fn parse_todo_keywords(&self) -> (Vec<String>, Vec<String>);

    /// Check if a keyword is "done" according to this document's configuration
    fn is_done(&self, keyword: &str) -> bool;

    /// The file's keyword lines before its first headline, as authored; empty
    /// for a page that never came from a file.
    fn header_lines(&self) -> anyhow::Result<Vec<String>>;

    fn set_header_lines(&mut self, lines: Vec<String>);

    /// The header lines with their places among the text before the first
    /// headline.
    fn header_keyword_lines(&self) -> anyhow::Result<Vec<KeywordLine>>;

    fn set_header_keyword_lines(&mut self, lines: Vec<KeywordLine>);

    fn line_breaks(&self) -> anyhow::Result<LineBreaks>;

    fn set_line_breaks(&mut self, line_breaks: LineBreaks);
}

impl OrgDocumentExt for Block {
    fn header_lines(&self) -> anyhow::Result<Vec<String>> {
        Ok(read_carrier(self, org_props::HEADER_LINES)?.unwrap_or_default())
    }

    fn set_header_lines(&mut self, lines: Vec<String>) {
        if lines.is_empty() {
            self.properties.remove(org_props::HEADER_LINES);
        } else {
            self.set_property(
                org_props::HEADER_LINES,
                serde_json::to_string(&lines).expect("a string list serializes to JSON"),
            );
        }
    }

    fn header_keyword_lines(&self) -> anyhow::Result<Vec<KeywordLine>> {
        #[derive(serde::Deserialize)]
        struct Place {
            before_line: usize,
            after_source: bool,
        }
        let lines = self.header_lines()?;
        let places: Vec<Place> = match read_carrier(self, org_props::HEADER_PLACES)? {
            Some(places) => places,
            None => lines
                .iter()
                .map(|_| Place {
                    before_line: 0,
                    after_source: false,
                })
                .collect(),
        };
        anyhow::ensure!(
            places.len() == lines.len(),
            "{} and {} of page {} disagree",
            org_props::HEADER_PLACES,
            org_props::HEADER_LINES,
            self.id
        );
        Ok(lines
            .into_iter()
            .zip(places)
            .map(|(raw, place)| KeywordLine {
                before_line: place.before_line,
                raw,
                after_source: place.after_source,
            })
            .collect())
    }

    fn set_header_keyword_lines(&mut self, lines: Vec<KeywordLine>) {
        let places: Vec<serde_json::Value> = lines
            .iter()
            .map(|l| serde_json::json!({"before_line": l.before_line, "after_source": l.after_source}))
            .collect();
        let placed = lines.iter().any(|l| l.before_line > 0 || l.after_source);
        self.set_header_lines(lines.into_iter().map(|l| l.raw).collect());
        if placed {
            self.set_property(
                org_props::HEADER_PLACES,
                serde_json::Value::Array(places).to_string(),
            );
        } else {
            self.properties.remove(org_props::HEADER_PLACES);
        }
    }

    fn line_breaks(&self) -> anyhow::Result<LineBreaks> {
        Ok(read_carrier(self, org_props::LINE_BREAKS)?.unwrap_or_default())
    }

    fn set_line_breaks(&mut self, line_breaks: LineBreaks) {
        if line_breaks == LineBreaks::Lf {
            self.properties.remove(org_props::LINE_BREAKS);
        } else {
            self.set_property(
                org_props::LINE_BREAKS,
                serde_json::to_string(&line_breaks).expect("LineBreaks serializes to JSON"),
            );
        }
    }

    fn file_title(&self) -> Option<String> {
        self.get_property(org_props::TITLE)
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    }

    fn set_file_title(&mut self, title: Option<String>) {
        if let Some(t) = title {
            self.set_property(org_props::TITLE, t);
        } else {
            self.properties.remove(org_props::TITLE);
        }
    }

    fn file_drawer(&self) -> anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>> {
        read_carrier(self, org_props::FILE_PROPERTIES)
    }

    fn set_file_drawer(&mut self, drawer: Option<serde_json::Map<String, serde_json::Value>>) {
        match drawer {
            Some(d) => self.set_property(
                org_props::FILE_PROPERTIES,
                serde_json::to_string(&d)
                    .expect("a file-level drawer is a string map — always serializable"),
            ),
            None => {
                self.properties.remove(org_props::FILE_PROPERTIES);
            }
        }
    }

    fn todo_keywords(&self) -> Option<Vec<TaskState>> {
        let value = self.get_property(org_props::TODO_KEYWORDS)?;
        let json_str = value.as_string()?;
        // Try new JSON array format first, fall back to legacy
        // "ACTIVE1,ACTIVE2|DONE1,DONE2"
        if let Ok(states) = serde_json::from_str::<Vec<TaskState>>(json_str) {
            return Some(states);
        }
        // Legacy format: "TODO,DOING|DONE,CANCELLED"
        let parts: Vec<&str> = json_str.split('|').collect();
        let done_kws: Vec<String> = parts
            .get(1)
            .map(|s| s.split(',').map(|k| k.trim().to_string()).collect())
            .unwrap_or_default();
        let mut states = Vec::new();
        if let Some(active_str) = parts.first() {
            for kw in active_str.split(',').map(|k| k.trim()) {
                if !kw.is_empty() {
                    states.push(TaskState::active(kw));
                }
            }
        }
        for kw in &done_kws {
            if !kw.is_empty() {
                states.push(TaskState::done(kw));
            }
        }
        if states.is_empty() {
            None
        } else {
            Some(states)
        }
    }

    fn set_todo_keywords(&mut self, keywords: Option<Vec<TaskState>>) {
        if let Some(kws) = keywords {
            let json = serde_json::to_string(&kws).expect("TaskState serializes to JSON");
            self.set_property(org_props::TODO_KEYWORDS, json);
        } else {
            self.properties.remove(org_props::TODO_KEYWORDS);
        }
    }

    fn parse_todo_keywords(&self) -> (Vec<String>, Vec<String>) {
        if let Some(states) = self.todo_keywords() {
            let active: Vec<String> = states
                .iter()
                .filter(|s| s.is_active())
                .map(|s| s.keyword.clone())
                .collect();
            let done: Vec<String> = states
                .iter()
                .filter(|s| s.is_done())
                .map(|s| s.keyword.clone())
                .collect();
            (
                if active.is_empty() {
                    vec!["TODO".to_string()]
                } else {
                    active
                },
                if done.is_empty() {
                    vec!["DONE".to_string()]
                } else {
                    done
                },
            )
        } else {
            (vec!["TODO".to_string()], vec!["DONE".to_string()])
        }
    }

    fn is_done(&self, keyword: &str) -> bool {
        let (_, done_keywords) = self.parse_todo_keywords();
        done_keywords.contains(&keyword.to_string())
    }
}

/// Renders the file-level org header (#+TITLE, #+TODO) from a document block's
/// properties.
/// Trim whole BLANK (whitespace-only) lines off both ends of a body, keeping
/// every surviving line's own indentation.
///
/// `str::trim` cannot do this: it also eats the FIRST content line's leading
/// spaces, so an indented body came back with line 1 flush-left and every
/// later line still indented. Both the renderer and the parser go through
/// here so the two ends agree and `render(parse(render(x))) == render(x)`.
pub(crate) fn trim_blank_lines(s: &str) -> &str {
    let mut start = 0usize;
    let mut end = s.len();
    loop {
        match s[start..end].find('\n') {
            Some(i) if s[start..start + i].trim().is_empty() => start += i + 1,
            _ => break,
        }
    }
    while let Some(i) = s[start..end].rfind('\n') {
        let nl = start + i;
        if s[nl + 1..end].trim().is_empty() {
            end = nl;
        } else {
            break;
        }
    }
    if s[start..end].trim().is_empty() {
        ""
    } else {
        &s[start..end]
    }
}

/// A page's file-level drawer, and its keyword lines before the first headline
/// with their places among the text there; refused when its id is one no
/// `#+ID:` line holds. A page with a `file:` id keeps its path identity and
/// gets no id line.
pub(crate) fn document_head(
    doc_block: &Block,
    edits: &crate::page_keywords::Edits,
    losses: &mut Vec<RenderLoss>,
) -> anyhow::Result<(String, Vec<KeywordLine>)> {
    let id = if doc_block.id.is_file() {
        None
    } else {
        Some(
            own_drawer_id(&doc_block.id)
                .map_err(|e| anyhow::anyhow!("org render of page {} refused: {e}", doc_block.id))?,
        )
    };
    let mut result = String::new();

    // A hand-authored FILE-LEVEL `:PROPERTIES:` drawer goes first and verbatim:
    // org-mode (and orgize's `document_node`) only recognize it as the file's
    // own drawer when it is the first element of the file, so any other
    // placement would silently demote it to body text on the next read.
    let file_drawer = carrier_or_loss(doc_block.file_drawer(), &doc_block.id, losses);
    let drawer_carries_id = match &file_drawer {
        Some(drawer) => {
            let mut carries_id = false;
            result.push_str(":PROPERTIES:\n");
            for (key, value) in drawer {
                let authored = match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                // The `:ID:` line is re-derived from the block's CURRENT
                // identity rather than replayed, so the drawer and the document
                // can never drift apart across a rename or a promotion.
                //
                // An EMPTY `:ID:` is NOT the identity carrier and must not be
                // filled in — `EntityUri::id()` of a name-chain document is its
                // PATH, so substituting there would write the file's own path
                // into the drawer as if the author had chosen it. This mirrors
                // the parser's `drawer_id`, which rejects an empty value for the
                // same reason; the two guards must agree or write-back invents
                // an identity the parse will not accept back.
                if key.eq_ignore_ascii_case("ID") && !authored.is_empty() {
                    let id = id.as_ref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "org render of page {} refused: its file drawer carries :ID: \
                             {authored:?} but the page has no block id to write there",
                            doc_block.id
                        )
                    })?;
                    carries_id = true;
                    result.push_str(&format!(":{key}: {}\n", id.as_str()));
                } else if key.eq_ignore_ascii_case("ID") {
                    result.push_str(&format!(":{key}: \n"));
                } else {
                    result.push_str(&drawer_line(
                        key,
                        value,
                        ValueCarrier::FileDrawer,
                        &doc_block.id,
                        losses,
                    ));
                }
            }
            result.push_str(":END:\n");
            carries_id
        }
        None => false,
    };

    // Document identity. Files identified by a stable `block:<uuid>` get a
    // `#+ID:` directive so the id travels with the file (rename-safe).
    // Files still using the transient path-derived `file:` URI render
    // without `#+ID:` — they keep name-chain identity until promoted.
    //
    // A drawer that already carries `:ID:` IS the identity carrier, so no
    // `#+ID:` is added beside it — the file keeps the single carrier its author
    // chose instead of growing a second one on every write-back. A file that
    // authored BOTH (in agreement — the parser rejects disagreement) keeps both.
    let authored_id_keyword = doc_block.get_property(org_props::FILE_ID_KEYWORD).is_some();
    let id = id.filter(|_| !drawer_carries_id || authored_id_keyword);

    // The page id is Holon's own header value: written in the first `#+ID:`
    // line while that line still reads as the id, regenerated there otherwise,
    // and added when the file has none. Title and task keywords follow `edits`.
    let mut id_line = Some(id.as_ref().map(|id| format!("#+ID: {}\n", id.as_str())));
    let mut header = Vec::new();
    let header_lines = carrier_or_loss(doc_block.header_keyword_lines(), &doc_block.id, losses);
    for (i, mut line) in header_lines.into_iter().enumerate() {
        if !writable_keyword_line(&line, &doc_block.id, losses) {
            continue;
        }
        if let Some(edit) = edits.lines.get(&crate::page_keywords::LineAt::Header(i)) {
            match edit {
                Some(raw) => line.raw = raw.clone(),
                None => continue,
            }
        } else if let Some(authored) = crate::comma_escape::page_id_keyword_value(&line.raw)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
        {
            match id_line.take() {
                None => {}
                Some(Some(_)) if id.as_ref().is_some_and(|id| id.as_str() == authored) => {}
                Some(Some(regenerated)) => line.raw = in_authored_line(&line.raw, &regenerated),
                Some(None) => continue,
            }
        }
        header.push(line);
    }
    header.extend(
        id_line
            .flatten()
            .into_iter()
            .chain(edits.header_additions.iter().cloned())
            .map(|raw| KeywordLine {
                before_line: 0,
                raw,
                after_source: false,
            }),
    );
    Ok((result, header))
}

/// `regenerated` (`#+KEY: value\n`) in the place of the keyword line in
/// `authored`: with its blank lines around it, its indentation and the key's
/// authored spelling.
pub(crate) fn in_authored_line(authored: &str, regenerated: &str) -> String {
    let (_, value) = regenerated
        .split_once(':')
        .expect("a regenerated header line is `#+KEY: value`");
    let mut out = String::new();
    for line in authored.split_inclusive('\n') {
        match line
            .split_once(':')
            .filter(|_| line.trim_start().starts_with("#+"))
        {
            Some((key, _)) => {
                out.push_str(key);
                out.push(':');
                out.push_str(value);
            }
            None => out.push_str(line),
        }
    }
    out
}

/// The file-level drawer and the keyword lines before the first headline, in
/// file order: [`document_head`]'s lines written one after the other.
pub fn render_document_header(
    doc_block: &Block,
    losses: &mut Vec<RenderLoss>,
) -> anyhow::Result<String> {
    let lines: Vec<(crate::page_keywords::LineAt, String)> = doc_block
        .header_keyword_lines()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, l)| (crate::page_keywords::LineAt::Header(i), l.raw))
        .collect();
    let current =
        crate::page_keywords::Reading::of_lines(lines.iter().map(|(_, raw)| raw.as_str()));
    let edits = crate::page_keywords::edits(doc_block, &current, &lines, losses);
    let (mut result, header) = document_head(doc_block, &edits, losses)?;
    for line in header {
        result.push_str(&line.raw);
        if !result.ends_with('\n') {
            result.push('\n');
        }
    }
    Ok(result)
}

// =============================================================================
// OrgBlockExt - Extension trait for Block with org-specific functionality
// =============================================================================

/// Extension trait for Block with org-mode specific functionality.
///
/// Provides accessors for org-specific fields stored in properties JSON:
/// - level: Headline level (number of stars)
/// - sequence: Ordering within file
/// - task_state: TODO keyword
/// - priority: the rank A=1, B=2, C=3 (ascending sort = by importance)
/// - tags: Comma-separated tag list
/// - scheduled/deadline: Planning timestamps
/// - source_blocks: Embedded source blocks
pub trait OrgBlockExt {
    /// Get the headline level (number of stars: 1-6)
    fn level(&self) -> i64;

    /// Set the headline level
    fn set_level(&mut self, level: i64);

    /// Get the sequence number for ordering
    fn sequence(&self) -> i64;

    /// Set the sequence number
    fn set_sequence(&mut self, sequence: i64);

    /// Get the headline title (first line of content)
    fn org_title(&self) -> String;

    /// Get the body text (content after first line)
    fn body(&self) -> Option<String>;

    /// Replace content with `title` + `body`, both plain literals. Any
    /// existing marks are dropped: their spans indexed the old content, and
    /// keeping them would project styling onto unrelated characters.
    fn set_title_and_body(&mut self, title: String, body: Option<String>);

    /// Get the task state (TODO keyword)
    fn task_state(&self) -> Option<TaskState>;

    /// Set the task state
    fn set_task_state(&mut self, state: Option<TaskState>);

    /// Get the priority
    fn priority(&self) -> Option<Priority>;

    /// Set the priority
    fn set_priority(&mut self, priority: Option<Priority>);

    /// The `:PROPERTIES:` drawer keys in the order the author wrote them, as
    /// recorded by the parser. Empty for blocks that never came from a file.
    fn authored_drawer_order(&self) -> anyhow::Result<Vec<String>>;

    /// The blank lines the block's file had around its text; none for a block
    /// that never came from a file.
    fn blank_lines(&self) -> anyhow::Result<BlankLines>;

    fn set_blank_lines(&mut self, blank_lines: BlankLines);

    /// The keyword lines the block's text had in its file; none for a block
    /// that never came from a file.
    fn keyword_lines(&self) -> anyhow::Result<Vec<KeywordLine>>;

    fn set_keyword_lines(&mut self, lines: Vec<KeywordLine>);

    /// Get the tags
    fn tags(&self) -> Tags;

    /// Set the tags
    fn set_tags(&mut self, tags: Tags);

    /// Get the scheduled timestamp
    fn scheduled(&self) -> Option<Timestamp>;

    /// Set the scheduled timestamp
    fn set_scheduled(&mut self, scheduled: Option<Timestamp>);

    /// Get the deadline timestamp
    fn deadline(&self) -> Option<Timestamp>;

    /// Set the deadline timestamp
    fn set_deadline(&mut self, deadline: Option<Timestamp>);

    /// Get the org properties drawer as JSON
    fn org_properties(&self) -> Option<String>;

    /// Set the org properties drawer
    fn set_org_properties(&mut self, properties: Option<String>);

    /// Get custom drawer properties (properties that are not internal org keys)
    fn drawer_properties(&self) -> HashMap<String, String>;

    /// Parse tags from comma-separated string
    fn get_tags(&self) -> Vec<String>;

    /// Check if this block is completed (using default keywords)
    fn is_completed(&self) -> bool;

    /// Get the block ID from the properties drawer
    fn get_block_id(&self) -> Option<String>;
}

impl OrgBlockExt for Block {
    fn level(&self) -> i64 {
        self.get_property(org_props::LEVEL)
            .and_then(|v| v.as_i64())
            .unwrap_or(1)
    }

    fn set_level(&mut self, level: i64) {
        self.set_property(org_props::LEVEL, holon_api::Value::Integer(level));
    }

    fn sequence(&self) -> i64 {
        self.get_property(org_props::SEQUENCE)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    }

    fn set_sequence(&mut self, sequence: i64) {
        self.set_property(org_props::SEQUENCE, holon_api::Value::Integer(sequence));
    }

    fn org_title(&self) -> String {
        self.content
            .lines()
            .next()
            .unwrap_or("")
            .trim_end()
            .to_string()
    }

    fn body(&self) -> Option<String> {
        let lines: Vec<&str> = self.content.lines().collect();
        if lines.len() > 1 {
            Some(lines[1..].join("\n"))
        } else {
            None
        }
    }

    fn set_title_and_body(&mut self, title: String, body: Option<String>) {
        if let Some(b) = body {
            self.content = format!("{}\n{}", title, b);
        } else {
            self.content = title;
        }
        self.marks = None;
        self.updated_at = holon_api::clock::now_millis();
    }

    fn task_state(&self) -> Option<TaskState> {
        let keyword = self
            .get_property(org_props::TASK_STATE)
            .and_then(|v| v.as_string().map(str::to_string))?;
        match self.get_property(org_props::TASK_STATE_CATEGORY) {
            Some(v) => {
                let category = match v.as_string() {
                    Some("active") => StateCategory::Active,
                    Some("done") => StateCategory::Done,
                    other => panic!(
                        "corrupt task_state_category {:?} on block {} (expected \"active\" or \
                         \"done\")",
                        other, self.id
                    ),
                };
                Some(TaskState::new(keyword, category))
            }
            // Legacy data / writers that only set the keyword.
            None => Some(TaskState::from_keyword(&keyword)),
        }
    }

    fn set_task_state(&mut self, state: Option<TaskState>) {
        if let Some(s) = state {
            let category = s.category.as_str();
            self.set_property(
                org_props::TASK_STATE,
                holon_api::Value::String(s.keyword.clone()),
            );
            self.set_property(
                org_props::TASK_STATE_CATEGORY,
                holon_api::Value::String(category.to_string()),
            );
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::TASK_STATE);
            props.remove(org_props::TASK_STATE_CATEGORY);
            self.set_properties_map(props);
        }
    }

    fn priority(&self) -> Option<Priority> {
        self.get_property(org_props::PRIORITY)
            .and_then(|v| v.as_i64())
            .and_then(|i| Priority::from_rank(i as i32).ok()) // ALLOW(ok):
        // boundary parse
    }

    fn set_priority(&mut self, priority: Option<Priority>) {
        if let Some(p) = priority {
            self.set_property(
                org_props::PRIORITY,
                holon_api::Value::Integer(p.rank() as i64),
            );
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::PRIORITY);
            self.set_properties_map(props);
        }
    }

    fn authored_drawer_order(&self) -> anyhow::Result<Vec<String>> {
        Ok(read_carrier(self, org_props::DRAWER_ORDER)?.unwrap_or_default())
    }

    fn blank_lines(&self) -> anyhow::Result<BlankLines> {
        let blank_lines: BlankLines =
            read_carrier(self, org_props::BLANK_LINES)?.unwrap_or_default();
        anyhow::ensure!(
            blank_lines
                .before_body
                .iter()
                .chain(&blank_lines.after)
                .all(|line| line.trim().is_empty() && !line.contains('\n')),
            "{} on block {} holds a line with text",
            org_props::BLANK_LINES,
            self.id
        );
        Ok(blank_lines)
    }

    fn set_blank_lines(&mut self, blank_lines: BlankLines) {
        if blank_lines == BlankLines::default() {
            self.properties.remove(org_props::BLANK_LINES);
        } else {
            self.set_property(
                org_props::BLANK_LINES,
                holon_api::Value::String(
                    serde_json::to_string(&blank_lines).expect("BlankLines serializes to JSON"),
                ),
            );
        }
    }

    fn keyword_lines(&self) -> anyhow::Result<Vec<KeywordLine>> {
        Ok(read_carrier(self, org_props::KEYWORD_LINES)?.unwrap_or_default())
    }

    fn set_keyword_lines(&mut self, lines: Vec<KeywordLine>) {
        if lines.is_empty() {
            self.properties.remove(org_props::KEYWORD_LINES);
        } else {
            self.set_property(
                org_props::KEYWORD_LINES,
                holon_api::Value::String(
                    serde_json::to_string(&lines).expect("KeywordLine serializes to JSON"),
                ),
            );
        }
    }

    fn tags(&self) -> Tags {
        self.tags.clone()
    }

    fn set_tags(&mut self, tags: Tags) {
        self.tags = tags;
    }

    fn scheduled(&self) -> Option<Timestamp> {
        self.get_property(org_props::SCHEDULED)
            // ALLOW(ok): boundary parse from org property
            .and_then(|v| v.as_string().and_then(|s| Timestamp::parse(s).ok()))
    }

    fn set_scheduled(&mut self, scheduled: Option<Timestamp>) {
        if let Some(s) = scheduled {
            self.set_property(
                org_props::SCHEDULED,
                holon_api::Value::String(s.to_string()),
            );
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::SCHEDULED);
            self.set_properties_map(props);
        }
    }

    fn deadline(&self) -> Option<Timestamp> {
        self.get_property(org_props::DEADLINE)
            // ALLOW(ok): boundary parse from org property
            .and_then(|v| v.as_string().and_then(|s| Timestamp::parse(s).ok()))
    }

    fn set_deadline(&mut self, deadline: Option<Timestamp>) {
        if let Some(d) = deadline {
            self.set_property(org_props::DEADLINE, holon_api::Value::String(d.to_string()));
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::DEADLINE);
            self.set_properties_map(props);
        }
    }

    fn org_properties(&self) -> Option<String> {
        self.get_property(org_props::ORG_PROPERTIES)
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    }

    fn set_org_properties(&mut self, properties: Option<String>) {
        if let Some(p) = properties {
            self.set_property(org_props::ORG_PROPERTIES, holon_api::Value::String(p));
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::ORG_PROPERTIES);
            self.set_properties_map(props);
        }
    }

    fn get_tags(&self) -> Vec<String> {
        self.tags().to_vec()
    }

    fn is_completed(&self) -> bool {
        self.task_state().map(|ts| ts.is_done()).unwrap_or(false)
    }

    fn drawer_properties(&self) -> HashMap<String, String> {
        let mut result = HashMap::new();

        // First, extract from the org_properties JSON if present: its keys
        // are drawer keys.
        if let Some(json) = self.org_properties() {
            if let Ok(props) = serde_json::from_str::<HashMap<String, String>>(&json) {
                for (k, v) in props {
                    if k != "ID" {
                        result.insert(k, v);
                    }
                }
            } else if let Ok(props) =
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
            {
                for (k, v) in props {
                    if k != "ID" {
                        let v_str = match &v {
                            serde_json::Value::String(s) => s.clone(),
                            _ => v.to_string(),
                        };
                        result.insert(k, v_str);
                    }
                }
            }
        }

        // Also include any flat properties that are not internal.
        for (k, v) in &self.properties {
            if is_hidden_drawer_key(k) {
                continue;
            }
            if let (Some(key), Some(s)) =
                (crate::drawer::AuthoredKey::from_property(k), v.as_string())
            {
                result
                    .entry(key.as_str().to_string())
                    .or_insert_with(|| s.to_string());
            }
        }

        // A priority the author spelled in the DRAWER is reconstructed from the
        // typed field under the key they used — `priority` is an INTERNAL_KEY,
        // so the flat loop above can never emit it, and without this the
        // authored line would simply vanish on write-back. Presence in the
        // authored order is what says the drawer carried it at all.
        if let Some(priority) = self.priority() {
            let authored_key = self
                .authored_drawer_order()
                .unwrap_or_default()
                .into_iter()
                .find(|k| k.eq_ignore_ascii_case(org_props::PRIORITY));
            if let Some(key) = authored_key {
                result.insert(key, priority.letter().to_string());
            }
        }

        // `requires` is a typed Vec<String> on Block (edge field, hydrated from
        // the block_requires junction) — the parser pulls it out of the drawer
        // and the renderer must put it back. Stored values are `block:` URIs
        // (added at parse boundary); strip the scheme on the way out so the
        // org file keeps bare slugs (per docs/Reference/ORG_SYNTAX.md). Joined with
        // spaces (org-edna convention).
        //
        // Rendered under the canonical `:REQUIRES:` drawer key (owner ruling
        // 2026-07-16). `:BLOCKED-BY:` is accepted as an input alias by the parser
        // and converges to `:REQUIRES:` on write-back — both name the SAME
        // `block_requires` edge (there is no distinct `BlockedBy` EdgeField; see
        // block_requires.sql and crates/holon-api/src/edge_field.rs).
        if !self.requires.is_empty() {
            // Sort the bare slugs: this edge is a SET of blockers (order is not
            // semantic), and the junction hydration (`json_group_array` over
            // `block_requires`, no ORDER BY) does not guarantee insertion order.
            // A sorted canonical form makes the org round-trip deterministic
            // through the store regardless of aggregation order.
            let mut bare: Vec<String> = self
                .requires
                .iter()
                .map(|uri| uri.id().to_string())
                .collect();
            bare.sort();
            result.insert("REQUIRES".to_string(), bare.join(" "));
        }

        // `advice_suppressed` mirrors `requires`: a typed edge field on Block
        // (hydrated from the advice_suppressed junction) reconstructed into the
        // `:ADVICE_SUPPRESSED:` drawer with the scheme stripped (bare slugs).
        // See ADR 0021.
        if !self.advice_suppressed.is_empty() {
            let bare: Vec<String> = self
                .advice_suppressed
                .iter()
                .map(|uri| uri.id().to_string())
                .collect();
            result.insert("ADVICE_SUPPRESSED".to_string(), bare.join(" "));
        }

        // `contributes_to` mirrors `requires` — a typed edge field rebuilt into
        // its drawer with the scheme stripped, sorted so the round-trip is
        // deterministic through the store's unordered junction hydration. The
        // key is lowercase kebab-case: Compass keys are lowercase by convention
        // so they can never collide with org's own UPPER-CASE typed keys
        // (docs/Reference/CompassConventions.md).
        if !self.contributes_to.is_empty() {
            let mut bare: Vec<String> = self
                .contributes_to
                .iter()
                .map(|uri| uri.id().to_string())
                .collect();
            bare.sort();
            result.insert("contributes-to".to_string(), bare.join(" "));
        }

        // `collapsed` is document state (Martin ruling 2026-07-11), written
        // only when folded — matches `requires`/`advice_suppressed`'s
        // only-if-non-empty convention so a never-collapsed file's drawer
        // stays exactly as before this field existed.
        if self.collapsed {
            result.insert("COLLAPSED".to_string(), "t".to_string());
        }

        // `widget_only` follows `collapsed`'s only-when-set convention so a
        // file that never uses widget-only rendering keeps its drawer verbatim.
        if self.widget_only {
            result.insert("WIDGET_ONLY".to_string(), "t".to_string());
        }

        result
    }

    fn get_block_id(&self) -> Option<String> {
        self.org_properties()
            // ALLOW(ok): org properties may contain non-JSON
            .and_then(|json| serde_json::from_str::<HashMap<String, String>>(&json).ok())
            .and_then(|props| props.get("ID").cloned())
            .or_else(|| {
                self.org_properties()
                    .and_then(|json| {
                        // ALLOW(ok): org properties may contain non-JSON
                        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
                            .ok()
                    })
                    .and_then(|props| {
                        props
                            .get("ID")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
            })
    }
}

/// How a headline block's stable identity is emitted.
///
/// The canonical org file carries identity in the `:PROPERTIES:/:ID:/:END:`
/// drawer. A DENSE projection (agent-facing, projection-only — see
/// `crate::dense`) instead compresses that three-line scaffolding to a single
/// trailing headline token `{#<alias>}`, where `<alias>` is a short per-query
/// handle. This enum is the ONE branch point between the two forms so the
/// headline-building logic stays a single implementation.
pub(crate) enum HeadlineIdentity<'a> {
    /// Canonical: `:ID:` inside the properties drawer, in a file whose task
    /// keywords are `vocabulary`.
    Drawer {
        id: DrawerId,
        vocabulary: &'a TaskKeywordVocabulary,
    },
    /// Dense projection: trailing `{#alias}` token, `:ID:` line suppressed.
    /// `gap` renders a `^` inside the token (`{#alias^}`) meaning one or more
    /// unselected ancestors were elided above this block — its rendered parent
    /// is NOT its true parent. Display-only (see `crate::dense`).
    DenseToken { alias: &'a str, gap: bool },
}

/// A block's org text, refused when its `:ID:` line cannot carry its id.
/// `vocabulary` is the task keywords of the file it is written to.
/// `minted_here` is the id the parser mints for a source block with no
/// `:id` where `block` is written.
pub(crate) fn block_to_org(
    block: &Block,
    vocabulary: &TaskKeywordVocabulary,
    minted_here: Option<&str>,
    losses: &mut Vec<RenderLoss>,
) -> anyhow::Result<String> {
    let refused =
        |e: UnrepresentableId| anyhow::anyhow!("org render of block {} refused: {e}", block.id);
    if block.content_type == ContentType::Source {
        return source_block_to_org(block, minted_here, losses);
    }

    // Image blocks render as [[file:path]] inline link
    if block.content_type == ContentType::Image {
        return Ok(format!("[[file:{}]]\n", block.content));
    }

    let id = headline_drawer_id(block).map_err(refused)?;
    if let Err(e) = read_carrier::<i64>(block, org_props::STARS) {
        carrier_or_loss::<()>(Err(e), &block.id, losses);
    }
    Ok(render_headline_block(
        block,
        HeadlineIdentity::Drawer { id, vocabulary },
        losses,
    ))
}

impl ToOrg for Block {
    fn to_org(&self) -> String {
        block_to_org(
            self,
            &TaskKeywordVocabulary::default(),
            None,
            &mut Vec::new(),
        )
        .unwrap_or_else(|e| panic!("{e:#}"))
    }
}

/// The org bytes for a block's `content` + `marks`, quoting every literal span
/// org would otherwise eat as emphasis so the bytes parse back to what the
/// store holds (`__default__` stays `__default__` instead of returning as
/// `default`). Marked blocks included: the literal gaps between marks are
/// exposed to the same loss.
///
/// Degradation ladder, worst outcome last, ordered by what each rung
/// SACRIFICES — and the order is dictated by [`MarkClass`], not by convenience:
///
/// 1. everything intact;
/// 2. STYLING marks dropped — bold on a word is recoverable annoyance;
/// 3. PROTECTIVE marks dropped too. Safe only because the check still demands
///    the ORIGINAL content back: if un-sealing a span would let it be
///    reinterpreted, the quoting pass re-seals it with `=…=` and the check
///    proves it; if it cannot, this rung is refused;
/// 4. EVERY mark dropped, including data-bearing ones. Link targets are lost —
///    they exist nowhere but the mark — so this rung is a last resort, but it
///    always represents the content and always settles.
///
/// Two conditions gate every rung, and neither is optional:
///
/// - **Content**: the emission must re-parse to `expected_reparse(content,
///   ORIGINAL marks)`. Re-deriving the expectation from each rung's reduced
///   mark set would make degradations self-justifying — dropping a `Verbatim`
///   also drops the reason its span must stay literal.
/// - **Settlement**: the emission must be a FIXED POINT — re-parsing and
///   re-rendering it must give the same bytes. Non-settling output is the
///   echo-loop condition: write-back rewrites the file, the watcher re-ingests,
///   and the two never agree. That is worse than any amount of lost formatting,
///   so it binds even on the rung that has already given up on the content.
///
/// Nothing here returns `Err`: `ToOrg::to_org` and the `FileFormatAdapter`
/// render methods are `-> String`, and this runs inside the org-sync select
/// loop where an unwind stops write-back vault-wide until restart. Threading
/// `Result` to a write-back gate is the follow-up.
pub fn render_block_content(block: &Block) -> String {
    render_block_content_checked(block).0
}

/// What [`render_block_content`] had to give up. `ContentUnpreserved` is the
/// one outcome a caller must never treat as routine: the emitted bytes do NOT
/// re-parse to the stored content. It is also the hook a write-back gate needs
/// to quarantine the file instead of writing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RenderFidelity {
    Exact,
    StylingDropped,
    ProtectiveDropped,
    AllMarksDropped,
    ContentUnpreserved,
}

/// The mark subsets this ladder will emit, most-valuable first, paired with
/// what giving up on each one costs.
fn ladder(marks: &[MarkSpan]) -> Vec<(Vec<MarkSpan>, RenderFidelity, DegradeReason, &'static str)> {
    let keep = |classes: &[MarkClass]| -> Vec<MarkSpan> {
        marks
            .iter()
            .filter(|m| classes.contains(&m.mark.class()))
            .cloned()
            .collect()
    };
    vec![
        (
            marks.to_vec(),
            RenderFidelity::Exact,
            DegradeReason::StylingDropped,
            "nothing",
        ),
        (
            keep(&[MarkClass::Protective, MarkClass::DataBearing]),
            RenderFidelity::StylingDropped,
            DegradeReason::StylingDropped,
            "styling",
        ),
        (
            keep(&[MarkClass::DataBearing]),
            RenderFidelity::ProtectiveDropped,
            DegradeReason::ProtectiveDropped,
            "styling and protective",
        ),
        (
            Vec::new(),
            RenderFidelity::AllMarksDropped,
            DegradeReason::AllMarksDropped,
            "EVERY mark, link targets included",
        ),
    ]
}

/// The emission the NEXT write-back cycle would produce for `(content, marks)`,
/// judged on the content contract alone.
///
/// Deliberately does not apply the settlement check that
/// [`render_block_content_checked`] layers on top: that check calls this, and
/// making it recursive would not terminate. One level is all a fixed-point test
/// needs — "would the next cycle emit these same bytes".
fn contract_only_emission(content: &str, marks: &[MarkSpan]) -> Option<String> {
    let contract = crate::inline_marks::expected_reparse(content, marks);
    ladder(marks)
        .into_iter()
        // "This rung cannot represent the block" is the ladder's normal control
        // flow, not a failure: the next rung is tried, and the reason is
        // reported by the caller that actually degrades.
        .find_map(|(retained, ..)| {
            // ALLOW(ok): a rung that does not render is control flow, not a
            // failure — the next rung is tried and the caller reports the reason.
            crate::inline_marks::render_expecting(content, &retained, &contract).ok()
        })
        .or_else(|| {
            crate::inline_marks::render_candidates(content, marks)
                .into_iter()
                .next()
        })
}

/// True when re-ingesting `bytes` and rendering them again reproduces `bytes`.
fn is_fixed_point(bytes: &str) -> bool {
    let (content, marks) = crate::inline_marks::extract_inline_marks(bytes);
    contract_only_emission(&content, &marks).as_deref() == Some(bytes)
}

/// [`render_block_content`] with the rung it landed on. The title line and
/// the body are rendered apart, as org reads them; a mark spanning both is
/// dropped and disclosed.
pub fn render_block_content_checked(block: &Block) -> (String, RenderFidelity) {
    let marks = block.marks.as_deref().unwrap_or(&[]);
    let Some((title, body)) = block.content.split_once('\n') else {
        return render_element_checked(block, &block.content, marks);
    };
    let split = crate::inline_marks::split_block_marks(&block.content, marks);
    let (title, title_fidelity) = render_element_checked(block, title, &split.title);
    let (body, body_fidelity) = render_element_checked(block, body, &split.body);
    let spanning_fidelity = split
        .spanning
        .iter()
        .map(|m| match m.mark.class() {
            MarkClass::DataBearing => RenderFidelity::AllMarksDropped,
            MarkClass::Protective => RenderFidelity::ProtectiveDropped,
            MarkClass::Styling => RenderFidelity::StylingDropped,
        })
        .max();
    if let Some(fidelity) = spanning_fidelity {
        let reason = match fidelity {
            RenderFidelity::AllMarksDropped => DegradeReason::AllMarksDropped,
            RenderFidelity::ProtectiveDropped => DegradeReason::ProtectiveDropped,
            _ => DegradeReason::StylingDropped,
        };
        disclose_degraded_render(
            block,
            reason,
            format_args!(
                "DROPPING {:?}: org reads the title line and the body as two elements, so no \
                 mark spans both",
                split.spanning
            ),
        );
    }
    let fidelity = [Some(title_fidelity), Some(body_fidelity), spanning_fidelity]
        .into_iter()
        .flatten()
        .max()
        .expect("two renders");
    (format!("{title}\n{body}"), fidelity)
}

/// One element of a block's text (its title line, or its body) with the rung
/// it landed on.
fn render_element_checked(
    block: &Block,
    content: &str,
    marks: &[MarkSpan],
) -> (String, RenderFidelity) {
    let contract = crate::inline_marks::expected_reparse(content, marks);
    let rungs = ladder(marks);

    // Pass 1 — both conditions. The first rung that keeps the content AND
    // settles wins, so nothing is sacrificed that did not have to be.
    let mut why_not = None;
    for (retained, fidelity, reason, what) in &rungs {
        let bytes = match crate::inline_marks::render_expecting(content, retained, &contract) {
            Ok(bytes) => bytes,
            Err(e) => {
                why_not.get_or_insert(e);
                continue;
            }
        };
        if !is_fixed_point(&bytes) {
            continue;
        }
        if *fidelity != RenderFidelity::Exact {
            disclose_degraded_render(
                block,
                *reason,
                format_args!(
                    "DROPPING {what} ({} mark(s)) — the content bytes survive: {}",
                    marks.len() - retained.len(),
                    why_not
                        .as_ref()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "unrepresentable with more marks".to_string())
                ),
            );
        }
        return (bytes.clone(), *fidelity);
    }

    // Pass 2 — the content is already unrepresentable, so settlement is the
    // only thing left worth protecting. Emitting churn on top of a corrupted
    // block turns one bad write into an endless argument between write-back and
    // the file watcher.
    for (retained, ..) in &rungs {
        for candidate in crate::inline_marks::render_candidates(content, retained) {
            if is_fixed_point(&candidate) {
                disclose_degraded_render(
                    block,
                    DegradeReason::Unrepresentable,
                    format_args!(
                        "org cannot represent this block's content with its marks; writing the \
                         closest form that at least STOPS CHANGING, so write-back does not loop: \
                         {}",
                        why_not.as_ref().map(|e| e.to_string()).unwrap_or_default()
                    ),
                );
                return (candidate, RenderFidelity::ContentUnpreserved);
            }
        }
    }

    // Unreached in every shape measured so far — the marks-free rung represents
    // any content and settles. Kept because "measured so far" is not a proof,
    // and a silent non-settling emission is exactly what this ladder exists to
    // prevent.
    disclose_degraded_render(
        block,
        DegradeReason::Unrepresentable,
        format_args!("NO emission of this block settles; write-back may loop on it"),
    );
    (
        crate::inline_marks::render_inline_marks(content, marks),
        RenderFidelity::ContentUnpreserved,
    )
}

/// Why a block degraded. Keyed alongside the block id so a block that later
/// degrades for a DIFFERENT reason gets its own loud first report instead of
/// being silenced by an earlier, unrelated one.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum DegradeReason {
    StylingDropped,
    ProtectiveDropped,
    AllMarksDropped,
    Unrepresentable,
}

impl DegradeReason {
    /// The rung's name, emitted as a structured field so a reader can tell
    /// WHICH branch of the ladder fired without re-deriving it from the prose.
    fn rung(self) -> &'static str {
        match self {
            Self::StylingDropped => "StylingDropped",
            Self::ProtectiveDropped => "ProtectiveDropped",
            Self::AllMarksDropped => "AllMarksDropped",
            Self::Unrepresentable => "Unrepresentable",
        }
    }

    /// True when the emitted bytes still re-parse to the stored content, i.e.
    /// only marks were given up.
    fn content_survives(self) -> bool {
        match self {
            Self::StylingDropped | Self::ProtectiveDropped | Self::AllMarksDropped => true,
            Self::Unrepresentable => false,
        }
    }
}

/// Loud once per (block, reason) per process, quiet after. A block whose marks
/// cross is re-rendered on every write-back pass, so an unconditional emission
/// would bury the log — but each distinct first occurrence must be visible.
///
/// Severity follows the error-handling priority order: a rung that keeps every
/// content byte is a disclosed degraded mode (priority 2) and emits WARN, while
/// a rung that cannot preserve the bytes is a real failure (priority 3) and
/// emits ERROR. The split matters because ERROR is what the keystone's
/// `inv-no-observed-errors` treats as a swallowed problem.
fn disclose_degraded_render(block: &Block, reason: DegradeReason, detail: std::fmt::Arguments) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<HashSet<(String, DegradeReason)>>> =
        std::sync::OnceLock::new();
    let first = SEEN
        .get_or_init(|| std::sync::Mutex::new(HashSet::new()))
        .lock()
        .expect("degraded-render disclosure set poisoned")
        .insert((block.id.as_str().to_string(), reason));
    if !first {
        tracing::debug!(
            block = block.id.as_str(),
            rung = reason.rung(),
            "org render still degraded: {detail}"
        );
    } else if reason.content_survives() {
        tracing::warn!(
            block = block.id.as_str(),
            rung = reason.rung(),
            "org render is DEGRADED for this block: {detail}"
        );
    } else {
        tracing::error!(
            block = block.id.as_str(),
            rung = reason.rung(),
            "org render is DEGRADED for this block: {detail}"
        );
    }
}

/// Records a loss when the block's written org text reads back with another
/// id, task keyword, priority cookie, title, tag set, body, content or inline
/// marks than the block meant, or splits into child blocks.
fn check_block_reads_back(
    block: &Block,
    meant: &BlockReading,
    meant_content: &(String, Option<Vec<MarkSpan>>),
    written: &str,
    vocabulary: &TaskKeywordVocabulary,
    losses: &mut Vec<RenderLoss>,
) {
    let read = crate::parser::read_block_text(written, vocabulary);
    let read_content = crate::parser::block_content(
        &read.headline.title,
        read.body.as_deref(),
        &Default::default(),
    );
    let tags = |reading: &BlockReading| Tags::from_tag_iter(reading.headline.tags.clone());
    if read.id == meant.id
        && read.headline.keyword == meant.headline.keyword
        && read.headline.cookie == meant.headline.cookie
        && read.headline.title == meant.headline.title
        && tags(&read) == tags(meant)
        && read.body == meant.body
        && read.keyword_lines == meant.keyword_lines
        && read.children == meant.children
        && read_content == *meant_content
    {
        return;
    }
    let detail = format!(
        "the org text {written:?} reads back as {read:?} with content and marks \
         {read_content:?}, not as {meant:?} with {meant_content:?}"
    );
    tracing::warn!(block = %block.id, "org render: {detail}");
    losses.push(RenderLoss {
        block: block.id.clone(),
        detail,
    });
}

/// Render a text/headline `Block` to org. `identity` selects the canonical
/// drawer form or the dense trailing-token form (`crate::dense`). Callers MUST
/// have already dispatched Source/Image content types (this only handles the
/// headline case). Free function (not an inherent method) because `Block` is
/// defined in `holon-api`.
fn headline_end(block: &Block) -> anyhow::Result<Option<String>> {
    let end = read_carrier::<String>(block, org_props::HEADLINE_END)?;
    if let Some(end) = &end {
        anyhow::ensure!(
            !end.is_empty() && end.chars().all(|c| c == ' ' || c == '\t'),
            "{} {end:?} on block {} is not spaces and tabs",
            org_props::HEADLINE_END,
            block.id
        );
    }
    Ok(end)
}

pub(crate) fn render_headline_block(
    block: &Block,
    identity: HeadlineIdentity,
    losses: &mut Vec<RenderLoss>,
) -> String {
    let (emitted, fidelity) = render_block_content_checked(block);
    let (meant_title, meant_body) = match emitted.split_once('\n') {
        Some((title, body)) => (title, Some(body)),
        None => (emitted.as_str(), None),
    };
    let title_str = emitted
        .lines()
        .next()
        .unwrap_or("")
        .trim_end_matches([' ', '\t'])
        .to_string();
    let body_str: Option<String> = {
        let lines: Vec<&str> = emitted.lines().collect();
        if lines.len() > 1 {
            Some(lines[1..].join("\n"))
        } else {
            None
        }
    };

    // Text blocks (headlines) render with stars, TODO, etc.
    let mut result = String::new();

    // Headline level (stars)
    result.push_str(&"*".repeat(block.level() as usize));
    result.push(' ');

    // TODO keyword
    if let Some(ref todo) = block.task_state() {
        result.push_str(&todo.to_string());
        result.push(' ');
    }

    // Priority. The drawer-only carrier suppresses the cookie so a file that
    // spelled its priority in the drawer does not grow one on write-back.
    let cookie = block
        .priority()
        .filter(|_| {
            block
                .get_property(org_props::PRIORITY_DRAWER_ONLY)
                .is_none()
        })
        .map(|priority| priority.letter().to_string());
    if let Some(letter) = &cookie {
        result.push_str(&format!("[#{letter}] "));
    }

    result.push_str(&title_str);

    let tags = block.tags();
    if !tags.is_empty() {
        let formatted_tags = tags.to_org();
        if !formatted_tags.is_empty() {
            result.push(' ');
            result.push_str(&formatted_tags);
        }
    }

    // Dense identity: the `:ID:` drawer scaffolding is compressed to a
    // trailing `{#alias}` token on the headline line itself; `^` flags an
    // elided-ancestor gap.
    if let HeadlineIdentity::DenseToken { alias, gap } = identity {
        let flag = if gap { "^" } else { "" };
        result.push_str(&format!(" {{#{}{}}}", alias, flag));
    }

    if matches!(identity, HeadlineIdentity::Drawer { .. }) {
        if let Some(end) = carrier_or_loss(headline_end(block), &block.id, losses) {
            result.truncate(result.trim_end_matches(' ').len());
            // Org reads stars as a headline only before a space.
            if result.bytes().all(|b| b == b'*') && !end.starts_with(' ') {
                result.push(' ');
            }
            result.push_str(&end);
        }
    }

    result.push('\n');

    // Planning (SCHEDULED/DEADLINE) — org syntax requires this line
    // directly after the headline, before any drawer (Emacs/LogSeq won't
    // parse it in a :PROPERTIES: drawer's wake).
    let sched_str = block.scheduled().map(|t| t.to_string());
    let dead_str = block.deadline().map(|t| t.to_string());
    let planning = format_planning(sched_str.as_deref(), dead_str.as_deref());
    if !planning.is_empty() {
        result.push_str(&planning);
    }

    // Properties drawer. In dense mode the `:ID:` line is dropped (identity
    // moved to the trailing token); any OTHER drawer properties are still
    // emitted, and a drawer that held only `:ID:` collapses to nothing.
    if let Some(props_json) = block.org_properties() {
        let props_drawer = match identity {
            HeadlineIdentity::Drawer { ref id, .. } => {
                let mut canonical_losses = Vec::new();
                let canonical = format_properties_drawer(
                    &props_json,
                    id,
                    &block.id,
                    &carrier_or_loss(
                        read_carrier(block, org_props::DRAWER_RAW),
                        &block.id,
                        losses,
                    )
                    .unwrap_or_default(),
                    &mut canonical_losses,
                );
                with_authored_drawer(
                    block,
                    &props_json,
                    id,
                    (canonical, canonical_losses),
                    losses,
                )
            }
            HeadlineIdentity::DenseToken { .. } => {
                format_properties_drawer_without_id(&props_json, &block.id, losses)
            }
        };
        if !props_drawer.is_empty() {
            result.push_str(&props_drawer);
            result.push('\n');
        }
    }

    // Body text (source blocks are child Block entities, rendered via tree
    // traversal)
    // A blank line after the body is emitted only when it is load-bearing —
    // i.e. when the body's last line starts a list item, which would otherwise
    // swallow a following `#+BEGIN_SRC` child into the list and LOSE the block
    // on re-parse. For every other body the blank line is presentational,
    // `trim_blank_lines` drops it on the way back in, and emitting it makes the
    // renderer permanently unequal to any hand-authored file (the echo-loop
    // `inv-org-render-fixed-point` reports forever).
    let authored = carrier_or_loss(
        read_carrier::<String>(block, org_props::AUTHORED_TEXT),
        &block.id,
        losses,
    );
    let written_body = body_str
        .as_deref()
        .map(|body| {
            written_text(
                CommaEscape::Body,
                trim_blank_lines(body),
                authored.as_deref(),
            )
        })
        .unwrap_or_default();
    if let Err(e) = block.authored_drawer_order() {
        carrier_or_loss::<()>(Err(e), &block.id, losses);
    }
    let keyword_lines: Vec<KeywordLine> = carrier_or_loss(block.keyword_lines(), &block.id, losses)
        .into_iter()
        .filter(|k| writable_keyword_line(k, &block.id, losses))
        .collect();
    let written_body = with_keyword_lines(&written_body, &keyword_lines);
    disclose_text_after_source(block, losses);
    if !written_body.is_empty() {
        for line in carrier_or_loss(block.blank_lines(), &block.id, losses).before_body {
            result.push_str(&line);
            result.push('\n');
        }
        result.push_str(&written_body);
        if !written_body.ends_with('\n') {
            result.push('\n');
        }
        if body_needs_list_terminator(&written_body) {
            result.push('\n');
        }
    }

    // Ensure result ends with newline if non-empty
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    if let HeadlineIdentity::Drawer { id, vocabulary } = &identity {
        let meant = BlockReading {
            id: Some(id.as_str().to_string()),
            headline: HeadlineReading {
                keyword: block.task_state().map(|state| state.keyword),
                cookie,
                title: meant_title.to_string(),
                tags: block.tags().to_vec(),
            },
            body: meant_body.map(str::to_string),
            keyword_lines: in_body(&keyword_lines, body_str.as_deref()),
            children: 0,
        };
        let meant_content = match fidelity {
            RenderFidelity::Exact => {
                crate::parser::block_content(meant_title, meant_body, &Default::default())
            }
            RenderFidelity::StylingDropped
            | RenderFidelity::ProtectiveDropped
            | RenderFidelity::AllMarksDropped
            | RenderFidelity::ContentUnpreserved => (
                crate::inline_marks::expected_block_reparse(
                    &block.content,
                    block.marks.as_deref().unwrap_or(&[]),
                ),
                block.marks.clone(),
            ),
        };
        check_block_reads_back(block, &meant, &meant_content, &result, vocabulary, losses);
    }

    result
}

/// `body` (the written lines, no final line break) with each keyword line
/// written raw before the body line it stands before, or after the last one.
pub(crate) fn with_keyword_lines(body: &str, keyword_lines: &[KeywordLine]) -> String {
    let lines: Vec<&str> = if body.is_empty() {
        Vec::new()
    } else {
        body.split('\n').collect()
    };
    let mut out = String::new();
    for i in 0..=lines.len() {
        for keyword in keyword_lines
            .iter()
            .filter(|k| k.before_line.min(lines.len()) == i)
        {
            out.push_str(&keyword.raw);
            if !keyword.raw.ends_with('\n') {
                out.push('\n');
            }
        }
        if let Some(line) = lines.get(i) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `text` as `codec` writes it, or as the file `authored` it while that still
/// reads as `text`.
pub(crate) fn written_text(codec: CommaEscape, text: &str, authored: Option<&str>) -> String {
    match authored {
        Some(authored) if codec.unescape(authored) == text => authored.to_string(),
        _ => codec.escape(text),
    }
}

/// Records a loss when `block`'s file had text after a source block child,
/// which the renderer writes before its source blocks.
pub(crate) fn disclose_text_after_source(block: &Block, losses: &mut Vec<RenderLoss>) {
    if block.get_property(org_props::TEXT_AFTER_SOURCE).is_none() {
        return;
    }
    let detail = "text that stood after a source block is written before it".to_string();
    tracing::warn!(block = %block.id, "org render: {detail}");
    losses.push(RenderLoss {
        block: block.id.clone(),
        detail,
    });
}

/// Whether the renderer writes `keyword_line`: one that is no single keyword
/// line is left out, and one that stood after a source block child is written
/// before it; both are recorded in `losses`.
pub(crate) fn writable_keyword_line(
    keyword_line: &KeywordLine,
    owner: &EntityUri,
    losses: &mut Vec<RenderLoss>,
) -> bool {
    let writable = crate::parser::is_keyword_line(&keyword_line.raw);
    let detail = if !writable {
        format!(
            "{:?} is no keyword line; it is not written",
            keyword_line.raw
        )
    } else if keyword_line.after_source {
        format!(
            "the keyword line {:?} stood after a source block and is written before it",
            keyword_line.raw.trim()
        )
    } else {
        return true;
    };
    tracing::warn!(block = %owner, "org render: {detail}");
    losses.push(RenderLoss {
        block: owner.clone(),
        detail,
    });
    writable
}

/// `keyword_lines` as the parser reads them back around `body`: one that
/// stood after the body's end stands at its end.
fn in_body(keyword_lines: &[KeywordLine], body: Option<&str>) -> Vec<KeywordLine> {
    let len = body
        .map(trim_blank_lines)
        .filter(|b| !b.is_empty())
        .map_or(0, |b| b.split('\n').count());
    keyword_lines
        .iter()
        .map(|k| KeywordLine {
            before_line: k.before_line.min(len),
            raw: k.raw.clone(),
            after_source: false,
        })
        .collect()
}

/// Does this body need a trailing blank line to close an open list?
///
/// An org list item runs until a blank line or a heading, absorbing anything
/// else that follows — including a `#+BEGIN_SRC` child, which then parses as
/// list content and is silently LOST (the parser's `SOURCE_BLOCK` sweep over
/// section children never sees it). A heading at column 0 terminates a list on
/// its own, which is why only the source-block case is at risk.
///
/// Deliberately generous: emitting a blank line that org did not strictly need
/// costs one byte of render-vs-disk churn, while omitting a needed one loses a
/// block. Ambiguous shapes therefore resolve to `true`.
fn body_needs_list_terminator(body: &str) -> bool {
    let Some(last) = body.lines().rev().find(|l| !l.trim().is_empty()) else {
        return false;
    };
    let trimmed = last.trim_start();
    let is_indented = trimmed.len() != last.len();

    let bullet_rest = trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('+'))
        // `*` is a bullet only when indented — at column 0 it is a heading.
        .or_else(|| is_indented.then(|| trimmed.strip_prefix('*')).flatten());
    if let Some(rest) = bullet_rest {
        return rest.is_empty() || rest.starts_with([' ', '\t']);
    }

    // Ordered counters: digits, or a single letter when org's alphabetical
    // lists are in play, followed by `.` or `)`.
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    let counter_len = if digits > 0 {
        digits
    } else if trimmed
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        1
    } else {
        return false;
    };
    let after_counter = &trimmed[counter_len..];
    let Some(rest) = after_counter
        .strip_prefix('.')
        .or_else(|| after_counter.strip_prefix(')'))
    else {
        return false;
    };
    rest.is_empty() || rest.starts_with([' ', '\t'])
}

/// The line that ends a source block as the renderer writes it.
pub(crate) const SOURCE_END: &str = "#+END_SRC\n";

/// A source block's `#+NAME:` and `#+BEGIN_SRC` lines as the renderer writes
/// them, its id as the `:id` header argument.
pub(crate) fn source_head(block: &Block, losses: &mut Vec<RenderLoss>) -> anyhow::Result<String> {
    let id = own_drawer_id(&block.id)
        .map_err(|e| anyhow::anyhow!("org render of block {} refused: {e}", block.id))?;
    let mut result = String::new();

    if let Some(ref name) = block.source_name {
        result.push_str("#+NAME: ");
        result.push_str(name);
        result.push('\n');
    }

    result.push_str("#+BEGIN_SRC");

    if let Some(ref lang) = block.source_language {
        result.push(' ');
        result.push_str(&lang.to_string());
    }

    result.push_str(" :id ");
    result.push_str(id.as_str());

    let header_args = block.get_source_header_args();
    let header_args_str = format_header_args_value(&header_args);
    if !header_args_str.is_empty() {
        result.push(' ');
        result.push_str(&header_args_str);
    }

    // Non-standard header args, which the parser stores as properties.
    let mut drawer_props: Vec<_> = block.drawer_properties().into_iter().collect();
    drawer_props.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (k, v) in &drawer_props {
        let key = match ValueCarrier::HeaderArg.key(k) {
            Ok(key) => key,
            Err(e) => {
                left_out_key(&block.id, &e, losses);
                continue;
            }
        };
        result.push_str(" :");
        result.push_str(key.as_str());
        result.push(' ');
        result.push_str(&key.encode(v));
    }

    // A source block has no headline to carry `:tag:` notation.
    if !block.tags.is_empty() {
        result.push_str(" :TAGS ");
        result.push_str(&block.tags.to_vec().join(" "));
    }

    result.push('\n');
    Ok(result)
}

/// Render a source-type Block as Org Mode #+BEGIN_SRC ... #+END_SRC.
/// `minted_here` is the id the parser mints for a source block with no `:id`
/// at the place the block is written; `None` when the place is unknown.
fn source_block_to_org(
    block: &Block,
    minted_here: Option<&str>,
    losses: &mut Vec<RenderLoss>,
) -> anyhow::Result<String> {
    if let Err(e) = block.blank_lines() {
        carrier_or_loss::<()>(Err(e), &block.id, losses);
    }
    let mut head_losses = Vec::new();
    let written = source_head(block, &mut head_losses)?;
    let authored = carrier_or_loss(
        read_carrier::<SourceLines>(block, org_props::SOURCE_LINES),
        &block.id,
        losses,
    );
    let (head, end) = match authored {
        Some(lines)
            if lines.written == written
                && (!lines.minted || minted_here == Some(block.id.id())) =>
        {
            (lines.head, lines.end)
        }
        authored => {
            if authored.is_some_and(|lines| lines.minted) {
                let detail = format!(
                    "the source block names no `:id` in its file; `:id {}` is written into its \
                     `#+BEGIN_SRC` line so it keeps its identity",
                    block.id.id()
                );
                tracing::warn!(block = %block.id, "org render: {detail}");
                losses.push(RenderLoss {
                    block: block.id.clone(),
                    detail,
                });
            }
            losses.extend(head_losses);
            (written, SOURCE_END.to_string())
        }
    };

    let authored = carrier_or_loss(
        read_carrier::<String>(block, org_props::AUTHORED_TEXT),
        &block.id,
        losses,
    );
    let mut result = head;
    result.push_str(&written_text(
        CommaEscape::Source,
        &block.content,
        authored.as_deref(),
    ));
    // The line break before `#+END_SRC` is not part of the block's text.
    result.push('\n');
    result.push_str(&end);
    Ok(result)
}

// Note: We re-export SourceBlock from holon_api to use it directly
pub use holon_api::SourceBlock;

/// Parse header arguments string into key-value pairs
/// Format: `:key1 value1 :key2 value2` or `:key1 :key2`
pub fn parse_header_args_from_str(params: &str) -> HashMap<String, String> {
    let mut args = HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_value = String::new();

    for token in params.split_whitespace() {
        if let Some(rest) = token.strip_prefix(':') {
            if let Some(key) = current_key.take() {
                args.insert(key, current_value.trim().to_string());
                current_value.clear();
            }
            current_key = Some(rest.to_string());
        } else if current_key.is_some() {
            if !current_value.is_empty() {
                current_value.push(' ');
            }
            current_value.push_str(token);
        }
    }

    if let Some(key) = current_key {
        args.insert(key, current_value.trim().to_string());
    }

    args
}

impl ToOrg for SourceBlock {
    fn to_org(&self) -> String {
        let mut result = String::new();

        if let Some(ref name) = self.name {
            result.push_str("#+NAME: ");
            result.push_str(name);
            result.push('\n');
        }

        result.push_str("#+BEGIN_SRC");

        if let Some(ref lang) = self.language {
            result.push(' ');
            result.push_str(lang);
        }

        let header_args_str = format_header_args_value(&self.header_args);
        if !header_args_str.is_empty() {
            result.push(' ');
            result.push_str(&header_args_str);
        }

        result.push('\n');
        result.push_str(&self.source);

        if !self.source.ends_with('\n') {
            result.push('\n');
        }

        result.push_str("#+END_SRC");

        // Ensure trailing newline
        if !result.ends_with('\n') {
            result.push('\n');
        }

        result
    }
}

// =============================================================================
// ParsedSectionContent - Helper for parsed section data
// =============================================================================

/// Parsed section content with both text and source blocks
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParsedSectionContent {
    /// Plain text content (paragraphs outside of source blocks)
    pub text: String,

    /// Source blocks found in this section
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_blocks: Vec<SourceBlock>,
}

impl ParsedSectionContent {
    /// Check if there are any source blocks
    pub fn has_source_blocks(&self) -> bool {
        !self.source_blocks.is_empty()
    }

    /// Get all PRQL source blocks
    pub fn prql_blocks(&self) -> impl Iterator<Item = &SourceBlock> {
        self.source_blocks.iter().filter(|b| b.is_prql())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_uri() -> EntityUri {
        EntityUri::file("/test.org")
    }

    fn make_doc_block() -> Block {
        let mut doc = Block::new_text(doc_uri(), EntityUri::no_parent(), "test.org");
        doc.set_page(true);
        doc
    }

    #[test]
    fn a_block_text_that_reads_back_with_another_id_is_a_loss() {
        let written = "* K\n:PROPERTIES:\n:ID: k\n:NOTE:\n:END:\nbody\n";
        let vocabulary = TaskKeywordVocabulary::default();
        let block = Block::new_text(EntityUri::block("k"), doc_uri(), "K\nbody");
        let check = |id: &str| {
            let mut meant = crate::parser::read_block_text(written, &vocabulary);
            assert_eq!(meant.id.as_deref(), Some("k"));
            meant.id = Some(id.to_string());
            let content = crate::parser::block_content(
                &meant.headline.title,
                meant.body.as_deref(),
                &Default::default(),
            );
            let mut losses = Vec::new();
            check_block_reads_back(&block, &meant, &content, written, &vocabulary, &mut losses);
            losses.len()
        };
        assert_eq!((check("k"), check("other")), (0, 1));
    }

    #[test]
    fn test_find_document_id_top_level() {
        let doc = make_doc_block();
        let block = Block::new_text(EntityUri::block("block1"), doc_uri(), "Test headline");
        let resolver = HashMapBlockResolver::from_blocks(vec![doc, block.clone()]);

        let doc_id = find_document_id(&block, &resolver);
        assert_eq!(doc_id, Some(doc_uri()));
    }

    #[test]
    fn test_find_document_id_nested() {
        let doc = make_doc_block();
        let block1 = Block::new_text(EntityUri::block("block1"), doc_uri(), "Parent headline");
        let block2 = Block::new_text(
            EntityUri::block("block2"),
            EntityUri::block("block1"),
            "Child headline",
        );

        let resolver = HashMapBlockResolver::from_blocks(vec![doc, block1.clone(), block2.clone()]);

        let doc_id = find_document_id(&block2, &resolver);
        assert_eq!(doc_id, Some(doc_uri()));
    }

    #[test]
    fn test_find_document_id_deeply_nested() {
        let doc = make_doc_block();
        let block1 = Block::new_text(EntityUri::block("block1"), doc_uri(), "Level 1");
        let block2 = Block::new_text(
            EntityUri::block("block2"),
            EntityUri::block("block1"),
            "Level 2",
        );
        let block3 = Block::new_text(
            EntityUri::block("block3"),
            EntityUri::block("block2"),
            "Level 3",
        );

        let resolver = HashMapBlockResolver::from_blocks(vec![
            doc,
            block1.clone(),
            block2.clone(),
            block3.clone(),
        ]);

        let doc_id = find_document_id(&block3, &resolver);
        assert_eq!(doc_id, Some(doc_uri()));
    }

    #[test]
    fn test_get_block_file_path() {
        let notes_uri = EntityUri::file("/path/to/notes.org");
        let mut notes_doc = Block::new_text(
            notes_uri.clone(),
            EntityUri::no_parent(),
            "/path/to/notes.org",
        );
        notes_doc.set_page(true);
        let block = Block::new_text(EntityUri::block("block1"), notes_uri.clone(), "Test");
        let resolver = HashMapBlockResolver::from_blocks(vec![notes_doc, block.clone()]);

        let path = get_block_file_path(&block, &resolver);
        assert_eq!(path, Some("/path/to/notes.org".to_string()));
    }

    #[test]
    fn test_is_done_keyword() {
        assert!(is_done_keyword("DONE"));
        assert!(is_done_keyword("CANCELLED"));
        assert!(is_done_keyword("CLOSED"));
        assert!(!is_done_keyword("TODO"));
        assert!(!is_done_keyword("INPROGRESS"));
    }

    #[test]
    fn test_document_todo_keywords() {
        let mut doc = Block::new_text(EntityUri::no_parent(), EntityUri::no_parent(), "test.org");
        doc.set_page(true);
        doc.set_todo_keywords(Some(vec![
            TaskState::active("TODO"),
            TaskState::active("INPROGRESS"),
            TaskState::done("DONE"),
            TaskState::done("CANCELLED"),
        ]));

        let (active, done) = doc.parse_todo_keywords();
        assert_eq!(active, vec!["TODO", "INPROGRESS"]);
        assert_eq!(done, vec!["DONE", "CANCELLED"]);
        assert!(doc.is_done("DONE"));
        assert!(doc.is_done("CANCELLED"));
        assert!(!doc.is_done("TODO"));
    }

    #[test]
    fn test_block_title_and_body() {
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            "Title line\nBody line 1\nBody line 2",
        );

        assert_eq!(block.org_title(), "Title line");
        assert_eq!(block.body(), Some("Body line 1\nBody line 2".to_string()));

        block.set_title_and_body("New title".to_string(), Some("New body".to_string()));
        assert_eq!(block.org_title(), "New title");
        assert_eq!(block.body(), Some("New body".to_string()));
    }

    #[test]
    fn test_block_org_properties() {
        let mut block =
            Block::new_text(EntityUri::block("id1"), EntityUri::block("parent1"), "Test");
        block.set_level(2);
        block.set_task_state(Some(TaskState::from_keyword("TODO")));
        block.set_priority(Some(Priority::B));
        block.set_tags(Tags::from_csv("work,urgent"));

        assert_eq!(block.level(), 2);
        assert_eq!(block.task_state(), Some(TaskState::from_keyword("TODO")));
        assert_eq!(block.priority(), Some(Priority::B));
        assert_eq!(block.tags(), Tags::from_csv("work,urgent"));
    }

    #[test]
    fn test_document_to_org() {
        let mut doc = Block::new_text(
            EntityUri::file("test.org"),
            EntityUri::no_parent(),
            "test.org",
        );
        doc.set_page(true);
        doc.set_file_title(Some("My Document".to_string()));
        doc.set_todo_keywords(Some(vec![
            TaskState::active("TODO"),
            TaskState::active("DOING"),
            TaskState::done("DONE"),
        ]));

        let org = render_document_header(&doc, &mut Vec::new()).unwrap();
        assert!(org.contains("#+TITLE: My Document"));
        assert!(org.contains("#+TODO: TODO DOING | DONE"));
    }

    #[test]
    fn test_block_to_org() {
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            "Test headline",
        );
        block.set_level(2);
        block.set_task_state(Some(TaskState::from_keyword("TODO")));
        block.set_priority(Some(Priority::A));
        block.set_tags(Tags::from_csv("work,urgent"));

        let org = block.to_org();
        // Tags render in sorted order.
        assert!(org.starts_with("** TODO [#A] Test headline :urgent:work:"));
    }

    #[test]
    fn to_org_renders_planning_lines() {
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            "Planned task",
        );
        block.set_level(1);
        block.set_scheduled(Some(Timestamp::parse("<2026-01-15 Thu>").unwrap()));
        block.set_deadline(Some(Timestamp::parse("<2026-02-01 Sun>").unwrap()));

        let org = block.to_org();
        assert!(
            org.contains("SCHEDULED: "),
            "planning dropped from to_org: {org:?}"
        );
        assert!(
            org.contains("2026-01-15"),
            "scheduled date missing: {org:?}"
        );
        assert!(
            org.contains("DEADLINE: "),
            "deadline dropped from to_org: {org:?}"
        );
        assert!(org.contains("2026-02-01"), "deadline date missing: {org:?}");
    }

    /// Org syntax requires SCHEDULED/DEADLINE directly after the headline,
    /// before any drawer — Emacs/LogSeq won't parse a planning line that
    /// follows :PROPERTIES:. Regression: writeback used to emit the drawer
    /// first.
    #[test]
    fn to_org_planning_lines_precede_properties_drawer() {
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            "Planned task",
        );
        block.set_level(1);
        block.set_scheduled(Some(Timestamp::parse("<2026-01-15 Thu>").unwrap()));
        block.set_deadline(Some(Timestamp::parse("<2026-02-01 Sun>").unwrap()));
        block.set_org_properties(Some(r#"{"ID":"id1"}"#.to_string()));

        let org = block.to_org();
        let scheduled_pos = org.find("SCHEDULED:").expect("SCHEDULED line missing");
        let drawer_pos = org.find(":PROPERTIES:").expect("drawer missing");
        assert!(
            scheduled_pos < drawer_pos,
            "planning line must precede the properties drawer: {org:?}"
        );

        // The headline itself must be the immediately preceding line — no
        // blank line or drawer between it and SCHEDULED.
        let headline_end = org.find('\n').expect("headline newline missing");
        assert_eq!(
            headline_end + 1,
            scheduled_pos,
            "planning line must come directly after the headline: {org:?}"
        );
    }

    #[test]
    fn to_org_properties_drawer_keeps_custom_keys_and_bare_id() {
        let mut block = Block::new_text(
            EntityUri::block("abc-123"),
            EntityUri::block("parent1"),
            "With drawer",
        );
        block.set_level(1);
        block.set_org_properties(Some(r#"{"ID":"abc-123","CUSTOM":"val"}"#.to_string()));

        let org = block.to_org();
        // ID renders bare (not JSON-quoted) and exactly once.
        assert!(
            org.contains(":ID: abc-123\n"),
            "ID not rendered bare: {org:?}"
        );
        assert_eq!(
            org.matches(":ID:").count(),
            1,
            "ID rendered more than once: {org:?}"
        );
        // Non-ID drawer keys must survive the round-trip.
        assert!(
            org.contains(":CUSTOM: val"),
            "custom drawer key dropped: {org:?}"
        );
        assert!(
            org.contains(":PROPERTIES:") && org.contains(":END:"),
            "{org:?}"
        );
    }

    #[test]
    fn source_block_to_org_renders_header_args() {
        let mut block = Block {
            id: EntityUri::block("src1"),
            parent_id: EntityUri::block("parent1"),
            content: "select 1".to_string(),
            content_type: ContentType::Source,
            ..Block::default()
        };
        let mut args = HashMap::new();
        args.insert(
            "connection".to_string(),
            holon_api::Value::String("main".to_string()),
        );
        args.insert(
            "results".to_string(),
            holon_api::Value::String("table".to_string()),
        );
        block.set_source_header_args(args);

        let org = block.to_org();
        assert!(org.starts_with("#+BEGIN_SRC"), "{org:?}");
        assert!(
            org.contains(":connection main"),
            "header arg dropped: {org:?}"
        );
        assert!(
            org.contains(":results table"),
            "header arg dropped: {org:?}"
        );
        assert!(org.contains(":id src1"), "{org:?}");
    }

    /// Ruling D27.b makes `Value::Null` a real, storable property value in the
    /// native substrate — but org still cannot carry it, and its profile says
    /// so (`crates/holon-org-format/profile.yaml`, `property_values.null:
    /// dropped`, `types: [string]`).
    ///
    /// The measured mechanism is the `as_string()` filter below: a Null is
    /// excluded from the drawer the same way an Integer or an Array is, so it
    /// never becomes an EMPTY property value — which is what the known
    /// write-back hazard (an empty value drops its key) would have turned into
    /// a second, silent loss on top. Pinned because the honest `null: dropped`
    /// declaration is only honest while this holds.
    #[test]
    fn a_null_valued_property_never_reaches_the_org_drawer() {
        let mut block = Block {
            id: EntityUri::block("n1"),
            parent_id: EntityUri::block("parent1"),
            content: "headline".to_string(),
            ..Block::default()
        };
        block
            .properties
            .insert("nullable".to_string(), holon_api::Value::Null);
        block
            .properties
            .insert("present".to_string(), holon_api::Value::String("v".into()));

        let drawer = block.drawer_properties();
        assert!(
            !drawer.contains_key("nullable"),
            "a Null property must not reach the drawer at all: {drawer:?}"
        );
        assert_eq!(
            drawer.get("present").map(String::as_str),
            Some("v"),
            "a string property must still render: {drawer:?}"
        );
        let org = block.to_org();
        assert!(
            !org.contains("nullable"),
            "the dropped key must not appear in the rendered org either: {org:?}"
        );
    }
}
