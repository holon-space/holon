//! Phase 2: FileSyncController-level mutation PBTs.
//!
//! Two property-based tests that exercise the full sync loop:
//! - `test_sync_block_change_to_file`: in-memory mutation → on_block_changed →
//!   file → parse → assert
//! - `test_sync_file_change_to_blocks`: org text mutation → on_file_changed →
//!   store → assert
//!
//! Requires the `di` feature (file_sync_controller is only compiled with it).

#![cfg(feature = "di")]
//!
//! Uses mock implementations of BlockReader, OperationProvider, and
//! DocumentManager.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::types::ContentType;
use holon_api::types::Priority;
use holon_api::types::Tags;
use holon_api::types::TaskState;
use holon_api::types::Timestamp;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as BlockOrderingResult;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_filesystem::FileSyncController;
use holon_orgmode::file_sync_controller::new_org_sync_controller;
use holon_orgmode::models::DEFAULT_ACTIVE_KEYWORDS;
use holon_orgmode::models::DEFAULT_DONE_KEYWORDS;
use holon_orgmode::models::OrgBlockExt;
use holon_orgmode::models::OrgDocumentExt;
use holon_orgmode::org_renderer::OrgRenderer;
use holon_orgmode::parser::parse_org_file;
use proptest::prelude::*;
use uuid::Uuid;

// ============================================================================
// Mock infrastructure
// ============================================================================

struct InMemoryBlockStore {
    blocks: RwLock<HashMap<String, Vec<Block>>>,
    /// Count of `delete_in_tree`/`apply_delete` removals — lets a test assert a
    /// cascade-delete did (or did NOT) run, independent of any later re-ingest
    /// that could restore a block's id from the org file bytes.
    delete_count: std::sync::atomic::AtomicUsize,
}

