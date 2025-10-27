//! Org-mode specific extensions for Document and Block types.
//!
//! This module provides extension traits that add org-mode specific functionality
//! to the generic Document and Block types. Org-specific fields are stored in the
//! `properties` JSON field.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Re-export the generic types
// Note: Block is NOT re-exported here to avoid duplicate type issues with flutter_rust_bridge
// Use holon_api::block::Block directly instead
pub use holon::sync::Document;

// Import Block for use in extension traits (not re-exported to avoid FRB issues)
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::{ContentType, Priority, Tags, TaskState, Timestamp};

// Re-export Directory and ROOT_ID from holon-filesystem
pub use holon_filesystem::directory::{Directory, ROOT_ID};

/// Property keys for org-specific fields stored in properties JSON.
pub mod org_props {
    pub const TITLE: &str = "title";
    pub const TODO_KEYWORDS: &str = "todo_keywords";
    pub const TASK_STATE: &str = "task_state";
    pub const PRIORITY: &str = "priority";
    pub const TAGS: &str = "tags";
    pub const LEVEL: &str = "level";
    pub const SEQUENCE: &str = "sequence";
    pub const SCHEDULED: &str = "scheduled";
    pub const DEADLINE: &str = "deadline";
    pub const ORG_PROPERTIES: &str = "org_properties";
}

// =============================================================================
// Path derivation utilities for org-mode
// =============================================================================

/// Trait for resolving blocks by ID (used for parent chain walking)
pub trait BlockResolver {
    /// Get a block by its ID
    fn get_block(&self, id: &str) -> Option<Block>;
}

/// Find the document ID for a block by walking up the parent chain
///
/// For org-mode blocks:
/// - Top-level blocks have parent_id == document_id (a document URI)
/// - Nested blocks have parent_id pointing to another block
///
/// This function walks up the parent chain until it finds a document ID.
pub fn find_document_id<R: BlockResolver>(block: &Block, resolver: &R) -> Option<String> {
    // Check if parent is already a document
    if block.parent_id.is_doc() {
        return Some(block.parent_id.to_string());
    }

    // Walk up the parent chain
    let mut current_parent_id = block.parent_id.to_string();
    let mut visited = std::collections::HashSet::new();

    while !EntityUri::from_raw(&current_parent_id).is_doc() {
        // Prevent infinite loops
        if visited.contains(&current_parent_id) {
            return None;
        }
        visited.insert(current_parent_id.clone());

        // Look up the parent block
        let parent = resolver.get_block(&current_parent_id)?;
        current_parent_id = parent.parent_id.to_string();
    }

    Some(current_parent_id)
}

/// Get the file path for a block by finding its document and extracting the path
pub fn get_block_file_path<R: BlockResolver>(block: &Block, resolver: &R) -> Option<String> {
    let doc_id = find_document_id(block, resolver)?;
    let uri = EntityUri::from_raw(&doc_id);
    if uri.is_doc() {
        Some(uri.id().to_string())
    } else {
        None
    }
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

/// Default active keywords when file doesn't specify custom TODO config
pub const DEFAULT_ACTIVE_KEYWORDS: &[&str] = &["TODO", "DOING"];

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
    let props: serde_json::Map<String, serde_json::Value> =
        match serde_json::from_str(properties_json) {
            Ok(map) => map,
            Err(_) => return String::new(),
        };

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

    // Render other properties (excluding ID which we already rendered)
    for (key, value) in &props {
        if key == "ID" {
            continue;
        }
        let value_str = match value {
            serde_json::Value::String(s) => s.clone(),
            _ => value.to_string(),
        };
        result.push_str(&format!(":{}: {}\n", key, value_str));
    }
    result.push_str(":END:");
    result
}

/// Format planning lines (SCHEDULED/DEADLINE)
fn format_planning(scheduled: Option<&str>, deadline: Option<&str>) -> String {
    let mut result = String::new();
    if let Some(sched) = scheduled {
        result.push_str(&format!("SCHEDULED: {}\n", sched.trim()));
    }
    if let Some(dead) = deadline {
        result.push_str(&format!("DEADLINE: {}\n", dead.trim()));
    }
    result
}

