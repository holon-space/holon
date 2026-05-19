//! Phase 2: OrgSyncController-level mutation PBTs.
//!
//! Two property-based tests that exercise the full sync loop:
//! - `test_sync_block_change_to_file`: in-memory mutation → on_block_changed → file → parse → assert
//! - `test_sync_file_change_to_blocks`: org text mutation → on_file_changed → store → assert
//!
//! Requires the `di` feature (org_sync_controller is only compiled with it).

#![cfg(feature = "di")]
//!
//! Uses mock implementations of BlockReader, OperationProvider, and DocumentManager.

use anyhow::Result;
use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::{ContentType, Priority, Tags, TaskState, Timestamp};
use holon_api::Value;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as BlockOrderingResult;
use holon_orgmode::models::{
    OrgBlockExt, OrgDocumentExt, DEFAULT_ACTIVE_KEYWORDS, DEFAULT_DONE_KEYWORDS,
};
use holon_orgmode::org_renderer::OrgRenderer;
use holon_orgmode::org_sync_controller::OrgSyncController;
use holon_orgmode::parser::parse_org_file;
use holon_orgmode::traits::{BlockReader, DocumentManager};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
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
        let mut store = self.blocks.write().unwrap();
        // Find the document this block belongs to by checking if parent_id matches a doc key
        // or if any existing block in a doc has this block's parent_id.
        let doc_id = store
            .keys()
            .find(|k| {
                k.as_str() == block.parent_id.as_str()
                    || store[*k].iter().any(|b| b.id == block.parent_id)
            })
            .cloned()
            .unwrap_or_else(|| block.parent_id.to_string());
        store.entry(doc_id).or_default().push(block);
    }

    fn apply_update(&self, block: Block) {
        let mut store = self.blocks.write().unwrap();
        for blocks in store.values_mut() {
            if let Some(existing) = blocks.iter_mut().find(|b| b.id == block.id) {
                *existing = block;
                return;
            }
        }
    }

    fn apply_delete(&self, block_id: &str) {
        let mut store = self.blocks.write().unwrap();
        for blocks in store.values_mut() {
            blocks.retain(|b| b.id.as_str() != block_id);
        }
    }

    /// Create-or-update: replace the block in place if it already exists under
    /// any document, otherwise create it. Mirrors the SqlOnly `update_in_tree`
    /// upsert the production `BlockOrdering` performs (the org write seam picks
    /// create vs update by prior presence).
    fn apply_upsert(&self, block: Block) {
        let exists = {
            let store = self.blocks.read().unwrap();
            store.values().any(|v| v.iter().any(|b| b.id == block.id))
        };
        if exists {
            self.apply_update(block);
        } else {
            self.apply_create(block);
        }
    }
}

#[async_trait]
impl BlockReader for InMemoryBlockStore {
    async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>> {
        Ok(self.get_all_blocks(doc_id.as_str()))
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        Ok(self
            .blocks
            .read()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                (
                    EntityUri::parse(k).expect("stored key must be valid URI"),
                    v.clone(),
                )
            })
            .collect())
    }

    // find_foreign_blocks: uses default implementation from BlockReader trait
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
        content,
        content_type,
        source_language,
        source_name,
        created_at,
        updated_at,
        ..Block::default()
    };

    if let Some(seq) = params.get("sequence").and_then(|v| v.as_i64()) {
        block.set_sequence(seq);
    }
    if let Some(ts) = params.get("task_state").and_then(|v| v.as_string()) {
        block.set_task_state(Some(TaskState::from_keyword(ts)));
    }
    if let Some(p) = params.get("priority").and_then(|v| v.as_i64()) {
        if let Ok(priority) = Priority::from_int(p as i32) {
            block.set_priority(Some(priority));
        }
    }
    if let Some(t) = params.get("tags").and_then(|v| v.as_string()) {
        block.set_tags(Tags::from_csv(t));
    }
    if let Some(s) = params.get("scheduled").and_then(|v| v.as_string()) {
        if let Ok(ts) = Timestamp::parse(s) {
            block.set_scheduled(Some(ts));
        }
    }
    if let Some(d) = params.get("deadline").and_then(|v| v.as_string()) {
        if let Ok(ts) = Timestamp::parse(d) {
            block.set_deadline(Some(ts));
        }
    }
    if let Some(id_val) = params.get("ID").and_then(|v| v.as_string()) {
        block.set_property("ID", Value::String(id_val.to_string()));
    }
    if let Some(args_json) = params.get("source_header_args").and_then(|v| v.as_string()) {
        if let Ok(args) = serde_json::from_str::<HashMap<String, Value>>(args_json) {
            block.set_source_header_args(args);
        }
    }

    const STANDARD_KEYS: &[&str] = &[
        "id",
        "parent_id",
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
    documents: RwLock<Vec<Block>>,
}

impl MockDocumentManager {
    fn new() -> Self {
        let mut root = Block::new_text(EntityUri::no_parent(), EntityUri::no_parent(), "");
        root.set_page(true);
        Self {
            documents: RwLock::new(vec![root]),
        }
    }

