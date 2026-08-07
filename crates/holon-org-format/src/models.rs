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
    /// A doc-root's FILE-LEVEL `:PROPERTIES:` drawer (org 9.0+, org-roam's
    /// identity carrier) as a JSON object in the order the author wrote it,
    /// `ID` included. Present only on a doc-root whose file had one, and its
    /// presence is what makes the renderer emit the drawer back.
    ///
    /// Deliberately NOT `_`-prefixed, unlike [`DRAWER_ORDER`]: `_` keys are
    /// dropped by `drawer_properties()` and so never reach the store, and a
    /// drawer that survives only until the first write-back is the very bug
    /// this carrier exists to fix.
    pub const FILE_PROPERTIES: &str = "file_properties";
    /// `"t"` on a doc-root whose file carried a `#+ID:` keyword. It is what
    /// keeps a file that declares its identity BOTH ways (drawer `:ID:` and
    /// `#+ID:`, in agreement) from losing one of the two on write-back.
    pub const FILE_ID_KEYWORD: &str = "file_id_keyword";
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

/// Default active keywords when file doesn't specify custom TODO config.
///
/// Includes the LogSeq dialect keywords `LATER` (TODO-family, not started)
/// and `NOW` (DOING-family, in progress) so foreign LogSeq vaults — which
/// carry no `#+TODO:` header — parse their headline keywords as task states
/// rather than title text (ForeignVaultCompat §4).
pub const DEFAULT_ACTIVE_KEYWORDS: &[&str] = &["TODO", "DOING", "LATER", "NOW"];

/// Default done keywords when file doesn't specify custom TODO config
pub const DEFAULT_DONE_KEYWORDS: &[&str] = &["DONE", "CANCELLED", "CLOSED"];

/// Check if a keyword is considered "done" using default keywords
pub fn is_done_keyword(keyword: &str) -> bool {
    DEFAULT_DONE_KEYWORDS.contains(&keyword)
}

/// Trait for converting entities to org-mode formatted strings
pub trait ToOrg {
    fn to_org(&self) -> String;
}

/// Format properties drawer from JSON
/// Input: JSON string -> Output: ":PROPERTIES:\n:KEY: VALUE\n:END:"
/// Ensures :ID: property is rendered first.
fn format_properties_drawer(properties_json: &str) -> String {
    let props: serde_json::Map<String, serde_json::Value> = serde_json::from_str(properties_json)
        .unwrap_or_else(|e| {
            panic!(
                "malformed org_properties JSON {properties_json:?}: {e} — silently dropping the \
                 :PROPERTIES: drawer would lose :ID: and churn block identity"
            )
        });

    if props.is_empty() {
        return String::new();
    }

    let mut result = String::from(":PROPERTIES:\n");

    // Render :ID: first if present
    if let Some(id_value) = props.get("ID") {
        let value_str = match id_value {
            serde_json::Value::String(s) => s.clone(),
            _ => id_value.to_string(),
        };
        result.push_str(&format!(":ID: {}\n", value_str));
    }

    // Render other properties (excluding ID which we already rendered) in the
    // JSON's own key order — serde_json::Map is an IndexMap (preserve_order
    // enabled by a transitive dependency), and the renderer built that order
    // from the author's drawer.
    for (key, value) in props.iter().filter(|(k, _)| k.as_str() != "ID") {
        let value_str = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => value.to_string(),
        };
        result.push_str(&format!(":{}: {}\n", key, value_str));
    }
    result.push_str(":END:");
    result
}

/// Format a properties drawer with the `:ID:` line omitted (dense projection).
/// Returns an empty string when no non-ID properties remain, so a block whose
/// only drawer content was its `:ID:` renders with no drawer at all.
fn format_properties_drawer_without_id(properties_json: &str) -> String {
    let props: serde_json::Map<String, serde_json::Value> = serde_json::from_str(properties_json)
        .unwrap_or_else(|e| {
            panic!(
                "malformed org_properties JSON {properties_json:?}: {e} — dense render must not \
                 silently drop drawer properties"
            )
        });

    let drawer_props: Vec<_> = props.iter().filter(|(k, _)| k.as_str() != "ID").collect();
    if drawer_props.is_empty() {
        return String::new();
    }

    let mut result = String::from(":PROPERTIES:\n");
    for (key, value) in drawer_props {
        let value_str = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => value.to_string(),
        };
        result.push_str(&format!(":{}: {}\n", key, value_str));
    }
    result.push_str(":END:");
    result
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
    /// wrote them and `ID` included. `None` when the file had no such drawer.
    fn file_drawer(&self) -> Option<serde_json::Map<String, serde_json::Value>>;

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
}

