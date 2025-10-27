//! Loro-based block storage using LoroTree for hierarchical structure.
//!
//! All block data is stored in a single LoroTree within one LoroDoc.
//! Each tree node's `get_meta()` LoroMap holds content (nested LoroText),
//! properties (JSON string), and metadata (timestamps, is_document, name).
//!
//! Each node carries a stable `id` (UUID) in its metadata that serves as the
//! block's business identity. This ID is assigned at creation, replicates via
//! CRDT, and is used as the SQL primary key — ensuring all peers share the
//! same block identity.

use super::repository::{CoreOperations, Lifecycle, P2POperations};
use super::types::NewBlock;
use crate::sync::LoroDocument;
use crate::sync::shared_tree::{SharedTreeStore, is_mount_node, read_mount_info};
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::streaming::{ChangeNotifications, ChangeSubscribers};
use holon_api::{
    ApiError, Block, BlockContent, Change, ChangeOrigin, ContentType, SourceBlock, StreamPosition,
    Value,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_stream::{Stream, StreamExt, wrappers::ReceiverStream};

// Field name constants
pub const CONTENT_TYPE: &str = "content_type";
pub const CONTENT_RAW: &str = "content_raw";
pub const SOURCE_LANGUAGE: &str = "source_language";
pub const SOURCE_CODE: &str = "source_code";
const SOURCE_NAME: &str = "source_name";
const SOURCE_HEADER_ARGS: &str = "source_header_args";
const PROPERTIES: &str = "properties";
/// Stable block identity — a UUID assigned at creation that travels with the
/// CRDT node across peers. Used as the SQL primary key.
pub const STABLE_ID: &str = "id";
pub const TREE_NAME: &str = "blocks";
/// Key in tree node metadata for a foreign-system block identifier (e.g.
/// the SQL/Turso UUID). Used to round-trip external IDs across sync.
pub const EXTERNAL_ID: &str = "external_id";

/// Inverse of [`mark_to_loro_value`]: reconstruct an `InlineMark` from the
/// `(key, value)` pair stored in a Peritext mark. Returns `None` if the key
/// is unknown or the value shape doesn't match (defensive against external
/// data, but unknown keys are dropped silently — they came from a peer
/// running a newer version).
pub fn mark_from_loro_value(key: &str, value: &loro::LoroValue) -> Option<holon_api::InlineMark> {
    use holon_api::{EntityRef, EntityUri, InlineMark};
    match key {
        "bold" => Some(InlineMark::Bold),
        "italic" => Some(InlineMark::Italic),
        "code" => Some(InlineMark::Code),
        "verbatim" => Some(InlineMark::Verbatim),
        "strike" => Some(InlineMark::Strike),
        "underline" => Some(InlineMark::Underline),
        "sub" => Some(InlineMark::Sub),
        "super" => Some(InlineMark::Super),
        "link" => {
            let map = match value {
                loro::LoroValue::Map(m) => m,
                _ => return None,
            };
            let label = map
                .get("label")
                .and_then(|v| match v {
                    loro::LoroValue::String(s) => Some(s.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            let kind = map.get("type").and_then(|v| match v {
                loro::LoroValue::String(s) => Some(s.to_string()),
                _ => None,
            })?;
            let target = match kind.as_str() {
                "external" => {
                    let url = map.get("url").and_then(|v| match v {
                        loro::LoroValue::String(s) => Some(s.to_string()),
                        _ => None,
                    })?;
                    EntityRef::External { url }
                }
                "internal" => {
                    let id_str = map.get("id").and_then(|v| match v {
                        loro::LoroValue::String(s) => Some(s.to_string()),
                        _ => None,
                    })?;
                    EntityRef::Internal {
                        id: EntityUri::from_raw(&id_str),
                    }
                }
                _ => return None,
            };
            Some(InlineMark::Link { target, label })
        }
        _ => None,
    }
}

/// Read the Peritext marks from a `LoroText` and reconstruct the
/// `Vec<MarkSpan>` projection in Unicode-scalar offsets.
///
/// Walks `text.to_delta()` (Quill-shaped insert ops with optional attribute
/// maps) and emits one `MarkSpan` per (key, value) attribute run. Adjacent
/// inserts that share an attribute are coalesced into a single span.
pub fn read_marks_from_text(text: &loro::LoroText) -> Vec<holon_api::MarkSpan> {
    use holon_api::MarkSpan;
    let delta = text.to_delta();
    let mut marks: Vec<MarkSpan> = Vec::new();
    // active: key → (start_char, value) — open mark runs.
    let mut active: std::collections::HashMap<String, (usize, loro::LoroValue)> =
        std::collections::HashMap::new();
    let mut char_pos: usize = 0;

    for op in delta {
        let loro::TextDelta::Insert { insert, attributes } = op else {
            continue;
        };
        let attrs: std::collections::HashMap<String, loro::LoroValue> = attributes
            .map(|m| m.into_iter().collect())
            .unwrap_or_default();

        // Close marks that are absent in `attrs` or have a different value.
        let to_close: Vec<String> = active
            .iter()
            .filter(|(k, (_, v))| match attrs.get(*k) {
                Some(new_v) => v != new_v,
                None => true,
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in to_close {
            let (start, value) = active.remove(&key).expect("key was just listed");
            if let Some(mark) = mark_from_loro_value(&key, &value) {
                marks.push(MarkSpan::new(start, char_pos, mark));
            }
        }

        // Open new marks for keys not yet active.
        for (k, v) in &attrs {
            active
                .entry(k.clone())
                .or_insert_with(|| (char_pos, v.clone()));
        }

        char_pos += insert.chars().count();
    }

    // Close any marks still open at end.
    for (key, (start, value)) in active {
        if let Some(mark) = mark_from_loro_value(&key, &value) {
            marks.push(MarkSpan::new(start, char_pos, mark));
        }
    }

    marks
}

/// Convert an `InlineMark` to the `LoroValue` we store in the Peritext mark.
///
/// For boolean marks (Bold/Italic/.../Sub/Super) the value is `true` — Loro
/// requires *some* value, and `true` is the canonical "this mark is present"
/// payload across the spike and the Loro test fixtures. For `Link`, the value
/// is a `LoroValue::Map` carrying `{ "type": "external"|"internal", "url"|"id":
/// ..., "label": ... }` so the render layer can reconstruct the full
/// `EntityRef`+label without going back to `Block.marks`.
pub fn mark_to_loro_value(mark: &holon_api::InlineMark) -> loro::LoroValue {
    use holon_api::{EntityRef, InlineMark};
    match mark {
        InlineMark::Bold
        | InlineMark::Italic
        | InlineMark::Code
        | InlineMark::Verbatim
        | InlineMark::Strike
        | InlineMark::Underline
        | InlineMark::Sub
        | InlineMark::Super => loro::LoroValue::Bool(true),
        InlineMark::Link { target, label } => {
            let mut map = std::collections::HashMap::new();
            map.insert("label".to_string(), loro::LoroValue::from(label.as_str()));
            match target {
                EntityRef::External { url } => {
                    map.insert("type".to_string(), loro::LoroValue::from("external"));
                    map.insert("url".to_string(), loro::LoroValue::from(url.as_str()));
                }
                EntityRef::Internal { id } => {
                    map.insert("type".to_string(), loro::LoroValue::from("internal"));
                    map.insert("id".to_string(), loro::LoroValue::from(id.as_str()));
                }
            }
            loro::LoroValue::from(map)
        }
    }
}

/// Install the per-mark `ExpandType` policy on a freshly-created `LoroDoc`.
///
/// **Call this exactly once per LoroDoc, immediately after `LoroDoc::new()`.**
///
/// Phase 0.1 spike S3 (`crates/holon/examples/loro_marks_spike.rs`) confirmed
/// that re-calling `config_text_style` with a conflicting `ExpandType` is a
/// silent no-op — the first config wins and there's no runtime "fix". The
/// policy must therefore be installed once at doc creation, before any
/// `LoroText` is created or any mark is applied.
///
/// Policy (per `holon_api::InlineMark::expand_after`):
/// - `bold/italic/code/strike/underline/sub/super` → `ExpandType::After`
///   (typing at the trailing edge inherits the mark)
/// - `link/verbatim` → `ExpandType::None` (typing at the boundary escapes)
pub fn configure_text_styles(doc: &loro::LoroDoc) {
    use holon_api::InlineMark;
    use loro::{ExpandType, StyleConfig, StyleConfigMap};

    let mut cfg = StyleConfigMap::new();
    for key in InlineMark::all_loro_keys() {
        let expand = if InlineMark::expand_after(key) {
            ExpandType::After
        } else {
            ExpandType::None
        };
        cfg.insert((*key).into(), StyleConfig { expand });
    }
    doc.config_text_style(cfg);
}

/// Helper trait for extracting typed values from Loro maps.
trait LoroMapExt {
    fn get_typed<T, F>(&self, key: &str, f: F) -> Option<T>
    where
        F: FnOnce(&loro::LoroValue) -> Option<T>;
}

impl LoroMapExt for loro::LoroMap {
    fn get_typed<T, F>(&self, key: &str, f: F) -> Option<T>
    where
        F: FnOnce(&loro::LoroValue) -> Option<T>,
    {
        self.get(key).and_then(|v| match v {
            loro::ValueOrContainer::Value(val) => f(&val),
            _ => None,
        })
    }
}

// -- TreeID <-> EntityUri conversion --

fn tree_id_to_uri(tid: loro::TreeID) -> EntityUri {
    EntityUri::block_from_tree_id(tid.peer, tid.counter)
}

fn uri_to_tree_id(uri: &EntityUri) -> Option<loro::TreeID> {
    let (peer, counter) = uri.to_tree_id_parts()?;
    Some(loro::TreeID::new(peer, counter))
}

fn str_to_tree_id(s: &str) -> Option<loro::TreeID> {
    let uri = EntityUri::from_raw(s);
    uri_to_tree_id(&uri)
}

// -- Reading block data from tree node metadata --

fn read_text_content(meta: &loro::LoroMap) -> String {
    match meta.get(CONTENT_RAW) {
        Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) => text.to_string(),
        Some(loro::ValueOrContainer::Value(val)) => {
            val.as_string().map(|s| s.to_string()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// Read marks from the `CONTENT_RAW` LoroText. Returns `Some(empty)` when
/// the text container exists but carries no marks (rich block with no active
/// marks); returns `None` when there's no LoroText container at all (legacy
/// plain block — preserves today's behavior). The discriminator at higher
/// layers is "marks IS NOT NULL" not "marks is non-empty".
fn read_text_marks(meta: &loro::LoroMap) -> Option<Vec<holon_api::MarkSpan>> {
    match meta.get(CONTENT_RAW) {
        Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) => {
            let marks = read_marks_from_text(&text);
            if marks.is_empty() { None } else { Some(marks) }
        }
        _ => None,
    }
}

fn read_source_code(meta: &loro::LoroMap) -> String {
    match meta.get(SOURCE_CODE) {
        Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) => text.to_string(),
        Some(loro::ValueOrContainer::Value(val)) => {
            val.as_string().map(|s| s.to_string()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn read_content_from_meta(meta: &loro::LoroMap) -> BlockContent {
    let content_type = meta.get_typed(CONTENT_TYPE, |val| val.as_string().map(|s| s.to_string()));

    match content_type.as_deref() {
        Some("source") => {
            let language = meta.get_typed(SOURCE_LANGUAGE, |val| {
                val.as_string().map(|s| s.to_string())
            });
            let source = read_source_code(meta);
            let name = meta.get_typed(SOURCE_NAME, |val| val.as_string().map(|s| s.to_string()));
            let header_args: HashMap<String, Value> = match meta
                .get_typed(SOURCE_HEADER_ARGS, |val| {
                    val.as_string().map(|s| s.to_string())
                }) {
                Some(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                    panic!("Corrupt header_args JSON in Loro tree: {json:?}: {e}")
                }),
                None => HashMap::new(),
            };

            BlockContent::Source(SourceBlock {
                language,
                source,
                name,
                header_args,
            })
        }
        Some("image") => {
            let path = read_text_content(meta);
            BlockContent::Text { raw: path }
        }
        Some("text") | None => {
            let raw = read_text_content(meta);
            match read_text_marks(meta) {
                Some(marks) => BlockContent::RichText { text: raw, marks },
                None => BlockContent::Text { raw },
            }
        }
        Some(unknown) => panic!("Unknown content_type in Loro tree: {unknown:?}"),
    }
}

fn read_properties_from_meta(meta: &loro::LoroMap) -> HashMap<String, Value> {
    match meta.get_typed(PROPERTIES, |val| val.as_string().map(|s| s.to_string())) {
        Some(json) => serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("Corrupt properties JSON in Loro tree: {json:?}: {e}")),
        None => HashMap::new(),
    }
}

/// Read the stable ID from a node's metadata.
fn read_stable_id(meta: &loro::LoroMap) -> Option<String> {
    meta.get_typed(STABLE_ID, |val| val.as_string().map(|s| s.to_string()))
}

/// Build an EntityUri from a node's stable ID metadata.
/// Panics if the node has no STABLE_ID — all nodes must have one.
fn block_uri_from_meta(meta: &loro::LoroMap, node: loro::TreeID) -> EntityUri {
    let stable_id = read_stable_id(meta)
        .unwrap_or_else(|| panic!("Node {:?} missing STABLE_ID metadata", node));
    EntityUri::block(&stable_id)
}

fn read_block_from_tree(
    tree: &loro::LoroTree,
    node: loro::TreeID,
    parent_tree_id: Option<loro::TreeID>,
) -> Block {
    let meta = tree
        .get_meta(node)
        .unwrap_or_else(|_| panic!("get_meta failed for node {:?}", node));
    let content = read_content_from_meta(&meta);
    let properties = read_properties_from_meta(&meta);

    let id = block_uri_from_meta(&meta, node);
    let parent_id = match parent_tree_id {
        Some(pid) => {
            let parent_meta = tree
                .get_meta(pid)
                .unwrap_or_else(|_| panic!("get_meta failed for parent {:?}", pid));
            block_uri_from_meta(&parent_meta, pid)
        }
        None => EntityUri::no_parent(),
    };

    let created_at = meta
        .get_typed("created_at", |val| val.as_i64().copied())
        .unwrap_or(0);
    let updated_at = meta
        .get_typed("updated_at", |val| val.as_i64().copied())
        .unwrap_or(0);

    let block_name = meta.get_typed("name", |val| val.as_string().map(|s| s.to_string()));

    let mut block = Block::from_block_content(id, parent_id, content);
    block.set_properties_map(properties);
    block.name = block_name;
    block.created_at = created_at;
    block.updated_at = updated_at;
    block
}

/// Check if two blocks differ in content, structure, or properties.
fn diff_blocks_changed(a: &Block, b: &Block) -> bool {
    a.content != b.content
        || a.parent_id != b.parent_id
        || a.content_type != b.content_type
        || a.source_language != b.source_language
        || a.properties_map() != b.properties_map()
}

// -- Writing block data to tree node metadata --

fn update_text_field(meta: &loro::LoroMap, key: &str, new_text: &str) -> anyhow::Result<()> {
    let text = meta.get_or_create_container(key, loro::LoroText::new())?;
    text.update(new_text, Default::default())
        .map_err(|e| anyhow::anyhow!("LoroText update failed: {:?}", e))?;
    Ok(())
}

fn write_content_to_meta(
    meta: &loro::LoroMap,
    content: &BlockContent,
    content_type_override: Option<ContentType>,
) -> anyhow::Result<()> {
    match content {
        BlockContent::Text { raw } => {
            let ct = content_type_override.unwrap_or(ContentType::Text);
            meta.insert(CONTENT_TYPE, loro::LoroValue::from(ct.to_string().as_str()))?;
            update_text_field(meta, CONTENT_RAW, raw)?;
        }
        BlockContent::RichText { text, marks: _ } => {
            // Phase 1.1 stub: write text via the existing Text path; Loro Peritext
            // mark application is wired in Task 5 (`update_block_marked`). The
            // marks JSON projection lives in the SQL `marks` column (Task 4),
            // sourced from `Block.marks` directly.
            let ct = content_type_override.unwrap_or(ContentType::Text);
            meta.insert(CONTENT_TYPE, loro::LoroValue::from(ct.to_string().as_str()))?;
            update_text_field(meta, CONTENT_RAW, text)?;
        }
        BlockContent::Source(source) => {
            meta.insert(CONTENT_TYPE, loro::LoroValue::from("source"))?;
            if let Some(lang) = &source.language {
                meta.insert(SOURCE_LANGUAGE, loro::LoroValue::from(lang.as_str()))?;
            }
            update_text_field(meta, SOURCE_CODE, &source.source)?;
            if let Some(name) = &source.name {
                meta.insert(SOURCE_NAME, loro::LoroValue::from(name.as_str()))?;
            }
            if !source.header_args.is_empty() {
                let json = serde_json::to_string(&source.header_args)?;
                meta.insert(SOURCE_HEADER_ARGS, loro::LoroValue::from(json.as_str()))?;
            }
        }
    }
    Ok(())
}

fn write_properties_to_meta(
    meta: &loro::LoroMap,
    properties: &HashMap<String, Value>,
) -> anyhow::Result<()> {
    if !properties.is_empty() {
        let json = serde_json::to_string(properties)?;
        meta.insert(PROPERTIES, loro::LoroValue::from(json.as_str()))?;
    }
    Ok(())
}

// -- Resolving parent TreeID from EntityUri --

fn resolve_parent_tree_id(
    tree: &loro::LoroTree,
    id_cache: &Arc<Mutex<HashMap<String, loro::TreeID>>>,
    parent_uri: &EntityUri,
) -> anyhow::Result<Option<loro::TreeID>> {
    if parent_uri.is_no_parent() || parent_uri.is_sentinel() {
        return Ok(None);
    }
    // Try TreeID format first, then stable ID cache
    let tree_id = uri_to_tree_id(parent_uri)
        .or_else(|| {
            if parent_uri.is_block() {
                id_cache.lock().unwrap().get(parent_uri.id()).copied()
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("Cannot resolve parent URI to TreeID: {}", parent_uri))?;
    tree.get_meta(tree_id)
        .map_err(|_| anyhow::anyhow!("Parent node does not exist: {}", parent_uri))?;
    Ok(Some(tree_id))
}

/// Get the parent TreeID of a node.
fn get_node_parent(tree: &loro::LoroTree, node: loro::TreeID) -> Option<loro::TreeID> {
    match tree.parent(node)? {
        loro::TreeParentId::Node(pid) => Some(pid),
        _ => None,
    }
}

/// Snapshot all alive blocks in a raw `LoroDoc`, keyed by stable ID.
///
/// This is the same logic `LoroBackend::snapshot_blocks` uses, but on a raw
/// `&LoroDoc` rather than a `CollabDoc`. It exists so `LoroSyncController`
/// can snapshot both the forked (old) state and the current state of the
/// doc during reconciliation without wrapping them in `LoroDocument`.
pub fn snapshot_blocks_from_doc(doc: &loro::LoroDoc) -> HashMap<String, Block> {
    let tree = doc.get_tree(TREE_NAME);
    let mut blocks = HashMap::new();
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        let parent_tid = get_node_parent(&tree, node.id);
        let block = read_block_from_tree(&tree, node.id, parent_tid);
        blocks.insert(block.id.to_string(), block);
    }
    blocks
}

/// Check if a node is alive (not deleted) in the tree.
fn is_node_alive(tree: &loro::LoroTree, node: loro::TreeID) -> bool {
    match tree.parent(node) {
        Some(loro::TreeParentId::Deleted | loro::TreeParentId::Unexist) | None => false,
        Some(_) => true,
    }
}

/// Compute the depth of a node from its parent chain.
/// Depth 1 = tree root (implicit depth 0 = virtual document root).
fn compute_depth(tree: &loro::LoroTree, parent: loro::TreeParentId) -> usize {
    let mut d = 1;
    let mut current = parent;
    loop {
        match current {
            loro::TreeParentId::Node(pid) => {
                d += 1;
                current = tree.parent(pid).unwrap_or(loro::TreeParentId::Root);
            }
            _ => break,
        }
    }
    d
}

/// Collect all alive blocks from a shared tree, grafting them into the personal tree hierarchy.
/// Shared tree roots get `mount_parent` as their parent (the mount node's parent in the
/// personal tree), making them appear inline. Deeper nodes keep their internal relationships.
fn collect_shared_tree_blocks(
    shared_tree: &loro::LoroTree,
    mount_parent: Option<loro::TreeID>,
    mount_depth: usize,
    traversal: &super::types::Traversal,
    result: &mut Vec<Block>,
) {
    for tree_node in shared_tree.get_nodes(false) {
        if matches!(
            tree_node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }

        // Compute depth relative to mount point: shared root is at mount_depth,
        // children at mount_depth+1, etc.
        let internal_depth = compute_depth(shared_tree, tree_node.parent);
        let total_depth = mount_depth + internal_depth - 1;

        if !traversal.includes_level(total_depth) {
            continue;
        }

        // Shared tree roots get the mount node's parent as their parent_id
        let parent_tid = match tree_node.parent {
            loro::TreeParentId::Root => mount_parent,
            loro::TreeParentId::Node(pid) => Some(pid),
            _ => None,
        };
        let block = read_block_from_tree(shared_tree, tree_node.id, parent_tid);
        result.push(block);
    }
}

// ============================================================
// LoroBackend
// ============================================================

pub struct LoroBackend {
    collab_doc: Arc<LoroDocument>,
    subscribers: ChangeSubscribers<Block>,
    event_log: Arc<Mutex<Vec<Change<Block>>>>,
    shared_trees: Option<Arc<dyn SharedTreeStore>>,
    /// Cache: stable_id (UUID string) → TreeID. Populated eagerly on create,
    /// lazily on lookup, invalidated on delete.
    id_cache: Arc<Mutex<HashMap<String, loro::TreeID>>>,
}

impl Clone for LoroBackend {
    fn clone(&self) -> Self {
        Self {
            collab_doc: self.collab_doc.clone(),
            subscribers: self.subscribers.clone(),
            event_log: self.event_log.clone(),
            shared_trees: self.shared_trees.clone(),
            id_cache: self.id_cache.clone(),
        }
    }
}

impl LoroBackend {
    pub fn from_document(collab_doc: Arc<LoroDocument>) -> Self {
        Self {
            collab_doc,
            subscribers: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            event_log: Arc::new(Mutex::new(Vec::new())),
            shared_trees: None,
            id_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attach a shared tree store for mount-node traversal.
    /// When set, get_block/get_all_blocks/list_children transparently follow
    /// mount nodes into shared tree LoroDocs.
    pub fn with_shared_trees(mut self, store: Arc<dyn SharedTreeStore>) -> Self {
        self.shared_trees = Some(store);
        self
    }

    pub fn set_shared_trees(&mut self, store: Arc<dyn SharedTreeStore>) {
        self.shared_trees = Some(store);
    }

    pub fn doc_id(&self) -> &str {
        self.collab_doc.doc_id()
    }

    #[cfg(test)]
    pub fn collab_for_test(&self) -> Arc<LoroDocument> {
        self.collab_doc.clone()
    }

    fn now_millis() -> i64 {
        crate::util::now_unix_millis()
    }

    pub(crate) fn emit_change(&self, change: Change<Block>) {
        self.event_log.lock().unwrap().push(change.clone());
        let batch = vec![change];
        let subscribers = self.subscribers.clone();
        tokio::spawn(async move {
            let mut subscribers = subscribers.lock().await;
            subscribers.retain(|sender| sender.try_send(Ok(batch.clone())).is_ok());
        });
    }

    // -- Schema initialization --

    pub async fn initialize_schema(collab_doc: &LoroDocument) -> Result<(), ApiError> {
        collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                tree.enable_fractional_index(0);

                let meta = doc.get_map("_meta");
                meta.insert("_schema_version", loro::LoroValue::from(2i64))?;

                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to initialize schema: {}", e),
            })
    }

    // -- Extra public methods used by callers --

    pub async fn find_block_by_uuid(&self, uuid: &str) -> Result<Option<String>, ApiError> {
        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                for tree_node in tree.get_nodes(false) {
                    if matches!(
                        tree_node.parent,
                        loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                    ) {
                        continue;
                    }
                    let meta = tree.get_meta(tree_node.id)?;
                    let properties = read_properties_from_meta(&meta);
                    if let Some(Value::String(prop_uuid)) = properties.get("ID") {
                        if prop_uuid == uuid {
                            return Ok(Some(tree_id_to_uri(tree_node.id).to_string()));
                        }
                    }
                }
                Ok(None)
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to find block by UUID: {}", e),
            })
    }

    pub async fn update_block_text(&self, id: &str, new_text: &str) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;

                let content_type: ContentType = meta
                    .get_typed(CONTENT_TYPE, |val| val.as_string().map(|s| s.to_string()))
                    .unwrap_or_else(|| "text".to_string())
                    .parse()
                    .expect("Invalid content_type");

                let field = if content_type == ContentType::Source {
                    SOURCE_CODE
                } else {
                    CONTENT_RAW
                };
                update_text_field(&meta, field, new_text)?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update block text: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    /// Update a block's text AND its inline marks together.
    ///
    /// Replaces the `LoroText` content via `update_text_field` (same as
    /// `update_block_text`) and then re-applies the mark set via Loro
    /// Peritext's `mark` API. Marks are addressed by Unicode-scalar offsets
    /// (matches `MarkSpan::start`/`end` and Loro's default `mark` flavor).
    ///
    /// **Mark replacement semantics**: this is "wholesale replace", not
    /// "diff and apply". The full mark set in `marks` becomes the new mark
    /// state. Existing marks of the same `key` outside the new ranges are
    /// removed via `unmark` over the full text range first.
    ///
    /// Source/Image blocks reject mark application — they always carry
    /// `marks = None` in SQL. This is enforced by checking `content_type`.
    pub async fn update_block_marked(
        &self,
        id: &str,
        new_text: &str,
        marks: &[holon_api::MarkSpan],
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        let marks_owned: Vec<holon_api::MarkSpan> = marks.to_vec();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;

                let content_type: ContentType = meta
                    .get_typed(CONTENT_TYPE, |val| val.as_string().map(|s| s.to_string()))
                    .unwrap_or_else(|| "text".to_string())
                    .parse()
                    .expect("Invalid content_type");
                if content_type == ContentType::Source {
                    return Err(anyhow::anyhow!(
                        "update_block_marked: source blocks cannot carry inline marks"
                    ));
                }

                update_text_field(&meta, CONTENT_RAW, new_text)?;

                // Re-apply marks. First clear every known mark key over the
                // full text range so removed marks disappear; then set the
                // new ones. `mark` is idempotent for the same key+range.
                let text = meta.get_or_create_container(CONTENT_RAW, loro::LoroText::new())?;
                let len_chars = text.len_unicode();
                if len_chars > 0 {
                    for key in holon_api::InlineMark::all_loro_keys() {
                        text.unmark(0..len_chars, key)
                            .map_err(|e| anyhow::anyhow!("LoroText unmark {key}: {:?}", e))?;
                    }
                }
                for span in &marks_owned {
                    let key = span.mark.loro_key();
                    let value: loro::LoroValue = mark_to_loro_value(&span.mark);
                    text.mark(span.start..span.end, key, value)
                        .map_err(|e| anyhow::anyhow!("LoroText mark {key}: {:?}", e))?;
                }

                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update block marked: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    /// Apply a single inline mark over `range` without touching other marks.
    ///
    /// Range is in Unicode-scalar offsets (matching `MarkSpan::start`/`end` and
    /// Loro's default `mark` flavor). Unlike `update_block_marked`, which
    /// wholesale-replaces the mark set, this is the incremental command used
    /// by interactive editors — Cmd+B over a selection adds Bold without
    /// nuking pre-existing Italic/Code/Link marks elsewhere in the block.
    ///
    /// Source blocks reject mark application (same carve-out as
    /// `update_block_marked`).
    pub async fn apply_inline_mark(
        &self,
        id: &str,
        range: std::ops::Range<usize>,
        mark: &holon_api::InlineMark,
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        let mark_owned = mark.clone();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;

                let content_type: ContentType = meta
                    .get_typed(CONTENT_TYPE, |val| val.as_string().map(|s| s.to_string()))
                    .unwrap_or_else(|| "text".to_string())
                    .parse()
                    .expect("Invalid content_type");
                if content_type == ContentType::Source {
                    return Err(anyhow::anyhow!(
                        "apply_inline_mark: source blocks cannot carry inline marks"
                    ));
                }

                let text = meta.get_or_create_container(CONTENT_RAW, loro::LoroText::new())?;
                let key = mark_owned.loro_key();
                let value: loro::LoroValue = mark_to_loro_value(&mark_owned);
                text.mark(range.clone(), key, value)
                    .map_err(|e| anyhow::anyhow!("LoroText mark {key}: {:?}", e))?;

                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to apply inline mark: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    /// Remove a single inline mark identified by `key` over `range`.
    ///
    /// Marks with other keys are unaffected. An existing mark of the same
    /// `key` that overlaps `range` is split or shortened by Loro's `unmark`
    /// — the disjoint portions remain. `key` is the stable Loro key returned
    /// by `InlineMark::loro_key()` (e.g. `"bold"`, `"italic"`, `"link"`).
    pub async fn remove_inline_mark(
        &self,
        id: &str,
        range: std::ops::Range<usize>,
        key: &str,
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        let key_owned = key.to_string();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;

                let text = meta.get_or_create_container(CONTENT_RAW, loro::LoroText::new())?;
                text.unmark(range.clone(), &key_owned)
                    .map_err(|e| anyhow::anyhow!("LoroText unmark {key_owned}: {:?}", e))?;

                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to remove inline mark: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    /// Get a stable Loro `Cursor` at scalar offset `pos` in this block's
    /// `LoroText`. The returned cursor anchors to the character boundary
    /// according to `side` and tracks the anchor across remote text edits
    /// (Phase 0.1 spike S8/S9 confirmed: cursor pos shifts when bytes are
    /// inserted to its left, stays fixed across mark-only changes).
    ///
    /// Returns `None` if the text is empty (no anchor character to bind to).
    pub async fn text_cursor_at(
        &self,
        id: &str,
        pos: usize,
        side: loro::cursor::Side,
    ) -> Result<Option<loro::cursor::Cursor>, ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                let text = meta.get_or_create_container(CONTENT_RAW, loro::LoroText::new())?;
                Ok(text.get_cursor(pos, side))
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to get text cursor: {}", e),
            })
    }

    /// Resolve a previously-acquired cursor to its current scalar position.
    ///
    /// Errors when the cursor's anchor character was deleted concurrently
    /// AND the relative-position history has been cleared (per Loro's
    /// `CannotFindRelativePosition` taxonomy). Frontends should treat that
    /// as "selection lost" and fall back to caret = 0 or some other
    /// safe default rather than panicking.
    pub async fn text_cursor_pos(&self, cursor: &loro::cursor::Cursor) -> Result<usize, ApiError> {
        let cursor_owned = cursor.clone();
        self.collab_doc
            .with_read(move |doc| {
                let result = doc
                    .get_cursor_pos(&cursor_owned)
                    .map_err(|e| anyhow::anyhow!("get_cursor_pos: {:?}", e))?;
                Ok(result.current.pos)
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to resolve cursor position: {}", e),
            })
    }

    /// Insert `s` at Unicode-scalar offset `pos` in this block's text.
    ///
    /// Incremental complement to `update_block_text`'s wholesale replace.
    /// Marks adjust according to their `ExpandType` policy (configured once
    /// at LoroDoc creation, see `configure_text_styles`):
    /// `ExpandType::After` keys (Bold/Italic/Code/Strike/Underline/Sub/Super)
    /// extend when typed-into at the right boundary; `ExpandType::None` keys
    /// (Link/Verbatim) do not.
    ///
    /// Source blocks reject text inserts via this path — they use a separate
    /// SOURCE_CODE field. Use `update_block_text` for source blocks.
    pub async fn insert_text(&self, id: &str, pos: usize, s: &str) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        let s_owned = s.to_string();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;

                let content_type: ContentType = meta
                    .get_typed(CONTENT_TYPE, |val| val.as_string().map(|s| s.to_string()))
                    .unwrap_or_else(|| "text".to_string())
                    .parse()
                    .expect("Invalid content_type");
                if content_type == ContentType::Source {
                    return Err(anyhow::anyhow!(
                        "insert_text: source blocks edit SOURCE_CODE via update_block_text"
                    ));
                }

                let text = meta.get_or_create_container(CONTENT_RAW, loro::LoroText::new())?;
                text.insert(pos, &s_owned)
                    .map_err(|e| anyhow::anyhow!("LoroText insert at {pos}: {:?}", e))?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to insert text: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    /// Delete `len` Unicode scalars starting at `pos` in this block's text.
    ///
    /// Incremental complement to `update_block_text`. Marks that fully fall
    /// inside the deleted range are removed; marks that span the boundary
    /// shrink to the surviving portion (Loro Peritext semantics).
    pub async fn delete_text(&self, id: &str, pos: usize, len: usize) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;

                let content_type: ContentType = meta
                    .get_typed(CONTENT_TYPE, |val| val.as_string().map(|s| s.to_string()))
                    .unwrap_or_else(|| "text".to_string())
                    .parse()
                    .expect("Invalid content_type");
                if content_type == ContentType::Source {
                    return Err(anyhow::anyhow!(
                        "delete_text: source blocks edit SOURCE_CODE via update_block_text"
                    ));
                }

                let text = meta.get_or_create_container(CONTENT_RAW, loro::LoroText::new())?;
                text.delete(pos, len)
                    .map_err(|e| anyhow::anyhow!("LoroText delete {len} at {pos}: {:?}", e))?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to delete text: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    pub async fn update_block_properties(
        &self,
        id: &str,
        properties: &HashMap<String, Value>,
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                let mut existing_props = read_properties_from_meta(&meta);
                existing_props.extend(properties.clone());
                write_properties_to_meta(&meta, &existing_props)?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update block properties: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    pub async fn update_block_fields(
        &self,
        id: &str,
        fields: &[(String, Value, Value)],
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                let mut properties = read_properties_from_meta(&meta);
                for (field_name, _old_value, new_value) in fields {
                    if new_value == &Value::Null {
                        properties.remove(field_name);
                    } else {
                        properties.insert(field_name.clone(), new_value.clone());
                    }
                }
                write_properties_to_meta(&meta, &properties)?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update block fields: {}", e),
            })?;

        let block = self.get_block(id).await?;
        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    pub async fn update_parent_id(&self, id: &str, new_parent_id: String) -> Result<(), ApiError> {
        // In the LoroTree model, changing parent_id means moving the node.
        let tree_id = self.require_tree_id(id).await?;
        let new_parent_uri = EntityUri::from_raw(&new_parent_id);
        let id_cache = self.id_cache.clone();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let new_parent = resolve_parent_tree_id(&tree, &id_cache, &new_parent_uri)?;
                tree.mov(tree_id, new_parent)?;
                let meta = tree.get_meta(tree_id)?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update parent_id: {}", e),
            })?;
        Ok(())
    }

    // -- Document metadata --

    /// Set document name on a tree node. A block with a name is a document.
    pub async fn set_document_metadata(
        &self,
        tree_id_str: &str,
        name: Option<&str>,
    ) -> anyhow::Result<()> {
        let tree_id = self.resolve_to_tree_id(tree_id_str).await.ok_or_else(|| {
            anyhow::anyhow!("set_document_metadata: block not found: {}", tree_id_str)
        })?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                if let Some(n) = name {
                    meta.insert("name", loro::LoroValue::from(n))?;
                } else {
                    meta.delete("name")?;
                }
                doc.commit();
                Ok(())
            })
            .await
    }

    // -- Stable ID (block business identity) --

    /// Resolve a stable ID (UUID) to a TreeID, using the cache.
    /// Returns `None` if the stable ID is not found.
    fn resolve_stable_id_cached(&self, stable_id: &str) -> Option<loro::TreeID> {
        self.id_cache.lock().unwrap().get(stable_id).copied()
    }

    /// Insert a stable_id → TreeID mapping into the cache.
    fn cache_stable_id(&self, stable_id: &str, tree_id: loro::TreeID) {
        self.id_cache
            .lock()
            .unwrap()
            .insert(stable_id.to_string(), tree_id);
    }

    /// Remove a stable_id from the cache (on delete).
    fn uncache_stable_id(&self, stable_id: &str) {
        self.id_cache.lock().unwrap().remove(stable_id);
    }

    /// Rebuild the stable ID cache from all alive nodes in the doc.
    /// Call after `doc.import(delta)` to ensure newly imported nodes are resolvable.
    pub async fn warm_stable_id_cache(&self) {
        let id_cache = self.id_cache.clone();
        let _ = self
            .collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let mut cache = id_cache.lock().unwrap();
                cache.clear();
                for node in tree.get_nodes(false) {
                    if matches!(
                        node.parent,
                        loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                    ) {
                        continue;
                    }
                    if let Ok(meta) = tree.get_meta(node.id) {
                        if let Some(sid) = read_stable_id(&meta) {
                            cache.insert(sid, node.id);
                        }
                    }
                }
                Ok(())
            })
            .await;
    }

    // -- Diff-based CDC after remote sync --

    /// Snapshot all alive blocks keyed by stable ID. Call before `doc.import(delta)`.
    pub async fn snapshot_blocks(&self) -> HashMap<String, Block> {
        self.collab_doc
            .with_read(|doc| Ok(snapshot_blocks_from_doc(doc)))
            .await
            .unwrap_or_default()
    }

    /// Compare current state against a pre-import snapshot, emit CDC events
    /// for all Created, Updated, and Deleted blocks, and return the changes.
    /// Also warms the stable ID cache.
    ///
    /// Call after `doc.import(delta)` with the snapshot from `snapshot_blocks()`.
    pub async fn diff_and_emit_after_import(
        &self,
        before: HashMap<String, Block>,
    ) -> Vec<Change<Block>> {
        let after = self.snapshot_blocks().await;
        self.warm_stable_id_cache().await;

        let remote_origin = ChangeOrigin::Remote {
            operation_id: None,
            trace_id: None,
        };

        let mut changes = Vec::new();

        // Deleted: in before, not in after
        for (id, _block) in &before {
            if !after.contains_key(id) {
                let change = Change::Deleted {
                    id: id.clone(),
                    origin: remote_origin.clone(),
                };
                self.emit_change(change.clone());
                changes.push(change);
            }
        }

        // Created or Updated
        for (id, block) in &after {
            match before.get(id) {
                None => {
                    let change = Change::Created {
                        data: block.clone(),
                        origin: remote_origin.clone(),
                    };
                    self.emit_change(change.clone());
                    changes.push(change);
                }
                Some(old) if diff_blocks_changed(old, block) => {
                    let change = Change::Updated {
                        id: id.clone(),
                        data: block.clone(),
                        origin: remote_origin.clone(),
                    };
                    self.emit_change(change.clone());
                    changes.push(change);
                }
                _ => {} // unchanged
            }
        }

        changes
    }

    /// Find a tree node's TreeID by its stable ID (UUID).
    /// Checks cache first, falls back to linear scan + cache population.
    pub async fn find_tree_id_by_stable_id(&self, stable_id: &str) -> Option<loro::TreeID> {
        if let Some(tid) = self.resolve_stable_id_cached(stable_id) {
            return Some(tid);
        }
        let stable_id_owned = stable_id.to_string();
        let id_cache = self.id_cache.clone();
        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                for tree_node in tree.get_nodes(false) {
                    if matches!(
                        tree_node.parent,
                        loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                    ) {
                        continue;
                    }
                    if let Ok(meta) = tree.get_meta(tree_node.id) {
                        let node_stable_id =
                            meta.get_typed(STABLE_ID, |val| val.as_string().map(|s| s.to_string()));
                        if let Some(ref sid) = node_stable_id {
                            // Populate cache for every node we encounter
                            id_cache.lock().unwrap().insert(sid.clone(), tree_node.id);
                            if *sid == stable_id_owned {
                                return Ok(Some(tree_node.id));
                            }
                        }
                    }
                }
                Ok(None)
            })
            .await
            .ok()
            .flatten()
    }

    /// Resolve a block ID string to a TreeID.
    /// Accepts both `block:{peer}:{counter}` (TreeID format) and `block:{uuid}` (stable ID).
    /// Uses cache for stable ID lookups.
    pub async fn resolve_to_tree_id(&self, id_str: &str) -> Option<loro::TreeID> {
        // Fast path: try parsing as TreeID directly
        if let Some(tid) = str_to_tree_id(id_str) {
            return Some(tid);
        }
        // Slow path: resolve via stable ID
        let uri = EntityUri::from_raw(id_str);
        if uri.is_block() || uri.is_sentinel() {
            return self.find_tree_id_by_stable_id(uri.id()).await;
        }
        None
    }

    /// Resolve a block ID string to TreeID, returning ApiError::BlockNotFound on failure.
    async fn require_tree_id(&self, id: &str) -> Result<loro::TreeID, ApiError> {
        self.resolve_to_tree_id(id)
            .await
            .ok_or_else(|| ApiError::BlockNotFound { id: id.to_string() })
    }

    // -- External ID mapping (foreign entity references) --

    /// Set the external ID on a tree node's metadata.
    /// This links a Loro node to a foreign entity (e.g., Todoist task).
    /// NOT used for block identity — use `STABLE_ID` for that.
    pub async fn set_external_id(
        &self,
        tree_id_str: &str,
        external_id: &str,
    ) -> anyhow::Result<()> {
        let tree_id = str_to_tree_id(tree_id_str)
            .ok_or_else(|| anyhow::anyhow!("Invalid tree ID: {}", tree_id_str))?;

        let ext_id = external_id.to_string();
        // STABLE_ID stores the raw ID (without block: prefix) since
        // block_uri_from_meta calls EntityUri::block() which adds the prefix.
        let raw_id = external_id
            .strip_prefix("block:")
            .unwrap_or(external_id)
            .to_string();
        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                meta.insert(STABLE_ID, loro::LoroValue::from(raw_id.as_str()))?;
                meta.insert(EXTERNAL_ID, loro::LoroValue::from(ext_id.as_str()))?;
                doc.commit();
                Ok(())
            })
            .await
    }

    /// Create a root-level placeholder node without emitting events.
    /// Used by reverse sync to represent document blocks that aren't in the EventBus.
    /// The `stable_id` becomes the node's STABLE_ID and is returned as a `block:` URI.
    pub async fn create_placeholder_root(&self, stable_id: &str) -> anyhow::Result<String> {
        let sid = stable_id.to_string();
        let id_cache = self.id_cache.clone();
        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let node = tree.create(None)?;
                let meta = tree.get_meta(node)?;
                meta.insert(STABLE_ID, loro::LoroValue::from(sid.as_str()))?;
                doc.commit();
                id_cache.lock().unwrap().insert(sid.clone(), node);
                Ok(EntityUri::block(&sid).to_string())
            })
            .await
    }

    /// Find a tree node's ID string by its external (SQL) ID.
    /// Returns the `block:{peer}:{counter}` string, or None if not found.
    pub async fn find_tree_id_by_external_id(&self, external_id: &str) -> Option<String> {
        let ext_id_owned = external_id.to_string();
        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                for tree_node in tree.get_nodes(false) {
                    if matches!(
                        tree_node.parent,
                        loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                    ) {
                        continue;
                    }
                    if let Ok(meta) = tree.get_meta(tree_node.id) {
                        let ext_id = meta
                            .get_typed(EXTERNAL_ID, |val| val.as_string().map(|s| s.to_string()));
                        if ext_id.as_deref() == Some(&ext_id_owned) {
                            return Ok(Some(tree_id_to_uri(tree_node.id).to_string()));
                        }
                    }
                }
                Ok(None)
            })
            .await
            .ok() // ALLOW(ok): deleted/moved tree node
            .flatten()
    }

    /// Given a Loro TreeID URI string (`block:{peer}:{counter}`), return
    /// the external_id (SQL UUID) stored on that node, if any.
    pub async fn get_external_id(&self, tree_id_str: &str) -> Option<String> {
        let tree_id = str_to_tree_id(tree_id_str)?;

        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                let ext_id =
                    meta.get_typed(EXTERNAL_ID, |val| val.as_string().map(|s| s.to_string()));
                Ok(ext_id)
            })
            .await
            .ok() // ALLOW(ok): deleted/moved tree node
            .flatten()
    }
}

