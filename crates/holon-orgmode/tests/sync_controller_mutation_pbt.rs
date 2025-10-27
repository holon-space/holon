//! Phase 2: OrgSyncController-level mutation PBTs.
//!
//! Two property-based tests that exercise the full sync loop:
//! - `test_sync_block_change_to_file`: in-memory mutation → on_block_changed → file → parse → assert
//! - `test_sync_file_change_to_blocks`: org text mutation → on_file_changed → store → assert
//!
//! Uses mock implementations of BlockReader, OperationProvider, and DocumentManager.

use anyhow::Result;
use async_trait::async_trait;
use holon::core::datasource::{OperationProvider, OperationResult, Result as CoreResult};
use holon::sync::Document;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::render_types::OperationDescriptor;
use holon_api::types::{ContentType, Priority, Tags, TaskState, Timestamp};
use holon_api::Value;
use holon_orgmode::models::{OrgBlockExt, DEFAULT_ACTIVE_KEYWORDS, DEFAULT_DONE_KEYWORDS};
use holon_orgmode::org_renderer::OrgRenderer;
use holon_orgmode::parser::parse_org_file;
use holon_orgmode::traits::{BlockReader, DocumentManager};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

// ============================================================================
// Mock infrastructure
// ============================================================================

struct InMemoryBlockStore {
    blocks: RwLock<HashMap<String, Vec<Block>>>,
}

impl InMemoryBlockStore {
    fn new() -> Self {
        Self {
            blocks: RwLock::new(HashMap::new()),
        }
    }

    fn seed_blocks(&self, doc_id: &str, blocks: Vec<Block>) {
        self.blocks
            .write()
            .unwrap()
            .insert(doc_id.to_string(), blocks);
    }

    fn get_all_blocks(&self, doc_id: &str) -> Vec<Block> {
        self.blocks
            .read()
            .unwrap()
            .get(doc_id)
            .cloned()
            .unwrap_or_default()
    }

    fn apply_create(&self, block: Block) {
        let doc_id = block.document_id.to_string();
        let mut store = self.blocks.write().unwrap();
        store.entry(doc_id).or_default().push(block);
    }

    fn apply_update(&self, block: Block) {
        let doc_id = block.document_id.to_string();
        let mut store = self.blocks.write().unwrap();
        if let Some(blocks) = store.get_mut(&doc_id) {
            if let Some(existing) = blocks.iter_mut().find(|b| b.id == block.id) {
                *existing = block;
            }
        }
    }

    fn apply_delete(&self, block_id: &str) {
        let mut store = self.blocks.write().unwrap();
        for blocks in store.values_mut() {
            blocks.retain(|b| b.id.as_str() != block_id);
        }
    }
}

#[async_trait]
impl BlockReader for InMemoryBlockStore {
    async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>> {
        Ok(self.get_all_blocks(doc_id.as_str()))
    }

    async fn iter_documents_with_blocks(&self) -> Vec<(EntityUri, Vec<Block>)> {
        self.blocks
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    EntityUri::parse(k).expect("stored key must be valid URI"),
                    v.clone(),
                )
            })
            .collect()
    }

    async fn find_foreign_blocks(
        &self,
        _block_ids: &[EntityUri],
        _expected_doc_uri: &EntityUri,
    ) -> Result<Vec<(EntityUri, EntityUri)>> {
        Ok(Vec::new())
    }
}

struct MockOperationProvider {
    store: Arc<InMemoryBlockStore>,
}

#[async_trait]
impl OperationProvider for MockOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        Vec::new()
    }

    async fn execute_operation(
        &self,
        entity_name: &str,
        op_name: &str,
        params: HashMap<String, Value>,
    ) -> CoreResult<OperationResult> {
        assert_eq!(entity_name, "block");

        match op_name {
            "create" | "update" => {
                let block = block_from_params(&params);
                if op_name == "create" {
                    self.store.apply_create(block);
                } else {
                    self.store.apply_update(block);
                }
            }
            "delete" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_string())
                    .expect("delete must have id param");
                self.store.apply_delete(&id);
            }
            other => panic!("unexpected operation: {other}"),
        }

        Ok(OperationResult::irreversible(Vec::new()))
    }
}