    fn add_document(&self, doc: Block) {
        self.documents.write().unwrap().push(doc);
    }
}

#[async_trait]
impl DocumentManager for MockDocumentManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> Result<Option<Block>> {
        let docs = self.documents.read().unwrap();
        Ok(docs
            .iter()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }

    async fn create(&self, doc: Block) -> Result<Block> {
        self.documents.write().unwrap().push(doc.clone());
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> Result<Option<Block>> {
        let docs = self.documents.read().unwrap();
        Ok(docs.iter().find(|d| d.id == *id).cloned())
    }

    async fn update_metadata(&self, doc: &Block) -> Result<()> {
        // Simulate the production SQL round-trip: build_block_params packs
        // doc-level metadata (todo_keywords, etc.) into flat params; the
        // SqlOperationProvider partitions known columns vs `properties` JSON;
        // a subsequent read deserializes that row via Block::try_from. The
        // previous stub stored the `Block` struct directly, which preserved
        // every field by reference identity and masked any field that
        // build_block_params silently drops on the way to SQL. The new flow
        // round-trips through (params → SQL-shaped row → Block::try_from)
        // so a dropped param surfaces as a missing field on read-back.
        let params = holon_orgmode::block_params::build_block_params(doc, &doc.parent_id, &doc.id);
        let row = simulate_sql_round_trip(doc, params);
        let reconstructed = Block::try_from(row)
            .map_err(|e| anyhow::anyhow!("simulated SQL round-trip failed: {e}"))?;
        let mut docs = self.documents.write().unwrap();
        if let Some(existing) = docs.iter_mut().find(|d| d.id == doc.id) {
            *existing = reconstructed;
        }
        Ok(())
    }
}

/// Mirror `SqlOperationProvider::partition_params` + `prepare_update`:
/// flat params split into known columns (top-level) vs extras (merged
/// into the `properties` JSON column). Any param key that
/// `build_block_params` emits but is not a known column flows into the
/// JSON blob; on read, `Block::try_from` only sees those keys via
/// `block.properties`. This is the exact serialization seam where the
/// `inv-org-render-fixed-point` flake's missing `todo_keywords` lives.
fn simulate_sql_round_trip(doc: &Block, params: HashMap<String, Value>) -> HashMap<String, Value> {
    // Matches BLOCKS_KNOWN_COLUMNS in crates/holon/src/core/sql_operation_provider.rs.
    // Kept in sync by convention; if the production list changes, this list must
    // change too — the PBT is the canary.
    const BLOCKS_KNOWN_COLUMNS: &[&str] = &[
        "id",
        "parent_id",
        "depth",
        "sort_key",
        "content",
        "content_type",
        "source_language",
        "source_name",
        "properties",
        "marks",
        "collapsed",
        "completed",
        "block_type",
        "created_at",
        "updated_at",
        "_change_origin",
    ];
    const EDGE_FIELDS: &[&str] = &["tags", "requires"];

    let mut row: HashMap<String, Value> = HashMap::new();
    let mut extras: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    // Seed extras with the doc's existing properties (mirrors the
    // existing-properties merge in prepare_update). Without this, a single
    // update would clobber keys set by an earlier write.
    for (k, v) in &doc.properties {
        extras.insert(k.clone(), value_to_serde_json(v));
    }

    for (key, value) in params.into_iter() {
        if key == "properties" {
            // Real provider merges; here we just take the param JSON as the
            // existing-properties seed (params rarely contain a properties key).
            continue;
        }
        if key == POSITION_AFTER_BLOCK_ID_PARAM
            || key.starts_with("_routing_")
            || key.starts_with("_expected_")
        {
            continue;
        }
        if BLOCKS_KNOWN_COLUMNS.contains(&key.as_str()) || EDGE_FIELDS.contains(&key.as_str()) {
            row.insert(key, value);
        } else {
            extras.insert(key, value_to_serde_json(&value));
        }
    }

    // Emit the merged properties JSON as a string Value — Block::try_from
    // accepts both Value::String and Value::Json for the properties column.
    let props_json = serde_json::to_string(&extras).expect("properties must serialize");
    row.insert("properties".to_string(), Value::String(props_json));
    row
}

fn value_to_serde_json(v: &Value) -> serde_json::Value {
    match v {
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Integer(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Null => serde_json::Value::Null,
        Value::DateTime(s) => serde_json::Value::String(s.clone()),
        Value::Json(s) => serde_json::from_str(s).unwrap_or(serde_json::Value::Null),
        Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(value_to_serde_json).collect())
        }
        Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), value_to_serde_json(v)))
                .collect(),
        ),
    }
}

const POSITION_AFTER_BLOCK_ID_PARAM: &str = "_after_block_id";

// ============================================================================
// Stub BlockOrdering for tests
// ============================================================================

/// Records `place()` calls for assertions. All other methods panic because
/// the existing PBT paths don't exercise positional reads.
struct StubBlockOrdering {
    pub calls: Mutex<Vec<(EntityUri, String, Option<String>)>>,
    /// Writes route here — the controller's only block sink now that the
    /// command bus is gone. The test asserts against this same store.
    store: Arc<InMemoryBlockStore>,
}