/// Format header arguments as Org Mode inline parameters.
/// Input: `{ "connection": "main", "results": "table" }`
/// Output: `:connection main :results table`
#[allow(dead_code)]
fn format_header_args(args: &HashMap<String, String>) -> String {
    if args.is_empty() {
        return String::new();
    }

    let mut parts: Vec<String> = args
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                format!(":{}", k)
            } else {
                format!(":{} {}", k, v)
            }
        })
        .collect();

    parts.sort();
    parts.join(" ")
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

/// Extension trait for Document with org-mode specific functionality.
///
/// Provides accessors for org-specific fields stored in the properties JSON:
/// - title: #+TITLE value
/// - todo_keywords: Custom TODO keyword configuration
pub trait OrgDocumentExt {
    /// Get the org title (#+TITLE value)
    fn org_title(&self) -> Option<String>;

    /// Set the org title
    fn set_org_title(&mut self, title: Option<String>);

    /// Get the TODO keywords as TaskState objects.
    fn todo_keywords(&self) -> Option<Vec<TaskState>>;

    /// Set the TODO keywords from TaskState objects.
    fn set_todo_keywords(&mut self, keywords: Option<Vec<TaskState>>);

    /// Parse TODO keywords configuration into (active, done) keyword lists.
    fn parse_todo_keywords(&self) -> (Vec<String>, Vec<String>);

    /// Check if a keyword is "done" according to this document's configuration
    fn is_done(&self, keyword: &str) -> bool;
}

impl OrgDocumentExt for Document {
    fn org_title(&self) -> Option<String> {
        self.get_property(org_props::TITLE)
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    }

    fn set_org_title(&mut self, title: Option<String>) {
        if let Some(t) = title {
            self.set_property(org_props::TITLE, t);
        } else {
            self.properties.remove(org_props::TITLE);
        }
    }