fn block_from_params(params: &HashMap<String, Value>) -> Block {
    let get_str = |key: &str| -> String {
        params
            .get(key)
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };

    let id = EntityUri::from_raw(&get_str("id"));
    let parent_id = EntityUri::from_raw(&get_str("parent_id"));
    let document_id = EntityUri::from_raw(&get_str("document_id"));
    let content = get_str("content");
    let content_type: ContentType = get_str("content_type").parse().unwrap_or(ContentType::Text);

    let source_language = params
        .get("source_language")
        .and_then(|v| v.as_string())
        .and_then(|s| s.parse().ok());
    let source_name = params
        .get("source_name")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());

    let created_at = params
        .get("created_at")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let updated_at = params
        .get("updated_at")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    let mut block = Block {
        id,
        parent_id,
        document_id,
        content,
        content_type,
        source_language,
        source_name,
        properties: HashMap::new(),
        created_at,
        updated_at,
    };

    if let Some(seq) = params.get("sequence").and_then(|v| v.as_i64()) {
        block.set_sequence(seq);
    }
    if let Some(ts) = params.get("task_state").and_then(|v| v.as_string()) {
        block.set_task_state(Some(TaskState::from_keyword(&ts)));
    }
    if let Some(p) = params.get("priority").and_then(|v| v.as_i64()) {
        if let Ok(priority) = Priority::from_int(p as i32) {
            block.set_priority(Some(priority));
        }
    }
    if let Some(t) = params.get("tags").and_then(|v| v.as_string()) {
        block.set_tags(Tags::from_csv(&t));
    }
    if let Some(s) = params.get("scheduled").and_then(|v| v.as_string()) {
        if let Ok(ts) = Timestamp::parse(&s) {
            block.set_scheduled(Some(ts));
        }
    }
    if let Some(d) = params.get("deadline").and_then(|v| v.as_string()) {
        if let Ok(ts) = Timestamp::parse(&d) {
            block.set_deadline(Some(ts));
        }
    }
    if let Some(id_val) = params.get("ID").and_then(|v| v.as_string()) {
        block.set_property("ID", Value::String(id_val.to_string()));
    }
    if let Some(args_json) = params.get("source_header_args").and_then(|v| v.as_string()) {
        if let Ok(args) = serde_json::from_str::<HashMap<String, Value>>(&args_json) {
            block.set_source_header_args(args);
        }
    }

    const STANDARD_KEYS: &[&str] = &[
        "id",
        "parent_id",
        "document_id",
        "content",
        "content_type",
        "source_language",
        "source_name",
        "source_header_args",
        "created_at",
        "updated_at",
        "sequence",
        "task_state",
        "priority",
        "tags",
        "scheduled",
        "deadline",
        "ID",
    ];
    for (k, v) in params {
        if !STANDARD_KEYS.contains(&k.as_str()) {
            if let Some(s) = v.as_string() {
                block.set_property(k, Value::String(s.to_string()));
            }
        }
    }

    block
}

struct MockDocumentManager {
    documents: RwLock<Vec<Document>>,
}

impl MockDocumentManager {
    fn new() -> Self {
        let root = Document::new(
            EntityUri::doc_root(),
            EntityUri::no_parent(),
            "".to_string(),
        );
        Self {
            documents: RwLock::new(vec![root]),
        }
    }

    fn add_document(&self, doc: Document) {
        self.documents.write().unwrap().push(doc);
    }
}

#[async_trait]
impl DocumentManager for MockDocumentManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        name: &str,
    ) -> Result<Option<Document>> {
        let docs = self.documents.read().unwrap();
        Ok(docs
            .iter()
            .find(|d| d.parent_id == *parent_id && d.name == name)
            .cloned())
    }

    async fn create(&self, doc: Document) -> Result<Document> {
        self.documents.write().unwrap().push(doc.clone());
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> Result<Option<Document>> {
        let docs = self.documents.read().unwrap();
        Ok(docs.iter().find(|d| d.id == *id).cloned())
    }
}

// ============================================================================
// Normalized comparison
// ============================================================================

/// Normalized block for comparison.
///
/// `level` is excluded: the renderer computes it from tree depth, and
/// `build_block_params` doesn't include it — so store blocks lack level
/// while parsed blocks have it. Comparing them would always mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedBlock {
    id: EntityUri,
    content_type: ContentType,
    title: String,
    task_state: Option<TaskState>,
    priority: Option<Priority>,
    tags: BTreeSet<String>,
    scheduled: Option<String>,
    deadline: Option<String>,
    source_language: Option<String>,
    source_name: Option<String>,
    header_args: BTreeMap<String, String>,
    drawer_properties: BTreeMap<String, String>,
}