impl StubBlockOrdering {
    fn new(store: Arc<InMemoryBlockStore>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            store,
        }
    }
}

#[async_trait]
impl BlockOrdering for StubBlockOrdering {
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &str,
        after_id: Option<&str>,
    ) -> BlockOrderingResult<()> {
        self.calls.lock().unwrap().push((
            uri.clone(),
            parent_id.to_string(),
            after_id.map(str::to_string),
        ));
        Ok(())
    }

    async fn new_child_anchor(&self, _: &str, _: Option<&str>) -> BlockOrderingResult<String> {
        unimplemented!("stub BlockOrdering: only place() is exercised by this test")
    }

    async fn prev_sibling(&self, _: &str) -> BlockOrderingResult<Option<String>> {
        // Return None so the misalignment check in on_file_changed treats every
        // block as first-child — safe for tests that don't assert order.
        Ok(None)
    }

    async fn next_sibling(&self, _: &str) -> BlockOrderingResult<Option<String>> {
        unimplemented!("stub BlockOrdering: only place() is exercised by this test")
    }

    async fn first_child(&self, _: &str) -> BlockOrderingResult<Option<String>> {
        unimplemented!("stub BlockOrdering: only place() is exercised by this test")
    }

    async fn last_child(&self, _: &str) -> BlockOrderingResult<Option<String>> {
        unimplemented!("stub BlockOrdering: only place() is exercised by this test")
    }

    async fn children(&self, _: &str) -> BlockOrderingResult<Vec<String>> {
        // Return empty so the misalignment check treats all blocks as misaligned
        // (they'll call place() once each). Tests that assert place() call count
        // should reflect this.
        Ok(vec![])
    }

    async fn update_in_tree(&self, params: HashMap<String, Value>) -> BlockOrderingResult<()> {
        self.store.apply_upsert(block_from_params(&params));
        Ok(())
    }

    async fn delete_in_tree(&self, params: HashMap<String, Value>) -> BlockOrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .expect("delete_in_tree: missing id");
        self.store.apply_delete(id);
        Ok(())
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
    /// Path segments from `root_dir` to the file (excluding the `.org` extension).
    /// Length 1 = flat `root_dir/<seg>.org`; length N = `root_dir/<seg0>/.../<segN-1>.org`.
    doc_path_segments: Vec<String>,
    doc_manager: Arc<MockDocumentManager>,
}

impl TestFixture {
    fn new(temp_dir: &std::path::Path) -> Self {
        Self::new_with(temp_dir, vec!["test".to_string()], true)
    }

    /// Generalized constructor. `path_segments` are the directory chain and
    /// filename stem; `pre_seed_doc` controls whether the leaf doc is inserted
    /// into `doc_manager` before any `on_file_changed` call (when false, the
    /// controller's new-doc creation path is exercised — including the
    /// directory chain → parent_id mapping).
    fn new_with(
        temp_dir: &std::path::Path,
        path_segments: Vec<String>,
        pre_seed_doc: bool,
    ) -> Self {
        assert!(
            !path_segments.is_empty(),
            "path_segments must contain at least one segment"
        );
        let store = Arc::new(InMemoryBlockStore::new());
        let doc_manager = Arc::new(MockDocumentManager::new());

        // Canonicalize so fixture-built paths match what OrgSyncController
        // stores internally (macOS: /var → /private/var symlink resolution).
        let root_dir = temp_dir
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.to_path_buf());
        let ordering = Arc::new(StubBlockOrdering::new(store.clone()));
        let controller = OrgSyncController::new(
            store.clone(),
            doc_manager.clone(),
            root_dir.clone(),
            ordering,
        );

        let doc_id = EntityUri::block_random();

        if pre_seed_doc {
            // Pre-seed the full directory chain — intermediate Pages with
            // fresh IDs and the leaf at `doc_id`. Reflects the production
            // invariant that a doc only enters `doc_manager` via a prior
            // ingest, which always honors the path → parent chain. Seeding
            // just the leaf with `parent=no_parent` would set up an
            // inconsistent fixture and mask whether on_file_changed
            // preserves the invariant.
            let mut current_parent = EntityUri::no_parent();
            let n = path_segments.len();
            for (idx, seg) in path_segments.iter().enumerate() {
                let id = if idx == n - 1 {
                    doc_id.clone()
                } else {
                    EntityUri::block_random()
                };
                let mut doc = Block::new_text(id.clone(), current_parent.clone(), seg.clone());
                doc.set_page(true);
                doc_manager.add_document(doc);
                current_parent = id;
            }
        }

        TestFixture {
            store,
            controller,
            root_dir,
            doc_id,
            doc_path_segments: path_segments,
            doc_manager,
        }
    }

    fn file_path(&self) -> PathBuf {
        let mut p = self.root_dir.clone();
        let n = self.doc_path_segments.len();
        for seg in &self.doc_path_segments[..n - 1] {
            p = p.join(seg);
        }
        p.join(format!("{}.org", self.doc_path_segments[n - 1]))
    }

    /// Create parent directories on disk so a write to `file_path()` succeeds
    /// even for nested-path fixtures.
    async fn ensure_parent_dirs(&self) {
        if let Some(parent) = self.file_path().parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
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

/// Filename- and Page-title-safe path segment: ASCII alphanumerics only, no
/// spaces or path separators. Used as both the directory name and the
/// matching Page title (which `path_to_name_chain` derives from the path).
fn path_segment() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9]{0,15}"
}