impl InMemoryBlockStore {
    fn new() -> Self {
        Self {
            blocks: RwLock::new(HashMap::new()),
            delete_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn delete_count(&self) -> usize {
        self.delete_count.load(std::sync::atomic::Ordering::SeqCst)
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

    /// Direct children of `parent_id` — like the real
    /// `SqlBlockOperations::children`, which filters the authority by
    /// `parent_id` regardless of which document the block belongs to.
    /// `on_file_changed`'s post-create wait loop polls this for NESTED
    /// parents too, so a doc-bucket read is not enough.
    fn children_of(&self, parent_id: &EntityUri) -> Vec<EntityUri> {
        self.blocks
            .read()
            .unwrap()
            .values()
            .flat_map(|v| v.iter())
            .filter(|b| b.parent_id == *parent_id)
            .map(|b| b.id.clone())
            .collect()
    }

    fn apply_create(&self, block: Block) {
        let mut store = self.blocks.write().unwrap();
        // Find the document this block belongs to by checking if parent_id matches a
        // doc key or if any existing block in a doc has this block's parent_id.
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
        let Some(cur_key) = store
            .iter()
            .find(|(_, v)| v.iter().any(|b| b.id == block.id))
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        // Prod SQL derives doc membership from the parent chain (recursive
        // CTE), so an update whose parent now lives under a different doc key
        // must follow it — an in-place replace would strand the block under a
        // stale doc bucket. Same target-key derivation as `apply_create`.
        let target_key = store
            .keys()
            .find(|k| {
                k.as_str() == block.parent_id.as_str()
                    || store[*k].iter().any(|b| b.id == block.parent_id)
            })
            .cloned()
            .unwrap_or_else(|| block.parent_id.to_string());
        if cur_key == target_key {
            // Same bucket: replace in place, preserving sibling order.
            let blocks = store.get_mut(&cur_key).expect("cur_key just found");
            let existing = blocks
                .iter_mut()
                .find(|b| b.id == block.id)
                .expect("block just found under cur_key");
            *existing = block;
        } else {
            store
                .get_mut(&cur_key)
                .expect("cur_key just found")
                .retain(|b| b.id != block.id);
            store.entry(target_key).or_default().push(block);
        }
    }

    fn apply_delete(&self, block_id: &str) {
        let mut store = self.blocks.write().unwrap();
        let mut removed = 0usize;
        for blocks in store.values_mut() {
            let before = blocks.len();
            blocks.retain(|b| b.id.as_str() != block_id);
            removed += before - blocks.len();
        }
        self.delete_count
            .fetch_add(removed, std::sync::atomic::Ordering::SeqCst);
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

    /// Upsert from an org write-seam params map, mirroring SQL UPDATE
    /// semantics: an edge-field param the controller stripped as unchanged
    /// (`strip_unchanged_edge_fields`) leaves the existing junction values
    /// untouched — a wholesale struct replace would silently clear them.
    fn upsert_from_params(&self, params: &holon_api::StorageEntity) {
        let mut block = block_from_params(params);
        let existing = {
            let store = self.blocks.read().unwrap();
            store
                .values()
                .flat_map(|v| v.iter())
                .find(|b| b.id == block.id)
                .cloned()
        };
        if let Some(existing) = existing {
            if params.get("tags").is_none() {
                block.set_tags(existing.tags.clone());
            }
            if params.get("requires").is_none() {
                block.requires = existing.requires.clone();
            }
            if params.get("advice_suppressed").is_none() {
                block.advice_suppressed = existing.advice_suppressed.clone();
            }
        }
        self.apply_upsert(block);
    }
}

#[async_trait]
impl BlockReader for InMemoryBlockStore {
    async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>> {
        Ok(self.get_all_blocks(doc_id.as_str()))
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> Result<Option<Block>> {
        let store = self.blocks.read().unwrap();
        Ok(store
            .values()
            .flat_map(|v| v.iter())
            .find(|b| b.id == *id)
            .cloned())
    }

    /// No junction to resolve against — this double stores marks as given.
    async fn resolve_link_marks(&self, _: &mut [Block]) -> anyhow::Result<()> {
        Ok(())
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

fn block_from_params(params: &holon_api::StorageEntity) -> Block {
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
    // `build_block_params` emits edge fields as typed `Value::Array` params
    // (routed to junctions by the SQL provider; the legacy CSV shape is gone).
    match params.get("tags") {
        Some(Value::Array(arr)) => {
            let tags: Vec<String> = arr
                .iter()
                .map(|v| {
                    v.as_string()
                        .map(str::to_string)
                        .expect("tags array element must be a string")
                })
                .collect();
            block.set_tags(Tags::from(tags));
        }
        Some(other) => panic!("tags param must be an Array, got {other:?}"),
        None => {}
    }
    let uri_array = |key: &str| -> Option<Vec<EntityUri>> {
        match params.get(key) {
            Some(Value::Array(arr)) => Some(
                arr.iter()
                    .map(|v| {
                        // ALLOW(entity_uri_from_raw): edge params carry full URIs
                        EntityUri::from_raw(
                            v.as_string().expect("edge array element must be a string"),
                        )
                    })
                    .collect(),
            ),
            Some(other) => panic!("{key} param must be an Array, got {other:?}"),
            None => None,
        }
    };
    if let Some(r) = uri_array("requires") {
        block.requires = r;
    }
    if let Some(a) = uri_array("advice_suppressed") {
        block.advice_suppressed = a;
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
        "requires",
        "advice_suppressed",
        "scheduled",
        "deadline",
        "ID",
        // Positional intent, not a field: prod strips it before the row write
        // (`update_in_tree` removes POSITION_AFTER_BLOCK_ID_PARAM); it must
        // never land in `block.properties`.
        "after_block_id",
    ];
    for (k, v) in params {
        if !STANDARD_KEYS.contains(&k.as_ref()) && !k.starts_with("_routing_") {
            if let Some(s) = v.as_string() {
                block.set_property(k.as_ref(), Value::String(s.to_string()));
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
fn simulate_sql_round_trip(
    doc: &Block,
    params: holon_api::StorageEntity,
) -> holon_api::StorageEntity {
    // Matches BLOCKS_KNOWN_COLUMNS in
    // crates/holon/src/core/sql_operation_provider.rs. Kept in sync by
    // convention; if the production list changes, this list must change too —
    // the PBT is the canary.
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
        "widget_only",
        "completed",
        "block_type",
        "created_at",
        "updated_at",
        "_change_origin",
    ];
    // Matches BlockSchemaModule::edge_fields (holon-turso/src/schema_modules.rs):
    // tags, requires, advice_suppressed. The `block` matview hydrates all three
    // as JSON arrays, and `Block::try_from` requires all three columns.
    const EDGE_FIELDS: &[&str] = &["tags", "requires", "advice_suppressed"];

    let mut row = holon_api::StorageEntity::new();
    let mut extras: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    // Seed extras with the doc's existing properties (mirrors the
    // existing-properties merge in prepare_update). Without this, a single
    // update would clobber keys set by an earlier write.
    for (k, v) in &doc.properties {
        extras.insert(k.clone(), value_to_serde_json(v));
    }

    for (key, value) in params.into_iter() {
        if key.as_ref() == "properties" {
            // Real provider merges; here we just take the param JSON as the
            // existing-properties seed (params rarely contain a properties key).
            continue;
        }
        if key.as_ref() == POSITION_AFTER_BLOCK_ID_PARAM
            || key.starts_with("_routing_")
            || key.starts_with("_expected_")
        {
            continue;
        }
        if BLOCKS_KNOWN_COLUMNS.contains(&key.as_ref()) || EDGE_FIELDS.contains(&key.as_ref()) {
            row.insert(key, value);
        } else {
            extras.insert(key.to_string(), value_to_serde_json(&value));
        }
    }

    // Emit the merged properties JSON as a string Value — Block::try_from
    // accepts both Value::String and Value::Json for the properties column.
    let props_json = serde_json::to_string(&extras).expect("properties must serialize");
    row.insert("properties".into(), Value::String(props_json));
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

// Mirror of the prod constant (holon-api/src/entity.rs) — a stale
// "_after_block_id" copy here let the positional param leak into simulated rows
// undetected.
const POSITION_AFTER_BLOCK_ID_PARAM: &str = holon_api::entity::POSITION_AFTER_BLOCK_ID_PARAM;

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
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> BlockOrderingResult<()> {
        self.calls.lock().unwrap().push((
            uri.clone(),
            parent_id.as_str().to_string(),
            after_id.map(|u| u.as_str().to_string()),
        ));
        Ok(())
    }

    async fn prev_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        // Return None so the misalignment check in on_file_changed treats every
        // block as first-child — safe for tests that don't assert order.
        Ok(None)
    }

    async fn next_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        unimplemented!("stub BlockOrdering: only place() is exercised by this test")
    }

    async fn first_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        unimplemented!("stub BlockOrdering: only place() is exercised by this test")
    }

    async fn last_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        unimplemented!("stub BlockOrdering: only place() is exercised by this test")
    }

    async fn children(&self, parent_id: &EntityUri) -> BlockOrderingResult<Vec<EntityUri>> {
        // Authority-backed like the real `SqlBlockOperations::children`:
        // `on_file_changed`'s post-create wait loop polls this until every
        // created block is visible, then hands the total order to `place_all`.
        Ok(self.store.children_of(parent_id))
    }

    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> BlockOrderingResult<()> {
        self.store.upsert_from_params(&params);
        Ok(())
    }

    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> BlockOrderingResult<()> {
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
    controller: FileSyncController,
    root_dir: PathBuf,
    doc_id: EntityUri,
    /// Path segments from `root_dir` to the file (excluding the `.org`
    /// extension). Length 1 = flat `root_dir/<seg>.org`; length N =
    /// `root_dir/<seg0>/.../<segN-1>.org`.
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

        // Canonicalize so fixture-built paths match what FileSyncController
        // stores internally (macOS: /var → /private/var symlink resolution).
        let root_dir = temp_dir
            .canonicalize()
            .unwrap_or_else(|_| temp_dir.to_path_buf());
        let ordering = Arc::new(StubBlockOrdering::new(store.clone()));
        let controller = new_org_sync_controller(
            store.clone(),
            doc_manager.clone(),
            root_dir.clone(),
            ordering,
            Arc::new(holon_filesystem::RealFileSystem),
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

/// Filename- and Page-title-safe path segment. Used as both the directory name
/// and the matching Page title (which `path_to_name_chain` derives from the
/// path).
///
/// Spaces and non-ASCII letters are generated deliberately: real vaults have
/// folders like `Agentic DPL`, and a space is what turned a path-shaped id into
/// an invalid RFC 3986 URI (the boot panic that motivated deleting the
/// `Directory` entity). The alphabet excludes `/` and guarantees a
/// non-whitespace first and last char, so segments stay filename-safe and
/// round-trip through `path_to_name_chain` as Page titles unchanged.
fn path_segment() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z][a-zA-Z0-9 ]{0,14}[a-zA-Z0-9]",
        "[a-zA-Zäöüéñ][a-zA-Z0-9äöüéñ ]{0,14}[a-zA-Z0-9äöüéñ]",
    ]
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
            // Find the end of this headline's section (next headline at same or higher
            // level)
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
///
/// The parse carries an injected `#+ID: <doc_id>` directive: without it the
/// parser derives a path-based `file:test.org` document identity and parents
/// every top-level headline to THAT, so the returned baseline would no longer
/// be renderable against `doc_id` (the WP-F dangling-parent projection
/// assertion in `OrgRenderer::render_entitys` fails loud on the mismatch).
/// Prod vault files carry `#+ID:` — the writeback path even force-persists it
/// (`needs_id_writeback`) — so the directive is the faithful fixture shape.
fn stabilize_blocks(
    blocks: &[Block],
    doc_id: &EntityUri,
    root_dir: &std::path::Path,
) -> Vec<Block> {
    let file_path = root_dir.join("test.org");
    let org_text = OrgRenderer::render_entitys(blocks, &file_path, doc_id);
    let org_text = format!("#+ID: {}\n{}", doc_id.id(), org_text);
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
        failure_persistence: None,
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
            let delta = holon_filesystem::BlockDelta::Upsert(mutated[block_idx].clone());
            fixture
                .controller
                .on_block_changed(&fixture.doc_id, &delta)
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
        failure_persistence: None,
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

            // The store should match what the final file on disk parses to.
            // Read under the RESOLVED leaf doc id (the page-chain walk above
            // ends with `expected_parent` = leaf): when `pre_seed_doc == false`
            // and the file carries no `#+ID:`, the controller mints its own
            // document id — `fixture.doc_id` never appears in the store.
            let stored = fixture.store.get_all_blocks(expected_parent.as_str());
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
//   2. Build a minimal org file: `#+ID: <doc_id>\n#+TODO: <active> |
//      <done>\n…`.
//   3. Run `on_file_changed`. Per the controller, this calls
//      `doc_manager.update_metadata(doc_with_kws)`.
//   4. The hardened mock routes the metadata write through `build_block_params`
//      → simulate_sql_round_trip → `Block::try_from`, mirroring the real SQL
//      serialization seam. Any field dropped by that seam surfaces as a missing
//      key on read-back.
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
        failure_persistence: None,
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
// Test 7: FileSyncController::on_file_changed ordering replay
// ============================================================================

#[cfg(test)]
mod ordering_replay_tests {
    use super::*;

    /// Ordering stub whose `children()` reads the shared block store — like the
    /// real `SqlBlockOperations::children`, which reads the authority directly
    /// so `on_file_changed`'s post-create wait loop sees freshly-created blocks
    /// (a static children list makes that loop time out and bail loud). Records
    /// every `place_all()` call for assertion. SqlOnly (no upstream
    /// consolidator) → the controller routes order intent through `place_all`.
    struct RecordingOrderingStub {
        /// (parent, ordered_ids) per `place_all` call.
        pub place_all_calls: Mutex<Vec<(EntityUri, Vec<EntityUri>)>>,
        /// Block sink — the controller's only write target now the command bus
        /// is gone; tests assert against this same store.
        store: Arc<InMemoryBlockStore>,
    }

    impl RecordingOrderingStub {
        fn new(store: Arc<InMemoryBlockStore>) -> Self {
            Self {
                place_all_calls: Mutex::new(Vec::new()),
                store,
            }
        }
    }

    #[async_trait]
    impl BlockOrdering for RecordingOrderingStub {
        async fn place(
            &self,
            _: &EntityUri,
            _: &EntityUri,
            _: Option<&EntityUri>,
        ) -> BlockOrderingResult<()> {
            unimplemented!("RecordingOrderingStub: SqlOnly ingest routes order through place_all")
        }

        async fn place_all(
            &self,
            parent_id: &EntityUri,
            ordered_ids: &[EntityUri],
        ) -> BlockOrderingResult<()> {
            self.place_all_calls
                .lock()
                .unwrap()
                .push((parent_id.clone(), ordered_ids.to_vec()));
            Ok(())
        }

        async fn prev_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }

        async fn next_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            unimplemented!("RecordingOrderingStub: only place_all() and children() are used")
        }

        async fn first_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            unimplemented!("RecordingOrderingStub: only place_all() and children() are used")
        }

        async fn last_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            unimplemented!("RecordingOrderingStub: only place_all() and children() are used")
        }

        async fn children(&self, parent_id: &EntityUri) -> BlockOrderingResult<Vec<EntityUri>> {
            Ok(self.store.children_of(parent_id))
        }

        async fn update_in_tree(
            &self,
            params: holon_api::StorageEntity,
        ) -> BlockOrderingResult<()> {
            self.store.upsert_from_params(&params);
            Ok(())
        }

        async fn delete_in_tree(
            &self,
            params: holon_api::StorageEntity,
        ) -> BlockOrderingResult<()> {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .expect("delete_in_tree: missing id");
            self.store.apply_delete(id);
            Ok(())
        }
    }

    fn build_recording_controller(
        temp_dir: &std::path::Path,
        doc_id: EntityUri,
    ) -> (
        Arc<InMemoryBlockStore>,
        FileSyncController,
        Arc<RecordingOrderingStub>,
        PathBuf,
    ) {
        let store = Arc::new(InMemoryBlockStore::new());
        let doc_manager = Arc::new(MockDocumentManager::new());

        // The ordering stub is the controller's only block sink (no command
        // bus). Share the store so the dispatched create/update/delete intents
        // land where the test asserts.
        let ordering = Arc::new(RecordingOrderingStub::new(store.clone()));

        let root_dir = temp_dir.to_path_buf();
        let controller = new_org_sync_controller(
            store.clone(),
            doc_manager.clone(),
            root_dir.clone(),
            ordering.clone(),
            Arc::new(holon_filesystem::RealFileSystem),
        );

        let doc_name = "order-test".to_string();

        let mut doc = Block::new_text(doc_id.clone(), EntityUri::no_parent(), doc_name.clone());
        doc.set_page(true);
        doc_manager.add_document(doc);

        let file_path = root_dir.join(format!("{doc_name}.org"));
        (store, controller, ordering, file_path)
    }

    /// Test 7a: SqlOnly ingest hands the order owner the file's TOTAL sibling
    /// order via `place_all` on EVERY `on_file_changed` — including an
    /// update-only pass whose disk order already matches the live order.
    ///
    /// This replaced the old per-block skip-if-aligned replay: incremental
    /// `place` can't converge a full reorder against a mutating store (the
    /// `inv-live-children-match-ref` divergence), so the SQL order owner now
    /// mints one fresh, gap-free key sequence per parent over its text
    /// children in document order (total by construction, idempotent when
    /// already aligned). See the `Consolidator::Store` branch of
    /// `on_file_changed` and `BlockOrdering::place_all`.
    #[tokio::test]
    async fn ordering_replay_hands_owner_total_order_even_when_aligned() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Single-block org file with a stable :ID: property so the parser
        // reuses the same block UUID across both on_file_changed calls.
        let stable_block_uuid = uuid::Uuid::new_v4().to_string();
        let single_block_org =
            format!("* only block\n:PROPERTIES:\n:ID: {stable_block_uuid}\n:END:\n");
        let doc_id = EntityUri::block_random();

        // First pass: CREATE path.
        let (store, mut controller, ordering_first, file_path) =
            build_recording_controller(temp_dir.path(), doc_id.clone());

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
        let block_uri = stored_blocks[0].id.clone();
        let first_calls = ordering_first.place_all_calls.lock().unwrap().clone();
        assert_eq!(
            first_calls,
            vec![(doc_id.clone(), vec![block_uri.clone()])],
            "create pass: order owner must receive the file's total order"
        );

        // Second pass: UPDATE path (store pre-seeded, no creates), disk order
        // trivially matches live order — place_all is STILL called with the
        // total order (idempotent re-key), by design.
        let (store2, mut controller2, ordering_second, file_path2) =
            build_recording_controller(temp_dir.path(), doc_id.clone());
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

        let second_calls = ordering_second.place_all_calls.lock().unwrap().clone();
        assert_eq!(
            second_calls,
            vec![(doc_id.clone(), vec![block_uri])],
            "update pass: total-order re-key must run even when already aligned"
        );
    }

    /// Test 7b: a freshly-created block reaches the order owner in document
    /// order — one `place_all` for its parent containing it.
    #[tokio::test]
    async fn ordering_replay_calls_place_for_misaligned_block() {
        let temp_dir = tempfile::tempdir().unwrap();

        let single_block_org = "\
* the block
";
        let doc_id = EntityUri::block_random();
        let (store, mut controller, ordering, file_path) =
            build_recording_controller(temp_dir.path(), doc_id.clone());

        controller.initialize().await.expect("initialize");
        tokio::fs::write(&file_path, single_block_org)
            .await
            .unwrap();
        // Canonicalize after writing (macOS: /var → /private/var symlink).
        let canonical_path = file_path.canonicalize().expect("canonicalize file_path");
        controller
            .on_file_changed(&canonical_path)
            .await
            .expect("on_file_changed");

        let stored_blocks = store.get_all_blocks(doc_id.as_str());
        assert_eq!(stored_blocks.len(), 1, "one block ingested");
        let place_all_calls = ordering.place_all_calls.lock().unwrap().clone();
        assert_eq!(
            place_all_calls,
            vec![(doc_id, vec![stored_blocks[0].id.clone()])],
            "exactly one total-order hand-off expected for the new block"
        );
    }
}

// ============================================================================
// Test 8: cold-boot fast-path must be Loro-aware (WP-D / I2)
//
// Regression guard for the 2026-07-06 reset hole: on cold boot the fast path
// skipped org ingest whenever the on-disk hash equalled the SQL-persisted
// `file.content_hash` — a SQL-ONLY check. After a reset (fresh empty `.loro`
// but SQL kept the matching hash) it wrongly decided "already ingested" and
// skipped, leaving the Loro tree empty and SQL/Loro silently diverged.
//
// The fix requires the content present in EVERY active store: skip only when
// the SQL hash matches AND (when Loro is active) the doc's root block is in the
// Loro tree. These tests inject the Loro-presence signal through
// `BlockOrdering::in_tree` (the exact seam the fix consults) and assert the
// skip decision flips on it.
// ============================================================================
#[cfg(test)]
mod fast_path_loro_presence_tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering as AtomicOrdering;

    use super::*;

    /// BlockReader that round-trips `file.content_hash` across a simulated
    /// reboot: `persist_file_hash` captures into a shared map,
    /// `load_file_hashes` serves it back — so a second controller's
    /// `initialize()` arms the fast path with the hash the first boot
    /// stamped (no hand-computed hash, no coupling to the renderer version
    /// or consolidator tag).
    struct HashCapturingReader {
        inner: Arc<InMemoryBlockStore>,
        hashes: Arc<Mutex<HashMap<String, String>>>,
    }

    #[async_trait]
    impl BlockReader for HashCapturingReader {
        async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>> {
            self.inner.get_blocks(doc_id).await
        }
        async fn get_block_authoritative(&self, id: &EntityUri) -> Result<Option<Block>> {
            self.inner.get_block_authoritative(id).await
        }
        /// No junction to resolve against — this double stores marks as given.
        async fn resolve_link_marks(&self, _: &mut [Block]) -> Result<()> {
            Ok(())
        }

        async fn iter_documents_with_blocks(&self) -> Result<Vec<(EntityUri, Vec<Block>)>> {
            self.inner.iter_documents_with_blocks().await
        }
        async fn load_file_hashes(&self) -> Result<Vec<(EntityUri, String)>> {
            Ok(self
                .hashes
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (EntityUri::parse(k).expect("stored file uri"), v.clone()))
                .collect())
        }
        async fn persist_file_hash(&self, uri: &EntityUri, hash: &str) -> Result<()> {
            self.hashes
                .lock()
                .unwrap()
                .insert(uri.as_str().to_string(), hash.to_string());
            Ok(())
        }
    }

    /// Ordering stub whose `in_tree` answer is injectable, and which counts
    /// every mutating call. A non-zero count means `on_file_changed` did NOT
    /// take the fast-path skip — it ran the ingest. `create_in_tree` keeps the
    /// default (`Ok(false)` → SqlOnly) so content-block creates route through
    /// `update_in_tree` (no downstream projection needed).
    struct PresenceOrdering {
        store: Arc<InMemoryBlockStore>,
        root_in_tree: AtomicBool,
        mutations: AtomicUsize,
        /// parent uri → child ids in insertion order. Populated by
        /// `update_in_tree` so the ingest's post-create `children()` wait loop
        /// (which polls until every just-created block is visible to the
        /// ordering layer) observes the blocks this ingest wrote.
        child_order: Mutex<HashMap<String, Vec<String>>>,
    }

    impl PresenceOrdering {
        fn new(store: Arc<InMemoryBlockStore>, root_in_tree: bool) -> Self {
            Self {
                store,
                root_in_tree: AtomicBool::new(root_in_tree),
                mutations: AtomicUsize::new(0),
                child_order: Mutex::new(HashMap::new()),
            }
        }
        fn bump(&self) {
            self.mutations.fetch_add(1, AtomicOrdering::SeqCst);
        }
        fn mutation_count(&self) -> usize {
            self.mutations.load(AtomicOrdering::SeqCst)
        }
    }

    #[async_trait]
    impl BlockOrdering for PresenceOrdering {
        async fn place(
            &self,
            _: &EntityUri,
            _: &EntityUri,
            _: Option<&EntityUri>,
        ) -> BlockOrderingResult<()> {
            self.bump();
            Ok(())
        }
        async fn prev_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }
        async fn next_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }
        async fn first_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }
        async fn last_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }
        async fn children(&self, parent_id: &EntityUri) -> BlockOrderingResult<Vec<EntityUri>> {
            Ok(self
                .child_order
                .lock()
                .unwrap()
                .get(parent_id.as_str())
                .map(|ids| ids.iter().map(|s| EntityUri::from_raw(s)).collect())
                .unwrap_or_default())
        }
        async fn in_tree(&self, _: &EntityUri) -> BlockOrderingResult<Option<bool>> {
            Ok(Some(self.root_in_tree.load(AtomicOrdering::SeqCst)))
        }
        async fn update_in_tree(
            &self,
            params: holon_api::StorageEntity,
        ) -> BlockOrderingResult<()> {
            self.bump();
            let block = block_from_params(&params);
            {
                let mut order = self.child_order.lock().unwrap();
                let siblings = order
                    .entry(block.parent_id.as_str().to_string())
                    .or_default();
                let child = block.id.as_str().to_string();
                if !siblings.contains(&child) {
                    siblings.push(child);
                }
            }
            self.store.apply_upsert(block);
            Ok(())
        }
        async fn delete_in_tree(
            &self,
            params: holon_api::StorageEntity,
        ) -> BlockOrderingResult<()> {
            self.bump();
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .expect("delete_in_tree: missing id");
            self.store.apply_delete(id);
            Ok(())
        }
    }

    /// Boot 1: fresh vault, empty hash cache → full ingest, stamps the hash.
    /// Returns the shared hash map, the shared doc manager, and the canonical
    /// on-disk (now renderer-canonical) file path.
    async fn boot_once_and_stamp_hash(
        root_dir: &std::path::Path,
    ) -> (
        Arc<Mutex<HashMap<String, String>>>,
        Arc<MockDocumentManager>,
        PathBuf,
        EntityUri,
    ) {
        let hashes = Arc::new(Mutex::new(HashMap::new()));
        let store = Arc::new(InMemoryBlockStore::new());
        let reader = Arc::new(HashCapturingReader {
            inner: store.clone(),
            hashes: hashes.clone(),
        });
        let doc_manager = Arc::new(MockDocumentManager::new());
        let ordering = Arc::new(PresenceOrdering::new(store.clone(), true));

        let doc_id = EntityUri::block_random();
        let mut doc = Block::new_text(
            doc_id.clone(),
            EntityUri::no_parent(),
            "reset-doc".to_string(),
        );
        doc.set_page(true);
        doc_manager.add_document(doc);

        let mut controller = new_org_sync_controller(
            reader,
            doc_manager.clone(),
            root_dir.to_path_buf(),
            ordering.clone(),
            Arc::new(holon_filesystem::RealFileSystem),
        );

        let stable_block_uuid = uuid::Uuid::new_v4().to_string();
        let file_path = root_dir.join("reset-doc.org");
        let initial_org = format!("* only block\n:PROPERTIES:\n:ID: {stable_block_uuid}\n:END:\n");
        tokio::fs::write(&file_path, &initial_org).await.unwrap();
        let canonical = file_path.canonicalize().expect("canonicalize");

        controller.initialize().await.expect("initialize boot 1");
        controller
            .on_file_changed(&canonical)
            .await
            .expect("boot 1 on_file_changed");

        assert!(
            !hashes.lock().unwrap().is_empty(),
            "boot 1 must stamp file.content_hash so the fast path can arm on boot 2"
        );
        (hashes, doc_manager, canonical, doc_id)
    }

    /// Boot 2: reuse the stamped hash so the fast path is armed, then vary only
    /// the Loro presence signal. Returns how many block mutations the ingest
    /// performed (0 = fast path skipped).
    async fn boot_again_with_loro_presence(
        root_dir: &std::path::Path,
        hashes: Arc<Mutex<HashMap<String, String>>>,
        doc_manager: Arc<MockDocumentManager>,
        canonical: &std::path::Path,
        loro_has_root: bool,
    ) -> usize {
        let store = Arc::new(InMemoryBlockStore::new());
        let reader = Arc::new(HashCapturingReader {
            inner: store.clone(),
            hashes,
        });
        let ordering = Arc::new(PresenceOrdering::new(store.clone(), loro_has_root));
        let mut controller = new_org_sync_controller(
            reader,
            doc_manager,
            root_dir.to_path_buf(),
            ordering.clone(),
            Arc::new(holon_filesystem::RealFileSystem),
        );

        controller.initialize().await.expect("initialize boot 2");
        controller
            .on_file_changed(canonical)
            .await
            .expect("boot 2 on_file_changed");
        ordering.mutation_count()
    }

    /// The bug: SQL hash matches but the Loro tree is empty (reset) → the fast
    /// path MUST NOT skip; ingest must run to repopulate Loro.
    #[tokio::test]
    async fn fast_path_reingests_when_loro_tree_empty() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (hashes, doc_manager, canonical, _doc_id) =
            boot_once_and_stamp_hash(temp_dir.path()).await;

        let mutations = boot_again_with_loro_presence(
            temp_dir.path(),
            hashes,
            doc_manager,
            &canonical,
            /* loro_has_root = */ false,
        )
        .await;

        assert!(
            mutations > 0,
            "fast path skipped ingest despite an empty Loro tree — SQL and Loro would stay \
             silently diverged (the WP-D / I2 regression)"
        );
    }

    /// Control: SQL hash matches AND the Loro tree holds the doc root → the
    /// fast path is still allowed to skip (no cold-boot perf regression).
    #[tokio::test]
    async fn fast_path_still_skips_when_content_present_in_all_stores() {
        let temp_dir = tempfile::tempdir().unwrap();
        let (hashes, doc_manager, canonical, _doc_id) =
            boot_once_and_stamp_hash(temp_dir.path()).await;

        let mutations = boot_again_with_loro_presence(
            temp_dir.path(),
            hashes,
            doc_manager,
            &canonical,
            /* loro_has_root = */ true,
        )
        .await;

        assert_eq!(
            mutations, 0,
            "fast path should skip when content is present in every active store"
        );
    }
}