impl NormalizedBlock {
    fn from_block(block: &Block) -> Self {
        let title = block.org_title().trim().to_string();
        let tags: BTreeSet<String> = block.tags().to_set();
        let header_args: BTreeMap<String, String> = block
            .get_source_header_args()
            .into_iter()
            .filter(|(k, _)| k != "id")
            .map(|(k, v)| (k, v.as_string().unwrap_or_default().to_string()))
            .collect();
        let drawer_properties: BTreeMap<String, String> =
            block.drawer_properties().into_iter().collect();

        NormalizedBlock {
            id: block.id.clone(),
            content_type: block.content_type,
            title,
            task_state: block.task_state(),
            priority: block.priority(),
            tags,
            scheduled: block.scheduled().map(|t| t.to_string()),
            deadline: block.deadline().map(|t| t.to_string()),
            source_language: block.source_language.as_ref().map(|l| l.to_string()),
            source_name: block.source_name.clone(),
            header_args,
            drawer_properties,
        }
    }
}

fn normalize_blocks(blocks: &[Block]) -> BTreeMap<String, NormalizedBlock> {
    blocks
        .iter()
        .map(|b| (b.id.as_str().to_string(), NormalizedBlock::from_block(b)))
        .collect()
}

fn assert_blocks_equivalent(expected: &[Block], actual: &[Block], context: &str) {
    let exp = normalize_blocks(expected);
    let act = normalize_blocks(actual);

    assert_eq!(
        exp.len(),
        act.len(),
        "[{context}] Block count mismatch.\nExpected IDs: {:?}\nActual IDs: {:?}",
        exp.keys().collect::<Vec<_>>(),
        act.keys().collect::<Vec<_>>(),
    );

    for (id, exp_block) in &exp {
        let act_block = act.get(id).unwrap_or_else(|| {
            panic!(
                "[{context}] Block '{id}' missing from actual. Actual IDs: {:?}",
                act.keys().collect::<Vec<_>>()
            )
        });

        assert_eq!(
            exp_block, act_block,
            "[{context}] Block '{id}' differs.\nExpected: {exp_block:#?}\nActual: {act_block:#?}"
        );
    }
}

// ============================================================================
// Test fixture
// ============================================================================

struct TestFixture {
    store: Arc<InMemoryBlockStore>,
    controller: OrgSyncController,
    root_dir: PathBuf,
    doc_id: EntityUri,
    doc_name: String,
}

impl TestFixture {
    fn new(temp_dir: &std::path::Path) -> Self {
        let store = Arc::new(InMemoryBlockStore::new());
        let op_provider = Arc::new(MockOperationProvider {
            store: store.clone(),
        });
        let doc_manager = Arc::new(MockDocumentManager::new());

        let root_dir = temp_dir.to_path_buf();
        let controller = OrgSyncController::new(
            store.clone(),
            op_provider,
            doc_manager.clone(),
            root_dir.clone(),
        );

        let doc_id = EntityUri::doc_random();
        let doc_name = "test".to_string();

        let doc = Document::new(doc_id.clone(), EntityUri::doc_root(), doc_name.clone());
        doc_manager.add_document(doc);

        TestFixture {
            store,
            controller,
            root_dir,
            doc_id,
            doc_name,
        }
    }

    fn file_path(&self) -> PathBuf {
        self.root_dir.join(format!("{}.org", self.doc_name))
    }

    fn seed_blocks(&self, blocks: &[Block]) {
        self.store
            .seed_blocks(self.doc_id.as_str(), blocks.to_vec());
    }

    fn get_stored_blocks(&self) -> Vec<Block> {
        self.store.get_all_blocks(self.doc_id.as_str())
    }
}

// ============================================================================
// Strategies (reused from Phase 1 round_trip_pbt.rs concepts)
// ============================================================================

fn valid_title() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9][a-zA-Z0-9 ]{0,48}[a-zA-Z0-9]"
}

fn valid_body() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?\n]{10,200}"
}

fn valid_tag() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,14}"
}

fn valid_property_value() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,30}"
}

fn valid_timestamp() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("<2024-01-15 Mon>".to_string()),
        Just("<2024-06-20 Thu 14:00>".to_string()),
        Just("<2024-12-31 Tue 09:30>".to_string()),
    ]
}

// -- BlockMutation: applied to blocks before on_block_changed ----------------

#[derive(Debug, Clone)]
enum BlockMutation {
    SetTitle(String),
    SetBody(Option<String>),
    SetTaskState(Option<TaskState>),
    SetPriority(Option<Priority>),
    SetTags(Tags),
    AddTag(String),
    RemoveAllTags,
    SetScheduled(Option<Timestamp>),
    SetDeadline(Option<Timestamp>),
    SetDrawerProperty { key: String, value: String },
}