/// 1..=3 segment directory chain ending at the `.org` file stem. Segments
/// within a chain are made unique so a single `(parent, title)` lookup in
/// `MockDocumentManager` is unambiguous when walking the chain — the
/// production code handles dupes per-parent, but distinct names keep the
/// invariant check readable in shrunk counterexamples.
fn path_chain_strategy() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(path_segment(), 1..=3).prop_filter(
        "path segments within a chain must be distinct",
        |segs| {
            let unique: BTreeSet<&String> = segs.iter().collect();
            unique.len() == segs.len()
        },
    )
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
            let mut current = block.tags().to_vec();
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
            {
                let kw = removed?;
                let stars = "*".repeat(hl.level);
                let rest = after_stars[kw.len()..].trim_start();
                lines[hl.line_idx] = format!("{} {}", stars, rest);
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
            let mut b1 = Block::new_text(id1.clone(), doc_uri.clone(), "Alpha");
            b1.set_level(1);
            b1.set_sequence(0);
            b1.set_property("ID", Value::String(id1.id().to_string()));
            let mut b2 = Block::new_text(id2.clone(), doc_uri.clone(), "Beta");
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

            let mut bp = Block::new_text(p.clone(), doc_uri.clone(), "Parent");
            bp.set_level(1);
            bp.set_sequence(0);
            bp.set_property("ID", Value::String(p.id().to_string()));

            let mut bc1 = Block::new_text(c1.clone(), p.clone(), "Child one");
            bc1.set_level(2);
            bc1.set_sequence(1);
            bc1.set_task_state(Some(TaskState::active("TODO")));
            bc1.set_property("ID", Value::String(c1.id().to_string()));

            let mut bc2 = Block::new_text(c2.clone(), p.clone(), "Child two");
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

            let mut b0 = Block::new_text(ids[0].clone(), doc_uri.clone(), "Inbox");
            b0.set_level(1);
            b0.set_sequence(0);
            b0.set_property("ID", Value::String(ids[0].id().to_string()));

            let mut b1 = Block::new_text(ids[1].clone(), doc_uri.clone(), "Projects");
            b1.set_level(1);
            b1.set_sequence(1);
            b1.set_task_state(Some(TaskState::active("DOING")));
            b1.set_scheduled(Timestamp::parse("<2024-06-20 Thu 14:00>").ok());
            b1.set_property("ID", Value::String(ids[1].id().to_string()));

            let mut b2 = Block::new_text(ids[2].clone(), doc_uri.clone(), "Archive");
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
    let org_text = OrgRenderer::render_entitys(blocks, &file_path, doc_id);
    let parse_result = parse_org_file(&file_path, &org_text, &EntityUri::no_parent(), root_dir)
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
            fixture.controller.initialize().await.expect("initialize must succeed");

            let initial_org =
                OrgRenderer::render_entitys(&baseline, &fixture.file_path(), &fixture.doc_id);
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
                &EntityUri::no_parent(),
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
        path_segments in path_chain_strategy(),
        inject_id_directive in any::<bool>(),
        pre_seed_doc in any::<bool>(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let temp_dir = tempfile::tempdir().unwrap();
            let mut fixture =
                TestFixture::new_with(temp_dir.path(), path_segments.clone(), pre_seed_doc);
            fixture.ensure_parent_dirs().await;

            // Generate + stabilize baseline
            let raw_blocks = generate_baseline_blocks(&fixture.doc_id, variant);
            let baseline = stabilize_blocks(&raw_blocks, &fixture.doc_id, &fixture.root_dir);
            prop_assume!(!baseline.is_empty());

            // Seed store + initialize + write + establish last_projection
            fixture.seed_blocks(&baseline);
            fixture.controller.initialize().await.expect("initialize must succeed");

            // OrgRenderer doesn't emit `#+ID:` at the file head, so we splice it
            // in when the generator picks `inject_id_directive`. Combined with
            // `pre_seed_doc == false`, this exercises the on_file_changed
            // branch that creates a new Page from a bare `#+ID:` — the one
            // that must honor the directory chain for `parent_id`.
            let mut initial_org =
                OrgRenderer::render_entitys(&baseline, &fixture.file_path(), &fixture.doc_id);
            if inject_id_directive {
                initial_org = format!("#+ID: {}\n{}", fixture.doc_id.id(), initial_org);
            }
            tokio::fs::write(&fixture.file_path(), &initial_org)
                .await
                .unwrap();

            fixture
                .controller
                .on_file_changed(&fixture.file_path())
                .await
                .unwrap();

            // After first ingest, every directory segment in the path chain
            // must exist as a Page block with the expected parent. This holds
            // regardless of which branch (`Some(Some)`, `Some(None)`, `None`)
            // the controller took — the directory layout dictates the Page
            // tree, not the presence of `#+ID:` or a pre-seed.
            let chain: Vec<&str> = path_segments.iter().map(String::as_str).collect();
            let mut expected_parent = EntityUri::no_parent();
            for (depth, seg) in chain.iter().enumerate() {
                let found = fixture
                    .doc_manager
                    .find_by_parent_and_name(&expected_parent, seg)
                    .await
                    .unwrap();
                prop_assert!(
                    found.is_some(),
                    "Page chain broken at depth {}: no Page named {:?} under parent {:?} \
                     (segments={:?}, inject_id_directive={}, pre_seed_doc={})",
                    depth, seg, expected_parent, path_segments, inject_id_directive, pre_seed_doc
                );
                let doc = found.unwrap();
                prop_assert!(
                    doc.is_page(),
                    "doc {:?} at depth {} must carry the Page tag",
                    seg, depth
                );
                prop_assert_eq!(
                    &doc.parent_id, &expected_parent,
                    "parent_id mismatch for {:?} at depth {} \
                     (segments={:?}, inject_id_directive={}, pre_seed_doc={})",
                    seg, depth, path_segments, inject_id_directive, pre_seed_doc
                );
                expected_parent = doc.id.clone();
            }

            // Skip mutation phase if the baseline has no headlines (every
            // mutation variant in `text_mutation_strategy` targets a headline
            // via `headlines[idx % headlines.len()]`, which panics on empty).
            // The page-chain assertions above have already exercised the
            // ingest path, which is what these new generator dimensions test.
            if find_headlines(&initial_org).is_empty() {
                return Ok::<(), TestCaseError>(());
            }

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
                &EntityUri::no_parent(),
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

// ============================================================================
// PBT: test_sync_todo_keywords_round_trip
// ============================================================================
//
// Targets the `inv-org-render-fixed-point` flake surfaced by the wide
// `general_e2e_pbt`: an org file with a `#+TODO:` header is ingested by
// `on_file_changed`, but `block_raw.properties.todo_keywords` ends up
// missing on the doc row — so a subsequent `OrgRenderer::render_document`
// from SQL drops the `#+TODO:` line, the renderer's output differs from
// the disk file, and the next `re_render_all_tracked` pass rewrites the
// file (echo-suppression loop risk).
//
// Detection path here:
//   1. Generate a varying TodoKeywordSet.
//   2. Build a minimal org file: `#+ID: <doc_id>\n#+TODO: <active> | <done>\n…`.
//   3. Run `on_file_changed`. Per the controller, this calls
//      `doc_manager.update_metadata(doc_with_kws)`.
//   4. The hardened mock routes the metadata write through
//      `build_block_params` → simulate_sql_round_trip → `Block::try_from`,
//      mirroring the real SQL serialization seam. Any field dropped by
//      that seam surfaces as a missing key on read-back.
//   5. Read the doc back from the mock store via `get_by_id` and assert
//      `todo_keywords()` returns the same set we generated.

fn arb_keyword() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "TODO",
        "DOING",
        "NEXT",
        "WAITING",
        "DONE",
        "CANCELLED",
        "CLOSED",
    ])
    .prop_map(String::from)
}

