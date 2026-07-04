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
/// Legacy single-blob property storage: one `LoroMap` key holding the whole
/// property map as one opaque JSON string. Read-only now (read when migrating a
/// pre-H3 block) — blob-level LWW meant two peers setting *different* properties
/// clobbered each other (H3). Superseded by [`PROPERTIES_MAP`].
const PROPERTIES: &str = "properties";
/// H3: properties stored as a nested `LoroMap`, one key per property. A `LoroMap`
/// resolves conflicts per key, so concurrent edits to *different* properties
/// (e.g. `TODO` vs `PRIORITY`) merge instead of one peer's blob winning. Each
/// value is its `serde_json::Value` JSON-encoded into a string.
const PROPERTIES_MAP: &str = "properties_map";
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
    // Prefer the nested per-property map (H3). A pre-H3 block that hasn't been
    // rewritten yet still carries the legacy single-blob string; read that until
    // the next write migrates it (writes delete the legacy key — self-healing).
    let mut props: HashMap<String, Value> = match meta.get(PROPERTIES_MAP) {
        Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) => {
            decode_properties_map(&map)
        }
        _ => match meta.get_typed(PROPERTIES, |val| val.as_string().map(|s| s.to_string())) {
            Some(json) => serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("Corrupt properties JSON in Loro tree: {json:?}: {e}")),
            None => HashMap::new(),
        },
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

/// Open (creating if absent) the nested per-property `LoroMap` (H3, [`PROPERTIES_MAP`]).
pub(crate) fn properties_map_container(meta: &loro::LoroMap) -> anyhow::Result<loro::LoroMap> {
    Ok(meta.get_or_create_container(PROPERTIES_MAP, loro::LoroMap::new())?)
}

/// Read one scalar block field's [`Value`] straight from a tree node's `meta`
/// map, mirroring the decode `read_properties_from_meta` performs at the whole-
/// block level: prefer the nested per-property map (H3), fall back to the legacy
/// single-blob until a write migrates it. `None` when the key is absent. This is
/// the per-field read the `LoroMetaCellBacking<T>` scalar cell projects — it must
/// agree with the whole-block projection so a cell read and a `get_block` read of
/// the same field never diverge.
pub(crate) fn read_scalar_field_from_meta(meta: &loro::LoroMap, key: &str) -> Option<Value> {
    if let Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) =
        meta.get(PROPERTIES_MAP)
    {
        if let Some(loro::ValueOrContainer::Value(v)) = map.get(key) {
            let json = v
                .as_string()
                .map(|s| s.to_string())
                .unwrap_or_else(|| panic!("Property {key:?} is not a JSON string: {v:?}"));
            let parsed = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("Corrupt property JSON for {key:?}: {json:?}: {e}"));
            return Some(parsed);
        }
        if let Some(loro::ValueOrContainer::Container(_)) = map.get(key) {
            panic!("Property {key:?} unexpectedly holds a container, not a JSON string");
        }
    }
    // Legacy pre-H3 single-blob path (self-heals on the next write, which migrates).
    let json = meta.get_typed(PROPERTIES, |val| val.as_string().map(|s| s.to_string()))?;
    let legacy: HashMap<String, Value> = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("Corrupt properties JSON in Loro tree: {json:?}: {e}"));
    legacy.get(key).cloned()
}

/// Encode one property value as the JSON string stored under its key. Properties
/// are arbitrary `serde_json::Value`; the per-key granularity (not per-field) is
/// what H3 needs — concurrent edits to *different* properties are different keys.
fn encode_property_value(value: &Value) -> anyhow::Result<loro::LoroValue> {
    Ok(loro::LoroValue::from(
        serde_json::to_string(value)?.as_str(),
    ))
}

/// Decode the nested per-property `LoroMap` back into a property map. Each value
/// must be the JSON string an H3 write produced — anything else is corruption,
/// so panic rather than silently dropping it.
fn decode_properties_map(map: &loro::LoroMap) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    map.for_each(|key, value| {
        let json = match value {
            loro::ValueOrContainer::Value(v) => v
                .as_string()
                .map(|s| s.to_string())
                .unwrap_or_else(|| panic!("Property {key:?} is not a JSON string: {v:?}")),
            loro::ValueOrContainer::Container(_) => {
                panic!("Property {key:?} unexpectedly holds a container, not a JSON string")
            }
        };
        let parsed = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("Corrupt property JSON for {key:?}: {json:?}: {e}"));
        out.insert(key.to_string(), parsed);
    });
    out
}

/// Drop the legacy single-blob `PROPERTIES` key. Authoritative full-set writers
/// ([`replace_properties_in_meta`]) use this — they already define the entire set,
/// so the blob's contents are intentionally discarded.
fn drop_legacy_properties_blob(meta: &loro::LoroMap) -> anyhow::Result<()> {
    if meta.get(PROPERTIES).is_some() {
        meta.delete(PROPERTIES)?;
    }
    Ok(())
}