fn block_mutation_strategy() -> impl Strategy<Value = BlockMutation> {
    prop_oneof![
        valid_title().prop_map(BlockMutation::SetTitle),
        prop::option::of(valid_body()).prop_map(BlockMutation::SetBody),
        prop::option::of(prop_oneof![
            Just(TaskState::active("TODO")),
            Just(TaskState::done("DONE")),
            Just(TaskState::active("DOING")),
            Just(TaskState::done("CANCELLED")),
            Just(TaskState::done("CLOSED")),
        ])
        .prop_map(BlockMutation::SetTaskState),
        prop::option::of(prop_oneof![
            Just(Priority::Low),
            Just(Priority::Medium),
            Just(Priority::High),
        ])
        .prop_map(BlockMutation::SetPriority),
        prop::collection::vec(valid_tag(), 0..=3)
            .prop_map(|v| BlockMutation::SetTags(Tags::from(v))),
        valid_tag().prop_map(BlockMutation::AddTag),
        Just(BlockMutation::RemoveAllTags),
        valid_timestamp().prop_map(|s| BlockMutation::SetScheduled(Timestamp::parse(&s).ok())),
        valid_timestamp().prop_map(|s| BlockMutation::SetDeadline(Timestamp::parse(&s).ok())),
        (
            prop_oneof![
                Just("VIEW".to_string()),
                Just("REGION".to_string()),
                Just("CUSTOM".to_string()),
                Just("column-order".to_string()),
            ],
            valid_property_value(),
        )
            .prop_map(|(key, value)| BlockMutation::SetDrawerProperty { key, value }),
    ]
}

fn apply_block_mutation(block: &mut Block, mutation: &BlockMutation) {
    match mutation {
        BlockMutation::SetTitle(new_title) => {
            let body = block.body();
            block.set_title_and_body(new_title.clone(), body);
        }
        BlockMutation::SetBody(new_body) => {
            let title = block.org_title().to_string();
            block.set_title_and_body(title, new_body.clone());
        }
        BlockMutation::SetTaskState(state) => {
            block.set_task_state(state.clone());
        }
        BlockMutation::SetPriority(priority) => {
            block.set_priority(*priority);
        }
        BlockMutation::SetTags(tags) => {
            block.set_tags(tags.clone());
        }
        BlockMutation::AddTag(tag) => {
            let mut current = block.tags().as_slice().to_vec();
            current.push(tag.clone());
            block.set_tags(Tags::from(current));
        }
        BlockMutation::RemoveAllTags => {
            block.set_tags(Tags::default());
        }
        BlockMutation::SetScheduled(ts) => {
            block.set_scheduled(ts.clone());
        }
        BlockMutation::SetDeadline(ts) => {
            block.set_deadline(ts.clone());
        }
        BlockMutation::SetDrawerProperty { key, value } => {
            block.set_property(key, Value::String(value.clone()));
            let mut drawer = block.drawer_properties();
            drawer.insert(key.clone(), value.clone());
            let mut org_map = serde_json::Map::new();
            let id_val = block
                .get_property("ID")
                .and_then(|v| v.as_string().map(|s| s.to_string()));
            if let Some(id_str) = id_val {
                org_map.insert("ID".to_string(), serde_json::Value::String(id_str));
            }
            for (k, v) in &drawer {
                org_map.insert(k.clone(), serde_json::Value::String(v.clone()));
            }
            block.set_org_properties(Some(serde_json::to_string(&org_map).unwrap()));
        }
    }
}

// -- TextMutation: applied to org text before on_file_changed ----------------

#[derive(Debug, Clone)]
enum TextMutation {
    ReplaceTitle {
        headline_idx: usize,
        new_title: String,
    },
    AddTodoKeyword {
        headline_idx: usize,
        keyword: String,
    },
    RemoveTodoKeyword {
        headline_idx: usize,
    },
    AddTag {
        headline_idx: usize,
        tag: String,
    },
    SetPriority {
        headline_idx: usize,
        letter: char,
    },
    RemovePriority {
        headline_idx: usize,
    },
    AddNewHeadline {
        id: String,
        title: String,
    },
    DeleteHeadline {
        headline_idx: usize,
    },
}