impl OrgDocumentExt for Block {
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

    fn file_drawer(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        let value = self.get_property(org_props::FILE_PROPERTIES)?;
        let json = value.as_string()?.to_string();
        Some(serde_json::from_str(&json).unwrap_or_else(|e| {
            panic!(
                "malformed {} {json:?}: {e} — we wrote it, so a parse failure means the \
                 file-level drawer carrier was corrupted in transit, and rendering on would \
                 delete the author's drawer",
                org_props::FILE_PROPERTIES
            )
        }))
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
    loop {
        match s[start..end].rfind('\n') {
            Some(i) => {
                let nl = start + i;
                if s[nl + 1..end].trim().is_empty() {
                    end = nl;
                } else {
                    break;
                }
            }
            None => break,
        }
    }
    if s[start..end].trim().is_empty() {
        ""
    } else {
        &s[start..end]
    }
}

pub fn render_document_header(doc_block: &Block) -> String {
    let mut result = String::new();

    // A hand-authored FILE-LEVEL `:PROPERTIES:` drawer goes first and verbatim:
    // org-mode (and orgize's `document_node`) only recognize it as the file's
    // own drawer when it is the first element of the file, so any other
    // placement would silently demote it to body text on the next read.
    let file_drawer = doc_block.file_drawer();
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
                let rendered = if key.eq_ignore_ascii_case("ID") && !authored.is_empty() {
                    carries_id = true;
                    doc_block.id.id().to_string()
                } else {
                    authored
                };
                // The space after the key is REQUIRED, empty value or not:
                // orgize's `node_property_node` matches `space1` between key and
                // value, so `:KEY:` with nothing after it does not parse as a
                // property — and one unparsable line makes the WHOLE drawer fail
                // to parse and decay into body text. The trailing space on an
                // empty value is the cost of the drawer surviving re-ingest.
                result.push_str(&format!(":{key}: {rendered}\n"));
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
    if doc_block.id.is_block() && (!drawer_carries_id || authored_id_keyword) {
        result.push_str(&format!("#+ID: {}\n", doc_block.id.id()));
    }

    // File title. A doc-root with no `file_title` — including one PROMOTED from
    // an inline `:Page:`-tagged headline — deliberately renders none: its name
    // is carried by its own filename (the page path is built from
    // `block.title()`), and emitting a synthetic `#+TITLE:` would break
    // `parse(render(doc)) == doc` for every title-less file.
    if let Some(title) = doc_block.file_title() {
        result.push_str(&format!("#+TITLE: {}\n", title));
    }

    // TODO keywords configuration
    if let Some(states) = doc_block.todo_keywords() {
        let active: Vec<&str> = states
            .iter()
            .filter(|s| s.is_active())
            .map(|s| s.keyword.as_str())
            .collect();
        let done: Vec<&str> = states
            .iter()
            .filter(|s| s.is_done())
            .map(|s| s.keyword.as_str())
            .collect();
        if !active.is_empty() || !done.is_empty() {
            result.push_str("#+TODO:");
            if !active.is_empty() {
                result.push_str(&format!(" {}", active.join(" ")));
            }
            if !done.is_empty() {
                result.push_str(&format!(" | {}", done.join(" ")));
            }
            result.push('\n');
        }
    }

    // Ensure result ends with newline if non-empty
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    result
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
/// - priority: A=3, B=2, C=1
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
            .and_then(|i| Priority::from_int(i as i32).ok()) // ALLOW(ok):
        // boundary parse
    }