// -- Lifecycle --

#[async_trait]
impl Lifecycle for LoroBackend {
    async fn create_new(doc_id: String) -> Result<Self, ApiError>
    where
        Self: Sized,
    {
        let collab_doc = LoroDocument::new(doc_id).map_err(|e| ApiError::InternalError {
            message: format!("Failed to create document: {}", e),
        })?;
        let collab_doc = Arc::new(collab_doc);
        Self::initialize_schema(&collab_doc).await?;
        Ok(Self {
            collab_doc,
            subscribers: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            event_log: Arc::new(Mutex::new(Vec::new())),
            shared_trees: None,
            id_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn open_existing(doc_id: String) -> Result<Self, ApiError>
    where
        Self: Sized,
    {
        Self::create_new(doc_id).await
    }

    async fn dispose(&self) -> Result<(), ApiError> {
        Ok(())
    }
}

// -- ChangeNotifications --

#[async_trait]
impl ChangeNotifications<Block> for LoroBackend {
    async fn watch_changes_since(
        &self,
        position: StreamPosition,
    ) -> Pin<Box<dyn Stream<Item = std::result::Result<Vec<Change<Block>>, ApiError>> + Send>> {
        let mut replay_items = Vec::new();

        if matches!(position, StreamPosition::Beginning) {
            match self
                .collab_doc
                .with_read(|doc| {
                    let tree = doc.get_tree(TREE_NAME);
                    let mut blocks = Vec::new();
                    for tree_node in tree.get_nodes(false) {
                        if matches!(
                            tree_node.parent,
                            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                        ) {
                            continue;
                        }
                        let parent_tid = match tree_node.parent {
                            loro::TreeParentId::Node(pid) => Some(pid),
                            _ => None,
                        };
                        let block = read_block_from_tree(&tree, tree_node.id, parent_tid);
                        blocks.push(block);
                    }
                    anyhow::Ok(blocks)
                })
                .await
                .map_err(|e| ApiError::InternalError {
                    message: format!("Failed to get current blocks: {}", e),
                }) {
                Ok(current_blocks) => {
                    for block in current_blocks {
                        replay_items.push(Change::Created {
                            data: block,
                            origin: ChangeOrigin::Remote {
                                operation_id: None,
                                trace_id: None,
                            },
                        });
                    }
                }
                Err(e) => {
                    let error_stream = tokio_stream::iter(vec![Err(e)]);
                    let (_tx, rx) =
                        mpsc::channel::<std::result::Result<Vec<Change<Block>>, ApiError>>(100);
                    let live_stream = ReceiverStream::new(rx);
                    return Box::pin(error_stream.chain(live_stream));
                }
            }
        }

        let backlog = self.event_log.lock().unwrap().clone();
        replay_items.extend(backlog);

        let (tx, rx) = mpsc::channel::<std::result::Result<Vec<Change<Block>>, ApiError>>(100);
        {
            let mut subscribers = self.subscribers.lock().await;
            subscribers.push(tx);
        }

        let replay_batch = if replay_items.is_empty() {
            vec![]
        } else {
            vec![replay_items]
        };
        let replay_stream = tokio_stream::iter(replay_batch.into_iter().map(Ok));
        let live_stream = ReceiverStream::new(rx);
        Box::pin(replay_stream.chain(live_stream))
    }

    async fn get_current_version(&self) -> std::result::Result<Vec<u8>, ApiError> {
        self.collab_doc
            .with_read(|doc| Ok(doc.export(loro::ExportMode::Snapshot)?))
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to get current version: {}", e),
            })
    }
}

// -- CoreOperations --

#[async_trait]
impl CoreOperations for LoroBackend {
    async fn get_block(&self, id: &str) -> Result<Block, ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        // Try the personal tree first
        let result = self
            .collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                if is_node_alive(&tree, tree_id) {
                    let parent_tid = get_node_parent(&tree, tree_id);
                    return Ok(Some(read_block_from_tree(&tree, tree_id, parent_tid)));
                }
                Ok(None)
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to get block: {}", e),
            })?;

        if let Some(block) = result {
            return Ok(block);
        }

        // Not in personal tree — search shared trees
        if let Some(store) = &self.shared_trees {
            for stid in store.shared_tree_ids() {
                if let Some(shared_doc) = store.get_shared_doc(&stid) {
                    let tree = shared_doc.get_tree(TREE_NAME);
                    if is_node_alive(&tree, tree_id) {
                        let parent_tid = get_node_parent(&tree, tree_id);
                        return Ok(read_block_from_tree(&tree, tree_id, parent_tid));
                    }
                }
            }
        }

        Err(ApiError::BlockNotFound { id: id.to_string() })
    }

    async fn get_all_blocks(
        &self,
        traversal: super::types::Traversal,
    ) -> Result<Vec<Block>, ApiError> {
        // Collect mount node info from personal tree so we can follow them after
        let shared_trees = self.shared_trees.clone();

        let (mut result, mounts) = self
            .collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let mut blocks = Vec::new();
                let mut mount_infos = Vec::new();

                for tree_node in tree.get_nodes(false) {
                    if matches!(
                        tree_node.parent,
                        loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                    ) {
                        continue;
                    }

                    let depth = compute_depth(&tree, tree_node.parent);

                    if !traversal.includes_level(depth) {
                        continue;
                    }

                    // Check if this is a mount node — skip it and record info for later
                    if is_mount_node(&tree, tree_node.id) {
                        if let Some(info) = read_mount_info(&tree, tree_node.id) {
                            let mount_parent = match tree_node.parent {
                                loro::TreeParentId::Node(pid) => Some(pid),
                                _ => None,
                            };
                            mount_infos.push((info, mount_parent, depth));
                        }
                        continue;
                    }

                    let parent_tid = match tree_node.parent {
                        loro::TreeParentId::Node(pid) => Some(pid),
                        _ => None,
                    };
                    let block = read_block_from_tree(&tree, tree_node.id, parent_tid);
                    blocks.push(block);
                }

                Ok((blocks, mount_infos))
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to get all blocks: {}", e),
            })?;

        // Follow mount nodes into shared trees
        if let Some(store) = &shared_trees {
            for (mount_info, mount_parent, mount_depth) in &mounts {
                if let Some(shared_doc) = store.get_shared_doc(&mount_info.shared_tree_id) {
                    let shared_tree = shared_doc.get_tree(TREE_NAME);
                    collect_shared_tree_blocks(
                        &shared_tree,
                        *mount_parent,
                        *mount_depth,
                        &traversal,
                        &mut result,
                    );
                }
            }
        }

        Ok(result)
    }

    async fn list_children(&self, parent_id: &str) -> Result<Vec<String>, ApiError> {
        let shared_trees = self.shared_trees.clone();
        let id_cache = self.id_cache.clone();

        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);

                let parent_uri = EntityUri::from_raw(parent_id);
                let children_tids = if parent_uri.is_no_parent() || parent_uri.is_sentinel() {
                    tree.roots()
                } else {
                    let tree_id = uri_to_tree_id(&parent_uri)
                        .or_else(|| {
                            if parent_uri.is_block() {
                                id_cache.lock().unwrap().get(parent_uri.id()).copied()
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| {
                            anyhow::anyhow!("Cannot resolve parent_id to TreeID: {}", parent_id)
                        })?;
                    tree.children(tree_id).unwrap_or_default()
                };

                let mut result = Vec::new();
                for tid in &children_tids {
                    if is_mount_node(&tree, *tid) {
                        if let (Some(store), Some(info)) =
                            (&shared_trees, read_mount_info(&tree, *tid))
                        {
                            if let Some(shared_doc) = store.get_shared_doc(&info.shared_tree_id) {
                                let shared_tree = shared_doc.get_tree(TREE_NAME);
                                for shared_root in shared_tree.roots() {
                                    let meta = shared_tree.get_meta(shared_root)?;
                                    result
                                        .push(block_uri_from_meta(&meta, shared_root).to_string());
                                }
                                continue;
                            }
                        }
                    }
                    let meta = tree.get_meta(*tid)?;
                    result.push(block_uri_from_meta(&meta, *tid).to_string());
                }
                Ok(result)
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to list children: {}", e),
            })
    }

    async fn create_block(
        &self,
        parent_id: EntityUri,
        content: BlockContent,
        id: Option<EntityUri>,
    ) -> Result<Block, ApiError> {
        let now = Self::now_millis();
        let stable_id = match &id {
            Some(uri) => uri.id().to_string(),
            None => uuid::Uuid::new_v4().to_string(),
        };

        let id_cache = self.id_cache.clone();
        let (created_block, tree_id) = self
            .collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let parent_tree_id = resolve_parent_tree_id(&tree, &id_cache, &parent_id)?;

                let node = tree.create(parent_tree_id)?;
                let meta = tree.get_meta(node)?;
                meta.insert(STABLE_ID, loro::LoroValue::from(stable_id.as_str()))?;
                write_content_to_meta(&meta, &content, None)?;
                meta.insert("created_at", loro::LoroValue::from(now))?;
                meta.insert("updated_at", loro::LoroValue::from(now))?;
                doc.commit();

                let block_id = EntityUri::block(&stable_id);
                let parent_uri = match parent_tree_id {
                    Some(pid) => {
                        let parent_meta = tree.get_meta(pid)?;
                        Ok::<_, anyhow::Error>(block_uri_from_meta(&parent_meta, pid))
                    }
                    None => Ok(EntityUri::no_parent()),
                }?;

                let mut block = Block::from_block_content(block_id, parent_uri, content);
                block.created_at = now;
                block.updated_at = now;
                Ok((block, node))
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to create block: {}", e),
            })?;

        self.cache_stable_id(&stable_id, tree_id);

        self.emit_change(Change::Created {
            data: created_block.clone(),
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });

        Ok(created_block)
    }

    async fn update_block(&self, id: &str, content: BlockContent) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        let block_before = self.get_block(id).await?;
        let content_clone = content.clone();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                write_content_to_meta(&meta, &content, None)?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update block: {}", e),
            })?;

        let mut updated_block = block_before;
        updated_block.set_block_content(content_clone);

        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: updated_block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    async fn delete_block(&self, id: &str) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                tree.delete(tree_id)?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to delete block: {}", e),
            })?;

        let uri = EntityUri::from_raw(id);
        if uri.is_block() {
            self.uncache_stable_id(uri.id());
        }

        self.emit_change(Change::Deleted {
            id: id.to_string(),
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    async fn move_block(
        &self,
        id: &str,
        new_parent: EntityUri,
        after: Option<EntityUri>,
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        let block_before = self.get_block(id).await?;
        let id_cache = self.id_cache.clone();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let new_parent_tree_id = resolve_parent_tree_id(&tree, &id_cache, &new_parent)?;

                // LoroTree.mov handles cycle detection natively
                tree.mov(tree_id, new_parent_tree_id)?;

                // Handle `after` positioning via mov_after
                if let Some(after_uri) = &after {
                    if let Some(after_tid) = uri_to_tree_id(after_uri) {
                        tree.mov_after(tree_id, after_tid)?;
                    }
                }

                let meta = tree.get_meta(tree_id)?;
                meta.insert("updated_at", loro::LoroValue::from(Self::now_millis()))?;
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to move block: {}", e),
            })?;

        let mut moved_block = block_before;
        moved_block.parent_id = new_parent;

        self.emit_change(Change::Updated {
            id: id.to_string(),
            data: moved_block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    async fn get_blocks(&self, ids: Vec<String>) -> Result<Vec<Block>, ApiError> {
        let mut tree_ids = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(tid) = self.resolve_to_tree_id(id).await {
                tree_ids.push(tid);
            }
        }
        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let mut blocks = Vec::new();
                for tid in tree_ids {
                    if is_node_alive(&tree, tid) {
                        let parent_tid = get_node_parent(&tree, tid);
                        blocks.push(read_block_from_tree(&tree, tid, parent_tid));
                    }
                }
                Ok(blocks)
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to get blocks: {}", e),
            })
    }

    async fn create_blocks(&self, blocks: Vec<NewBlock>) -> Result<Vec<Block>, ApiError> {
        let now = Self::now_millis();

        let id_cache = self.id_cache.clone();
        let created_blocks = self
            .collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let mut created = Vec::new();
                let mut id_cache_entries: Vec<(String, loro::TreeID)> = Vec::new();

                for new_block in blocks {
                    let parent_tree_id =
                        resolve_parent_tree_id(&tree, &id_cache, &new_block.parent_id)?;
                    let stable_id = match &new_block.id {
                        Some(uri) => uri.id().to_string(),
                        None => uuid::Uuid::new_v4().to_string(),
                    };
                    let node = tree.create(parent_tree_id)?;
                    let meta = tree.get_meta(node)?;
                    meta.insert(STABLE_ID, loro::LoroValue::from(stable_id.as_str()))?;
                    write_content_to_meta(
                        &meta,
                        &new_block.content,
                        new_block.content_type_override,
                    )?;
                    meta.insert("created_at", loro::LoroValue::from(now))?;
                    meta.insert("updated_at", loro::LoroValue::from(now))?;

                    // Handle `after` positioning
                    if let Some(after_uri) = &new_block.after {
                        if let Some(after_tid) = uri_to_tree_id(after_uri) {
                            tree.mov_after(node, after_tid)?;
                        }
                    }

                    id_cache_entries.push((stable_id.clone(), node));

                    let block_id = EntityUri::block(&stable_id);
                    let parent_uri = match parent_tree_id {
                        Some(pid) => {
                            let parent_meta = tree.get_meta(pid)?;
                            block_uri_from_meta(&parent_meta, pid)
                        }
                        None => new_block.parent_id.clone(),
                    };

                    let mut block =
                        Block::from_block_content(block_id, parent_uri, new_block.content);
                    block.created_at = now;
                    block.updated_at = now;
                    created.push(block);
                }

                doc.commit();
                {
                    let mut cache = id_cache.lock().unwrap();
                    for (sid, tid) in id_cache_entries {
                        cache.insert(sid, tid);
                    }
                }
                Ok(created)
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to create blocks: {}", e),
            })?;

        for block in &created_blocks {
            self.emit_change(Change::Created {
                data: block.clone(),
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            });
        }

        Ok(created_blocks)
    }

    async fn delete_blocks(&self, ids: Vec<String>) -> Result<(), ApiError> {
        let mut seen = std::collections::HashSet::new();
        let unique_ids: Vec<_> = ids
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect();
        let mut resolved = Vec::with_capacity(unique_ids.len());
        for id in &unique_ids {
            let tid = self.require_tree_id(id).await?;
            resolved.push(tid);
        }

        self.collab_doc
            .with_write(move |doc| {
                let tree = doc.get_tree(TREE_NAME);
                for tid in &resolved {
                    tree.delete(*tid)?;
                }
                doc.commit();
                Ok(())
            })
            .await
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to delete blocks: {}", e),
            })?;

        for id in &unique_ids {
            let uri = EntityUri::from_raw(id);
            if uri.is_block() {
                self.uncache_stable_id(uri.id());
            }
        }

        for id in unique_ids {
            self.emit_change(Change::Deleted {
                id,
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            });
        }

        Ok(())
    }
}