// ============================================================================
// Test 8: initial-scan batched feed barrier (boot ingest latency, Options 0+1)
// ============================================================================

#[cfg(test)]
mod initial_scan_batched_barrier_tests {
    use std::path::Path;

    use super::*;

    /// Ordering stub whose `children()` reads the shared block store (like the
    /// real `SqlBlockOperations::children`, which reads the authority directly
    /// so the scan's `place` loop sees freshly-created blocks) — so wait B
    /// resolves immediately instead of hanging. SqlOnly by default (no
    /// upstream consolidator), so creates flow through `update_in_tree`
    /// into the store.
    struct ScanOrderingStub {
        store: Arc<InMemoryBlockStore>,
    }

    #[async_trait]
    impl BlockOrdering for ScanOrderingStub {
        async fn place(
            &self,
            _: &EntityUri,
            _: &EntityUri,
            _: Option<&EntityUri>,
        ) -> BlockOrderingResult<()> {
            Ok(())
        }

        async fn place_all(&self, _: &EntityUri, _: &[EntityUri]) -> BlockOrderingResult<()> {
            Ok(())
        }

        async fn prev_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }
        async fn next_sibling(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }
        async fn first_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }
        async fn last_child(&self, _: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
            Ok(None)
        }

        async fn children(&self, parent_id: &EntityUri) -> BlockOrderingResult<Vec<EntityUri>> {
            Ok(self.store.children_of(parent_id))
        }

        async fn update_in_tree(
            &self,
            params: holon_api::StorageEntity,
        ) -> BlockOrderingResult<()> {
            self.store.upsert_from_params(&params);
            Ok(())
        }

        async fn delete_in_tree(
            &self,
            params: holon_api::StorageEntity,
        ) -> BlockOrderingResult<()> {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .expect("delete_in_tree: missing id");
            self.store.apply_delete(id);
            Ok(())
        }
    }

    /// A BlockReader that delegates reads to the shared store but whose feed
    /// NEVER converges — models a stalled projection/CDC. Drives the
    /// `finish_initial_scan` fail-loud path. `blocks_in_feed_count` reports 0
    /// (no progress ever) so the progress-grounded barrier declares a stall
    /// after ONE no-progress window.
    struct StallingReader {
        store: Arc<InMemoryBlockStore>,
    }

    #[async_trait]
    impl BlockReader for StallingReader {
        async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>> {
            self.store.get_blocks(doc_id).await
        }
        async fn get_block_authoritative(&self, id: &EntityUri) -> Result<Option<Block>> {
            self.store.get_block_authoritative(id).await
        }
        /// No junction to resolve against — this double stores marks as given.
        async fn resolve_link_marks(&self, _: &mut [Block]) -> Result<()> {
            Ok(())
        }

        async fn iter_documents_with_blocks(&self) -> Result<Vec<(EntityUri, Vec<Block>)>> {
            self.store.iter_documents_with_blocks().await
        }
        async fn wait_for_blocks_in_feed(&self, _: &[String], _: u64) -> bool {
            false // feed never converges
        }
        async fn blocks_in_feed_count(&self, _: &[String]) -> usize {
            0 // and never makes progress
        }
    }

    /// A BlockReader whose feed converges SLOWLY: every
    /// `wait_for_blocks_in_feed` slice "times out" (returns false) but
    /// releases another chunk of ids, so `blocks_in_feed_count` keeps
    /// rising. Models a healthy projection under cold-boot load — exactly
    /// the condition where the old fixed wall-clock budget expired early
    /// (real vault 2026-07-12). The progress-grounded barrier must keep
    /// waiting to completion; the pre-fix single fixed-budget wait fails on
    /// the first slice. Deterministic: progress is per-CALL, not
    /// per-elapsed-time, so the test is not tuned to timing.
    struct SlowFeedReader {
        store: Arc<InMemoryBlockStore>,
        released: std::sync::atomic::AtomicUsize,
        chunk: usize,
    }

    #[async_trait]
    impl BlockReader for SlowFeedReader {
        async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>> {
            self.store.get_blocks(doc_id).await
        }
        async fn get_block_authoritative(&self, id: &EntityUri) -> Result<Option<Block>> {
            self.store.get_block_authoritative(id).await
        }
        /// No junction to resolve against — this double stores marks as given.
        async fn resolve_link_marks(&self, _: &mut [Block]) -> Result<()> {
            Ok(())
        }

        async fn iter_documents_with_blocks(&self) -> Result<Vec<(EntityUri, Vec<Block>)>> {
            self.store.iter_documents_with_blocks().await
        }
        async fn wait_for_blocks_in_feed(&self, ids: &[String], _: u64) -> bool {
            let now = self
                .released
                .fetch_add(self.chunk, std::sync::atomic::Ordering::SeqCst)
                + self.chunk;
            now >= ids.len()
        }
        async fn blocks_in_feed_count(&self, ids: &[String]) -> usize {
            self.released
                .load(std::sync::atomic::Ordering::SeqCst)
                .min(ids.len())
        }
    }

    fn build_scan_controller(
        root: &Path,
        reader: Arc<dyn BlockReader>,
        store: Arc<InMemoryBlockStore>,
    ) -> FileSyncController {
        build_scan_controller_with_docs(root, reader, store, Arc::new(MockDocumentManager::new()))
    }

    fn build_scan_controller_with_docs(
        root: &Path,
        reader: Arc<dyn BlockReader>,
        store: Arc<InMemoryBlockStore>,
        doc_manager: Arc<MockDocumentManager>,
    ) -> FileSyncController {
        let ordering = Arc::new(ScanOrderingStub { store });
        new_org_sync_controller(
            reader,
            doc_manager,
            root.to_path_buf(),
            ordering,
            Arc::new(holon_filesystem::RealFileSystem),
        )
    }

    fn org_file(idx: usize, blocks: usize) -> String {
        let mut s = format!("#+ID: file-{idx}\n\n");
        for j in 0..blocks {
            s.push_str(&format!(
                "* Block {idx}-{j}\n:PROPERTIES:\n:ID: p{idx}_{j}\n:END:\nBody {idx}-{j}.\n\n"
            ));
        }
        s
    }

    /// Batched barrier ingests every file's blocks correctly and `place_all`
    /// order is preserved — the end-of-scan convergence wait succeeds and the
    /// scan flag is cleared. Proves the batched path does not drop blocks.
    #[tokio::test]
    async fn initial_scan_batched_barrier_ingests_all_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryBlockStore::new());
        let mut controller = build_scan_controller(temp_dir.path(), store.clone(), store.clone());
        controller.initialize().await.expect("initialize");

        const FILES: usize = 12;
        const BLOCKS: usize = 5;
        let mut paths = Vec::new();
        for i in 0..FILES {
            let p = temp_dir.path().join(format!("page-{i}.org"));
            tokio::fs::write(&p, org_file(i, BLOCKS)).await.unwrap();
            paths.push(p.canonicalize().expect("canonicalize"));
        }

        // Drive the initial scan exactly as `run_file_sync_controller` does.
        controller.begin_initial_scan();
        assert!(controller.in_initial_scan(), "flag on during scan");
        for p in &paths {
            controller
                .on_file_changed(p)
                .await
                .unwrap_or_else(|e| panic!("on_file_changed {}: {e:#}", p.display()));
        }
        controller
            .finish_initial_scan(30_000)
            .await
            .expect("finish_initial_scan should converge (in-memory feed = true)");

        // Steady-state guard: flag cleared after finish.
        assert!(
            !controller.in_initial_scan(),
            "scan flag must be off after finish_initial_scan"
        );

        // Every file's every block landed in block_raw (the store).
        for i in 0..FILES {
            let doc_blocks = store.get_all_blocks(&format!("block:file-{i}"));
            let ids: BTreeSet<String> = doc_blocks.iter().map(|b| b.id.id().to_string()).collect();
            for j in 0..BLOCKS {
                assert!(
                    ids.contains(&format!("p{i}_{j}")),
                    "file {i}: block p{i}_{j} missing after batched scan; got {ids:?}"
                );
            }
        }
    }

    /// A stalled feed makes the SINGLE end-of-scan convergence wait fail loud —
    /// never a silent continue. The per-file ingest still succeeds (block_raw
    /// is synchronous; the count-check passes), so the failure surfaces
    /// only at `finish_initial_scan`, which is where
    /// `run_file_sync_controller` routes it into `signal_error`.
    #[tokio::test]
    async fn initial_scan_feed_stall_fails_loud() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryBlockStore::new());
        let reader = Arc::new(StallingReader {
            store: store.clone(),
        });
        let mut controller = build_scan_controller(temp_dir.path(), reader, store.clone());
        controller.initialize().await.expect("initialize");

        let p = temp_dir.path().join("page-0.org");
        tokio::fs::write(&p, org_file(0, 3)).await.unwrap();
        let p = p.canonicalize().expect("canonicalize");

        controller.begin_initial_scan();
        controller
            .on_file_changed(&p)
            .await
            .expect("per-file ingest succeeds (block_raw synchronous)");

        let err = controller
            .finish_initial_scan(200)
            .await
            .expect_err("finish_initial_scan must bail loud when the feed never converges");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("did not converge"),
            "expected a fail-loud convergence error, got: {msg}"
        );
        // Even on failure the flag is cleared (take()), so no leak into runtime.
        assert!(!controller.in_initial_scan());
    }

    /// The scan flag never leaks into steady-state: it is on between begin and
    /// finish and off afterwards, even with an empty vault.
    #[tokio::test]
    async fn scan_flag_off_after_finish() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryBlockStore::new());
        let mut controller = build_scan_controller(temp_dir.path(), store.clone(), store.clone());
        controller.initialize().await.expect("initialize");

        assert!(!controller.in_initial_scan(), "off before begin");
        controller.begin_initial_scan();
        assert!(controller.in_initial_scan(), "on after begin");
        controller
            .finish_initial_scan(30_000)
            .await
            .expect("empty scan converges trivially");
        assert!(!controller.in_initial_scan(), "off after finish");
    }

    /// Real-vault cold-boot escape (2026-07-12, Martin's vault): a
    /// folder-companion file (`Journals.org`, `#+ID: journals`) inlines OTHER
    /// page-files' doc-roots as headings TOGETHER WITH their child blocks. The
    /// pre-fix ingest (a) re-parented/updated those children into the
    /// companion's document — stealing them from the owning page-file — and
    /// (b) counted the whole inlined subtree in the post-ingest `get_blocks`
    /// gate, which the Page-boundary doc walk can structurally NEVER return
    /// ("expected 22 blocks, cache has 5"), so the file failed ingest forever,
    /// was quarantined from write-back, and every retry flooded the log.
    ///
    /// With file-authority extended to the whole inlined subtree: ingest
    /// succeeds, the owner's blocks are untouched (stale companion copies do
    /// NOT clobber them), the companion doc holds exactly its own blocks, and
    /// the file on disk is left byte-identical (write-back deferred, no
    /// de-inline from this path).
    #[tokio::test]
    async fn companion_inlining_foreign_page_subtree_ingests_clean() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryBlockStore::new());
        let doc_manager = Arc::new(MockDocumentManager::new());

        // Pre-existing page-file document `block:d1` (as the scan would have
        // created from `Journals/2026-07-10.org`) with two children.
        let d1 = EntityUri::from_raw("block:d1");
        let mut page = Block::new_text(d1.clone(), EntityUri::no_parent(), "2026-07-10");
        page.set_page(true);
        doc_manager.add_document(page);
        let c1 = Block::new_text(EntityUri::from_raw("block:c1"), d1.clone(), "entry one");
        let c2 = Block::new_text(EntityUri::from_raw("block:c2"), d1.clone(), "entry two");
        store.seed_blocks("block:d1", vec![c1, c2]);

        let mut controller = build_scan_controller_with_docs(
            temp_dir.path(),
            store.clone(),
            store.clone(),
            doc_manager,
        );
        controller.initialize().await.expect("initialize");

        // Companion file: two own blocks + the foreign page root inlined as a
        // heading with STALE copies of its children.
        let companion = "#+ID: journals\n\n* Own block A\n:PROPERTIES:\n:ID: own-a\n:END:\n\n* \
                         2026-07-10\n:PROPERTIES:\n:ID: d1\n:END:\n\n** entry one \
                         STALE\n:PROPERTIES:\n:ID: c1\n:END:\n\n** entry two \
                         STALE\n:PROPERTIES:\n:ID: c2\n:END:\n\n* Own block B\n:PROPERTIES:\n:ID: \
                         own-b\n:END:\n";
        let p = temp_dir.path().join("Journals.org");
        tokio::fs::write(&p, companion).await.unwrap();
        let p = p.canonicalize().expect("canonicalize");

        controller.begin_initial_scan();
        controller
            .on_file_changed(&p)
            .await
            .expect("companion ingest must succeed — the inlined foreign subtree is skipped");
        controller
            .finish_initial_scan(30_000)
            .await
            .expect("scan converges");

        // Owner authoritative: the page-file's blocks are untouched — the
        // companion's stale copies did not clobber content or re-parent.
        let owner_blocks = store.get_all_blocks("block:d1");
        let contents: BTreeSet<String> = owner_blocks.iter().map(|b| b.content.clone()).collect();
        assert!(
            contents.contains("entry one") && contents.contains("entry two"),
            "owner page-file blocks must survive un-clobbered, got {contents:?}"
        );

        // Companion doc holds exactly its own blocks.
        let journal_blocks = store.get_all_blocks("block:journals");
        let ids: BTreeSet<String> = journal_blocks
            .iter()
            .map(|b| b.id.id().to_string())
            .collect();
        assert!(
            ids.contains("own-a") && ids.contains("own-b"),
            "companion's own blocks must land, got {ids:?}"
        );
        assert!(
            !ids.contains("c1") && !ids.contains("c2") && !ids.contains("d1"),
            "foreign page subtree must NOT be stolen into the companion doc, got {ids:?}"
        );

        // Disk byte-identical: this ingest never de-inlines the user's file.
        let disk_after = tokio::fs::read_to_string(&p).await.unwrap();
        assert_eq!(disk_after, companion, "companion file must be left as-is");

        // No quarantine: a subsequent external change ingests fine too.
        controller
            .on_file_changed(&p)
            .await
            .expect("re-ingest of the unchanged companion stays clean");
    }

    /// Scaled cold boot: hundreds of files (thousands of blocks) over a feed
    /// that is HEALTHY but SLOW — every wait slice "times out" while ids keep
    /// landing. The old fixed wall-clock budget (`wait_for_blocks_in_feed(ids,
    /// budget)` once) fails on the first slice; the progress-grounded barrier
    /// must ride the progress to completion. Progress is per-call, not
    /// per-elapsed-time, so the test is deterministic and the fix cannot be
    /// "tuned" to its timing.
    #[tokio::test]
    async fn scaled_cold_boot_slow_feed_converges_via_progress() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(InMemoryBlockStore::new());
        let reader = Arc::new(SlowFeedReader {
            store: store.clone(),
            released: std::sync::atomic::AtomicUsize::new(0),
            chunk: 37, // ids released per wait slice; many slices needed
        });
        let mut controller = build_scan_controller(temp_dir.path(), reader, store.clone());
        controller.initialize().await.expect("initialize");

        const FILES: usize = 250;
        const BLOCKS: usize = 5;
        let mut paths = Vec::new();
        for i in 0..FILES {
            let p = temp_dir.path().join(format!("page-{i}.org"));
            tokio::fs::write(&p, org_file(i, BLOCKS)).await.unwrap();
            paths.push(p.canonicalize().expect("canonicalize"));
        }

        controller.begin_initial_scan();
        for p in &paths {
            controller
                .on_file_changed(p)
                .await
                .unwrap_or_else(|e| panic!("on_file_changed {}: {e:#}", p.display()));
        }
        // Small stall window: irrelevant to a feed that keeps progressing.
        controller
            .finish_initial_scan(50)
            .await
            .expect("a slow-but-progressing feed must converge, not time out");

        for i in 0..FILES {
            let doc_blocks = store.get_all_blocks(&format!("block:file-{i}"));
            assert_eq!(
                doc_blocks.len(),
                BLOCKS,
                "file {i}: all blocks must land at scale"
            );
        }
    }
}