    fn todo_keywords(&self) -> Option<Vec<TaskState>> {
        let value = self.get_property(org_props::TODO_KEYWORDS)?;
        let json_str = value.as_string()?;
        // Try new JSON array format first, fall back to legacy "ACTIVE1,ACTIVE2|DONE1,DONE2"
        if let Ok(states) = serde_json::from_str::<Vec<TaskState>>(&json_str) {
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

impl ToOrg for Document {
    fn to_org(&self) -> String {
        let mut result = String::new();

        // File title
        if let Some(title) = self.org_title() {
            result.push_str(&format!("#+TITLE: {}\n", title));
        }

        // TODO keywords configuration
        if let Some(states) = self.todo_keywords() {
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

    /// Set content from title and body
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

    /// Get sort key as zero-padded sequence
    fn computed_sort_key(&self) -> String;

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
        self.content.lines().next().unwrap_or("").to_string()
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
        self.updated_at = chrono::Utc::now().timestamp_millis();
    }

    fn task_state(&self) -> Option<TaskState> {
        self.get_property(org_props::TASK_STATE)
            .and_then(|v| v.as_string().map(|s| TaskState::from_keyword(&s)))
    }

    fn set_task_state(&mut self, state: Option<TaskState>) {
        if let Some(s) = state {
            self.set_property(
                org_props::TASK_STATE,
                holon_api::Value::String(s.to_string()),
            );
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::TASK_STATE);
            self.set_properties_map(props);
        }
    }

    fn priority(&self) -> Option<Priority> {
        self.get_property(org_props::PRIORITY)
            .and_then(|v| v.as_i64())
            .and_then(|i| Priority::from_int(i as i32).ok())
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
        self.get_property(org_props::TAGS)
            .and_then(|v| v.as_string().map(|s| Tags::from_csv(&s)))
            .unwrap_or_default()
    }

    fn set_tags(&mut self, tags: Tags) {
        if !tags.is_empty() {
            self.set_property(org_props::TAGS, holon_api::Value::String(tags.to_csv()));
        } else {
            let mut props = self.properties_map();
            props.remove(org_props::TAGS);
            self.set_properties_map(props);
        }
    }

    fn scheduled(&self) -> Option<Timestamp> {
        self.get_property(org_props::SCHEDULED)
            .and_then(|v| v.as_string().and_then(|s| Timestamp::parse(&s).ok()))
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
            .and_then(|v| v.as_string().and_then(|s| Timestamp::parse(&s).ok()))
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

    fn computed_sort_key(&self) -> String {
        format!("{:012}", self.sequence())
    }

    fn get_tags(&self) -> Vec<String> {
        self.tags().as_slice().to_vec()
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
            "priority",
            "tags",
            "scheduled",
            "deadline",
            "org_properties",
            "TODO",
            "PRIORITY",
            "TAGS",
            "SCHEDULED",
            "DEADLINE",
            "ID",
            "_source_header_args",
            "_source_results",
        ];

        let mut result = HashMap::new();

        // First, extract from the org_properties JSON if present
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

        // Also include any flat properties that are not internal
        for (k, v) in &self.properties {
            if !INTERNAL_KEYS.contains(&k.as_str()) {
                if let Some(s) = v.as_string() {
                    result.entry(k.clone()).or_insert_with(|| s.to_string());
                }
            }
        }

        result
    }

    fn get_block_id(&self) -> Option<String> {
        self.org_properties()
            .and_then(|json| serde_json::from_str::<HashMap<String, String>>(&json).ok())
            .and_then(|props| props.get("ID").cloned())
            .or_else(|| {
                self.org_properties()
                    .and_then(|json| {
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

impl ToOrg for Block {
    fn to_org(&self) -> String {
        // Source blocks render as #+BEGIN_SRC ... #+END_SRC
        if self.content_type == ContentType::Source {
            return source_block_to_org(self);
        }

        // Text blocks (headlines) render with stars, TODO, etc.
        let mut result = String::new();

        // Headline level (stars)
        result.push_str(&"*".repeat(self.level() as usize));
        result.push(' ');

        // TODO keyword
        if let Some(ref todo) = self.task_state() {
            result.push_str(&todo.to_string());
            result.push(' ');
        }

        // Priority
        if let Some(priority) = self.priority() {
            result.push_str(&format!("[#{}] ", priority.to_letter()));
        }

        // Title
        result.push_str(&self.org_title());

        // Tags
        let tags = self.tags();
        if !tags.is_empty() {
            let formatted_tags = tags.to_org();
            if !formatted_tags.is_empty() {
                result.push(' ');
                result.push_str(&formatted_tags);
            }
        }

        result.push('\n');

        // Properties drawer
        if let Some(props_json) = self.org_properties() {
            let props_drawer = format_properties_drawer(&props_json);
            if !props_drawer.is_empty() {
                result.push_str(&props_drawer);
                result.push('\n');
            }
        }

        // Planning (SCHEDULED/DEADLINE)
        let sched_str = self.scheduled().map(|t| t.to_string());
        let dead_str = self.deadline().map(|t| t.to_string());
        let planning = format_planning(sched_str.as_deref(), dead_str.as_deref());
        if !planning.is_empty() {
            result.push_str(&planning);
        }

        // Body text (source blocks are child Block entities, rendered via tree traversal)
        if let Some(body) = self.body() {
            let trimmed_body = body.trim();
            if !trimmed_body.is_empty() {
                result.push_str(trimmed_body);
                if !trimmed_body.ends_with('\n') {
                    result.push('\n');
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

    // Custom properties stored as flat keys by the parser (non-standard header args)
    // These were split out during parsing and need to be rendered back as header args.
    let mut drawer_props: Vec<_> = block.drawer_properties().into_iter().collect();
    drawer_props.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (k, v) in &drawer_props {
        result.push_str(" :");
        result.push_str(k);
        result.push(' ');
        result.push_str(v);
    }

    result.push('\n');

    // Source code
    result.push_str(&block.content);
    if !block.content.ends_with('\n') {
        result.push('\n');
    }

    result.push_str("#+END_SRC\n");

    result
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
        if token.starts_with(':') {
            if let Some(key) = current_key.take() {
                args.insert(key, current_value.trim().to_string());
                current_value.clear();
            }
            current_key = Some(token[1..].to_string());
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

    #[test]
    fn test_is_document_uri() {
        assert!(EntityUri::from_raw("doc:/path/to/file.org").is_doc());
        assert!(EntityUri::from_raw("doc:relative/path.org").is_doc());
        assert!(!EntityUri::from_raw("block:some-block-id").is_doc());
    }

    fn doc_uri() -> EntityUri {
        EntityUri::file("/test.org")
    }

    #[test]
    fn test_find_document_id_top_level() {
        let block = Block::new_text(
            EntityUri::block("block1"),
            doc_uri(),
            doc_uri(),
            "Test headline",
        );
        let resolver = HashMapBlockResolver::new();

        let doc_id = find_document_id(&block, &resolver);
        assert_eq!(doc_id, Some(doc_uri().to_string()));
    }

    #[test]
    fn test_find_document_id_nested() {
        let block1 = Block::new_text(
            EntityUri::block("block1"),
            doc_uri(),
            doc_uri(),
            "Parent headline",
        );
        let block2 = Block::new_text(
            EntityUri::block("block2"),
            EntityUri::block("block1"),
            doc_uri(),
            "Child headline",
        );

        let resolver = HashMapBlockResolver::from_blocks(vec![block1.clone(), block2.clone()]);

        let doc_id = find_document_id(&block2, &resolver);
        assert_eq!(doc_id, Some(doc_uri().to_string()));
    }

    #[test]
    fn test_find_document_id_deeply_nested() {
        let block1 = Block::new_text(EntityUri::block("block1"), doc_uri(), doc_uri(), "Level 1");
        let block2 = Block::new_text(
            EntityUri::block("block2"),
            EntityUri::block("block1"),
            doc_uri(),
            "Level 2",
        );
        let block3 = Block::new_text(
            EntityUri::block("block3"),
            EntityUri::block("block2"),
            doc_uri(),
            "Level 3",
        );

        let resolver =
            HashMapBlockResolver::from_blocks(vec![block1.clone(), block2.clone(), block3.clone()]);

        let doc_id = find_document_id(&block3, &resolver);
        assert_eq!(doc_id, Some(doc_uri().to_string()));
    }

    #[test]
    fn test_get_block_file_path() {
        let notes_doc = EntityUri::file("/path/to/notes.org");
        let block = Block::new_text(
            EntityUri::block("block1"),
            notes_doc.clone(),
            notes_doc,
            "Test",
        );
        let resolver = HashMapBlockResolver::new();

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
        let mut doc = Document::new(
            EntityUri::doc("test"),
            EntityUri::doc_root(),
            "test.org".to_string(),
        );
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
    fn test_block_computed_sort_key() {
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            doc_uri(),
            "Test headline",
        );
        block.set_sequence(42);

        assert_eq!(block.computed_sort_key(), "000000000042");
    }

    #[test]
    fn test_block_title_and_body() {
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            doc_uri(),
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
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            doc_uri(),
            "Test",
        );
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
        let mut doc = Document::new(
            EntityUri::doc("test"),
            EntityUri::doc_root(),
            "test.org".to_string(),
        );
        doc.set_org_title(Some("My Document".to_string()));
        doc.set_todo_keywords(Some(vec![
            TaskState::active("TODO"),
            TaskState::active("DOING"),
            TaskState::done("DONE"),
        ]));

        let org = doc.to_org();
        assert!(org.contains("#+TITLE: My Document"));
        assert!(org.contains("#+TODO: TODO DOING | DONE"));
    }

    #[test]
    fn test_block_to_org() {
        let mut block = Block::new_text(
            EntityUri::block("id1"),
            EntityUri::block("parent1"),
            doc_uri(),
            "Test headline",
        );
        block.set_level(2);
        block.set_task_state(Some(TaskState::from_keyword("TODO")));
        block.set_priority(Some(Priority::High));
        block.set_tags(Tags::from_csv("work,urgent"));

        let org = block.to_org();
        assert!(org.starts_with("** TODO [#A] Test headline :work:urgent:"));
    }
}
