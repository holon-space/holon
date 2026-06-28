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

use crate::LoroDocument;
use crate::event_ring::{DEFAULT_EVENT_RING_CAPACITY, EventRing, deliver_to_subscribers};
use crate::shared_tree::{SharedTreeStore, is_mount_node, read_mount_info};
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::block_mutation::{BlockMutation, BlockTreeView};
use holon_api::repository::NewBlock;
use holon_api::repository::{CoreOperations, Lifecycle, P2POperations};
use holon_api::streaming::{ChangeNotifications, ChangeSubscribers};
use holon_api::{
    ApiError, Block, BlockContent, Change, ChangeOrigin, ContentType, SourceBlock, StreamPosition,
    Tags, Value,
};
use holon_core::fractional_index::default_sort_key;
use std::collections::{HashMap, HashSet};
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
                        // ALLOW(entity_uri_from_raw): internal-link target id from Loro mark map field
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

    // Overlapping runs close in `HashMap` iteration order; canonicalize so the
    // result compares equal to the SQL-stored mark set for the same block.
    holon_api::canonicalize_marks(&mut marks);
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
pub trait LoroMapExt {
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
    // ALLOW(entity_uri_from_raw): str_to_tree_id(&str) backend string-id resolve surface
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
    let mut props: HashMap<String, Value> =
        match meta.get_typed(PROPERTIES, |val| val.as_string().map(|s| s.to_string())) {
            Some(json) => serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("Corrupt properties JSON in Loro tree: {json:?}: {e}")),
            None => HashMap::new(),
        };
    // Edge-typed fields (`tags`, `requires`) are stored in dedicated meta keys
    // and typed `Block` slots — never in the generic PROPERTIES blob. Strip any
    // that leaked in (legacy pollution from an older build that flattened
    // `requires` into PROPERTIES, e.g. as the debug string `"Array([])"`).
    // Stripping here — the single PROPERTIES read boundary — keeps them out of
    // params (avoids the `SqlOperationProvider` edge-partition panic) AND
    // self-heals storage: the read-merge-write update paths re-persist without
    // the stray keys.
    props.remove("tags");
    props.remove("requires");
    props
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
    // `read_properties_from_meta` already strips edge-typed keys (`tags`,
    // `requires`) that legacy pollution may have flattened into the PROPERTIES
    // blob — they live in dedicated meta keys + typed `Block` slots instead.
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

    let tags = read_tags_from_meta(&meta);
    let requires = read_requires_from_meta(&meta);

    let mut block = Block::from_block_content(id, parent_id, content);
    block.set_properties_map(properties);
    block.tags = tags.into();
    block.requires = requires;
    block.created_at = created_at;
    block.updated_at = updated_at;
    block
}

// `SnapshotBlock` is a backend-neutral data type (a `Block` + its fractional
// `sort_key`); it now lives in `holon-api` (along with the `SnapshotBlockWire`
// lossless-serde representation — BUG H1). The Loro adapter still builds it here
// from a `LoroDoc` (see `snapshot_blocks_from_doc*`), but the type itself is
// shared. Re-exported so `crate::loro_backend::SnapshotBlock` keeps resolving.
pub use holon_api::SnapshotBlock;

/// Read the `tags` JSON-encoded list from a node's metadata. Returns an empty
/// `Vec` when the key is absent ("no tags"). Malformed JSON in a present value
/// is a corruption of metadata we wrote ourselves — fail loud rather than
/// silently dropping the tags.
fn read_tags_from_meta(meta: &loro::LoroMap) -> Vec<String> {
    meta.get_typed("tags", |val| val.as_string().map(|s| s.to_string()))
        .map(|s| {
            serde_json::from_str::<Vec<String>>(&s)
                .unwrap_or_else(|e| panic!("corrupt `tags` metadata JSON {s:?}: {e}"))
        })
        .unwrap_or_default()
}

/// Read the `requires` JSON-encoded list (org-edna dependency edge field) from a
/// node's metadata. Stored under a dedicated `requires` meta key — like `tags`,
/// it is an edge field (the `block_requires` junction), never part of the
/// generic `properties` blob. Returns an empty `Vec` when absent. Malformed
/// JSON in a present value is corruption of our own metadata — fail loud.
fn read_requires_from_meta(meta: &loro::LoroMap) -> Vec<EntityUri> {
    meta.get_typed("requires", |val| val.as_string().map(|s| s.to_string()))
        .map(|s| {
            serde_json::from_str::<Vec<String>>(&s)
                .unwrap_or_else(|e| panic!("corrupt `requires` metadata JSON {s:?}: {e}"))
                .into_iter()
                .map(|r| EntityUri::parse_owned(r).expect("stored requires must be a valid URI"))
                .collect()
        })
        .unwrap_or_default()
}