fn arb_keyword_set() -> impl Strategy<Value = Vec<TaskState>> {
    (
        prop::collection::vec(arb_keyword(), 1..=3),
        prop::collection::vec(arb_keyword(), 1..=2),
    )
        .prop_map(|(active, done)| {
            let mut states = Vec::new();
            for k in active {
                states.push(TaskState::active(&k));
            }
            for k in done {
                states.push(TaskState::done(&k));
            }
            states
        })
}

fn format_todo_line(states: &[TaskState]) -> String {
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
    let mut out = String::from("#+TODO:");
    if !active.is_empty() {
        out.push_str(&format!(" {}", active.join(" ")));
    }
    if !done.is_empty() {
        out.push_str(&format!(" | {}", done.join(" ")));
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 50,
        ..ProptestConfig::default()
    })]

    /// `#+TODO:` keyword set survives `on_file_changed` → metadata store
    /// round-trip. Fails if `build_block_params` (or any intermediate
    /// in the simulated SQL serialization) drops `todo_keywords` on the
    /// way to `block_raw.properties`.
    #[test]
    fn test_sync_todo_keywords_round_trip(kws in arb_keyword_set()) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let temp_dir = tempfile::tempdir().unwrap();
            // pre_seed_doc=true: the leaf doc is already in MockDocumentManager,
            // so the doc-metadata path is exercised without the StubBlockOrdering
            // misalignment-check timing out on a separate ordering bug
            // (pre-existing infra issue in the ordering_replay tests).
            let mut fixture = TestFixture::new_with(
                temp_dir.path(),
                vec!["todo_round_trip".to_string()],
                true,
            );
            fixture.ensure_parent_dirs().await;
            fixture.controller.initialize().await.expect("initialize must succeed");

            let todo_line = format_todo_line(&kws);
            // Header-only org file: no headlines. The doc-metadata branch in
            // `on_file_changed` (compare parsed_kws vs existing_kws → call
            // update_metadata) fires regardless, and we skip the ordering
            // replay entirely.
            let org = format!("#+ID: {}\n{}\n", fixture.doc_id.id(), todo_line);
            tokio::fs::write(&fixture.file_path(), &org).await.unwrap();

            fixture
                .controller
                .on_file_changed(&fixture.file_path())
                .await
                .unwrap();

            let stored_doc = fixture
                .doc_manager
                .get_by_id(&fixture.doc_id)
                .await
                .unwrap()
                .expect("doc must exist in MockDocumentManager after on_file_changed");

            let actual_kws = stored_doc.todo_keywords();
            prop_assert!(
                actual_kws.is_some(),
                "stored doc lost todo_keywords after on_file_changed → metadata round-trip.\n\
                 Generated #+TODO line: {todo_line}\n\
                 Stored doc.properties: {:?}",
                stored_doc.properties,
            );
            let actual = actual_kws.unwrap();
            // Compare as canonical-JSON to ignore Vec ordering jitter from
            // parser internals (parser collects active then done; we generate
            // the same shape so direct equality on (active_set, done_set) is
            // also valid).
            let to_pair = |v: &[TaskState]| {
                let mut a: Vec<String> =
                    v.iter().filter(|s| s.is_active()).map(|s| s.keyword.clone()).collect();
                let mut d: Vec<String> =
                    v.iter().filter(|s| s.is_done()).map(|s| s.keyword.clone()).collect();
                a.sort();
                d.sort();
                (a, d)
            };
            prop_assert_eq!(
                to_pair(&actual),
                to_pair(&kws),
                "stored doc todo_keywords differ from generated set",
            );

            Ok::<(), TestCaseError>(())
        })?;
    }
}