// ============================================================================
// Atomic file-rename port (Rename lane, 2026-07-27)
// ============================================================================
//
// A user `mv A.org B.org` inside the vault. The pre-atomic pipeline saw this as
// Remove(A) + Create(B): the Remove half (or a poll tick that finds A gone)
// routes to `on_file_deleted`, whose D3 guard CANNOT tell a rename from a
// delete when the page title has not yet followed the file (its authoritative
// title-chain still points at A), so it cascade-deletes the very doc the move
// re-homed. The atomic `on_file_renamed(from, to)` carries BOTH paths in one
// call and re-homes the doc WITHOUT any delete window.
//
// Driven at the `FileSyncController` boundary (real controller, in-memory store
// + doc manager, real on-disk files) — the level the parked keystone.jsonl case
// could not reach without the atomic port. The doc-root Page lives in the mock
// `DocumentManager` (as it lives in `block_raw` in prod); child blocks land in
// the store. The cascade's observable here is the CHILD deletion; the atomic
// path's observables are child survival + the doc-root retitled in the store.
mod atomic_rename_tests {
    use super::*;

    /// Write `test.org` (`#+ID:` + one child) and ingest it so the store +
    /// `last_projection` are established.
    async fn seed_and_ingest(fx: &mut TestFixture, child: &Block) {
        fx.ensure_parent_dirs().await;
        let baseline = vec![child.clone()];
        fx.seed_blocks(&baseline);
        let org_children = OrgRenderer::render_entitys(&baseline, &fx.file_path(), &fx.doc_id);
        let org = format!("#+ID: {}\n{}", fx.doc_id.id(), org_children);
        tokio::fs::write(&fx.file_path(), org.as_bytes())
            .await
            .expect("write test.org");
        fx.controller
            .on_file_changed(&fx.file_path())
            .await
            .expect("initial ingest of test.org");
    }