    fn set_priority(&mut self, priority: Option<Priority>) {
        if let Some(p) = priority {
            self.set_property(
                org_props::PRIORITY,
                holon_api::Value::Integer(p.to_int() as i64),
            );
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::PRIORITY);
            self.set_properties_map(props);
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
        // Known internal keys that are NOT drawer properties
        const INTERNAL_KEYS: &[&str] = &[
            "level",
            "sequence",
            "task_state",
            "task_state_category",
            "priority",
            "tags",
            "requires",
            "advice_suppressed",
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

        let mut result = HashMap::new();

        // First, extract from the org_properties JSON if present
        if let Some(json) = self.org_properties() {
            if let Ok(props) = serde_json::from_str::<HashMap<String, String>>(&json) {
                for (k, v) in props {
                    if k != "ID" && !k.starts_with('_') {
                        result.insert(k, v);
                    }
                }
            } else if let Ok(props) =
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
            {
                for (k, v) in props {
                    if k != "ID" && !k.starts_with('_') {
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
        // Keys starting with `_` are routing metadata (e.g. _routing_doc_uri)
        // — never part of the org drawer.
        for (k, v) in &self.properties {
            if !INTERNAL_KEYS.contains(&k.as_str()) && !k.starts_with('_') {
                if let Some(s) = v.as_string() {
                    result.entry(k.clone()).or_insert_with(|| s.to_string());
                }
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
    /// Canonical: `:ID:` inside the properties drawer.
    Drawer,
    /// Dense projection: trailing `{#alias}` token, `:ID:` line suppressed.
    /// `gap` renders a `^` inside the token (`{#alias^}`) meaning one or more
    /// unselected ancestors were elided above this block — its rendered parent
    /// is NOT its true parent. Display-only (see `crate::dense`).
    DenseToken { alias: &'a str, gap: bool },
}

impl ToOrg for Block {
    fn to_org(&self) -> String {
        // Source blocks render as #+BEGIN_SRC ... #+END_SRC
        if self.content_type == ContentType::Source {
            return source_block_to_org(self);
        }

        // Image blocks render as [[file:path]] inline link
        if self.content_type == ContentType::Image {
            return format!("[[file:{}]]\n", self.content);
        }

        render_headline_block(self, HeadlineIdentity::Drawer)
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
            match crate::inline_marks::render_expecting(content, &retained, &contract) {
                Ok(bytes) => Some(bytes),
                Err(_) => None,
            }
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

/// [`render_block_content`] with the rung it landed on.
pub fn render_block_content_checked(block: &Block) -> (String, RenderFidelity) {
    let marks = block.marks.as_deref().unwrap_or(&[]);
    let contract = crate::inline_marks::expected_reparse(&block.content, marks);
    let rungs = ladder(marks);

    // Pass 1 — both conditions. The first rung that keeps the content AND
    // settles wins, so nothing is sacrificed that did not have to be.
    let mut why_not = None;
    for (retained, fidelity, reason, what) in &rungs {
        let bytes = match crate::inline_marks::render_expecting(&block.content, retained, &contract)
        {
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
        for candidate in crate::inline_marks::render_candidates(&block.content, retained) {
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
        crate::inline_marks::render_inline_marks(&block.content, marks),
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

/// Render a text/headline `Block` to org. `identity` selects the canonical
/// drawer form or the dense trailing-token form (`crate::dense`). Callers MUST
/// have already dispatched Source/Image content types (this only handles the
/// headline case). Free function (not an inherent method) because `Block` is
/// defined in `holon-api`.
pub(crate) fn render_headline_block(block: &Block, identity: HeadlineIdentity) -> String {
    let emitted: String = render_block_content(block);
    let title_str = emitted.lines().next().unwrap_or("").trim_end().to_string();
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

    // Priority
    if let Some(priority) = block.priority() {
        result.push_str(&format!("[#{}] ", priority.to_letter()));
    }

    // Title
    result.push_str(&title_str);

    // Tags
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
            HeadlineIdentity::Drawer => format_properties_drawer(&props_json),
            HeadlineIdentity::DenseToken { .. } => format_properties_drawer_without_id(&props_json),
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
    if let Some(body) = body_str {
        let trimmed_body = trim_blank_lines(&body);
        if !trimmed_body.is_empty() {
            result.push_str(trimmed_body);
            if !trimmed_body.ends_with('\n') {
                result.push('\n');
            }
            if body_needs_list_terminator(trimmed_body) {
                result.push('\n');
            }
        }
    }

    // Ensure result ends with newline if non-empty
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    result
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

/// Render a source-type Block as Org Mode #+BEGIN_SRC ... #+END_SRC
fn source_block_to_org(block: &Block) -> String {
    let mut result = String::new();

    // #+NAME: if present
    if let Some(ref name) = block.source_name {
        result.push_str("#+NAME: ");
        result.push_str(name);
        result.push('\n');
    }

    result.push_str("#+BEGIN_SRC");

    // Language
    if let Some(ref lang) = block.source_language {
        result.push(' ');
        result.push_str(&lang.to_string());
    }

    // Include block ID in header arguments so it survives round-trips
    // This is critical for preventing orphan blocks when Org files are re-parsed
    result.push_str(" :id ");
    result.push_str(block.id.id());

    // Header arguments (standard known args)
    let header_args = block.get_source_header_args();
    let header_args_str = format_header_args_value(&header_args);
    if !header_args_str.is_empty() {
        result.push(' ');
        result.push_str(&header_args_str);
    }

    // Custom properties stored as flat keys by the parser (non-standard header
    // args) These were split out during parsing and need to be rendered back as
    // header args.
    let mut drawer_props: Vec<_> = block.drawer_properties().into_iter().collect();
    drawer_props.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (k, v) in &drawer_props {
        result.push_str(" :");
        result.push_str(k);
        result.push(' ');
        result.push_str(v);
    }

    // Tags: a Source block has no headline to carry `:tag:` notation, so route
    // them through a `:TAGS <space-joined>` header arg (symmetric with the
    // `:REQUIRES`/`:ADVICE_SUPPRESSED` edge-field lift). Without this a tag on a
    // rule/source block is destroyed on org re-ingest.
    if !block.tags.is_empty() {
        result.push_str(" :TAGS ");
        result.push_str(&block.tags.to_vec().join(" "));
    }

    result.push('\n');

    // Source code, with org-mode comma-escape applied so lines starting with
    // `*` or `#+` don't terminate the source block on re-parse. The parser
    // (parser.rs::source_block extraction) strips one leading comma on these
    // lines to invert this transformation.
    let escaped = escape_source_lines(&block.content);
    result.push_str(&escaped);
    if !escaped.ends_with('\n') {
        result.push('\n');
    }

    result.push_str("#+END_SRC\n");

    result
}

/// Org-mode comma escape for source/example block bodies: lines starting with
/// `*` or `#+` (optionally already-escaped with leading commas) get one extra
/// leading comma. The parser inverts this by stripping one leading comma.
fn escape_source_lines(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for (i, line) in content.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line_needs_comma_escape(line) {
            out.push(',');
        }
        out.push_str(line);
    }
    out
}

fn line_needs_comma_escape(line: &str) -> bool {
    let stripped = line.trim_start_matches(',');
    stripped.starts_with('*') || stripped.starts_with("#+")
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
        block.set_priority(Some(Priority::Medium));
        block.set_tags(Tags::from_csv("work,urgent"));

        assert_eq!(block.level(), 2);
        assert_eq!(block.task_state(), Some(TaskState::from_keyword("TODO")));
        assert_eq!(block.priority(), Some(Priority::Medium));
        assert_eq!(block.tags(), Tags::from_csv("work,urgent"));
    }

    #[test]
    fn test_document_to_org() {
        let mut doc = Block::new_text(EntityUri::no_parent(), EntityUri::no_parent(), "test.org");
        doc.set_page(true);
        doc.set_file_title(Some("My Document".to_string()));
        doc.set_todo_keywords(Some(vec![
            TaskState::active("TODO"),
            TaskState::active("DOING"),
            TaskState::done("DONE"),
        ]));

        let org = render_document_header(&doc);
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
        block.set_priority(Some(Priority::High));
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
}