// ============================================================================
// find_foreign_blocks regression tests
// ============================================================================

#[cfg(test)]
mod find_foreign_blocks_tests {
    use super::*;

    fn make_block(id: &str, parent_id: &str) -> Block {
        Block::new_text(EntityUri::from_raw(id), EntityUri::from_raw(parent_id), "")
    }

    #[tokio::test]
    async fn nested_blocks_not_flagged_as_foreign() {
        // Simulate ClaudeCode.org's structure:
        //   doc:claude (document, parent=sentinel:no_parent)
        //     block:root (parent=doc:claude)
        //       block:child-a (parent=block:root)
        //       block:child-b (parent=block:root)
        //         block:grandchild (parent=block:child-b)
        let store = Arc::new(InMemoryBlockStore::new());

        let mut doc = make_block("block:doc-claude", "sentinel:no_parent");
        doc.content = "ClaudeCode".to_string();
        doc.set_page(true);
        store.seed_blocks(
            "block:doc-claude",
            vec![
                make_block("block:root", "block:doc-claude"),
                make_block("block:child-a", "block:root"),
                make_block("block:child-b", "block:root"),
                make_block("block:grandchild", "block:child-b"),
            ],
        );

        let doc_uri = EntityUri::from_raw("block:doc-claude");

        // All these blocks belong to doc:claude — none should be flagged as foreign
        let query_ids: Vec<EntityUri> = vec![
            EntityUri::from_raw("block:root"),
            EntityUri::from_raw("block:child-a"),
            EntityUri::from_raw("block:child-b"),
            EntityUri::from_raw("block:grandchild"),
        ];

        let conflicts = store
            .find_foreign_blocks(&query_ids, &doc_uri)
            .await
            .unwrap();
        assert!(
            conflicts.is_empty(),
            "Nested blocks should not be flagged as foreign, got: {:?}",
            conflicts,
        );
    }

    #[tokio::test]
    async fn blocks_from_other_document_flagged_correctly() {
        let store = Arc::new(InMemoryBlockStore::new());

        // Doc A owns block:x
        store.seed_blocks("block:doc-a", vec![make_block("block:x", "block:doc-a")]);
        // Doc B is separate
        store.seed_blocks("block:doc-b", vec![make_block("block:y", "block:doc-b")]);

        let doc_b_uri = EntityUri::from_raw("block:doc-b");

        // Ask if block:x is foreign to doc-b — it should be
        let query_ids = vec![EntityUri::from_raw("block:x")];
        let conflicts = store
            .find_foreign_blocks(&query_ids, &doc_b_uri)
            .await
            .unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, EntityUri::from_raw("block:x"));
    }

    #[tokio::test]
    async fn deeply_nested_foreign_block_detected() {
        let store = Arc::new(InMemoryBlockStore::new());

        // Doc A has block:deep nested 3 levels deep
        store.seed_blocks(
            "block:doc-a",
            vec![
                make_block("block:l1", "block:doc-a"),
                make_block("block:l2", "block:l1"),
                make_block("block:deep", "block:l2"),
            ],
        );
        store.seed_blocks(
            "block:doc-b",
            vec![make_block("block:other", "block:doc-b")],
        );

        let doc_b_uri = EntityUri::from_raw("block:doc-b");

        // block:deep belongs to doc-a, should be foreign to doc-b
        let query_ids = vec![EntityUri::from_raw("block:deep")];
        let conflicts = store
            .find_foreign_blocks(&query_ids, &doc_b_uri)
            .await
            .unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].1, EntityUri::from_raw("block:doc-a"));
    }

    #[tokio::test]
    async fn empty_query_returns_empty() {
        let store = Arc::new(InMemoryBlockStore::new());
        store.seed_blocks("block:doc-a", vec![make_block("block:x", "block:doc-a")]);
        let conflicts = store
            .find_foreign_blocks(&[], &EntityUri::from_raw("block:doc-a"))
            .await
            .unwrap();
        assert!(conflicts.is_empty());
    }
}