    /// The POLL-backstop path is safety-netted too. When the destination is
    /// ingested (Create half) and the source's disappearance is discovered by
    /// `poll_tracked_files` (rather than an explicit Remove event), the
    /// resulting `on_file_deleted` for the source must NOT cascade-delete the
    /// re-homed doc — the id-based reunification finds the doc alive at the new
    /// path and re-homes instead. (Before the refutation fix this poll path
    /// cascade-deleted the live doc.)
    #[tokio::test]
    async fn nonatomic_rename_via_poll_is_rescued_by_reunification() {
        let temp = tempfile::tempdir().unwrap();
        let mut fx = TestFixture::new(temp.path());
        let child = Block::new_text(
            EntityUri::block_random(),
            fx.doc_id.clone(),
            "child body".to_string(),
        );
        seed_and_ingest(&mut fx, &child).await;

        assert!(
            BlockReader::get_block_authoritative(&*fx.store, &child.id)
                .await
                .unwrap()
                .is_some(),
            "precondition: child block ingested into the store"
        );

        let old = fx.file_path();
        let moved = fx.root_dir.join("moved.org");
        tokio::fs::rename(&old, &moved).await.unwrap();

        // Create(B) half, then a poll tick discovers A gone → on_file_deleted(A).
        fx.controller.on_file_changed(&moved).await.unwrap();
        fx.controller.poll_tracked_files().await.unwrap();

        assert_eq!(
            fx.store.delete_count(),
            0,
            "the poll-discovered disappearance of the renamed-away source must be rescued by \
             id-based reunification — NOT cascade-deleted"
        );
        assert!(
            BlockReader::get_block_authoritative(&*fx.store, &child.id)
                .await
                .unwrap()
                .is_some(),
            "child block survives a poll-discovered rename"
        );
    }