fn text_mutation_strategy() -> impl Strategy<Value = TextMutation> {
    prop_oneof![
        // Index capped later via modulo
        (0..10usize, valid_title()).prop_map(|(i, t)| TextMutation::ReplaceTitle {
            headline_idx: i,
            new_title: t
        }),
        (
            0..10usize,
            prop_oneof![
                Just("TODO".to_string()),
                Just("DOING".to_string()),
                Just("DONE".to_string()),
            ]
        )
            .prop_map(|(i, kw)| TextMutation::AddTodoKeyword {
                headline_idx: i,
                keyword: kw
            }),
        (0..10usize).prop_map(|i| TextMutation::RemoveTodoKeyword { headline_idx: i }),
        (0..10usize, valid_tag()).prop_map(|(i, t)| TextMutation::AddTag {
            headline_idx: i,
            tag: t
        }),
        (0..10usize, prop_oneof![Just('A'), Just('B'), Just('C')]).prop_map(|(i, l)| {
            TextMutation::SetPriority {
                headline_idx: i,
                letter: l,
            }
        }),
        (0..10usize).prop_map(|i| TextMutation::RemovePriority { headline_idx: i }),
        valid_title().prop_map(|title| TextMutation::AddNewHeadline {
            id: Uuid::new_v4().to_string(),
            title,
        }),
        (0..10usize).prop_map(|i| TextMutation::DeleteHeadline { headline_idx: i }),
    ]
}

struct HeadlineInfo {
    line_idx: usize,
    level: usize,
}

fn find_headlines(org_text: &str) -> Vec<HeadlineInfo> {
    org_text
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            if line.starts_with('*') {
                let level = line.chars().take_while(|c| *c == '*').count();
                if level > 0 && line.chars().nth(level) == Some(' ') {
                    return Some(HeadlineInfo { line_idx: i, level });
                }
            }
            None
        })
        .collect()
}