/// Copy any legacy single-blob properties into the nested map (only keys not
/// already present) and drop the legacy key. Partial writers (merge / per-field)
/// call this first so a pre-H3 block's *untouched* properties survive its first
/// partial write instead of being dropped with the blob.
fn migrate_legacy_blob_into_map(meta: &loro::LoroMap, map: &loro::LoroMap) -> anyhow::Result<()> {
    let Some(json) = meta.get_typed(PROPERTIES, |val| val.as_string().map(|s| s.to_string()))
    else {
        return Ok(());
    };
    let legacy: HashMap<String, Value> = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("Corrupt properties JSON in Loro tree: {json:?}: {e}"));
    for (key, value) in &legacy {
        if map.get(key).is_none() {
            map.insert(key, encode_property_value(value)?)?;
        }
    }
    meta.delete(PROPERTIES)?;
    Ok(())
}

/// Insert/overwrite only the given properties — other keys are left untouched
/// (merge semantics). Writing only the keys that changed is what gives H3 its
/// convergence: an update touching `TODO` leaves a concurrent peer's `PRIORITY`
/// write intact, instead of a read-modify-write re-stamping it with a stale value.
fn merge_properties_into_meta(
    meta: &loro::LoroMap,
    properties: &HashMap<String, Value>,
) -> anyhow::Result<()> {
    let map = properties_map_container(meta)?;
    migrate_legacy_blob_into_map(meta, &map)?;
    for (key, value) in properties {
        map.insert(key, encode_property_value(value)?)?;
    }
    Ok(())
}

/// Replace the block's EXACT property set: keys absent from `properties` are
/// deleted. Authoritative full-set writes (block creation, org re-parse) use this.
fn replace_properties_in_meta(
    meta: &loro::LoroMap,
    properties: &HashMap<String, Value>,
) -> anyhow::Result<()> {
    let map = properties_map_container(meta)?;
    let stale: Vec<String> = map
        .keys()
        .map(|k| k.to_string())
        .filter(|k| !properties.contains_key(k))
        .collect();
    for key in stale {
        map.delete(&key)?;
    }
    for (key, value) in properties {
        map.insert(key, encode_property_value(value)?)?;
    }
    drop_legacy_properties_blob(meta)
}