    /// The atomic port: the SAME `mv`, one `on_file_renamed` call, keeps the
    /// child intact (NO cascade), and retitles the doc-root to the new file
    /// stem (file-move spec D2). A poll tick after the rename must NOT
    /// cascade-delete.
    #[tokio::test]
    async fn atomic_rename_keeps_blocks_alive_and_retitles() {
        let temp = tempfile::tempdir().unwrap();
        let mut fx = TestFixture::new(temp.path());
        let child = Block::new_text(
            EntityUri::block_random(),
            fx.doc_id.clone(),
            "child body".to_string(),
        );
        seed_and_ingest(&mut fx, &child).await;
        let doc_id = fx.doc_id.clone();

        let old = fx.file_path();
        let moved = fx.root_dir.join("moved.org");
        tokio::fs::rename(&old, &moved).await.unwrap();

        // Atomic path — one call carrying both sides, no delete window.
        fx.controller.on_file_renamed(&old, &moved).await.unwrap();
        // A poll tick must be a no-op: the old path was re-homed, not left
        // dangling in the tracked set.
        fx.controller.poll_tracked_files().await.unwrap();

        // Child survives — the anti-cascade property, the core fix.
        assert!(
            BlockReader::get_block_authoritative(&*fx.store, &child.id)
                .await
                .unwrap()
                .is_some(),
            "child block must survive an atomic rename (no cascade window)"
        );

        // Doc-root retitled to the new file stem — the file-move spec (D2). The
        // retitle went through the same org->block write seam prod uses; the
        // Page-tag preservation is prod's `update_in_tree` (minimal params)
        // contract, exercised by `idonly_title_heal.rs` — not re-asserted here
        // where the doc-root lives in the mock DocumentManager, not the store.
        let doc_after = BlockReader::get_block_authoritative(&*fx.store, &doc_id)
            .await
            .unwrap()
            .expect("atomic rename must materialize the retitled doc-root (id stable, no re-mint)");
        assert_eq!(
            doc_after.content, "moved",
            "page title must follow the new file name (file-move spec D2)"
        );
    }