fn apply_text_mutation(org_text: &str, mutation: &TextMutation) -> Option<String> {
    let mut lines: Vec<String> = org_text.lines().map(|l| l.to_string()).collect();
    let headlines = find_headlines(org_text);

    match mutation {
        TextMutation::ReplaceTitle {
            headline_idx,
            new_title,
        } => {
            let hl = headlines.get(*headline_idx % headlines.len())?;
            let line = &lines[hl.line_idx];
            lines[hl.line_idx] = replace_title_in_headline(line, hl.level, new_title);
        }
        TextMutation::AddTodoKeyword {
            headline_idx,
            keyword,
        } => {
            let hl = headlines.get(*headline_idx % headlines.len())?;
            let line = &lines[hl.line_idx];
            let after_stars = line[hl.level..].trim_start();
            // Skip if already has a TODO keyword
            let has_todo = DEFAULT_ACTIVE_KEYWORDS
                .iter()
                .chain(DEFAULT_DONE_KEYWORDS.iter())
                .any(|kw| after_stars.starts_with(kw) && after_stars[kw.len()..].starts_with(' '));
            if has_todo {
                return None;
            }
            let stars = "*".repeat(hl.level);
            let rest = after_stars;
            lines[hl.line_idx] = format!("{} {} {}", stars, keyword, rest);
        }
        TextMutation::RemoveTodoKeyword { headline_idx } => {
            let hl = headlines.get(*headline_idx % headlines.len())?;
            let line = &lines[hl.line_idx];
            let after_stars = line[hl.level..].trim_start();
            let removed = DEFAULT_ACTIVE_KEYWORDS
                .iter()
                .chain(DEFAULT_DONE_KEYWORDS.iter())
                .find(|kw| {
                    after_stars.starts_with(*kw) && after_stars[kw.len()..].starts_with(' ')
                });
            match removed {
                Some(kw) => {
                    let stars = "*".repeat(hl.level);
                    let rest = after_stars[kw.len()..].trim_start();
                    lines[hl.line_idx] = format!("{} {}", stars, rest);
                }
                None => return None,
            }
        }
        TextMutation::AddTag { headline_idx, tag } => {
            let hl = headlines.get(*headline_idx % headlines.len())?;
            let line = &lines[hl.line_idx];
            let trimmed = line.trim_end();
            if trimmed.ends_with(':') {
                // Has existing tags — append before final colon
                lines[hl.line_idx] = format!("{}{}:", trimmed, tag);
            } else {
                lines[hl.line_idx] = format!("{} :{}:", trimmed, tag);
            }
        }
        TextMutation::SetPriority {
            headline_idx,
            letter,
        } => {
            let hl = headlines.get(*headline_idx % headlines.len())?;
            let line = &lines[hl.line_idx];
            lines[hl.line_idx] = set_priority_in_headline(line, hl.level, *letter);
        }
        TextMutation::RemovePriority { headline_idx } => {
            let hl = headlines.get(*headline_idx % headlines.len())?;
            let line = &lines[hl.line_idx];
            lines[hl.line_idx] = remove_priority_in_headline(line, hl.level);
        }
        TextMutation::AddNewHeadline { id, title } => {
            lines.push(format!("* {}", title));
            lines.push(":PROPERTIES:".to_string());
            lines.push(format!(":ID: {}", id));
            lines.push(":END:".to_string());
        }
        TextMutation::DeleteHeadline { headline_idx } => {
            if headlines.len() <= 1 {
                return None; // Don't delete the last headline
            }
            let hl = headlines.get(*headline_idx % headlines.len())?;
            let start = hl.line_idx;
            // Find the end of this headline's section (next headline at same or higher level)
            let end = headlines
                .iter()
                .find(|h| h.line_idx > start && h.level <= hl.level)
                .map(|h| h.line_idx)
                .unwrap_or(lines.len());
            lines.drain(start..end);
        }
    }

    let mut result = lines.join("\n");
    if org_text.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

fn replace_title_in_headline(line: &str, level: usize, new_title: &str) -> String {
    let after_stars = line[level..].trim_start();
    let stars = "*".repeat(level);
    let mut prefix_parts = Vec::new();
    let mut rest = after_stars;

    // Preserve TODO keyword
    let all_keywords: Vec<&&str> = DEFAULT_ACTIVE_KEYWORDS
        .iter()
        .chain(DEFAULT_DONE_KEYWORDS.iter())
        .collect();
    for kw in &all_keywords {
        if rest.starts_with(**kw) && rest[kw.len()..].starts_with(' ') {
            prefix_parts.push(kw.to_string());
            rest = rest[kw.len()..].trim_start();
            break;
        }
    }

    // Preserve priority
    if rest.starts_with("[#") && rest.len() >= 4 && rest.as_bytes()[3] == b']' {
        prefix_parts.push(rest[..4].to_string());
        rest = rest[4..].trim_start();
    }

    // Preserve tags at end
    let tags_suffix = extract_trailing_tags(rest).unwrap_or("");

    let prefix = prefix_parts.join(" ");
    let mut result = stars;
    result.push(' ');
    if !prefix.is_empty() {
        result.push_str(&prefix);
        result.push(' ');
    }
    result.push_str(new_title);
    if !tags_suffix.is_empty() {
        result.push(' ');
        result.push_str(tags_suffix);
    }
    result
}

fn extract_trailing_tags(text: &str) -> Option<&str> {
    let trimmed = text.trim_end();
    if trimmed.ends_with(':') {
        if let Some(pos) = trimmed.rfind(' ') {
            let candidate = &trimmed[pos + 1..];
            if candidate.starts_with(':') && candidate.ends_with(':') && candidate.len() > 2 {
                return Some(candidate);
            }
        }
    }
    None
}

fn set_priority_in_headline(line: &str, level: usize, letter: char) -> String {
    let after_stars = line[level..].trim_start();
    let stars = "*".repeat(level);
    let mut rest = after_stars;
    let mut todo = None;

    let all_keywords: Vec<&&str> = DEFAULT_ACTIVE_KEYWORDS
        .iter()
        .chain(DEFAULT_DONE_KEYWORDS.iter())
        .collect();
    for kw in &all_keywords {
        if rest.starts_with(**kw) && rest[kw.len()..].starts_with(' ') {
            todo = Some(kw.to_string());
            rest = rest[kw.len()..].trim_start();
            break;
        }
    }

    if rest.starts_with("[#") && rest.len() >= 4 && rest.as_bytes()[3] == b']' {
        rest = rest[4..].trim_start();
    }

    let mut result = stars;
    result.push(' ');
    if let Some(kw) = todo {
        result.push_str(&kw);
        result.push(' ');
    }
    result.push_str(&format!("[#{}] {}", letter, rest));
    result
}

fn remove_priority_in_headline(line: &str, level: usize) -> String {
    let after_stars = line[level..].trim_start();
    let stars = "*".repeat(level);
    let mut rest = after_stars;
    let mut todo = None;

    let all_keywords: Vec<&&str> = DEFAULT_ACTIVE_KEYWORDS
        .iter()
        .chain(DEFAULT_DONE_KEYWORDS.iter())
        .collect();
    for kw in &all_keywords {
        if rest.starts_with(**kw) && rest[kw.len()..].starts_with(' ') {
            todo = Some(kw.to_string());
            rest = rest[kw.len()..].trim_start();
            break;
        }
    }

    if rest.starts_with("[#") && rest.len() >= 4 && rest.as_bytes()[3] == b']' {
        rest = rest[4..].trim_start();
    }

    let mut result = stars;
    result.push(' ');
    if let Some(kw) = todo {
        result.push_str(&kw);
        result.push(' ');
    }
    result.push_str(rest);
    result
}

// ============================================================================
// Block generation: render → parse round-trip to get stable baseline blocks
// ============================================================================

fn generate_baseline_blocks(doc_id: &EntityUri, variant: u8) -> Vec<Block> {
    let doc_uri = doc_id.clone();

    match variant % 3 {
        // Two flat siblings
        0 => {
            let id1 = EntityUri::block(&Uuid::new_v4().to_string());
            let id2 = EntityUri::block(&Uuid::new_v4().to_string());
            let mut b1 = Block::new_text(id1.clone(), doc_uri.clone(), doc_uri.clone(), "Alpha");
            b1.set_level(1);
            b1.set_sequence(0);
            b1.set_property("ID", Value::String(id1.id().to_string()));
            let mut b2 = Block::new_text(id2.clone(), doc_uri.clone(), doc_uri.clone(), "Beta");
            b2.set_level(1);
            b2.set_sequence(1);
            b2.set_property("ID", Value::String(id2.id().to_string()));
            vec![b1, b2]
        }
        // Parent with two children, one has TODO+priority+tags
        1 => {
            let p = EntityUri::block(&Uuid::new_v4().to_string());
            let c1 = EntityUri::block(&Uuid::new_v4().to_string());
            let c2 = EntityUri::block(&Uuid::new_v4().to_string());

            let mut bp = Block::new_text(p.clone(), doc_uri.clone(), doc_uri.clone(), "Parent");
            bp.set_level(1);
            bp.set_sequence(0);
            bp.set_property("ID", Value::String(p.id().to_string()));

            let mut bc1 = Block::new_text(c1.clone(), p.clone(), doc_uri.clone(), "Child one");
            bc1.set_level(2);
            bc1.set_sequence(1);
            bc1.set_task_state(Some(TaskState::active("TODO")));
            bc1.set_property("ID", Value::String(c1.id().to_string()));

            let mut bc2 = Block::new_text(c2.clone(), p.clone(), doc_uri.clone(), "Child two");
            bc2.set_level(2);
            bc2.set_sequence(2);
            bc2.set_task_state(Some(TaskState::active("TODO")));
            bc2.set_priority(Some(Priority::High));
            bc2.set_tags(Tags::from(vec!["work".to_string()]));
            bc2.set_property("ID", Value::String(c2.id().to_string()));

            vec![bp, bc1, bc2]
        }
        // Three flat siblings with varied properties
        _ => {
            let ids: Vec<EntityUri> = (0..3)
                .map(|_| EntityUri::block(&Uuid::new_v4().to_string()))
                .collect();

            let mut b0 = Block::new_text(ids[0].clone(), doc_uri.clone(), doc_uri.clone(), "Inbox");
            b0.set_level(1);
            b0.set_sequence(0);
            b0.set_property("ID", Value::String(ids[0].id().to_string()));

            let mut b1 =
                Block::new_text(ids[1].clone(), doc_uri.clone(), doc_uri.clone(), "Projects");
            b1.set_level(1);
            b1.set_sequence(1);
            b1.set_task_state(Some(TaskState::active("DOING")));
            b1.set_scheduled(Timestamp::parse("<2024-06-20 Thu 14:00>").ok());
            b1.set_property("ID", Value::String(ids[1].id().to_string()));

            let mut b2 =
                Block::new_text(ids[2].clone(), doc_uri.clone(), doc_uri.clone(), "Archive");
            b2.set_level(1);
            b2.set_sequence(2);
            b2.set_tags(Tags::from(vec!["archive".to_string(), "old".to_string()]));
            b2.set_deadline(Timestamp::parse("<2024-12-31 Tue 09:30>").ok());
            b2.set_property("ID", Value::String(ids[2].id().to_string()));

            vec![b0, b1, b2]
        }
    }
}

/// Render blocks → parse to get a stable round-tripped baseline.
fn stabilize_blocks(
    blocks: &[Block],
    doc_id: &EntityUri,
    root_dir: &std::path::Path,
) -> Vec<Block> {
    let file_path = root_dir.join("test.org");
    let org_text = OrgRenderer::render_blocks(blocks, &file_path, doc_id);
    let parse_result = parse_org_file(&file_path, &org_text, &EntityUri::doc_root(), 0, root_dir)
        .expect("stabilize: parse must succeed");
    parse_result.blocks
}

// ============================================================================
// PBT: test_sync_block_change_to_file
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 100,
        ..ProptestConfig::default()
    })]

    #[test]
    fn test_sync_block_change_to_file(
        variant in 0..3u8,
        mutation in block_mutation_strategy(),
        target_idx in any::<prop::sample::Index>(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let temp_dir = tempfile::tempdir().unwrap();
            let mut fixture = TestFixture::new(temp_dir.path());

            // Generate + stabilize baseline
            let raw_blocks = generate_baseline_blocks(&fixture.doc_id, variant);
            let baseline = stabilize_blocks(&raw_blocks, &fixture.doc_id, &fixture.root_dir);
            prop_assume!(!baseline.is_empty());

            let text_indices: Vec<usize> = baseline
                .iter()
                .enumerate()
                .filter(|(_, b)| b.content_type == ContentType::Text)
                .map(|(i, _)| i)
                .collect();
            prop_assume!(!text_indices.is_empty());

            // Seed store + initialize controller + write initial file
            fixture.seed_blocks(&baseline);
            fixture.controller.initialize().await;

            let initial_org =
                OrgRenderer::render_blocks(&baseline, &fixture.file_path(), &fixture.doc_id);
            tokio::fs::write(&fixture.file_path(), &initial_org)
                .await
                .unwrap();

            // Apply mutation to a clone and seed into store
            let mut mutated = baseline.clone();
            let idx = target_idx.index(text_indices.len());
            let block_idx = text_indices[idx];
            apply_block_mutation(&mut mutated[block_idx], &mutation);
            fixture.seed_blocks(&mutated);

            // on_block_changed → file write
            fixture
                .controller
                .on_block_changed(&fixture.doc_id)
                .await
                .unwrap();

            // Parse written file
            let file_content = tokio::fs::read_to_string(&fixture.file_path())
                .await
                .unwrap();
            let parsed = parse_org_file(
                &fixture.file_path(),
                &file_content,
                &EntityUri::doc_root(),
                0,
                &fixture.root_dir,
            )
            .unwrap();

            assert_blocks_equivalent(&mutated, &parsed.blocks, "block_change_to_file");

            Ok::<(), TestCaseError>(())
        })?;
    }
}