/// Apply per-field changes: a `Null` new value deletes the key, anything else
/// inserts it. Only the named fields are touched (per-key convergence, H3).
fn apply_field_changes_to_meta(
    meta: &loro::LoroMap,
    fields: &[(String, Value, Value)],
) -> anyhow::Result<()> {
    let map = properties_map_container(meta)?;
    migrate_legacy_blob_into_map(meta, &map)?;
    for (name, _old_value, new_value) in fields {
        if new_value == &Value::Null {
            if map.get(name).is_some() {
                map.delete(name)?;
            }
        } else {
            map.insert(name, encode_property_value(new_value)?)?;
        }
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
    let mut sibling_keys: HashMap<Option<loro::TreeID>, HashMap<loro::TreeID, Option<String>>> =
        HashMap::new();
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
        // Computed per sibling group so concurrently-created siblings whose
        // peers minted the SAME fi still project DISTINCT keys in Loro's true
        // child order (see `effective_sibling_sort_keys`).
        //
        // A live node with no fractional index (inner `None`) or one absent from
        // its parent's child list (outer `None`) is an ordering-invariant
        // violation (ADR 0005). We MUST NOT fake a `default_sort_key()` ("A0")
        // here: that is the exact historical fi-corruption shape and a fail-loud
        // violation (CLAUDE.md "never fake"). Instead disclose the degraded node
        // loudly and withhold it (like the missing-meta skip above), marking the
        // snapshot unsettled so the caller withholds deletes.
        let key_opt = sibling_keys
            .entry(parent_tid)
            .or_insert_with(|| {
                let siblings = match parent_tid {
                    Some(p) => tree.children(p).unwrap_or_default(),
                    None => tree.roots(),
                };
                let keys = effective_sibling_sort_keys(&tree, &siblings);
                siblings.into_iter().zip(keys).collect()
            })
            .get(&node.id)
            .cloned();
        let Some(Some(sort_key)) = key_opt else {
            tracing::error!(
                block_id = %block.id,
                ?parent_tid,
                node = ?node.id,
                "loro projection: live node has no fractional index (ADR 0005 \
                 ordering-invariant violation); withholding from snapshot rather \
                 than faking an A0 sort key"
            );
            settled = false;
            continue;
        };
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

/// Effective SQL sort keys for one sibling group, in Loro's true child order
/// (`tree.children`/`tree.roots`). Normally each key IS the node's fractional
/// index — but concurrent peer creates at the same position mint the SAME fi
/// (jitter is 0), and Loro breaks that tie internally by op id, an order the
/// plain fi string loses. Projecting the tied string would collapse SQL to its
/// `ORDER BY sort_key, id` fallback — id-string order, random from the user's
/// PoV and divergent from the Loro authority. Tied runs therefore get a
/// `.<position>` suffix in child order; `.` (0x2E) sorts below every fi hex
/// char, so a suffixed key keeps its place relative to every distinctly-keyed
/// sibling (ties share the exact fi string as a common prefix).
fn effective_sibling_sort_keys(
    tree: &loro::LoroTree,
    siblings: &[loro::TreeID],
) -> Vec<Option<String>> {
    // `None` marks a sibling with no fractional index — a live-node ordering
    // invariant violation (ADR 0005). It is propagated (never defaulted to
    // "A0") so the caller can withhold that node and fail loud.
    let fis: Vec<Option<String>> = siblings
        .iter()
        .map(|&tid| tree.fractional_index(tid))
        .collect();
    fis.iter()
        .enumerate()
        .map(|(i, fi)| {
            let fi = fi.as_ref()?;
            let tied = fis
                .iter()
                .filter(|f| f.as_deref() == Some(fi.as_str()))
                .count()
                > 1;
            if tied {
                let run_pos = fis[..i]
                    .iter()
                    .filter(|f| f.as_deref() == Some(fi.as_str()))
                    .count();
                Some(format!("{fi}.{run_pos:06x}"))
            } else {
                Some(fi.clone())
            }
        })
        .collect()
}

/// A doc's Lamport height: 1 + the max lamport of any applied op, computed
/// from public API only (frontiers + `ChangeMeta`). This scalar is the ONLY
/// value the E-solid shadow-mesh oracle reads from the SUT (clock sync at
/// fork/sync boundaries); see `multi_peer::clock_parity_spike` for the parity
/// proof and the negative control showing the padding is load-bearing.
pub fn doc_lamport_height(doc: &loro::LoroDoc) -> u32 {
    doc.oplog_frontiers()
        .iter()
        .map(|id| {
            let c = doc.get_change(id).expect("frontier change present");
            c.lamport + (id.counter - c.id.counter) as u32 + 1
        })
        .max()
        .unwrap_or(0)
}

/// Check if a node is alive (not deleted) in the tree.
fn is_node_alive(tree: &loro::LoroTree, node: loro::TreeID) -> bool {
    match tree.parent(node) {
        Some(loro::TreeParentId::Deleted | loro::TreeParentId::Unexist) | None => false,
        Some(_) => true,
    }
}

/// Scan a raw `LoroDoc`'s block tree for the alive node whose `STABLE_ID`
/// equals `needle`. Used to resolve a shared block's business id inside a
/// shared subtree doc, where the id is absent from the global tree.
fn find_stable_id_in_doc(doc: &loro::LoroDoc, needle: &str) -> Option<loro::TreeID> {
    let tree = doc.get_tree(TREE_NAME);
    for node in tree.get_nodes(false) {
        if matches!(
            node.parent,
            loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
        ) {
            continue;
        }
        if let Ok(meta) = tree.get_meta(node.id)
            && let Some(sid) = meta.get_typed(STABLE_ID, |v| v.as_string().map(|s| s.to_string()))
            && sid == needle
        {
            return Some(node.id);
        }
    }
    None
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

/// Where a resolved write must land. A block that was pruned into a shared
/// subtree doc no longer lives in the global tree, so writing to it through
/// the global doc silently no-ops (or fails `BlockNotFound`). `resolve_write_target`
/// routes each write to the doc that actually holds the live node.
enum WriteTarget {
    Global(loro::TreeID),
    Shared {
        shared_tree_id: String,
        doc: Arc<loro::LoroDoc>,
        tree_id: loro::TreeID,
    },
}

impl WriteTarget {
    /// Doc-identity key: `None` = the global doc, `Some(shared_tree_id)` = a
    /// shared subtree doc. Two writes land in the same doc iff their keys match.
    fn doc_key(&self) -> Option<&str> {
        match self {
            WriteTarget::Global(_) => None,
            WriteTarget::Shared { shared_tree_id, .. } => Some(shared_tree_id),
        }
    }
}

/// Where a *newly created* block must land, resolved by its parent. A child of
/// a shared block is created in the shared doc, not the global doc. Unlike
/// [`WriteTarget`] this carries no TreeID: the parent's TreeID is resolved
/// inside the target doc's tree at write time (via `resolve_parent_tree_id`).
enum ParentWriteTarget {
    Global,
    Shared {
        shared_tree_id: String,
        doc: Arc<loro::LoroDoc>,
    },
}

impl ParentWriteTarget {
    fn doc_key(&self) -> Option<&str> {
        match self {
            ParentWriteTarget::Global => None,
            ParentWriteTarget::Shared { shared_tree_id, .. } => Some(shared_tree_id),
        }
    }
}

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

    /// Route a write for block `id` to the doc that holds its live node.
    ///
    /// Global tree first (the common case), then — on a global miss or a stale
    /// tombstoned candidate — the shared subtree docs. A shared block's stable
    /// id is absent from the global tree (its subtree was pruned at share time),
    /// so the global resolver returns `None`; TreeIDs are globally unique
    /// (peer+counter), so a stale global TreeID is still a valid key to probe
    /// the shared docs with.
    async fn resolve_write_target(&self, id: &str) -> Result<WriteTarget, ApiError> {
        if let Some(tree_id) = self.resolve_to_tree_id(id).await {
            let alive_global = self
                .collab_doc
                .with_read(|doc| Ok(is_node_alive(&doc.get_tree(TREE_NAME), tree_id)))
                .map_err(|e| ApiError::InternalError {
                    message: format!("resolve_write_target: read global tree failed: {}", e),
                })?;
            if alive_global {
                return Ok(WriteTarget::Global(tree_id));
            }
            if let Some(target) = self.scan_shared_for_tree_id(tree_id) {
                return Ok(target);
            }
        }

        if let Some(target) = self.scan_shared_for_stable_id(id) {
            return Ok(target);
        }

        Err(ApiError::BlockNotFound { id: id.to_string() })
    }

    /// Find the shared doc whose tree holds `tree_id` as a live node.
    fn scan_shared_for_tree_id(&self, tree_id: loro::TreeID) -> Option<WriteTarget> {
        let store = self.shared_trees.as_ref()?;
        for shared_tree_id in store.shared_tree_ids() {
            if let Some(doc) = store.get_shared_doc(&shared_tree_id)
                && is_node_alive(&doc.get_tree(TREE_NAME), tree_id)
            {
                return Some(WriteTarget::Shared {
                    shared_tree_id,
                    doc,
                    tree_id,
                });
            }
        }
        None
    }

    /// Find the shared doc whose tree holds a live node with stable id `id`.
    fn scan_shared_for_stable_id(&self, id: &str) -> Option<WriteTarget> {
        let store = self.shared_trees.as_ref()?;
        // ALLOW(entity_uri_from_raw): backend string-id resolve surface (accepts both id formats)
        let uri = EntityUri::from_raw(id);
        let needle = uri.id();
        for shared_tree_id in store.shared_tree_ids() {
            if let Some(doc) = store.get_shared_doc(&shared_tree_id)
                && let Some(tree_id) = find_stable_id_in_doc(&doc, needle)
                && is_node_alive(&doc.get_tree(TREE_NAME), tree_id)
            {
                return Some(WriteTarget::Shared {
                    shared_tree_id,
                    doc,
                    tree_id,
                });
            }
        }
        None
    }

    /// Wrap the resolved target's doc in a `LoroDocument` for writing. Both arms
    /// use `from_existing` to reuse the already-configured inner `Arc<LoroDoc>`
    /// (the shared doc's text styles were latched at accept; re-`configure` via
    /// `LoroDocument::new` would corrupt them). A bare `doc.commit()` inside
    /// `with_write` fires the shared doc's already-attached save/sync/projection
    /// workers, so routed writes need no extra outbound plumbing.
    fn target_doc(&self, target: &WriteTarget) -> (LoroDocument, loro::TreeID) {
        match target {
            WriteTarget::Global(tree_id) => (
                LoroDocument::from_existing(self.collab_doc.doc(), self.collab_doc.doc_id()),
                *tree_id,
            ),
            WriteTarget::Shared {
                shared_tree_id,
                doc,
                tree_id,
            } => (
                LoroDocument::from_existing(doc.clone(), shared_tree_id.clone()),
                *tree_id,
            ),
        }
    }

    /// Wrap the doc that owns a to-be-created child (the parent's doc) for
    /// writing. The global arm reuses `collab_doc`; the shared arm reuses the
    /// already-configured shared `Arc<LoroDoc>` (same `from_existing` rationale
    /// as [`Self::target_doc`]).
    fn parent_doc(&self, target: &ParentWriteTarget) -> LoroDocument {
        match target {
            ParentWriteTarget::Global => {
                LoroDocument::from_existing(self.collab_doc.doc(), self.collab_doc.doc_id())
            }
            ParentWriteTarget::Shared {
                shared_tree_id,
                doc,
            } => LoroDocument::from_existing(doc.clone(), shared_tree_id.clone()),
        }
    }

    /// Is the resolved node a mount node (a pointer into a shared subtree)?
    /// Mounts are not editable content — deleting/moving/editing one is an
    /// unshare concern, not a block edit — so writers reject them.
    fn target_is_mount(&self, target: &WriteTarget) -> Result<bool, ApiError> {
        match target {
            WriteTarget::Global(tree_id) => self
                .collab_doc
                .with_read(|doc| Ok(is_mount_node(&doc.get_tree(TREE_NAME), *tree_id)))
                .map_err(|e| ApiError::InternalError {
                    message: format!("mount-node check (global) failed: {e}"),
                }),
            WriteTarget::Shared { doc, tree_id, .. } => {
                Ok(is_mount_node(&doc.get_tree(TREE_NAME), *tree_id))
            }
        }
    }

    /// Resolve a write target for a content/field edit and reject writes to a
    /// mount node. Centralized here so every routed content writer inherits the
    /// mount guard; reads call [`Self::resolve_write_target`] directly and may
    /// still resolve a mount node (reads of a mount are harmless).
    async fn resolve_write_target_checked(&self, id: &str) -> Result<WriteTarget, ApiError> {
        let target = self.resolve_write_target(id).await?;
        if self.target_is_mount(&target)? {
            return Err(ApiError::InvalidOperation {
                message: format!(
                    "block {id} is a mount node (a pointer into a shared subtree); \
                     mounts are not editable content — unshare instead of editing"
                ),
            });
        }
        Ok(target)
    }

    /// Resolve the doc a to-be-created child must land in, by its parent.
    /// `no_parent`/sentinel → the global doc (roots live globally). Otherwise
    /// probe the parent's owning doc exactly as [`Self::resolve_write_target`]
    /// does (global-first, then shared by stale TreeID, then shared by stable
    /// id). A parent found nowhere falls back to Global: it may be seeded later
    /// in the same batch, and `resolve_parent_tree_id`'s tree-walk covers that
    /// (or errors loudly there).
    async fn resolve_write_target_for_parent(
        &self,
        parent: &EntityUri,
    ) -> Result<ParentWriteTarget, ApiError> {
        if parent.is_no_parent() || parent.is_sentinel() {
            return Ok(ParentWriteTarget::Global);
        }
        if let Some(tree_id) = self.resolve_to_tree_id(parent.as_str()).await {
            let alive_global = self
                .collab_doc
                .with_read(|doc| Ok(is_node_alive(&doc.get_tree(TREE_NAME), tree_id)))
                .map_err(|e| ApiError::InternalError {
                    message: format!(
                        "resolve_write_target_for_parent: read global tree failed: {e}"
                    ),
                })?;
            if alive_global {
                return Ok(ParentWriteTarget::Global);
            }
            if let Some(WriteTarget::Shared {
                shared_tree_id,
                doc,
                ..
            }) = self.scan_shared_for_tree_id(tree_id)
            {
                return Ok(ParentWriteTarget::Shared {
                    shared_tree_id,
                    doc,
                });
            }
        }
        if let Some(WriteTarget::Shared {
            shared_tree_id,
            doc,
            ..
        }) = self.scan_shared_for_stable_id(parent.as_str())
        {
            return Ok(ParentWriteTarget::Shared {
                shared_tree_id,
                doc,
            });
        }
        // ALLOW(fallback): parent not resolvable yet ⇒ global create; a genuinely
        // missing parent errors loudly at `resolve_parent_tree_id` inside the write.
        Ok(ParentWriteTarget::Global)
    }

    pub async fn update_block_text(&self, id: &str, new_text: &str) -> Result<(), ApiError> {
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);

        write_doc
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

        // `get_block` now resolves shared stable ids through `resolve_write_target`,
        // so a uniform re-read works for both global and shared targets.
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);
        let marks_owned: Vec<holon_api::MarkSpan> = marks.to_vec();

        write_doc
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);
        let mark_owned = mark.clone();

        write_doc
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);
        let key_owned = key.to_string();

        write_doc
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);
        let s_owned = s.to_string();

        write_doc
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);

        write_doc
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

        // Route the create by parent: a child of a shared block is born in the
        // shared doc. The global `id_cache` must never receive a shared TreeID
        // (its keys index the global tree only), so the shared arm resolves the
        // parent against a throwaway cache and skips `cache_stable_id` below.
        let parent_target = self.resolve_write_target_for_parent(&parent_id).await?;
        let write_doc = self.parent_doc(&parent_target);
        let is_global = matches!(parent_target, ParentWriteTarget::Global);
        let id_cache = if is_global {
            self.id_cache.clone()
        } else {
            Arc::new(Mutex::new(HashMap::new()))
        };
        let (created_block, tree_id) = write_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let parent_tree_id = resolve_parent_tree_id(&tree, &id_cache, &parent_id)?;

                let node = tree.create(parent_tree_id)?;
                let meta = tree.get_meta(node)?;
                meta.insert(STABLE_ID, loro::LoroValue::from(stable_id.as_str()))?;
                write_content_to_meta(&meta, &content, None)?;
                replace_properties_in_meta(&meta, properties)?;
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

        if is_global {
            self.cache_stable_id(&stable_id, tree_id);
        }

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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);

        write_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                // Merge: write only the provided keys so a concurrent peer's
                // edit to a *different* property survives (H3). No read-modify-
                // write of the whole set — that would re-stamp untouched keys.
                merge_properties_into_meta(&meta, properties)?;
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);

        write_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                // Authoritative full set (org re-parse): keys absent from the new
                // set are deleted, including down to the empty set.
                replace_properties_in_meta(&meta, properties)?;
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);

        write_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                // Touch only the named fields (per-key convergence, H3): a
                // concurrent peer editing a different field is not clobbered.
                apply_field_changes_to_meta(&meta, fields)?;
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
        // ALLOW(entity_uri_from_raw): new_parent_id String from cell-registry field write Value
        let new_parent_uri = EntityUri::from_raw(&new_parent_id);

        // Reject a cross-doc re-parent (into/out of a shared subtree) before any
        // mutation; same-doc re-parents route to the owning doc.
        let source_target = self.resolve_write_target_checked(id).await?;
        let parent_target = self
            .resolve_write_target_for_parent(&new_parent_uri)
            .await?;
        if source_target.doc_key() != parent_target.doc_key() {
            return Err(ApiError::InvalidOperation {
                message: format!(
                    "cross-boundary move of a shared subtree is not supported yet: \
                     block {id} lives in doc {:?} but new parent {new_parent_id} lives in doc {:?}",
                    source_target.doc_key(),
                    parent_target.doc_key()
                ),
            });
        }
        let (write_doc, tree_id) = self.target_doc(&source_target);
        let id_cache = if source_target.doc_key().is_none() {
            self.id_cache.clone()
        } else {
            Arc::new(Mutex::new(HashMap::new()))
        };

        write_doc
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
        // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both id formats)
        let new_parent_uri = EntityUri::from_raw(new_parent_id);

        // Reject a cross-doc positioned move before any mutation; same-doc moves
        // route to the owning doc.
        let source_target = self.resolve_write_target_checked(target_id).await?;
        let parent_target = self
            .resolve_write_target_for_parent(&new_parent_uri)
            .await?;
        if source_target.doc_key() != parent_target.doc_key() {
            return Err(ApiError::InvalidOperation {
                message: format!(
                    "cross-boundary move of a shared subtree is not supported yet: \
                     block {target_id} lives in doc {:?} but new parent {new_parent_id} \
                     lives in doc {:?}",
                    source_target.doc_key(),
                    parent_target.doc_key()
                ),
            });
        }
        let (write_doc, target) = self.target_doc(&source_target);
        // Predecessor must resolve within the same owning doc. `resolve_write_target`
        // hands back that doc's TreeID (TreeIDs are globally unique).
        let predecessor = match predecessor_id {
            Some(p) => Some(self.target_doc(&self.resolve_write_target(p).await?).1),
            None => None,
        };
        let id_cache = if source_target.doc_key().is_none() {
            self.id_cache.clone()
        } else {
            Arc::new(Mutex::new(HashMap::new()))
        };

        let noop = write_doc
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

        write_doc
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
        let target = self
            .resolve_write_target_checked(tree_id_str)
            .await
            .map_err(|e| anyhow::anyhow!("set_block_tags: {e}"))?;
        let (write_doc, tree_id) = self.target_doc(&target);

        let serialized = serde_json::to_string(tags)?;

        write_doc.with_write(|doc| {
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
        let target = self
            .resolve_write_target_checked(tree_id_str)
            .await
            .map_err(|e| anyhow::anyhow!("set_block_requires: {e}"))?;
        let (write_doc, tree_id) = self.target_doc(&target);

        let serialized = serde_json::to_string(requires)?;

        write_doc.with_write(|doc| {
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
        let target = self
            .resolve_write_target_checked(tree_id_str)
            .await
            .map_err(|e| anyhow::anyhow!("set_source_language: {e}"))?;
        let (write_doc, tree_id) = self.target_doc(&target);

        write_doc.with_write(|doc| {
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

    /// Test-only: peek the stable-id cache WITHOUT the tree-walk that
    /// `find_tree_id_by_stable_id` performs on a miss. Used to assert that a
    /// shared child's id never leaks into the global `id_cache`.
    #[cfg(test)]
    pub fn peek_id_cache(&self, stable_id: &str) -> Option<loro::TreeID> {
        self.resolve_stable_id_cached(stable_id)
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

    /// The live doc's Lamport height — the E-solid oracle's clock-sync
    /// scalar (see `multi_peer::lamport_height` / `clock_parity_spike`).
    pub async fn lamport_height(&self) -> Result<u32, ApiError> {
        self.collab_doc
            .with_read(|doc| Ok(doc_lamport_height(doc)))
            .map_err(|e| ApiError::InternalError {
                message: format!("lamport_height: {e}"),
            })
    }

    /// The Loro tree's fractional index for `id` — the adapter's internal
    /// ordering encoding the projector writes to SQL `sort_key` (ADR 0005).
    /// `None` when the node carries no index yet. Tie-disambiguated the same
    /// way as the snapshot projection (`effective_sibling_sort_keys`) so the
    /// org-scan order writeback can never overwrite a disambiguated key with
    /// the raw tied fi.
    pub async fn block_sort_key(&self, id: &str) -> Result<Option<String>, ApiError> {
        let tree_id = self.require_tree_id(id).await?;
        self.collab_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                if tree.fractional_index(tree_id).is_none() {
                    return Ok(None);
                }
                let siblings = match get_node_parent(&tree, tree_id) {
                    Some(p) => tree.children(p).unwrap_or_default(),
                    None => tree.roots(),
                };
                let keys = effective_sibling_sort_keys(&tree, &siblings);
                Ok(siblings
                    .iter()
                    .position(|t| *t == tree_id)
                    .and_then(|i| keys[i].clone()))
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
        let target = self
            .resolve_write_target_checked(tree_id_str)
            .await
            .map_err(|e| anyhow::anyhow!("set_external_id: {e}"))?;
        let (write_doc, tree_id) = self.target_doc(&target);

        let ext_id = external_id.to_string();
        // STABLE_ID stores the raw ID (without block: prefix) since
        // block_uri_from_meta calls EntityUri::block() which adds the prefix.
        let raw_id = external_id
            .strip_prefix("block:")
            .unwrap_or(external_id)
            .to_string();
        write_doc.with_write(|doc| {
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
        // Route the read the same way writes route: global tree first, then the
        // shared subtree docs. A shared block's stable id is absent from the
        // global tree (pruned at share time), so a global-only resolver would
        // return `BlockNotFound`; `resolve_write_target` finds it in the owning
        // shared doc and hands back that doc's TreeID (only valid against it).
        let target = self.resolve_write_target(id).await?;
        let (read_doc, tree_id) = self.target_doc(&target);
        read_doc
            .with_read(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let parent_tid = get_node_parent(&tree, tree_id);
                Ok(read_block_from_tree(&tree, tree_id, parent_tid))
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to get block: {}", e),
            })
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
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);
        let block_before = self.get_block(id).await?;
        let content_clone = content.clone();

        write_doc
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
        // Route to the doc that actually holds the live node (global or a shared
        // subtree). `BlockNotFound` means already-gone (never seeded or
        // concurrently deleted) — idempotent success, no emit (R-2).
        let target = match self.resolve_write_target(id).await {
            Ok(target) => target,
            Err(ApiError::BlockNotFound { .. }) => return Ok(()),
            Err(e) => return Err(e),
        };
        let (write_doc, tree_id) = self.target_doc(&target);

        let mut did_delete = false;
        write_doc
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
        // Route by the doc that owns the source node, and reject a cross-doc
        // relocation up front (before any mutation). A move whose source and
        // destination-parent resolve to different docs (into/out of a shared
        // subtree, or between two shared subtrees) is not expressible as a
        // single Loro `mov`. Same-doc moves route to the owning doc.
        let source_target = self.resolve_write_target(id.as_str()).await?;
        if self.target_is_mount(&source_target)? {
            return Err(ApiError::InvalidOperation {
                message: format!(
                    "block {id} is a mount node; moving a mount is an unshare concern, \
                     not a block move"
                ),
            });
        }
        let parent_target = self.resolve_write_target_for_parent(&new_parent).await?;
        if source_target.doc_key() != parent_target.doc_key() {
            return Err(ApiError::InvalidOperation {
                message: format!(
                    "cross-boundary move of a shared subtree is not supported yet: \
                     block {id} lives in doc {:?} but new parent {new_parent} lives in doc {:?}",
                    source_target.doc_key(),
                    parent_target.doc_key()
                ),
            });
        }

        let block_before = self.get_block(id.as_str()).await?;
        let (write_doc, tree_id) = self.target_doc(&source_target);
        // Shared arm gets a throwaway cache (the global `id_cache` must not hold
        // shared TreeIDs); global arm uses the real cache.
        let id_cache = if source_target.doc_key().is_none() {
            self.id_cache.clone()
        } else {
            Arc::new(Mutex::new(HashMap::new()))
        };

        // Domain-level precondition (ADR 0005): the primary cycle / structure
        // guard, run before adapter dispatch. `tree.mov` below re-checks cycles
        // natively as defense-in-depth.
        write_doc
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

        write_doc
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

        if blocks.is_empty() {
            return Ok(Vec::new());
        }

        // Route the whole batch by parent. Every block must land in the SAME doc:
        // a batch straddling the global doc and a shared subtree (or two shared
        // subtrees) is rejected loudly — a Loro commit can't span docs, and
        // intra-batch parent chains (a block whose parent is a sibling created
        // earlier in the same batch, not yet in any tree) make per-block
        // regrouping ambiguous. Interim: single-doc batches only.
        let mut owning: Option<ParentWriteTarget> = None;
        for nb in &blocks {
            let target = self.resolve_write_target_for_parent(&nb.parent_id).await?;
            match &owning {
                None => owning = Some(target),
                Some(prev) if prev.doc_key() == target.doc_key() => {}
                Some(prev) => {
                    return Err(ApiError::InvalidOperation {
                        message: format!(
                            "create_blocks: batch straddles two docs ({:?} vs {:?}); \
                             cross-doc batch creation into a shared subtree is not supported",
                            prev.doc_key(),
                            target.doc_key()
                        ),
                    });
                }
            }
        }
        let owning = owning.expect("non-empty batch resolves at least one parent target");
        let write_doc = self.parent_doc(&owning);
        // Shared arm gets a throwaway cache: the global `id_cache` must never
        // hold a shared TreeID. Global arm uses (and populates) the real cache.
        let id_cache = if matches!(owning, ParentWriteTarget::Global) {
            self.id_cache.clone()
        } else {
            Arc::new(Mutex::new(HashMap::new()))
        };
        let created_blocks = write_doc
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

#[cfg(test)]
mod h3_property_convergence_tests {
    use super::*;
    use loro::{ExportMode, LoroDoc};

    /// Bidirectional delta exchange — the deterministic equivalent of two peers
    /// reaching the same merged state.
    fn sync_pair(a: &LoroDoc, b: &LoroDoc) {
        let a_delta = a.export(ExportMode::updates(&b.oplog_vv())).unwrap();
        if !a_delta.is_empty() {
            b.import(&a_delta).unwrap();
        }
        let b_delta = b.export(ExportMode::updates(&a.oplog_vv())).unwrap();
        if !b_delta.is_empty() {
            a.import(&b_delta).unwrap();
        }
    }

    fn seed_node(doc: &LoroDoc, props: &HashMap<String, Value>) -> loro::TreeID {
        let tree = doc.get_tree(TREE_NAME);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        replace_properties_in_meta(&meta, props).unwrap();
        doc.commit();
        node
    }

    fn read_props(doc: &LoroDoc, node: loro::TreeID) -> HashMap<String, Value> {
        let tree = doc.get_tree(TREE_NAME);
        let meta = tree.get_meta(node).unwrap();
        read_properties_from_meta(&meta)
    }

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    /// H3 core: two peers concurrently set *different* properties. Per-property
    /// LoroMap keys merge so both survive. The pre-H3 single-JSON-blob would have
    /// dropped one peer's whole change to blob-level LWW.
    #[test]
    fn concurrent_distinct_properties_both_survive() {
        let base = LoroDoc::new();
        let node = seed_node(&base, &HashMap::from([("STATUS".to_string(), s("open"))]));

        let peer_a = base.fork();
        let peer_b = base.fork();

        let meta_a = peer_a.get_tree(TREE_NAME).get_meta(node).unwrap();
        apply_field_changes_to_meta(&meta_a, &[("TODO".to_string(), Value::Null, s("DONE"))])
            .unwrap();
        peer_a.commit();

        let meta_b = peer_b.get_tree(TREE_NAME).get_meta(node).unwrap();
        apply_field_changes_to_meta(&meta_b, &[("PRIORITY".to_string(), Value::Null, s("A"))])
            .unwrap();
        peer_b.commit();

        sync_pair(&peer_a, &peer_b);

        for doc in [&peer_a, &peer_b] {
            let props = read_props(doc, node);
            assert_eq!(props.get("STATUS"), Some(&s("open")), "untouched key kept");
            assert_eq!(props.get("TODO"), Some(&s("DONE")), "peer A's change kept");
            assert_eq!(props.get("PRIORITY"), Some(&s("A")), "peer B's change kept");
        }
    }

    /// A pre-H3 block carries the legacy single-blob string. It reads back
    /// correctly, and the first write migrates it to the nested map and drops the
    /// legacy key (self-healing — no lingering dual representation).
    #[test]
    fn legacy_blob_is_read_then_migrated_on_write() {
        let doc = LoroDoc::new();
        let tree = doc.get_tree(TREE_NAME);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        let json =
            serde_json::to_string(&HashMap::from([("TODO".to_string(), s("TODO"))])).unwrap();
        meta.insert(PROPERTIES, loro::LoroValue::from(json.as_str()))
            .unwrap();
        doc.commit();

        assert_eq!(
            read_properties_from_meta(&meta).get("TODO"),
            Some(&s("TODO"))
        );

        merge_properties_into_meta(&meta, &HashMap::from([("PRIORITY".to_string(), s("B"))]))
            .unwrap();
        doc.commit();

        assert!(
            meta.get(PROPERTIES).is_none(),
            "legacy blob deleted after migrating write"
        );
        let props = read_properties_from_meta(&meta);
        assert_eq!(
            props.get("TODO"),
            Some(&s("TODO")),
            "legacy value carried into nested map"
        );
        assert_eq!(props.get("PRIORITY"), Some(&s("B")));
    }

    /// `replace_properties_in_meta` is the EXACT-set writer: keys absent from the
    /// new set are deleted, down to the empty set.
    #[test]
    fn replace_deletes_absent_keys() {
        let doc = LoroDoc::new();
        let node = seed_node(
            &doc,
            &HashMap::from([("A".to_string(), s("1")), ("B".to_string(), s("2"))]),
        );
        let meta = doc.get_tree(TREE_NAME).get_meta(node).unwrap();

        replace_properties_in_meta(&meta, &HashMap::from([("A".to_string(), s("9"))])).unwrap();
        doc.commit();
        let props = read_properties_from_meta(&meta);
        assert_eq!(props.get("A"), Some(&s("9")), "kept key updated");
        assert_eq!(props.get("B"), None, "absent key deleted");

        replace_properties_in_meta(&meta, &HashMap::new()).unwrap();
        doc.commit();
        assert!(
            read_properties_from_meta(&meta).is_empty(),
            "empty set clears all"
        );
    }
}