    /// REFUTATION RED (verifier 2026-07-27): the atomic port's fallback. When
    /// the watcher's pairing degrades to a bare `Remove` + `Create` (a
    /// byte-syncer / lock-file interposed between the two rename halves, or the
    /// pair timed out), the stray `Remove` reaches `on_file_deleted` for a path
    /// whose `#+ID` NOW LIVES at the moved file. The title-based D3 guard
    /// cannot fire (the title has not followed the rename), so today the
    /// live doc is cascade-deleted. The id-based reunification safety net
    /// must re-home instead: NO cascade (delete_count == 0), child + doc
    /// survive, retitled.
    #[tokio::test]
    async fn rename_fallback_remove_does_not_cascade_a_live_doc() {
        let temp = tempfile::tempdir().unwrap();
        let mut fx = TestFixture::new(temp.path());
        let child = Block::new_text(
            EntityUri::block_random(),
            fx.doc_id.clone(),
            "child body".to_string(),
        );
        seed_and_ingest(&mut fx, &child).await;
        let doc_id = fx.doc_id.clone();

        let old = fx.file_path();
        let moved = fx.root_dir.join("moved.org");
        tokio::fs::rename(&old, &moved).await.unwrap();

        // Fallback ordering: the moved file's `Create` is processed first (so
        // its `#+ID` is now tracked at `moved`), THEN the stray `Remove` of the
        // old path arrives — the exact sequence a flush-then-create fallback
        // hands the controller.
        fx.controller.on_file_changed(&moved).await.unwrap();
        fx.controller.on_file_changed(&old).await.unwrap();

        assert_eq!(
            fx.store.delete_count(),
            0,
            "a stray Remove of a renamed-away file must NOT cascade-delete a doc whose #+ID now              lives at another tracked path — the id-based reunification safety net must re-home"
        );
        assert!(
            BlockReader::get_block_authoritative(&*fx.store, &child.id)
                .await
                .unwrap()
                .is_some(),
            "child block must survive the rename fallback"
        );
        let doc_after = BlockReader::get_block_authoritative(&*fx.store, &doc_id)
            .await
            .unwrap()
            .expect("doc-root must stay alive through the rename fallback");
        assert_eq!(
            doc_after.content, "moved",
            "reunification re-homes AND retitles to the new file stem"
        );
    }

    /// ENVIRONMENT-PARITY RUNG (BugFunnel 2026-07-27). The composed keystone
    /// enters BELOW `NotifyWatcher` (on `InMemoryFileSystem`), so
    /// `RenamePairing` and the bridge's kind->`FileEvent` routing are
    /// structurally UNTRAVERSED — the prod-only layer where the adversarial
    /// verifier found the cascade-delete-on-interposition defect. This rung
    /// closes that parity gap: it drives SYNTHETIC notify-shaped signals
    /// through the REAL `RenamePairing::classify` -> the REAL bridge
    /// routing (`classify_change_to_event`, the same fn the production
    /// `OrgFileWatcher` uses) -> `FileEvent` -> the controller. Sequence:
    /// From -> interposing byte-syncer write -> To. With the relevance-gate
    /// + timeout-only flush the interposer does not disturb the pending, so
    /// the pair collapses to a SINGLE atomic `Rename` — no `Remove`, no
    /// cascade. (Full composed-keystone integration of a notify-shaped
    /// source remains open parity work.)
    #[tokio::test]
    async fn notify_shaped_interposed_rename_traverses_pairing_and_routing_no_cascade() {
        use std::path::Path;
        use std::time::Instant;

        use holon_filesystem::FileChange;
        use holon_filesystem::RawFsSignal;
        use holon_filesystem::RenamePairing;
        use holon_orgmode::FileEvent;
        use holon_orgmode::classify_change_to_event;

        let temp = tempfile::tempdir().unwrap();
        let mut fx = TestFixture::new(temp.path());
        let child = Block::new_text(
            EntityUri::block_random(),
            fx.doc_id.clone(),
            "child body".to_string(),
        );
        seed_and_ingest(&mut fx, &child).await;
        let doc_id = fx.doc_id.clone();

        let old = fx.file_path();
        let moved = fx.root_dir.join("moved.org");
        tokio::fs::rename(&old, &moved).await.unwrap();

        // (1) REAL pairing state machine over synthetic notify-shaped signals.
        let mut pairing = RenamePairing::new();
        let now = Instant::now();
        let rel = |p: &Path| p.extension().is_some_and(|e| e == "org");
        let mut emissions = Vec::new();
        emissions.extend(pairing.classify(&RawFsSignal::RenameFrom(old.clone()), now, &rel));
        emissions.extend(pairing.classify(
            &RawFsSignal::Create(fx.root_dir.join(".syncthing.tmp")),
            now,
            &rel,
        ));
        emissions.extend(pairing.classify(&RawFsSignal::RenameTo(moved.clone()), now, &rel));

        // (2) REAL bridge routing (classify_change_to_event) -> FileEvent, then
        // (3) the sync-loop dispatch onto the controller.
        for (seq, (path, kind)) in emissions.into_iter().enumerate() {
            let change = FileChange {
                path,
                kind,
                seq: seq as u64,
            };
            match classify_change_to_event(change, &rel) {
                Some(FileEvent::Renamed { from, to }) => {
                    fx.controller.on_file_renamed(&from, &to).await.unwrap()
                }
                Some(FileEvent::Changed(p)) => fx.controller.on_file_changed(&p).await.unwrap(),
                None => {}
            }
        }

        assert_eq!(
            fx.store.delete_count(),
            0,
            "an interposed rename must pair into a single atomic Rename through the REAL pairing \
             + routing — no cascade reaches the controller"
        );
        assert!(
            BlockReader::get_block_authoritative(&*fx.store, &child.id)
                .await
                .unwrap()
                .is_some(),
            "child survives the interposed rename"
        );
        let doc_after = BlockReader::get_block_authoritative(&*fx.store, &doc_id)
            .await
            .unwrap()
            .expect("doc-root alive");
        assert_eq!(doc_after.content, "moved", "retitled to the new file stem");
    }
}