// -- P2POperations (stubs) --

#[async_trait]
impl P2POperations for LoroBackend {
    async fn get_node_id(&self) -> String {
        "local-only".to_string()
    }

    async fn connect_to_peer(&self, _peer_node_id: String) -> Result<(), ApiError> {
        Err(ApiError::NetworkError {
            message: "P2P sync requires IrohSyncAdapter (not wired to LoroBackend)".to_string(),
        })
    }

    async fn accept_connections(&self) -> Result<(), ApiError> {
        Err(ApiError::NetworkError {
            message: "P2P sync requires IrohSyncAdapter (not wired to LoroBackend)".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::repository::{CoreOperations, Lifecycle};

    async fn create_test_backend() -> LoroBackend {
        LoroBackend::create_new("test-doc".to_string())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn create_and_get_block() {
        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "Hello".into(),
                },
                None,
            )
            .await
            .unwrap();

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        assert_eq!(fetched.content_text(), "Hello");
    }

    #[tokio::test]
    async fn marks_round_trip_through_loro() {
        // Phase 1.3 verification: apply marks via update_block_marked, then
        // get_block returns Block.marks reconstructed from Loro Peritext.
        use holon_api::{InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "hello world".into(),
                },
                None,
            )
            .await
            .unwrap();

        let marks = vec![
            MarkSpan::new(0, 5, InlineMark::Bold),
            MarkSpan::new(6, 11, InlineMark::Italic),
        ];
        backend
            .update_block_marked(block.id.as_str(), "hello world", &marks)
            .await
            .expect("update_block_marked");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        assert_eq!(fetched.content, "hello world");
        let got = fetched.marks.expect("marks projected");
        // Order is not guaranteed by the delta walk; sort for comparison.
        let mut got_sorted = got;
        got_sorted.sort_by_key(|m| (m.start, m.end));
        assert_eq!(got_sorted, marks);
    }

    #[tokio::test]
    async fn marks_round_trip_with_link() {
        use holon_api::{EntityRef, EntityUri as Uri, InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "click here please".into(),
                },
                None,
            )
            .await
            .unwrap();

        let link_mark = InlineMark::Link {
            target: EntityRef::Internal {
                id: Uri::block("abc-123"),
            },
            label: "here".to_string(),
        };
        let marks = vec![MarkSpan::new(6, 10, link_mark.clone())];
        backend
            .update_block_marked(block.id.as_str(), "click here please", &marks)
            .await
            .expect("update_block_marked link");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        let got = fetched.marks.expect("marks projected");
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].start, got[0].end), (6, 10));
        assert_eq!(got[0].mark, link_mark);
    }

    #[tokio::test]
    async fn marks_replace_clears_old() {
        // update_block_marked is wholesale replace: setting marks=[] should
        // clear any previously-applied marks (per the documented semantics).
        use holon_api::{InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "abcde".into(),
                },
                None,
            )
            .await
            .unwrap();

        backend
            .update_block_marked(
                block.id.as_str(),
                "abcde",
                &[MarkSpan::new(0, 3, InlineMark::Bold)],
            )
            .await
            .expect("apply bold");
        let with_bold = backend.get_block(block.id.as_str()).await.unwrap();
        assert!(with_bold.marks.as_ref().is_some_and(|m| !m.is_empty()));

        backend
            .update_block_marked(block.id.as_str(), "abcde", &[])
            .await
            .expect("clear marks");
        let cleared = backend.get_block(block.id.as_str()).await.unwrap();
        // After clearing, the LoroText still exists (so read_text_marks
        // returns None for empty), and Block.marks is None.
        assert!(
            cleared.marks.is_none(),
            "expected None after clear, got {:?}",
            cleared.marks
        );
    }

    #[tokio::test]
    async fn config_text_style_installed_at_doc_creation() {
        // Smoke: config_text_style is invoked once per LoroDocument::new,
        // so a freshly-created backend can mark text without panicking.
        // (Phase 0.1 spike S3: re-config is silent no-op.)
        use holon_api::{InlineMark, MarkSpan};
        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "x".into() },
                None,
            )
            .await
            .unwrap();
        backend
            .update_block_marked(
                block.id.as_str(),
                "x",
                &[MarkSpan::new(0, 1, InlineMark::Bold)],
            )
            .await
            .expect("mark on freshly-configured doc");
    }

    /// Reproducer for the share-flow finding from `share_subtree_pbt`:
    /// when a fresh `LoroDoc` has `configure_text_styles` called and then
    /// imports a snapshot from another doc, can subsequent `LoroText::mark`
    /// calls round-trip via `read_marks_from_text`?
    ///
    /// This isolates whether the share-flow mark loss is in the "fresh
    /// doc + import + mark + read" pipeline (this test) vs. somewhere
    /// else in the share machinery (advertiser, manager, sync workers).
    #[tokio::test]
    async fn mark_after_import_into_configured_doc_round_trips() {
        // Source doc — applies a mark, exports.
        let src = loro::LoroDoc::new();
        configure_text_styles(&src);
        let src_text = src.get_text("body");
        src_text.insert(0, "hello world").unwrap();
        src.commit();
        let snapshot = src.export(loro::ExportMode::Snapshot).expect("export");

        // Fresh receiver — configure first, then import the snapshot.
        // This is the exact pattern in `accept_shared_subtree`.
        let dst = loro::LoroDoc::new();
        configure_text_styles(&dst);
        dst.import(&snapshot).expect("import");
        dst.commit();

        // After import, apply a Bold mark to "hello" on the receiver and
        // read it back.
        let dst_text = dst.get_text("body");
        let value = mark_to_loro_value(&holon_api::InlineMark::Bold);
        dst_text.mark(0..5, "bold", value).expect("mark");
        dst.commit();

        let marks = read_marks_from_text(&dst_text);
        assert!(
            !marks.is_empty(),
            "Bold mark applied after import should be readable; got empty marks"
        );
        assert_eq!(marks.len(), 1);
        assert_eq!((marks[0].start, marks[0].end), (0, 5));
        assert_eq!(marks[0].mark, holon_api::InlineMark::Bold);
    }

    #[tokio::test]
    async fn apply_inline_mark_preserves_other_marks() {
        // Phase 3.1a: incremental apply must not nuke pre-existing marks (the
        // forcing function for not regressing back to wholesale replace).
        use holon_api::{InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "hello world".into(),
                },
                None,
            )
            .await
            .unwrap();

        backend
            .update_block_marked(
                block.id.as_str(),
                "hello world",
                &[MarkSpan::new(0, 5, InlineMark::Bold)],
            )
            .await
            .expect("seed bold");

        backend
            .apply_inline_mark(block.id.as_str(), 6..11, &InlineMark::Italic)
            .await
            .expect("apply italic");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        let mut got = fetched.marks.expect("marks present");
        got.sort_by_key(|m| (m.start, m.end));
        assert_eq!(
            got,
            vec![
                MarkSpan::new(0, 5, InlineMark::Bold),
                MarkSpan::new(6, 11, InlineMark::Italic),
            ]
        );
    }

    #[tokio::test]
    async fn remove_inline_mark_splits_overlapping_span() {
        // Phase 3.1a: removing a mark over an interior subrange must split
        // the existing span, leaving the disjoint portions intact.
        use holon_api::{InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "abcdefghij".into(),
                },
                None,
            )
            .await
            .unwrap();

        backend
            .update_block_marked(
                block.id.as_str(),
                "abcdefghij",
                &[MarkSpan::new(0, 10, InlineMark::Bold)],
            )
            .await
            .expect("seed bold over full range");

        backend
            .remove_inline_mark(block.id.as_str(), 3..6, "bold")
            .await
            .expect("remove bold over 3..6");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        let mut got = fetched.marks.expect("marks present");
        got.sort_by_key(|m| (m.start, m.end));
        assert_eq!(
            got,
            vec![
                MarkSpan::new(0, 3, InlineMark::Bold),
                MarkSpan::new(6, 10, InlineMark::Bold),
            ]
        );
    }

    #[tokio::test]
    async fn text_cursor_tracks_remote_inserts() {
        // Phase 3.1a: cursor at scalar pos=5; after a 3-char insert at pos 0
        // the cursor's resolved pos becomes 8. (Phase 0.1 spike S8 already
        // proved this against bare Loro; this test smokes our wrapper.)
        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "0123456789".into(),
                },
                None,
            )
            .await
            .unwrap();

        let cursor = backend
            .text_cursor_at(block.id.as_str(), 5, loro::cursor::Side::Middle)
            .await
            .expect("text_cursor_at")
            .expect("cursor for non-empty text");
        assert_eq!(
            backend
                .text_cursor_pos(&cursor)
                .await
                .expect("cursor pos initial"),
            5
        );

        backend
            .update_block_text(block.id.as_str(), "abc0123456789")
            .await
            .expect("prepend abc");

        // After the prepend, the anchored cursor's resolved pos shifts by 3.
        let resolved = backend
            .text_cursor_pos(&cursor)
            .await
            .expect("cursor pos after insert");
        assert_eq!(resolved, 8, "cursor should track to scalar pos 8");
    }

    #[tokio::test]
    async fn text_cursor_at_empty_text_resolves_to_pos_zero() {
        // Locks Loro 1.11 semantics: get_cursor on an empty container still
        // returns a Cursor (with id=None internally), which resolves to pos=0
        // until the text grows. The editor relies on this so a freshly-empty
        // block can park its caret without special-casing.
        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "".into() },
                None,
            )
            .await
            .unwrap();

        let cursor = backend
            .text_cursor_at(block.id.as_str(), 0, loro::cursor::Side::Middle)
            .await
            .expect("text_cursor_at on empty");
        let cursor = cursor.expect("cursor returned even for empty text");
        let pos = backend
            .text_cursor_pos(&cursor)
            .await
            .expect("resolves on empty text");
        assert_eq!(pos, 0);
    }

    #[tokio::test]
    async fn insert_text_at_start_middle_end() {
        // Phase 3.2a: incremental inserts compose at any position.
        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "world".into(),
                },
                None,
            )
            .await
            .unwrap();

        backend
            .insert_text(block.id.as_str(), 0, "hello ")
            .await
            .expect("insert at start");
        assert_eq!(
            backend.get_block(block.id.as_str()).await.unwrap().content,
            "hello world"
        );

        // 'hello world'.len() in chars = 11; insert "!" at the end.
        backend
            .insert_text(block.id.as_str(), 11, "!")
            .await
            .expect("insert at end");
        assert_eq!(
            backend.get_block(block.id.as_str()).await.unwrap().content,
            "hello world!"
        );

        // Insert ", brave" at scalar pos 5 (between "hello" and " world!").
        backend
            .insert_text(block.id.as_str(), 5, ", brave")
            .await
            .expect("insert in middle");
        assert_eq!(
            backend.get_block(block.id.as_str()).await.unwrap().content,
            "hello, brave world!"
        );
    }

    #[tokio::test]
    async fn insert_text_extends_after_expanding_mark() {
        // Phase 3.2a: ExpandType::After keys (Bold) extend on right-boundary
        // insert. This proves config_text_style is honored end-to-end (the
        // Phase 0.1 spike verified bare Loro; this verifies our wrapper).
        use holon_api::{InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "hello".into(),
                },
                None,
            )
            .await
            .unwrap();

        backend
            .update_block_marked(
                block.id.as_str(),
                "hello",
                &[MarkSpan::new(0, 5, InlineMark::Bold)],
            )
            .await
            .expect("seed bold");

        // Insert " world" at scalar pos 5 (right boundary of Bold).
        backend
            .insert_text(block.id.as_str(), 5, " world")
            .await
            .expect("insert at boundary");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        assert_eq!(fetched.content, "hello world");
        let marks = fetched.marks.expect("bold survives");
        assert_eq!(marks.len(), 1);
        // Bold should now span [0, 11) — extended by 6 from the inserted " world".
        assert_eq!((marks[0].start, marks[0].end), (0, 11));
    }

    #[tokio::test]
    async fn insert_text_does_not_extend_no_expand_link() {
        // Phase 3.2a: ExpandType::None keys (Link) do NOT extend on right-
        // boundary insert.
        use holon_api::{EntityRef, EntityUri as Uri, InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "click".into(),
                },
                None,
            )
            .await
            .unwrap();

        let link = InlineMark::Link {
            target: EntityRef::Internal {
                id: Uri::block("xyz"),
            },
            label: "click".into(),
        };
        backend
            .update_block_marked(block.id.as_str(), "click", &[MarkSpan::new(0, 5, link)])
            .await
            .expect("seed link");

        backend
            .insert_text(block.id.as_str(), 5, " here")
            .await
            .expect("insert past link boundary");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        assert_eq!(fetched.content, "click here");
        let marks = fetched.marks.expect("link survives");
        assert_eq!(marks.len(), 1);
        // Link does NOT extend — still [0, 5).
        assert_eq!((marks[0].start, marks[0].end), (0, 5));
    }

    #[tokio::test]
    async fn insert_text_handles_multibyte_scalars() {
        // Phase 3.2a: positions are Unicode scalars, not bytes — multibyte
        // chars don't break the offset model.
        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "ab".into() },
                None,
            )
            .await
            .unwrap();

        // Insert "你好" at scalar position 1 (between 'a' and 'b').
        backend
            .insert_text(block.id.as_str(), 1, "你好")
            .await
            .expect("insert multibyte");
        assert_eq!(
            backend.get_block(block.id.as_str()).await.unwrap().content,
            "a你好b"
        );

        // Delete the two CJK chars at scalar offset 1, length 2.
        backend
            .delete_text(block.id.as_str(), 1, 2)
            .await
            .expect("delete multibyte");
        assert_eq!(
            backend.get_block(block.id.as_str()).await.unwrap().content,
            "ab"
        );
    }

    #[tokio::test]
    async fn delete_text_preserves_marks_on_disjoint_regions() {
        // Phase 3.2a: deleting an interior subrange shrinks marks that
        // fully cover it; marks on disjoint regions survive untouched.
        use holon_api::{InlineMark, MarkSpan};

        let backend = create_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "abcdefghij".into(),
                },
                None,
            )
            .await
            .unwrap();

        backend
            .update_block_marked(
                block.id.as_str(),
                "abcdefghij",
                &[
                    MarkSpan::new(0, 4, InlineMark::Bold),    // "abcd"
                    MarkSpan::new(6, 10, InlineMark::Italic), // "ghij"
                ],
            )
            .await
            .expect("seed marks");

        // Delete "ef" (scalars 4..6) — the gap between Bold and Italic.
        backend
            .delete_text(block.id.as_str(), 4, 2)
            .await
            .expect("delete gap");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        assert_eq!(fetched.content, "abcdghij");
        let mut marks = fetched.marks.expect("marks survive");
        marks.sort_by_key(|m| (m.start, m.end));
        // Bold [0..4) keeps its position; Italic [6..10) shifts left by 2.
        assert_eq!(
            marks,
            vec![
                MarkSpan::new(0, 4, InlineMark::Bold),
                MarkSpan::new(4, 8, InlineMark::Italic),
            ]
        );
    }

    #[tokio::test]
    async fn create_nested_and_list_children() {
        let backend = create_test_backend().await;
        let root = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "root".into() },
                None,
            )
            .await
            .unwrap();

        let child = backend
            .create_block(
                root.id.clone(),
                BlockContent::Text {
                    raw: "child".into(),
                },
                None,
            )
            .await
            .unwrap();

        let children = backend.list_children(root.id.as_str()).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], child.id.to_string());
    }

    #[tokio::test]
    async fn get_all_blocks_includes_root_blocks() {
        let backend = create_test_backend().await;
        let root = backend
            .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
            .await
            .unwrap();
        let child = backend
            .create_block(root.id.clone(), BlockContent::text("child"), None)
            .await
            .unwrap();

        let all = backend
            .get_all_blocks(crate::api::types::Traversal::ALL_BUT_ROOT)
            .await
            .unwrap();
        let ids: Vec<_> = all.iter().map(|b| b.id.to_string()).collect();
        assert!(
            ids.contains(&root.id.to_string()),
            "Root block should be in ALL_BUT_ROOT (depth 1). Got: {ids:?}"
        );
        assert!(
            ids.contains(&child.id.to_string()),
            "Child block should be in ALL_BUT_ROOT (depth 2). Got: {ids:?}"
        );
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn move_block_cycle_rejected() {
        let backend = create_test_backend().await;
        let a = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "A".into() },
                None,
            )
            .await
            .unwrap();
        let b = backend
            .create_block(a.id.clone(), BlockContent::Text { raw: "B".into() }, None)
            .await
            .unwrap();

        let result = backend.move_block(a.id.as_str(), b.id.clone(), None).await;
        assert!(result.is_err(), "Moving parent under child should fail");
    }

    #[tokio::test]
    async fn move_block_valid() {
        let backend = create_test_backend().await;
        let a = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "A".into() },
                None,
            )
            .await
            .unwrap();
        let b = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "B".into() },
                None,
            )
            .await
            .unwrap();

        backend
            .move_block(b.id.as_str(), a.id.clone(), None)
            .await
            .unwrap();
        let children = backend.list_children(a.id.as_str()).await.unwrap();
        assert_eq!(children.len(), 1);
    }

    #[tokio::test]
    async fn delete_block_hides_it() {
        let backend = create_test_backend().await;
        let root = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "root".into() },
                None,
            )
            .await
            .unwrap();
        let child = backend
            .create_block(
                root.id.clone(),
                BlockContent::Text {
                    raw: "child".into(),
                },
                None,
            )
            .await
            .unwrap();

        backend.delete_block(child.id.as_str()).await.unwrap();
        let children = backend.list_children(root.id.as_str()).await.unwrap();
        assert_eq!(children.len(), 0);
    }

    // Diagnostic tests removed — the PBT state machine test validates this end-to-end.

    #[tokio::test]
    #[ignore] // Kept as reference but not run routinely
    async fn pbt_simulated_two_transitions() {
        use crate::api::memory_backend::MemoryBackend;
        use crate::api::pbt_infrastructure::*;
        use crate::api::repository::Lifecycle;
        use crate::api::types::Traversal;

        let mem = MemoryBackend::create_new("ref".to_string()).await.unwrap();
        let loro = create_test_backend().await;
        let mut id_map = std::collections::HashMap::new();

        // Transition 1: CreateBlock at root
        let t1 = BlockTransition::CreateBlock {
            parent_id: "sentinel:no_parent".to_string(),
            content: "ju".to_string(),
        };
        let _ref_created = apply_transition(&mem, &t1).await.unwrap();
        let sut_t1 = translate_transition(&t1, &id_map);
        let sut_created = apply_transition(&loro, &sut_t1).await.unwrap();
        let ref_blocks = mem.get_all_blocks(Traversal::ALL_BUT_ROOT).await.unwrap();
        update_id_map_after_create(&mut id_map, &t1, &ref_blocks, &sut_created);
        tracing::debug!("After t1: id_map = {:?}", id_map);

        // Transition 2: CreateBlocks under local://0
        let t2 = BlockTransition::CreateBlocks {
            blocks: vec![
                ("local://0".to_string(), "hxhwz".to_string()),
                ("local://0".to_string(), "oppetb".to_string()),
            ],
        };
        let _ref_created2 = apply_transition(&mem, &t2).await.unwrap();
        let sut_t2 = translate_transition(&t2, &id_map);
        let sut_created2 = apply_transition(&loro, &sut_t2).await.unwrap();
        tracing::debug!("SUT created {} blocks in t2", sut_created2.len());

        // Update id_map for t2
        let ref_blocks2 = mem.get_all_blocks(Traversal::ALL_BUT_ROOT).await.unwrap();
        update_id_map_after_create(&mut id_map, &t2, &ref_blocks2, &sut_created2);

        // Compare block counts and content
        let ref_all = mem.get_all_blocks(Traversal::ALL_BUT_ROOT).await.unwrap();
        let sut_all = loro.get_all_blocks(Traversal::ALL_BUT_ROOT).await.unwrap();
        assert_eq!(
            ref_all.len(),
            sut_all.len(),
            "Block count mismatch: ref={}, sut={}",
            ref_all.len(),
            sut_all.len()
        );

        let mut ref_contents: Vec<_> = ref_all
            .iter()
            .map(|b| b.content_text().to_string())
            .collect();
        let mut sut_contents: Vec<_> = sut_all
            .iter()
            .map(|b| b.content_text().to_string())
            .collect();
        ref_contents.sort();
        sut_contents.sort();
        assert_eq!(ref_contents, sut_contents, "Content mismatch");
    }

    #[tokio::test]
    #[ignore]
    async fn pbt_debug_id_mapping() {
        use crate::api::memory_backend::MemoryBackend;
        use crate::api::pbt_infrastructure::*;
        use crate::api::repository::Lifecycle;
        use crate::api::types::Traversal;

        let mem = MemoryBackend::create_new("ref".to_string()).await.unwrap();
        let loro = create_test_backend().await;

        // Create root block on both
        let mem_block = mem
            .create_block(EntityUri::no_parent(), BlockContent::text("ju"), None)
            .await
            .unwrap();
        let loro_block = loro
            .create_block(EntityUri::no_parent(), BlockContent::text("ju"), None)
            .await
            .unwrap();

        tracing::debug!("MemoryBackend block id: {}", mem_block.id);
        tracing::debug!("LoroBackend block id: {}", loro_block.id);
        tracing::debug!("MemoryBackend parent_id: {}", mem_block.parent_id);
        tracing::debug!("LoroBackend parent_id: {}", loro_block.parent_id);

        // Simulate update_id_map_after_create
        let mut id_map = std::collections::HashMap::new();
        let ref_blocks = mem.get_all_blocks(Traversal::ALL_BUT_ROOT).await.unwrap();
        tracing::debug!("ref_blocks count: {}", ref_blocks.len());
        for b in &ref_blocks {
            tracing::debug!(
                "  ref block: id={}, parent={}, content={}",
                b.id,
                b.parent_id,
                b.content_text()
            );
        }

        let transition = BlockTransition::CreateBlock {
            parent_id: "sentinel:no_parent".to_string(),
            content: "ju".to_string(),
        };
        update_id_map_after_create(&mut id_map, &transition, &ref_blocks, &[loro_block.clone()]);
        tracing::debug!("id_map after: {:?}", id_map);

        assert!(
            id_map.contains_key(mem_block.id.as_str()),
            "Should map mem ID to loro ID"
        );

        // Now translate a transition using the map
        let create_child = BlockTransition::CreateBlock {
            parent_id: mem_block.id.to_string(),
            content: "child".to_string(),
        };
        let translated = translate_transition(&create_child, &id_map);
        match &translated {
            BlockTransition::CreateBlock { parent_id, .. } => {
                assert_eq!(
                    parent_id,
                    loro_block.id.as_str(),
                    "Parent should be translated to LoroBackend ID"
                );
            }
            _ => panic!("unexpected"),
        }
    }

    // -- Mount-node traversal tests --

    mod mount_traversal {
        use super::*;
        use crate::sync::shared_tree::{HistoryRetention, InMemorySharedTreeStore, share_subtree};

        /// Set up a personal tree with a shared subtree mounted in it.
        /// Returns (backend, shared_tree_id, ids of blocks in shared tree).
        async fn setup_with_mount() -> (LoroBackend, String, Vec<String>) {
            let backend = create_test_backend().await;

            // Create personal tree:
            //   doc_root
            //     +-- kept_heading ("Kept")
            //     +-- shared_heading ("Shared heading")
            //         +-- shared_child ("Shared child")
            let doc_root = backend
                .create_block(EntityUri::no_parent(), BlockContent::text("doc_root"), None)
                .await
                .unwrap();
            let doc_root_tid = backend
                .find_tree_id_by_stable_id(doc_root.id.id())
                .await
                .unwrap();
            backend
                .collab_doc
                .with_write(|doc| {
                    let tree = doc.get_tree(TREE_NAME);
                    let meta = tree.get_meta(doc_root_tid)?;
                    meta.insert("name", "test_doc")?;
                    Ok(())
                })
                .await
                .unwrap();

            let _kept = backend
                .create_block(doc_root.id.clone(), BlockContent::text("Kept"), None)
                .await
                .unwrap();
            let shared_heading = backend
                .create_block(
                    doc_root.id.clone(),
                    BlockContent::text("Shared heading"),
                    None,
                )
                .await
                .unwrap();
            let shared_child = backend
                .create_block(
                    shared_heading.id.clone(),
                    BlockContent::text("Shared child"),
                    None,
                )
                .await
                .unwrap();

            let shared_heading_tid = backend
                .find_tree_id_by_stable_id(shared_heading.id.id())
                .await
                .unwrap();

            // Share the subtree
            let stid = "test-collab-1".to_string();
            let share_result = backend
                .collab_doc
                .with_write(|doc| {
                    share_subtree(
                        doc,
                        shared_heading_tid,
                        Some(doc_root_tid),
                        stid.clone(),
                        HistoryRetention::Full,
                    )
                })
                .await
                .unwrap();

            // Register shared doc in store and attach to backend
            let mut store = InMemorySharedTreeStore::new();
            store.insert(stid.clone(), share_result.extracted.shared_doc);
            let mut backend = backend;
            backend.set_shared_trees(Arc::new(store));

            let shared_block_ids = vec![shared_heading.id.to_string(), shared_child.id.to_string()];

            (backend, stid, shared_block_ids)
        }

        #[tokio::test]
        async fn get_block_finds_block_in_shared_tree() {
            let (backend, _stid, shared_ids) = setup_with_mount().await;

            // The shared heading should be findable via get_block
            let block = backend.get_block(&shared_ids[0]).await.unwrap();
            assert_eq!(block.content_text(), "Shared heading");

            let child = backend.get_block(&shared_ids[1]).await.unwrap();
            assert_eq!(child.content_text(), "Shared child");
        }

        #[tokio::test]
        async fn get_block_still_finds_personal_blocks() {
            let (backend, _stid, _shared_ids) = setup_with_mount().await;

            // "Kept" block should still be accessible
            let all = backend
                .get_all_blocks(crate::api::types::Traversal::ALL_BUT_ROOT)
                .await
                .unwrap();
            let kept = all.iter().find(|b| b.content_text() == "Kept");
            assert!(kept.is_some(), "Personal block 'Kept' should be accessible");
        }

        #[tokio::test]
        async fn get_all_blocks_includes_shared_tree_blocks() {
            let (backend, _stid, _shared_ids) = setup_with_mount().await;

            let all = backend
                .get_all_blocks(crate::api::types::Traversal::ALL_BUT_ROOT)
                .await
                .unwrap();
            let contents: Vec<_> = all.iter().map(|b| b.content_text().to_string()).collect();

            // Should include personal blocks AND shared tree blocks
            assert!(
                contents.contains(&"doc_root".to_string()),
                "Should include doc_root. Got: {contents:?}"
            );
            assert!(
                contents.contains(&"Kept".to_string()),
                "Should include Kept. Got: {contents:?}"
            );
            assert!(
                contents.contains(&"Shared heading".to_string()),
                "Should include shared heading. Got: {contents:?}"
            );
            assert!(
                contents.contains(&"Shared child".to_string()),
                "Should include shared child. Got: {contents:?}"
            );

            // Mount node itself should NOT be in the results
            let has_mount = all
                .iter()
                .any(|b| b.properties_map().get("mount_kind").is_some());
            assert!(!has_mount, "Mount node should not appear in results");
        }

        #[tokio::test]
        async fn list_children_follows_mount_node() {
            let (backend, _stid, shared_ids) = setup_with_mount().await;

            // Get doc_root's children — should include Kept + shared tree root
            // (mount node replaced by shared heading)
            let all = backend
                .get_all_blocks(crate::api::types::Traversal::ALL_BUT_ROOT)
                .await
                .unwrap();
            let doc_root = all.iter().find(|b| b.content_text() == "doc_root").unwrap();

            let children = backend.list_children(doc_root.id.as_str()).await.unwrap();

            // Should have 2 children: "Kept" block + shared tree root (replacing mount)
            assert_eq!(
                children.len(),
                2,
                "doc_root should have 2 children (kept + shared root). Got: {children:?}"
            );

            // One of them should be the shared heading
            assert!(
                children.contains(&shared_ids[0]),
                "Children should include shared heading ID {}. Got: {children:?}",
                shared_ids[0]
            );
        }

        #[tokio::test]
        async fn get_block_returns_not_found_for_nonexistent() {
            let (backend, _stid, _shared_ids) = setup_with_mount().await;

            let result = backend.get_block("block:999:999").await;
            assert!(result.is_err());
        }
    }
}