// ============================================================================
// Test 7: OrgSyncController::on_file_changed ordering replay
// ============================================================================

#[cfg(test)]
mod ordering_replay_tests {
    use super::*;

    /// A configurable stub that returns specific live children for given parents,
    /// and records every `place()` call for assertion.
    struct ConfigurableOrderingStub {
        /// Maps parent_id → ordered list of child ids representing the LIVE order.
        live_order: std::collections::HashMap<String, Vec<String>>,
        pub calls: Mutex<Vec<(EntityUri, String, Option<String>)>>,
        /// Block sink — the controller's only write target now the command bus
        /// is gone; tests assert against this same store.
        store: Arc<InMemoryBlockStore>,
    }

    impl ConfigurableOrderingStub {
        fn new(
            live_order: std::collections::HashMap<String, Vec<String>>,
            store: Arc<InMemoryBlockStore>,
        ) -> Self {
            Self {
                live_order,
                calls: Mutex::new(Vec::new()),
                store,
            }
        }
    }

    #[async_trait]
    impl BlockOrdering for ConfigurableOrderingStub {
        async fn place(
            &self,
            uri: &EntityUri,
            parent_id: &str,
            after_id: Option<&str>,
        ) -> BlockOrderingResult<()> {
            self.calls.lock().unwrap().push((
                uri.clone(),
                parent_id.to_string(),
                after_id.map(str::to_string),
            ));
            Ok(())
        }

        async fn new_child_anchor(&self, _: &str, _: Option<&str>) -> BlockOrderingResult<String> {
            unimplemented!("ConfigurableOrderingStub: only place() and children() are used")
        }

        async fn prev_sibling(&self, _: &str) -> BlockOrderingResult<Option<String>> {
            Ok(None)
        }

        async fn next_sibling(&self, _: &str) -> BlockOrderingResult<Option<String>> {
            unimplemented!("ConfigurableOrderingStub: only place() and children() are used")
        }

        async fn first_child(&self, _: &str) -> BlockOrderingResult<Option<String>> {
            unimplemented!("ConfigurableOrderingStub: only place() and children() are used")
        }

        async fn last_child(&self, _: &str) -> BlockOrderingResult<Option<String>> {
            unimplemented!("ConfigurableOrderingStub: only place() and children() are used")
        }

        async fn children(&self, parent_id: &str) -> BlockOrderingResult<Vec<String>> {
            Ok(self.live_order.get(parent_id).cloned().unwrap_or_default())
        }

        async fn update_in_tree(&self, params: HashMap<String, Value>) -> BlockOrderingResult<()> {
            self.store.apply_upsert(block_from_params(&params));
            Ok(())
        }

        async fn delete_in_tree(&self, params: HashMap<String, Value>) -> BlockOrderingResult<()> {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .expect("delete_in_tree: missing id");
            self.store.apply_delete(id);
            Ok(())
        }
    }

    fn build_controller_with_live_order(
        temp_dir: &std::path::Path,
        live_order: std::collections::HashMap<String, Vec<String>>,
    ) -> (
        Arc<InMemoryBlockStore>,
        Arc<MockDocumentManager>,
        OrgSyncController,
        Arc<ConfigurableOrderingStub>,
        EntityUri,
        PathBuf,
    ) {
        build_controller_with_live_order_and_doc_id(temp_dir, live_order, EntityUri::block_random())
    }