// ============================================================================
// Residual write-back hole: an INTERMEDIATE-ancestor convert re-homes a DEEP
// descendant whose own parent_id/tags never change, so the block-driven
// write-back cheap path (file_sync_controller.rs render_with_cache:3855 gate +
// :3872 authority re-check) cannot see that the descendant left this document.
//
// Documented at ~/.claude/plans/stale-delta-redesign-options-2026-08-04.md
// §2.3. The gate and the authority re-check both compare the block's OWN
// parent_id / tags against the doc's cached copy — neither verifies the block
// still BELONGS to `doc_id`. When an intermediate ancestor (not X's direct
// parent) gains the `Page` tag via `convert_block_to_page`, X re-homes to the
// new page's document while X.parent_id and X.tags stay identical. A queued
// delta for X, routed to the OLD document before the convert (T1), drains at T2
// on the cheap path and re-renders X into the OLD file — an edit meant for the
// new page lands in the wrong file. reseeded=false, so the removal veto is
// skipped and last_projection is stamped over the divergence.
//
// Topology: P_a (page, file `test.org`) owns  A > M > N > X  (M is an
// INTERMEDIATE, non-leaf ancestor of X; N sits between them). `convert_block_to
// _page` on M mints page P_m under P_a, re-homes M's direct child N under P_m,
// and X follows as N's child WITHOUT its own parent_id changing.
mod intermediate_ancestor_writeback_hole {
    use super::*;

    fn text_block(id: &str, parent: &EntityUri, content: &str, level: i64, seq: i64) -> Block {
        let uri = EntityUri::block(id);
        let mut b = Block::new_text(uri.clone(), parent.clone(), content);
        b.set_level(level);
        b.set_sequence(seq);
        b.set_property("ID", Value::String(id.to_string()));
        b
    }

    /// Build `P_a { A > M > N > X }`, warm the controller's per-doc cache so X
    /// is cached under `P_a`, then apply the `convert_block_to_page(M)`
    /// EFFECT to the authority: N and X move into P_m's bucket (N.parent =
    /// P_m, X.parent = N UNCHANGED), P_m becomes a page in both the block
    /// authority and the doc manager, and P_m's file is materialized on
    /// disk. Returns the ids and the EDITED X block whose (stale,
    /// routed-to-P_a) delta the caller then drains.
    async fn build_and_convert(fixture: &mut TestFixture) -> (EntityUri, EntityUri, Block) {
        let p_a = fixture.doc_id.clone(); // fixture pre-seeds "test" page at doc_id
        let a = text_block("la-a", &p_a, "la-a-outer", 1, 0);
        let m = text_block("la-m", &a.id, "la-m-middle", 2, 0);
        let n = text_block("la-n", &m.id, "la-n-inner", 3, 0);
        let x = text_block("la-x", &n.id, "la-x-leaf", 4, 0);

        fixture.seed_blocks(&[a.clone(), m.clone(), n.clone(), x.clone()]);
        fixture
            .controller
            .initialize()
            .await
            .expect("initialize must succeed");

        // WARM: a first block delta for X reseeds P_a's cache to {A,M,N,X} and
        // writes `test.org` = A>M>N>X (X legitimately belongs to P_a here).
        fixture
            .controller
            .on_block_changed(&p_a, &holon_filesystem::BlockDelta::Upsert(x.clone()))
            .await
            .expect("warm write must succeed");

        // --- convert_block_to_page(M) EFFECT on the AUTHORITATIVE store ---
        // P_a bucket keeps A and M (M becomes a link to P_m, modeled as a plain
        // block that stays put — the point is N and X LEAVE). P_m bucket gains a
        // Page block, N (re-parented to P_m), and X (parent UNCHANGED = N).
        let p_m = EntityUri::block("la-pm");
        let mut pm_page = Block::new_text(p_m.clone(), p_a.clone(), "Pm");
        pm_page.set_page(true);
        pm_page.set_property("ID", Value::String("la-pm".to_string()));

        let n_moved = text_block("la-n", &p_m, "la-n-inner", 1, 0); // parent M -> P_m
        // X's OWN parent_id (N) and tags are IDENTICAL to the cached copy; only
        // the content carries the queued edit. This is the crux of the hole.
        let x_edited = text_block("la-x", &n.id, "la-x-EDITED-after-rehome", 2, 0);

        fixture
            .store
            .seed_blocks(p_a.as_str(), vec![a.clone(), m.clone()]);
        fixture.store.seed_blocks(
            p_m.as_str(),
            vec![pm_page.clone(), n_moved.clone(), x_edited.clone()],
        );
        fixture.doc_manager.add_document(pm_page);

        // Materialize P_m's identity file on disk (prod's convert does this).
        let pm_path = fixture.root_dir.join("test").join("Pm.org");
        tokio::fs::create_dir_all(pm_path.parent().unwrap())
            .await
            .unwrap();
        let pm_body = OrgRenderer::render_entitys(&[n_moved, x_edited.clone()], &pm_path, &p_m);
        tokio::fs::write(&pm_path, format!("#+ID: la-pm\n{pm_body}"))
            .await
            .unwrap();

        (p_a, p_m, x_edited)
    }

    /// RED: after the convert, draining X's stale (routed-to-P_a) edit delta
    /// takes the cheap path and re-renders X's EDITED content into
    /// `test.org` — the OLD document's file — even though X now belongs to
    /// P_m. The file that P_a owns must not carry X's subtree after the
    /// re-home; it does → the hole is real.
    #[ignore = "deliberate RED: Option-C evidence for the §2.3 write-back hole \
(docs/Plans/option-c-holder-design.md); un-ignore at Inc 2 when the home_by \
holder replaces the cheap-path qualification"]
    #[tokio::test]
    async fn convert_leaks_deep_descendant_edit_into_old_doc() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut fixture = TestFixture::new(temp_dir.path());
        let (p_a, _p_m, x_edited) = build_and_convert(&mut fixture).await;

        // Drain X's queued delta, routed to the OLD doc P_a (stale T1 routing).
        fixture
            .controller
            .on_block_changed(&p_a, &holon_filesystem::BlockDelta::Upsert(x_edited))
            .await
            .expect("stale delta drain returned an unexpected error");

        let test_org = tokio::fs::read_to_string(fixture.file_path())
            .await
            .unwrap();

        assert!(
            !test_org.contains("la-x"),
            "WRITE-BACK HOLE: P_a's file `test.org` still renders re-homed descendant \
             `la-x` (it belongs to P_m now). An edit routed to the OLD document leaked \
             into the OLD file via the cheap path (reseed skipped). On-disk `test.org`:\n{test_org}"
        );
    }

    /// DURABILITY PROBE: once the intermediate ancestor N's OWN structural
    /// delta drains, P_a is reseeded from the authority (get_blocks no
    /// longer returns N/X) and the removal veto grounds their absence as a
    /// MOVE (grounding reads the authority, which reflects X under P_m). So
    /// the divergence SELF-HEALS — `test.org` drops X and P_m's file owns
    /// it. This documents the verdict: TRANSIENT, not durable data loss
    /// (matches the plan's own assessment).
    #[tokio::test]
    async fn divergence_self_heals_after_ancestor_reseed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut fixture = TestFixture::new(temp_dir.path());
        let (p_a, p_m, x_edited) = build_and_convert(&mut fixture).await;

        // 1. Stale X edit drains first → transient leak into test.org.
        fixture
            .controller
            .on_block_changed(&p_a, &holon_filesystem::BlockDelta::Upsert(x_edited))
            .await
            .expect("stale delta drain");
        let leaked = tokio::fs::read_to_string(fixture.file_path())
            .await
            .unwrap();
        assert!(
            leaked.contains("la-x"),
            "precondition: the transient leak must be present before the heal; test.org:\n{leaked}"
        );

        // 2. N's OWN structural delta (parent M -> P_m) drains → reseed P_a.
        let n_moved = text_block("la-n", &p_m, "la-n-inner", 1, 0);
        fixture
            .controller
            .on_block_changed(&p_a, &holon_filesystem::BlockDelta::Upsert(n_moved))
            .await
            .expect("ancestor reseed must succeed (removal grounded as a move, not vetoed)");

        let healed = tokio::fs::read_to_string(fixture.file_path())
            .await
            .unwrap();
        assert!(
            !healed.contains("la-x"),
            "self-heal expected: after N's reseed P_a's file must drop the re-homed \
             descendant. test.org:\n{healed}"
        );
        let pm_file = tokio::fs::read_to_string(fixture.root_dir.join("test").join("Pm.org"))
            .await
            .unwrap();
        assert!(
            pm_file.contains("la-x"),
            "after the heal X must live in exactly one document — P_m's file. Pm.org:\n{pm_file}"
        );
    }
}