/// Check if two snapshotted blocks differ in content, structure, ordering, or
/// properties. Ordering is compared via the adapter-internal `sort_key`
/// (fractional index) on [`SnapshotBlock`], not the domain `Block`.
fn diff_blocks_changed(a: &SnapshotBlock, b: &SnapshotBlock) -> bool {
    a.block.content != b.block.content
        || a.block.parent_id != b.block.parent_id
        || a.block.content_type != b.block.content_type
        || a.block.source_language != b.block.source_language
        || a.sort_key != b.sort_key
        || a.block.properties_map() != b.block.properties_map()
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
    // Try TreeID format first, then stable ID cache, then walk the tree.
    // The tree walk handles the seed phase: when blocks are
    // created in the same batch with dependency chains >1 level deep,
    // a parent node may already exist in the tree but hasn't been added
    // to the id_cache yet (cache is populated lazily by create_block).
    // ALLOW(fallback): seed-time recovery is a deliberate disclosed path;
    // the alternative (eagerly populating id_cache across the batch) would
    // need a multi-pass create, which is the larger refactor.
    let tree_id = uri_to_tree_id(parent_uri)
        .or_else(|| {
            if parent_uri.is_block() {
                id_cache.lock().unwrap().get(parent_uri.id()).copied()
            } else {
                None
            }
        })
        .or_else(|| {
            // ALLOW(fallback): tree walk after id_cache miss covers seed-time
            // ordering — same disclosed path as the comment block above.
            if parent_uri.is_block() {
                for node in tree.get_nodes(false) {
                    if matches!(
                        node.parent,
                        loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
                    ) {
                        continue;
                    }
                    if let Ok(meta) = tree.get_meta(node.id)
                        && let Some(loro::ValueOrContainer::Value(v)) = meta.get(STABLE_ID)
                        && v.as_string()
                            .map(|s| s.as_ref() == parent_uri.id())
                            .unwrap_or(false)
                    {
                        // Found it — also populate the cache for next time
                        id_cache
                            .lock()
                            .unwrap()
                            .insert(parent_uri.id().to_string(), node.id);
                        return Some(node.id);
                    }
                }
            }
            None
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

/// A [`BlockTreeView`] over a Loro tree, built by scanning live nodes once.
/// Lets the domain run the ADR-0005 move preconditions
/// ([`BlockMutation::validate`]) *before* `tree.mov` dispatches, so cycle /
/// structure detection is the domain's primary guard; Loro's native `mov`
/// cycle check then acts as defense-in-depth.
pub struct LoroTreeView {
    /// child URI → parent URI (root-parented nodes have no entry).
    parents: HashMap<EntityUri, EntityUri>,
    existing: HashSet<EntityUri>,
}

impl LoroTreeView {
    fn build(tree: &loro::LoroTree) -> Self {
        let mut parents = HashMap::new();
        let mut existing = HashSet::new();
        for node in tree.get_nodes(false) {
            if matches!(
                node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            ) {
                continue;
            }
            let Ok(meta) = tree.get_meta(node.id) else {
                continue;
            };
            let uri = block_uri_from_meta(&meta, node.id);
            existing.insert(uri.clone());
            if let Some(ptid) = get_node_parent(tree, node.id)
                && let Ok(pmeta) = tree.get_meta(ptid)
            {
                parents.insert(uri, block_uri_from_meta(&pmeta, ptid));
            }
        }
        Self { parents, existing }
    }
}

impl BlockTreeView for LoroTreeView {
    fn block_exists(&self, id: &EntityUri) -> bool {
        self.existing.contains(id)
    }
    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
        self.parents.get(id).cloned()
    }
    fn children_of(&self, parent: &EntityUri) -> Vec<EntityUri> {
        self.parents
            .iter()
            .filter(|(_, p)| *p == parent)
            .map(|(c, _)| c.clone())
            .collect()
    }
}

/// Snapshot all alive blocks in a raw `LoroDoc`, keyed by stable ID.
///
/// This is the same logic `LoroBackend::snapshot_blocks` uses, but on a raw
/// `&LoroDoc` rather than a `CollabDoc`. It exists so `LoroSyncController`
/// can snapshot both the forked (old) state and the current state of the
/// doc during reconciliation without wrapping them in `LoroDocument`.
pub fn snapshot_blocks_from_doc(doc: &loro::LoroDoc) -> HashMap<String, SnapshotBlock> {
    snapshot_blocks_from_doc_settled(doc).0
}

/// Like [`snapshot_blocks_from_doc`], but also reports whether the snapshot is
/// **settled**: `true` when every *live* (non-deleted) tree node was
/// projectable, `false` when at least one live node was skipped because its
/// meta / `STABLE_ID` was transiently missing (an in-flight create/move commits
/// the node and its meta in separate doc-state steps within one `with_write`,
/// so a concurrent reader can observe a node before its meta lands).
///
/// Why callers need this: an unsettled snapshot under-reports the live set. A
/// caller that diffs it for **deletes** must withhold them — the missing block
/// still exists in the tree; treating it as absent would spuriously delete its
/// sink row, which the next settled snapshot re-creates (an add/remove CDC churn
/// cycle — `inv-editable-text-has-draggable`). A genuinely deleted node parents
/// to `Deleted`/`Unexist` (the first `continue` below) and does **not** flip
/// `settled`, so real deletes still flow.
pub fn snapshot_blocks_from_doc_settled(
    doc: &loro::LoroDoc,
) -> (HashMap<String, SnapshotBlock>, bool) {
    let tree = doc.get_tree(TREE_NAME);
    let mut blocks: HashMap<String, SnapshotBlock> = HashMap::new();
    let mut settled = true;
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        // A live node without readable meta / `STABLE_ID` is transiently
        // incomplete (mid-mutation), not absent: skip it for projection but
        // mark the snapshot unsettled so the caller withholds deletes. Panicking
        // here (`block_uri_from_meta`) would kill the projection task entirely.
        let Ok(meta) = tree.get_meta(node.id) else {
            settled = false;
            continue;
        };
        if read_stable_id(&meta).is_none() {
            settled = false;
            continue;
        }
        let parent_tid = get_node_parent(&tree, node.id);
        let block = read_block_from_tree(&tree, node.id, parent_tid);
        // The fractional index is the Loro adapter's internal ordering encoding
        // (ADR 0005): captured here for the SQL projection, never on the block.
        let sort_key = tree
            .fractional_index(node.id)
            .unwrap_or_else(default_sort_key);
        if std::env::var("HOLON_LORO_DUP_DEBUG").is_ok()
            && let Some(prev) = blocks.get(&block.id.to_string())
        {
            tracing::warn!(
                "[LORO_DUP] duplicate stable id {} in tree: prev content {:?} vs {:?}",
                block.id,
                prev.block.content,
                block.content
            );
        }
        blocks.insert(block.id.to_string(), SnapshotBlock { block, sort_key });
    }
    (blocks, settled)
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
    while let loro::TreeParentId::Node(pid) = current {
        d += 1;
        current = tree.parent(pid).unwrap_or(loro::TreeParentId::Root);
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
    traversal: &holon_api::repository::Traversal,
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
    event_log: Arc<Mutex<EventRing<Change<Block>>>>,
    shared_trees: Option<Arc<dyn SharedTreeStore>>,
    /// Cache: stable_id (UUID string) → TreeID. Populated eagerly on create,
    /// lazily on lookup, invalidated on delete.
    id_cache: Arc<Mutex<HashMap<String, loro::TreeID>>>,
    clock: std::sync::Arc<dyn holon_api::Clock>,
}

impl Clone for LoroBackend {
    fn clone(&self) -> Self {
        Self {
            collab_doc: self.collab_doc.clone(),
            subscribers: self.subscribers.clone(),
            event_log: self.event_log.clone(),
            shared_trees: self.shared_trees.clone(),
            id_cache: self.id_cache.clone(),
            clock: self.clock.clone(),
        }
    }
}

impl LoroBackend {
    pub fn from_document(collab_doc: Arc<LoroDocument>) -> Self {
        Self {
            collab_doc,
            subscribers: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            event_log: Arc::new(Mutex::new(EventRing::new(DEFAULT_EVENT_RING_CAPACITY))),
            shared_trees: None,
            id_cache: Arc::new(Mutex::new(HashMap::new())),
            clock: std::sync::Arc::new(holon_api::SystemClock),
        }
    }

    pub fn with_clock(mut self, clock: std::sync::Arc<dyn holon_api::Clock>) -> Self {
        self.clock = clock;
        self
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

    pub fn collab_for_test(&self) -> Arc<LoroDocument> {
        self.collab_doc.clone()
    }

    fn now_millis(&self) -> i64 {
        self.clock.now_millis()
    }

    pub(crate) fn emit_change(&self, change: Change<Block>) {
        self.event_log.lock().unwrap().push(change.clone());
        let batch = vec![change];
        let subscribers = self.subscribers.clone();
        tokio::spawn(async move {
            let mut subscribers = subscribers.lock().await;
            deliver_to_subscribers(&mut subscribers, batch).await;
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
                    if let Some(Value::String(prop_uuid)) = properties.get("ID")
                        && prop_uuid == uuid
                    {
                        return Ok(Some(tree_id_to_uri(tree_node.id).to_string()));
                    }
                }
                Ok(None)
            })
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
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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

                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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

                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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

                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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

    /// Like the `create_block` trait method, but writes `properties` into the
    /// new node's meta **in the same Loro commit** as the node, content, and
    /// STABLE_ID. This keeps the Loro authority *complete at creation*: a block
    /// born with drawer properties (org ingestion) carries them in Loro, so the
    /// outbound projector reflects them to SQL instead of round-tripping an
    /// empty `properties` over the row the create wrote (the `props_check` /
    /// "Value::Object serialization" divergence). The trait `create_block`
    /// delegates here with an empty map.
    pub async fn create_block_with_properties(
        &self,
        parent_id: EntityUri,
        content: BlockContent,
        id: Option<EntityUri>,
        properties: &HashMap<String, Value>,
        tags: &Tags,
        requires: &[EntityUri],
    ) -> Result<Block, ApiError> {
        let now = self.now_millis();
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
                write_properties_to_meta(&meta, properties)?;
                // Tags are edge fields (block_tags), stored in Loro meta as a
                // JSON list under "tags" (mirrors `set_block_tags`). Carrying
                // them in the create commit is essential: the downstream
                // projection reads them via `read_block_from_tree` and writes
                // `block_tags`. The `Page` tag in particular makes a document
                // resolvable — dropping it here orphans every doc.
                if !tags.is_empty() {
                    let serialized = serde_json::to_string(tags)
                        .map_err(|e| anyhow::anyhow!("serialize tags: {e}"))?;
                    meta.insert("tags", loro::LoroValue::from(serialized.as_str()))?;
                }
                // `requires` mirrors `tags`: an edge field carried in the create
                // commit under its own meta key, so the downstream projection
                // reads it via `read_block_from_tree` and writes `block_requires`.
                // Dropping it here loses every org-edna dependency in Loro mode.
                if !requires.is_empty() {
                    let serialized = serde_json::to_string(requires)
                        .map_err(|e| anyhow::anyhow!("serialize requires: {e}"))?;
                    meta.insert("requires", loro::LoroValue::from(serialized.as_str()))?;
                }
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
                block.set_properties_map(properties.clone());
                block.tags = tags.clone();
                block.requires = requires.to_vec();
                block.created_at = now;
                block.updated_at = now;
                Ok((block, node))
            })
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
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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

    /// Write `properties` as the block's EXACT property set — keys absent from
    /// the map are removed. `update_block_properties` merges and therefore can
    /// never clear a stale key (e.g. `todo_keywords` after the `#+TODO:` header
    /// was deleted from the org file); callers holding the authoritative full
    /// set use this instead.
    pub async fn replace_block_properties(
        &self,
        id: &str,
        properties: &HashMap<String, Value>,
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id).await?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                // Bypass `write_properties_to_meta` — its empty-map fast path
                // would silently keep the old blob when the new set is empty.
                let json = serde_json::to_string(properties)?;
                meta.insert(PROPERTIES, loro::LoroValue::from(json.as_str()))?;
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to replace block properties: {}", e),
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
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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
        // ALLOW(entity_uri_from_raw): new_parent_id String from cell-registry field write Value
        let new_parent_uri = EntityUri::from_raw(&new_parent_id);
        let id_cache = self.id_cache.clone();

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let new_parent = resolve_parent_tree_id(&tree, &id_cache, &new_parent_uri)?;
                // No-op when the parent is unchanged: `tree.mov` APPENDS the
                // node to the END of the new parent's children, so an
                // unchanged-parent "move" (e.g. an org re-ingest update op
                // that carries parent_id verbatim) silently re-keys the
                // node's fractional index and reorders siblings. Sibling
                // order is owned by `place()`/`mov_after`; a same-parent
                // field write must not touch it.
                if get_node_parent(&tree, tree_id) == new_parent {
                    return Ok(());
                }
                tree.mov(tree_id, new_parent)?;
                let meta = tree.get_meta(tree_id)?;
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update parent_id: {}", e),
            })?;
        Ok(())
    }

    /// Phase 3.4 positioned move. Drives Loro's built-in fractional-index
    /// machinery (`tree.mov_after` / `tree.mov_to`) instead of writing a
    /// shadow `sort_key` meta key. Callers that previously paired
    /// `set_field("parent_id", …)` with `set_field("sort_key", …)` now
    /// dispatch through this single call: the `target` ends up as a child
    /// of `new_parent_id`, placed immediately after `predecessor_id` when
    /// it is `Some(_)`, or as the first child when it is `None`.
    ///
    /// Block.sort_key is no longer stored separately; it is projected on
    /// read from `tree.fractional_index(target)` (see
    /// `read_block_from_tree`).
    pub async fn update_block_position(
        &self,
        target_id: &str,
        new_parent_id: &str,
        predecessor_id: Option<&str>,
    ) -> Result<(), ApiError> {
        let target = self.require_tree_id(target_id).await?;
        // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both id formats)
        let new_parent_uri = EntityUri::from_raw(new_parent_id);
        let predecessor = match predecessor_id {
            Some(p) => Some(self.require_tree_id(p).await?),
            None => None,
        };
        let id_cache = self.id_cache.clone();

        let noop = self
            .collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let current_parent_tid = get_node_parent(&tree, target);
                let want_parent_uri = &new_parent_uri;
                // If the parent can't be resolved yet (not in the tree), treat as a
                // real move — fall through to the actual mov_after which will error
                // if the parent truly doesn't exist.
                let want_parent_tid =
                    match resolve_parent_tree_id(&tree, &id_cache, want_parent_uri) {
                        Ok(tid) => tid,
                        Err(_) => return Ok::<bool, anyhow::Error>(false),
                    };

                // If the parent changed, definitely not a no-op.
                if current_parent_tid != want_parent_tid {
                    return Ok::<bool, anyhow::Error>(false);
                }

                // Parent matches — check predecessor.
                let siblings: Vec<loro::TreeID> = match current_parent_tid {
                    Some(p) => tree.children(p).unwrap_or_default(),
                    None => tree.roots(),
                };
                let my_idx = siblings.iter().position(|&s| s == target);
                let current_pred = my_idx.and_then(|i| {
                    if i == 0 {
                        None
                    } else {
                        siblings.get(i - 1).copied()
                    }
                });

                // Resolve want predecessor to TreeID for comparison.
                let want_pred_tid = predecessor;
                Ok(current_pred == want_pred_tid)
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to check block position: {}", e),
            })?;

        if noop {
            return Ok(());
        }

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                match predecessor {
                    Some(pred_id) => {
                        tree.mov_after(target, pred_id)?;
                    }
                    None => {
                        let new_parent = resolve_parent_tree_id(&tree, &id_cache, &new_parent_uri)?;
                        match new_parent {
                            Some(p) => tree.mov_to(target, p, 0)?,
                            None => tree.mov_to(target, loro::TreeParentId::Root, 0)?,
                        }
                    }
                }
                let meta = tree.get_meta(target)?;
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to update block position: {}", e),
            })?;

        let block = self.get_block(target_id).await?;
        self.emit_change(Change::Updated {
            id: target_id.to_string(),
            data: block,
            origin: ChangeOrigin::Local {
                operation_id: None,
                trace_id: None,
            },
        });
        Ok(())
    }

    // -- Tags (page marker + user tags) --

    /// Replace the `tags` list on a tree node. The literal `"Page"` tag in
    /// the list marks the block as a page (formerly `is_document`).
    /// An empty `tags` list deletes the meta key entirely.
    pub async fn set_block_tags(&self, tree_id_str: &str, tags: &[String]) -> anyhow::Result<()> {
        let tree_id = self
            .resolve_to_tree_id(tree_id_str)
            .await
            .ok_or_else(|| anyhow::anyhow!("set_block_tags: block not found: {}", tree_id_str))?;

        let serialized = serde_json::to_string(tags)?;

        self.collab_doc.with_write(|doc| {
            let tree = doc.get_tree(TREE_NAME);
            let meta = tree.get_meta(tree_id)?;
            if tags.is_empty() {
                meta.delete("tags")?;
            } else {
                meta.insert("tags", loro::LoroValue::from(serialized.as_str()))?;
            }
            doc.commit();
            Ok(())
        })
    }

    /// Set the `requires` edge field (org-edna dependencies) on a block's Loro
    /// meta. Mirrors [`set_block_tags`](Self::set_block_tags): stored under a
    /// dedicated `requires` meta key as a JSON list, read back by
    /// `read_block_from_tree`, projected to the `block_requires` junction.
    pub async fn set_block_requires(
        &self,
        tree_id_str: &str,
        requires: &[EntityUri],
    ) -> anyhow::Result<()> {
        let tree_id = self.resolve_to_tree_id(tree_id_str).await.ok_or_else(|| {
            anyhow::anyhow!("set_block_requires: block not found: {}", tree_id_str)
        })?;

        let serialized = serde_json::to_string(requires)?;

        self.collab_doc.with_write(|doc| {
            let tree = doc.get_tree(TREE_NAME);
            let meta = tree.get_meta(tree_id)?;
            if requires.is_empty() {
                meta.delete("requires")?;
            } else {
                meta.insert("requires", loro::LoroValue::from(serialized.as_str()))?;
            }
            doc.commit();
            Ok(())
        })
    }

    /// Set the `source_language` meta field on a source block's Loro node.
    /// Needed by the org re-ingest update path: an `index.org` swap can change
    /// a `#+BEGIN_SRC` block's language (e.g. `holon_prql` → `holon_gql`);
    /// routing that write SQL-direct in Loro mode silently forks the
    /// authority (the projector overwrites it back on the next snapshot).
    pub async fn set_source_language(&self, tree_id_str: &str, lang: &str) -> anyhow::Result<()> {
        let tree_id = self.resolve_to_tree_id(tree_id_str).await.ok_or_else(|| {
            anyhow::anyhow!("set_source_language: block not found: {}", tree_id_str)
        })?;

        self.collab_doc.with_write(|doc| {
            let tree = doc.get_tree(TREE_NAME);
            let meta = tree.get_meta(tree_id)?;
            meta.insert(SOURCE_LANGUAGE, loro::LoroValue::from(lang))?;
            meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
            doc.commit();
            Ok(())
        })
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
        let _ = self.collab_doc.with_read(|doc| {
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
                if let Ok(meta) = tree.get_meta(node.id)
                    && let Some(sid) = read_stable_id(&meta)
                {
                    cache.insert(sid, node.id);
                }
            }
            Ok(())
        });
    }

    // -- Diff-based CDC after remote sync --

    /// Snapshot all alive blocks keyed by stable ID. Call before `doc.import(delta)`.
    pub async fn snapshot_blocks(&self) -> HashMap<String, SnapshotBlock> {
        self.collab_doc
            .with_read(|doc| Ok(snapshot_blocks_from_doc(doc)))
            .unwrap_or_default()
    }

    /// The Loro tree's fractional index for `id` — the adapter's internal
    /// ordering encoding the projector writes to SQL `sort_key` (ADR 0005).
    /// `None` when the node carries no index yet.
    pub async fn block_sort_key(&self, id: &str) -> Result<Option<String>, ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                Ok(tree.fractional_index(tree_id))
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("block_sort_key({id}): {e}"),
            })
    }

    /// Compare current state against a pre-import snapshot, emit CDC events
    /// for all Created, Updated, and Deleted blocks, and return the changes.
    /// Also warms the stable ID cache.
    ///
    /// Call after `doc.import(delta)` with the snapshot from `snapshot_blocks()`.
    pub async fn diff_and_emit_after_import(
        &self,
        before: HashMap<String, SnapshotBlock>,
    ) -> Vec<Change<Block>> {
        let after = self.snapshot_blocks().await;
        self.warm_stable_id_cache().await;

        let remote_origin = ChangeOrigin::Remote {
            operation_id: None,
            trace_id: None,
        };

        let mut changes = Vec::new();

        // Deleted: in before, not in after
        for id in before.keys() {
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
        for (id, snap) in &after {
            match before.get(id) {
                None => {
                    let change = Change::Created {
                        data: snap.block.clone(),
                        origin: remote_origin.clone(),
                    };
                    self.emit_change(change.clone());
                    changes.push(change);
                }
                Some(old) if diff_blocks_changed(old, snap) => {
                    let change = Change::Updated {
                        id: id.clone(),
                        data: snap.block.clone(),
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
            // ALLOW(ok): returning Option<TreeID> at the API surface; the
            // with_read error is a "couldn't acquire read lock" diagnostic
            // that the lookup callers (resolve_to_tree_id) already treat as
            // "not found" — preserving the Option signature here is the
            // intended behavior of the resolver.
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
        // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both id formats)
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
        self.collab_doc.with_write(|doc| {
            let tree = doc.get_tree(TREE_NAME);
            let meta = tree.get_meta(tree_id)?;
            meta.insert(STABLE_ID, loro::LoroValue::from(raw_id.as_str()))?;
            meta.insert(EXTERNAL_ID, loro::LoroValue::from(ext_id.as_str()))?;
            doc.commit();
            Ok(())
        })
    }

    /// Create a root-level placeholder node without emitting events.
    /// Used by reverse sync to represent document blocks that aren't in the EventBus.
    /// The `stable_id` becomes the node's STABLE_ID and is returned as a `block:` URI.
    pub async fn create_placeholder_root(&self, stable_id: &str) -> anyhow::Result<String> {
        let sid = stable_id.to_string();
        let id_cache = self.id_cache.clone();
        self.collab_doc.with_write(|doc| {
            let tree = doc.get_tree(TREE_NAME);
            let node = tree.create(None)?;
            let meta = tree.get_meta(node)?;
            meta.insert(STABLE_ID, loro::LoroValue::from(sid.as_str()))?;
            doc.commit();
            id_cache.lock().unwrap().insert(sid.clone(), node);
            Ok(EntityUri::block(&sid).to_string())
        })
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
            event_log: Arc::new(Mutex::new(EventRing::new(DEFAULT_EVENT_RING_CAPACITY))),
            shared_trees: None,
            id_cache: Arc::new(Mutex::new(HashMap::new())),
            clock: std::sync::Arc::new(holon_api::SystemClock),
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

        if let StreamPosition::Version(ref watermark) = position {
            // Watermark replay from the bounded ring (see `event_ring`):
            // exactly the changes with seq >= watermark. A watermark that has
            // been evicted fails loud — the subscriber must re-sync from
            // `Beginning` — instead of silently replaying partial history.
            let watermark = u64::from_le_bytes(watermark.as_slice().try_into().unwrap_or([0; 8]));
            match self.event_log.lock().unwrap().replay_since(watermark) {
                Ok(events) => replay_items.extend(events),
                Err(expired) => {
                    tracing::error!("[LoroBackend] watch_changes_since: {expired}");
                    let error_stream = tokio_stream::iter(vec![Err(ApiError::InternalError {
                        message: expired.to_string(),
                    })]);
                    let (_tx, rx) =
                        mpsc::channel::<std::result::Result<Vec<Change<Block>>, ApiError>>(100);
                    return Box::pin(error_stream.chain(ReceiverStream::new(rx)));
                }
            }
        }

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
        // Watermark into the change ring (was: a full-history Loro snapshot
        // export per call — document-sized and semantically unused). Mirrors
        // `MemoryBackend::get_current_version`.
        Ok(self
            .event_log
            .lock()
            .unwrap()
            .next_seq()
            .to_le_bytes()
            .to_vec())
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
        traversal: holon_api::repository::Traversal,
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

                // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both id formats)
                let parent_uri = EntityUri::from_raw(parent_id);
                // Use the shared `resolve_parent_tree_id` (TreeID → id_cache → tree-walk,
                // populating the cache on a hit) rather than a cache-only lookup: a backend
                // attached via `from_document` (e.g. the composed PBT's Loro read cap over the
                // frontend's authority doc, or a peer-merged doc) has an EMPTY id_cache, so a
                // cache-only resolve fails for a block parent that is genuinely present in the
                // tree. `Ok(None)` ⇒ no_parent/sentinel ⇒ the tree roots.
                let children_tids = match resolve_parent_tree_id(&tree, &id_cache, &parent_uri)? {
                    None => tree.roots(),
                    Some(tree_id) => tree.children(tree_id).unwrap_or_default(),
                };

                let mut result = Vec::new();
                for tid in &children_tids {
                    if is_mount_node(&tree, *tid)
                        && let (Some(store), Some(info)) =
                            (&shared_trees, read_mount_info(&tree, *tid))
                        && let Some(shared_doc) = store.get_shared_doc(&info.shared_tree_id)
                    {
                        let shared_tree = shared_doc.get_tree(TREE_NAME);
                        for shared_root in shared_tree.roots() {
                            let meta = shared_tree.get_meta(shared_root)?;
                            result.push(block_uri_from_meta(&meta, shared_root).to_string());
                        }
                        continue;
                    }
                    let meta = tree.get_meta(*tid)?;
                    result.push(block_uri_from_meta(&meta, *tid).to_string());
                }
                Ok(result)
            })
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
        self.create_block_with_properties(
            parent_id,
            content,
            id,
            &HashMap::new(),
            &Tags::default(),
            &[],
        )
        .await
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
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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

    /// Delete a block from the Loro tree. Idempotent: already-deleted and
    /// never-seeded blocks are a no-op success (no `Change::Deleted` emitted —
    /// the concurrent deleter or prior operation already handled it).
    async fn delete_block(&self, id: &str) -> Result<(), ApiError> {
        let Some(tree_id) = self.resolve_to_tree_id(id).await else {
            // Block not in Loro tree (never seeded or concurrently deleted).
            // No emit — the other deleter already did (R-2).
            return Ok(());
        };

        let mut did_delete = false;
        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                match tree.delete(tree_id) {
                    Ok(()) => {
                        doc.commit();
                        did_delete = true;
                        Ok(())
                    }
                    Err(loro::LoroError::TreeError(
                        loro::LoroTreeError::TreeNodeNotExist(_)
                        | loro::LoroTreeError::TreeNodeDeletedOrNotExist(_),
                    )) => Ok(()),
                    Err(e) => Err(e.into()),
                }
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to delete block: {}", e),
            })?;

        if did_delete {
            // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both id formats)
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
        }
        Ok(())
    }

    async fn move_block(
        &self,
        id: &EntityUri,
        new_parent: EntityUri,
        after: Option<EntityUri>,
    ) -> Result<(), ApiError> {
        let tree_id = self.require_tree_id(id.as_str()).await?;
        let block_before = self.get_block(id.as_str()).await?;
        let id_cache = self.id_cache.clone();

        // Domain-level precondition (ADR 0005): the primary cycle / structure
        // guard, run before adapter dispatch. `tree.mov` below re-checks cycles
        // natively as defense-in-depth.
        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                Ok(BlockMutation::Move {
                    id: id.clone(),
                    new_parent: new_parent.clone(),
                    after: after.clone(),
                }
                .validate(&LoroTreeView::build(&tree)))
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("move_block precondition read failed: {e}"),
            })?
            .map_err(ApiError::from)?;

        self.collab_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let new_parent_tree_id = resolve_parent_tree_id(&tree, &id_cache, &new_parent)?;

                // LoroTree.mov re-checks cycles natively (defense-in-depth).
                tree.mov(tree_id, new_parent_tree_id)?;

                // Handle `after` positioning via mov_after
                if let Some(after_uri) = &after
                    && let Some(after_tid) = uri_to_tree_id(after_uri)
                {
                    tree.mov_after(tree_id, after_tid)?;
                }

                let meta = tree.get_meta(tree_id)?;
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
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
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to get blocks: {}", e),
            })
    }

    async fn create_blocks(&self, blocks: Vec<NewBlock>) -> Result<Vec<Block>, ApiError> {
        let now = self.now_millis();

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
                    if let Some(after_uri) = &new_block.after
                        && let Some(after_tid) = uri_to_tree_id(after_uri)
                    {
                        tree.mov_after(node, after_tid)?;
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
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to delete blocks: {}", e),
            })?;

        for id in &unique_ids {
            // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both id formats)
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

    async fn connect_to_peer(&self, _: String) -> Result<(), ApiError> {
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