// ============================================================================
// PBT: test_sync_file_change_to_blocks
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 100,
        ..ProptestConfig::default()
    })]

    #[test]
    fn test_sync_file_change_to_blocks(
        variant in 0..3u8,
        mutation in text_mutation_strategy(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let temp_dir = tempfile::tempdir().unwrap();
            let mut fixture = TestFixture::new(temp_dir.path());

            // Generate + stabilize baseline
            let raw_blocks = generate_baseline_blocks(&fixture.doc_id, variant);
            let baseline = stabilize_blocks(&raw_blocks, &fixture.doc_id, &fixture.root_dir);
            prop_assume!(!baseline.is_empty());

            // Seed store + initialize + write + establish last_projection
            fixture.seed_blocks(&baseline);
            fixture.controller.initialize().await;

            let initial_org =
                OrgRenderer::render_blocks(&baseline, &fixture.file_path(), &fixture.doc_id);
            tokio::fs::write(&fixture.file_path(), &initial_org)
                .await
                .unwrap();

            fixture
                .controller
                .on_file_changed(&fixture.file_path())
                .await
                .unwrap();

            // Apply text mutation to org file
            let mutated_org = match apply_text_mutation(&initial_org, &mutation) {
                Some(text) => text,
                None => return Ok::<(), TestCaseError>(()),
            };

            tokio::fs::write(&fixture.file_path(), &mutated_org)
                .await
                .unwrap();

            // on_file_changed → store update (also re-renders + rewrites the file)
            fixture
                .controller
                .on_file_changed(&fixture.file_path())
                .await
                .unwrap();

            // Read back the final file (on_file_changed may have re-rendered it)
            let final_org = tokio::fs::read_to_string(&fixture.file_path())
                .await
                .unwrap();
            let expected_parse = parse_org_file(
                &fixture.file_path(),
                &final_org,
                &EntityUri::doc_root(),
                0,
                &fixture.root_dir,
            )
            .unwrap();

            // The store should match what the final file on disk parses to
            let stored = fixture.get_stored_blocks();
            assert_blocks_equivalent(&expected_parse.blocks, &stored, "file_change_to_blocks");

            Ok::<(), TestCaseError>(())
        })?;
    }
}