    fn build_controller_with_live_order_and_doc_id(
        temp_dir: &std::path::Path,
        live_order: std::collections::HashMap<String, Vec<String>>,
        doc_id: EntityUri,
    ) -> (
        Arc<InMemoryBlockStore>,
        Arc<MockDocumentManager>,
        OrgSyncController,
        Arc<ConfigurableOrderingStub>,
        EntityUri,
        PathBuf,
    ) {
        let store = Arc::new(InMemoryBlockStore::new());
        let doc_manager = Arc::new(MockDocumentManager::new());

        // The ordering stub is the controller's only block sink (no command
        // bus). Share the store so the dispatched create/update/delete intents
        // land where the test asserts.
        let ordering = Arc::new(ConfigurableOrderingStub::new(live_order, store.clone()));

        let root_dir = temp_dir.to_path_buf();
        let controller = OrgSyncController::new(
            store.clone(),
            doc_manager.clone(),
            root_dir.clone(),
            ordering.clone(),
        );

        let doc_name = "order-test".to_string();

        let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), doc_name.clone());
        doc.set_page(true);
        doc_manager.add_document(doc);

        let file_path = root_dir.join(format!("{doc_name}.org"));
        (store, doc_manager, controller, ordering, doc_id, file_path)
    }

    /// Test 7a: file order matches live order → place() is never called.
    #[tokio::test]
    async fn ordering_replay_skips_place_when_order_matches() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Disk org file has two children: alpha then beta.
        let _org_content = "\
* alpha
* beta
";

        // Live order also has alpha before beta.
        // The controller uses the block's bare id (UUID) for the children list.
        // Use a single-block org file with a stable :ID: property so the parser
        // reuses the same block UUID across both on_file_changed calls.
        // Both controllers use the same doc_id so the live_order key matches.
        let stable_block_uuid = uuid::Uuid::new_v4().to_string();
        let single_block_org =
            format!("* only block\n:PROPERTIES:\n:ID: {stable_block_uuid}\n:END:\n");
        let doc_id = EntityUri::block_random();

        // First pass: empty live_order → place() will be called (one block).
        let (store, _doc_mgr, mut controller, _ordering_first, _, file_path) =
            build_controller_with_live_order_and_doc_id(
                temp_dir.path(),
                std::collections::HashMap::new(),
                doc_id.clone(),
            );

        controller.initialize().await.expect("initialize");
        tokio::fs::write(&file_path, &single_block_org)
            .await
            .unwrap();
        // Canonicalize after writing (macOS: /var → /private/var symlink).
        let canonical_path = file_path.canonicalize().expect("canonicalize file_path");
        controller
            .on_file_changed(&canonical_path)
            .await
            .expect("first on_file_changed");

        let stored_blocks = store.get_all_blocks(doc_id.as_str());
        assert_eq!(
            stored_blocks.len(),
            1,
            "should have one block after first parse"
        );
        let block_id = stored_blocks[0].id.id().to_string();

        // Second pass: live_order already contains the parsed block → disk order
        // matches live order → place() must NOT be called.
        let mut live_order = std::collections::HashMap::new();
        live_order.insert(doc_id.id().to_string(), vec![block_id.clone()]);
        let (store2, _doc_mgr2, mut controller2, ordering_second, _, file_path2) =
            build_controller_with_live_order_and_doc_id(
                temp_dir.path(),
                live_order,
                doc_id.clone(),
            );
        // Pre-seed the store so the parse sees an existing block → UPDATE path.
        store2.seed_blocks(doc_id.as_str(), vec![stored_blocks[0].clone()]);

        controller2.initialize().await.expect("initialize2");
        tokio::fs::write(&file_path2, &single_block_org)
            .await
            .unwrap();
        let canonical_path2 = file_path2.canonicalize().expect("canonicalize file_path2");
        controller2
            .on_file_changed(&canonical_path2)
            .await
            .expect("second on_file_changed");

        let place_calls = ordering_second.calls.lock().unwrap().clone();
        assert!(
            place_calls.is_empty(),
            "place() must not be called when disk order matches live order; got {place_calls:?}"
        );
    }

    /// Test 7b: file reorders one block → exactly one place() call.
    #[tokio::test]
    async fn ordering_replay_calls_place_for_misaligned_block() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Org file has block B then block A (B is first on disk).
        // Live order has A then B (A is first in live DB).
        // The replay should call place() exactly once — for B (first on disk but
        // second in live) — to move it before A.
        //
        // Since we don't control what IDs the parser assigns, we use a
        // single-block file where the live order lists a DIFFERENT block as the
        // only child. From the parser's perspective the block is NOT in the live
        // children list → current_idx = None → misaligned → place() called.

        let single_block_org = "\
* the block
";

        let dummy_other_id = "some-other-block-id".to_string();
        let mut live_order = std::collections::HashMap::new();
        // Live order for the doc: a different block is listed → our parsed block
        // is not in the list → current_idx = None → misaligned.
        live_order.insert(
            EntityUri::no_parent().id().to_string(),
            vec![dummy_other_id],
        );

        // We need the doc_id for the live_order key. Build a controller first
        // to learn the doc_id, then rebuild with the right live_order.
        let (_store, _doc_mgr, _ctrl, _ordering_probe, doc_id, _file_path) =
            build_controller_with_live_order(temp_dir.path(), std::collections::HashMap::new());

        // Build the real controller with a live_order that doesn't include our block.
        let dummy_other_id2 = "some-other-block-id".to_string();
        let mut live_order2 = std::collections::HashMap::new();
        live_order2.insert(doc_id.id().to_string(), vec![dummy_other_id2]);
        let (_store2, _doc_mgr2, mut controller, ordering, _doc_id2, file_path2) =
            build_controller_with_live_order(temp_dir.path(), live_order2);

        controller.initialize().await.expect("initialize");
        tokio::fs::write(&file_path2, single_block_org)
            .await
            .unwrap();
        // Canonicalize after writing (macOS: /var → /private/var symlink).
        let canonical_path2 = file_path2.canonicalize().expect("canonicalize file_path2");
        controller
            .on_file_changed(&canonical_path2)
            .await
            .expect("on_file_changed");

        let place_calls = ordering.calls.lock().unwrap().clone();
        assert_eq!(
            place_calls.len(),
            1,
            "exactly one place() call expected for the misaligned block; got {place_calls:?}"
        );
    }
}
