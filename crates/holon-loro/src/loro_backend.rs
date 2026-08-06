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

use std::collections::HashMap;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::ApiError;
use holon_api::Block;
use holon_api::BlockContent;
use holon_api::Change;
use holon_api::ChangeOrigin;
use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::SourceBlock;
use holon_api::StreamPosition;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block_mutation::BlockMutation;
use holon_api::block_mutation::BlockTreeView;
use holon_api::repository::CoreOperations;
use holon_api::repository::Lifecycle;
use holon_api::repository::NewBlock;
use holon_api::repository::P2POperations;
use holon_api::streaming::ChangeNotifications;
use holon_api::streaming::ChangeSubscribers;
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::LoroDocument;
use crate::event_ring::DEFAULT_EVENT_RING_CAPACITY;
use crate::event_ring::EventRing;
use crate::event_ring::deliver_to_subscribers;
use crate::shared_tree::SharedTreeStore;
use crate::shared_tree::is_mount_node;
use crate::shared_tree::read_mount_info;

// Field name constants
pub const CONTENT_TYPE: &str = "content_type";
pub const CONTENT_RAW: &str = "content_raw";
pub const SOURCE_LANGUAGE: &str = "source_language";
pub const SOURCE_CODE: &str = "source_code";
const SOURCE_NAME: &str = "source_name";
const SOURCE_HEADER_ARGS: &str = "source_header_args";
/// Legacy single-blob property storage: one `LoroMap` key holding the whole
/// property map as one opaque JSON string. Read-only now (read when migrating a
/// pre-H3 block) — blob-level LWW meant two peers setting *different*
/// properties clobbered each other (H3). Superseded by [`PROPERTIES_MAP`].
const PROPERTIES: &str = "properties";
/// H3: properties stored as a nested `LoroMap`, one key per property. A
/// `LoroMap` resolves conflicts per key, so concurrent edits to *different*
/// properties (e.g. `TODO` vs `PRIORITY`) merge instead of one peer's blob
/// winning. Each value is its `serde_json::Value` JSON-encoded into a string.
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
    use holon_api::EntityRef;
    use holon_api::InlineMark;
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
                // `internal` and `unknown_scheme` are the two pre-merge
                // spellings of a scheme-shaped target (they differed only in
                // whether the scheme was registered at ingest, which is no
                // longer persisted); their payload key differed too.
                // `internal` (key `id`) and `unknown_scheme` (key `uri`) are the
                // two pre-merge spellings. The target is stored verbatim, never
                // parsed here: `unknown_scheme` also held wiki paths that are
                // not valid URIs, and coercing those would silently rewrite
                // authored link text.
                "scheme" | "internal" | "unknown_scheme" => {
                    let stored = map
                        .get("raw")
                        .or_else(|| map.get("uri"))
                        .or_else(|| map.get("id"));
                    let raw = stored.and_then(|v| match v {
                        loro::LoroValue::String(s) => Some(s.to_string()),
                        _ => None,
                    })?;
                    EntityRef::Scheme { raw }
                }
                "name" => {
                    let name = map.get("name").and_then(|v| match v {
                        loro::LoroValue::String(s) => Some(s.to_string()),
                        _ => None,
                    })?;
                    EntityRef::Name { name }
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
/// is a `LoroValue::Map` carrying
/// `{ "type": "external"|"scheme"|"name", "url"|"raw"|"name": ..., "label": ...
/// }` so the render layer can reconstruct the full `EntityRef`+label without
/// going back to `Block.marks`. The reader also accepts the two pre-merge
/// spellings of `scheme` (`internal` with an `id` key, `unknown_scheme` with a
/// `uri` key); nothing writes them any more.
pub fn mark_to_loro_value(mark: &holon_api::InlineMark) -> loro::LoroValue {
    use holon_api::EntityRef;
    use holon_api::InlineMark;
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
                EntityRef::Scheme { raw } => {
                    map.insert("type".to_string(), loro::LoroValue::from("scheme"));
                    map.insert("raw".to_string(), loro::LoroValue::from(raw.as_str()));
                }
                EntityRef::Name { name } => {
                    map.insert("type".to_string(), loro::LoroValue::from("name"));
                    map.insert("name".to_string(), loro::LoroValue::from(name.as_str()));
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
    use loro::ExpandType;
    use loro::StyleConfig;
    use loro::StyleConfigMap;

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
    // ALLOW(entity_uri_from_raw): str_to_tree_id(&str) backend string-id resolve
    // surface
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
            BlockContent::Image { path }
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
    //
    // Same treatment for the first-class block COLUMNS (`created_at`,
    // `updated_at`, …): each is a typed `Block` field read from its own
    // top-level meta key (`created_at`/`updated_at` via `get_typed` below;
    // `id`/`content`/`parent`/`sort_key` via their own readers), and the SQL
    // sink stores each in its own column — NEVER in the `properties` JSON blob.
    // But org ingest lifts drawer keys (`:UPDATED_AT:`, `:CREATED_AT:`, …) into
    // `block.properties`, which then land in the Loro PROPERTIES_MAP. Left in,
    // the Loro-read block's `properties` carries e.g. `{"updated_at": …}` while
    // the SQL-read block's is `{}` — so `blocks_differ` fires spuriously, yet
    // `block_diff_params` can't represent the change (`updated_at` collides with
    // the always-emitted bookkeeping `updated_at` via `or_insert`), producing a
    // bookkeeping-only update that decodes to zero typed ops and trips the
    // consolidator's `agrees_with_ops` divergence. Stripping here makes Loro's
    // `properties` agree with SQL's by construction. (`RESERVED_PROPERTY_KEYS`
    // mirrors `schema_modules::BLOCK_RAW_COLUMNS`.)
    //
    // Edge-typed fields (junction tables, not raw columns) get the same
    // treatment for the same reason — they leak in as `Array([])` and, being
    // absent from the SQL `properties` blob, trip the identical divergence.
    props.remove("tags");
    props.remove("requires");
    props.remove("advice_suppressed");
    for key in RESERVED_PROPERTY_KEYS {
        props.remove(*key);
    }
    props
}

/// Block columns / typed fields that must never appear in the generic
/// `properties` map — each has a dedicated typed `Block` slot and SQL column.
/// Mirrors `holon_turso::schema_modules::BLOCK_RAW_COLUMNS` (minus `properties`
/// itself). Edge fields (`tags`/`requires`/`advice_suppressed`) are stripped
/// separately above. Kept as a local const (rather than a cross-crate import)
/// to match the existing hardcoded edge strip in this module.
///
/// `collapsed` and `widget_only` are deliberately EXCLUDED: unlike the other
/// columns they ARE stored in the Loro properties map (a `set_field` scalar),
/// and `read_block_from_tree` lifts them out of `properties` into their typed
/// slots — stripping them here would make that lift always read the default.
const RESERVED_PROPERTY_KEYS: &[&str] = &[
    "id",
    "parent_id",
    "depth",
    "sort_key",
    "content",
    "content_type",
    "source_language",
    "source_name",
    "marks",
    "completed",
    "block_type",
    "created_at",
    "updated_at",
    "_change_origin",
];

/// Read the stable ID from a node's metadata.
fn read_stable_id(meta: &loro::LoroMap) -> Option<String> {
    meta.get_typed(STABLE_ID, |val| val.as_string().map(|s| s.to_string()))
}

/// Disclose a child withheld from a `list_children` answer because its
/// `STABLE_ID` had not landed yet.
fn warn_half_born(node: loro::TreeID, parent_id: &str) {
    tracing::warn!(
        ?node,
        parent_id,
        "list_children: live child has no STABLE_ID yet (in-flight create, meta not landed); \
         withholding it from this answer — callers re-read until it appears"
    );
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
    let mut properties = read_properties_from_meta(&meta);
    // `collapsed` is a typed Block field (document state, 2026-07-11 ruling)
    // but `set_field(collapsed)` lands in the Loro properties map like every
    // other scalar (apply_field_changes_to_meta). Lift it into the typed slot
    // at this read boundary — parse-don't-validate — so a Loro-derived Block
    // agrees field-for-field with a SQL-derived one (whose TryFrom reads the
    // `collapsed` column) and org writeback emits one `:COLLAPSED:` drawer.
    let collapsed = match properties.remove("collapsed") {
        None => false,
        Some(Value::Boolean(b)) => b,
        Some(Value::Integer(i)) => i != 0,
        Some(other) => panic!("corrupt `collapsed` property in Loro tree: {other:?}"),
    };
    let widget_only = match properties.remove("widget_only") {
        None => false,
        Some(Value::Boolean(b)) => b,
        Some(Value::Integer(i)) => i != 0,
        Some(other) => panic!("corrupt `widget_only` property in Loro tree: {other:?}"),
    };

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
    let advice_suppressed = read_advice_suppressed_from_meta(&meta);

    let mut block = Block::from_block_content(id, parent_id, content);
    block.set_properties_map(properties);
    block.tags = tags.into();
    block.requires = requires;
    block.advice_suppressed = advice_suppressed;
    block.collapsed = collapsed;
    block.widget_only = widget_only;
    block.created_at = created_at;
    block.updated_at = updated_at;
    block
}

// `SnapshotBlock` is a backend-neutral data type (a `Block` + its fractional
// `sort_key`); it now lives in `holon-api` (along with the `SnapshotBlockWire`
// lossless-serde representation — BUG H1). The Loro adapter still builds it
// here from a `LoroDoc` (see `snapshot_blocks_from_doc*`), but the type itself
// is shared. Re-exported so `crate::loro_backend::SnapshotBlock` keeps
// resolving.
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

/// Whether the tree node `tid` carries the `Page` tag in its Loro meta — the
/// same `tags` metadata the SQL `block_tags` projection mirrors. Used by the
/// share backend to resolve a mount's nearest page ancestor (Amendment A)
/// directly from Loro, without a SQL read seam. `get_meta` failing on a node
/// we are actively walking is a real corruption, not "no page" — propagate it.
pub(crate) fn node_is_page(tree: &loro::LoroTree, tid: loro::TreeID) -> anyhow::Result<bool> {
    let meta = tree
        .get_meta(tid)
        .map_err(|e| anyhow::anyhow!("node_is_page get_meta({tid:?}): {e}"))?;
    Ok(read_tags_from_meta(&meta)
        .iter()
        .any(|t| t == holon_api::block::PAGE_TAG))
}

/// Read the `requires` JSON-encoded list (org-edna dependency edge field) from
/// a node's metadata. Stored under a dedicated `requires` meta key — like
/// `tags`, it is an edge field (the `block_requires` junction), never part of
/// the generic `properties` blob. Returns an empty `Vec` when absent. Malformed
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

/// Read the `advice_suppressed` JSON-encoded list (advice-suppression exclusion
/// set, ADR 0021) from a node's metadata. Mirrors `read_requires_from_meta`:
/// an edge field stored under its own meta key (the `advice_suppressed`
/// junction), never in the generic `properties` blob. Empty when absent;
/// malformed JSON is corruption of our own metadata — fail loud.
fn read_advice_suppressed_from_meta(meta: &loro::LoroMap) -> Vec<EntityUri> {
    meta.get_typed("advice_suppressed", |val| {
        val.as_string().map(|s| s.to_string())
    })
    .map(|s| {
        serde_json::from_str::<Vec<String>>(&s)
            .unwrap_or_else(|e| panic!("corrupt `advice_suppressed` metadata JSON {s:?}: {e}"))
            .into_iter()
            .map(|r| {
                EntityUri::parse_owned(r).expect("stored advice_suppressed must be a valid URI")
            })
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
    let text = crate::mergeable_child::ensure_text(meta, key)?;
    text.update(new_text, Default::default())
        .map_err(|e| anyhow::anyhow!("LoroText update failed: {:?}", e))?;
    Ok(())
}

fn write_content_to_meta(meta: &loro::LoroMap, content: &BlockContent) -> anyhow::Result<()> {
    match content {
        BlockContent::Text { raw } => {
            meta.insert(
                CONTENT_TYPE,
                loro::LoroValue::from(ContentType::Text.to_string().as_str()),
            )?;
            update_text_field(meta, CONTENT_RAW, raw)?;
        }
        BlockContent::RichText { text, marks } => {
            // Write the text, then apply the inline marks as Loro Peritext — the
            // SAME write `update_block_marked` performs. Without this, a block
            // CREATED with rich content (e.g. an org-ingested `[[link]]` whose
            // `to_block_content()` yields `RichText`) would land in the tree as
            // plain text: the Peritext marks would never exist, so readback
            // (`read_text_marks`) returns `None`, the SQL `marks` column projects
            // NULL, and write-back re-renders the stripped label — destroying the
            // link syntax on disk (`inv-blocks-match-ref/org` marks divergence).
            meta.insert(
                CONTENT_TYPE,
                loro::LoroValue::from(ContentType::Text.to_string().as_str()),
            )?;
            update_text_field(meta, CONTENT_RAW, text)?;
            let loro_text = crate::mergeable_child::ensure_text(meta, CONTENT_RAW)?;
            // Clear every known mark key over the full range first so a re-write
            // with fewer marks drops the stale ones, then set the current spans.
            let len_chars = loro_text.len_unicode();
            if len_chars > 0 {
                for key in holon_api::InlineMark::all_loro_keys() {
                    loro_text
                        .unmark(0..len_chars, key)
                        .map_err(|e| anyhow::anyhow!("LoroText unmark {key}: {:?}", e))?;
                }
            }
            for span in marks {
                let key = span.mark.loro_key();
                let value: loro::LoroValue = mark_to_loro_value(&span.mark);
                loro_text
                    .mark(span.start..span.end, key, value)
                    .map_err(|e| anyhow::anyhow!("LoroText mark {key}: {:?}", e))?;
            }
        }
        BlockContent::Image { path } => {
            meta.insert(
                CONTENT_TYPE,
                loro::LoroValue::from(ContentType::Image.to_string().as_str()),
            )?;
            update_text_field(meta, CONTENT_RAW, path)?;
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

/// Open (creating if absent) the nested per-property `LoroMap` (H3,
/// [`PROPERTIES_MAP`]).
pub(crate) fn properties_map_container(meta: &loro::LoroMap) -> anyhow::Result<loro::LoroMap> {
    crate::mergeable_child::ensure_map(meta, PROPERTIES_MAP)
}

/// Read one scalar block field's [`Value`] straight from a tree node's `meta`
/// map, mirroring the decode `read_properties_from_meta` performs at the whole-
/// block level: prefer the nested per-property map (H3), fall back to the
/// legacy single-blob until a write migrates it. `None` when the key is absent.
/// This is the per-field read the `LoroMetaCellBacking<T>` scalar cell projects
/// — it must agree with the whole-block projection so a cell read and a
/// `get_block` read of the same field never diverge.
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
    // Legacy pre-H3 single-blob path (self-heals on the next write, which
    // migrates).
    let json = meta.get_typed(PROPERTIES, |val| val.as_string().map(|s| s.to_string()))?;
    let legacy: HashMap<String, Value> = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("Corrupt properties JSON in Loro tree: {json:?}: {e}"));
    legacy.get(key).cloned()
}

/// Encode one property value as the JSON string stored under its key.
/// Properties are arbitrary `serde_json::Value`; the per-key granularity (not
/// per-field) is what H3 needs — concurrent edits to *different* properties are
/// different keys.
fn encode_property_value(value: &Value) -> anyhow::Result<loro::LoroValue> {
    Ok(loro::LoroValue::from(
        serde_json::to_string(value)?.as_str(),
    ))
}

/// Decode the nested per-property `LoroMap` back into a property map. Each
/// value must be the JSON string an H3 write produced — anything else is
/// corruption, so panic rather than silently dropping it.
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
/// ([`replace_properties_in_meta`]) use this — they already define the entire
/// set, so the blob's contents are intentionally discarded.
fn drop_legacy_properties_blob(meta: &loro::LoroMap) -> anyhow::Result<()> {
    if meta.get(PROPERTIES).is_some() {
        meta.delete(PROPERTIES)?;
    }
    Ok(())
}

/// Copy any legacy single-blob properties into the nested map (only keys not
/// already present) and drop the legacy key. Partial writers (merge /
/// per-field) call this first so a pre-H3 block's *untouched* properties
/// survive its first partial write instead of being dropped with the blob.
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
/// write intact, instead of a read-modify-write re-stamping it with a stale
/// value.
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
/// deleted. Authoritative full-set writes (block creation, org re-parse) use
/// this.
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

/// Outcome of resolving a parent `EntityUri` to a live tree node. Classifying
/// the failure here (rather than returning an error) lets the write-boundary
/// wrappers pick the right *typed* error: the create path surfaces the shared
/// [`holon_api::ParentNotFound`], while read/move paths keep a generic message.
enum ParentResolution {
    /// No parent — the node is a tree root (no_parent / sentinel URI).
    Root,
    /// Parent resolved to this live tree node.
    Node(loro::TreeID),
    /// The parent URI could not be resolved to a live node in this tree.
    Unresolvable,
}

fn resolve_parent_core(
    tree: &loro::LoroTree,
    id_cache: &Arc<Mutex<HashMap<String, loro::TreeID>>>,
    parent_uri: &EntityUri,
) -> ParentResolution {
    if parent_uri.is_no_parent() || parent_uri.is_sentinel() {
        return ParentResolution::Root;
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
                let cached = id_cache.lock().unwrap().get(parent_uri.id()).copied();
                match cached {
                    // Live cache hit — use it.
                    Some(tid) if !node_deleted_now(tree, tid) => Some(tid),
                    // Stale hit: the cached node is TOMBSTONED. loro 1.12's
                    // get_meta returns Ok for a tombstoned (deleted-but-still-
                    // existing) node, so the old `get_meta(tid).is_ok()` guard
                    // would serve it as a live parent — attaching children under
                    // a dead node. This bites cache entries tombstoned by a
                    // REMOTE / CRDT-merge delete, which never runs delete_block's
                    // uncache. Drop the stale entry and fall through to the tree
                    // walk below, which re-resolves to the live node if the same
                    // stable id was recreated under a new TreeID.
                    Some(_) => {
                        id_cache.lock().unwrap().remove(parent_uri.id());
                        None
                    }
                    None => None,
                }
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
        });
    match tree_id {
        // Confirm the resolved node actually carries meta (i.e. it is a live
        // node, not a stale cache hit into a deleted/unexist slot).
        Some(tid) if tree.get_meta(tid).is_ok() => ParentResolution::Node(tid),
        _ => ParentResolution::Unresolvable,
    }
}

/// Read/move-path parent resolution: a missing parent is a generic anyhow
/// error.
fn resolve_parent_tree_id(
    tree: &loro::LoroTree,
    id_cache: &Arc<Mutex<HashMap<String, loro::TreeID>>>,
    parent_uri: &EntityUri,
) -> anyhow::Result<Option<loro::TreeID>> {
    match resolve_parent_core(tree, id_cache, parent_uri) {
        ParentResolution::Root => Ok(None),
        ParentResolution::Node(tid) => Ok(Some(tid)),
        ParentResolution::Unresolvable => Err(anyhow::anyhow!(
            "Cannot resolve parent URI to TreeID: {parent_uri}"
        )),
    }
}

/// Create-path parent resolution at the write boundary: an unresolvable parent
/// is the shared, typed [`holon_api::ParentNotFound`] — the anyhow *source*, so
/// callers up the stack can downcast to it (and can add context freely on top).
fn resolve_parent_tree_id_for_create(
    tree: &loro::LoroTree,
    id_cache: &Arc<Mutex<HashMap<String, loro::TreeID>>>,
    parent_uri: &EntityUri,
    child_uri: &EntityUri,
) -> anyhow::Result<Option<loro::TreeID>> {
    match resolve_parent_core(tree, id_cache, parent_uri) {
        ParentResolution::Root => Ok(None),
        ParentResolution::Node(tid) => Ok(Some(tid)),
        ParentResolution::Unresolvable => Err(anyhow::Error::new(holon_api::ParentNotFound {
            parent_id: parent_uri.clone(),
            child_id: child_uri.clone(),
        })),
    }
}

/// One block create with its full authority payload — everything that must
/// land in the SAME Loro commit as the node itself, so the outbound projector
/// never sees a half-born block (a dropped `Page` tag orphans a document).
#[derive(Debug, Clone)]
pub struct NewBlockWithProperties {
    /// Where the node is created. Already resolved to a live parent (or the
    /// placeholder standing in for one) by the caller.
    pub parent_id: EntityUri,
    /// The block's stable identity.
    pub id: EntityUri,
    pub content: BlockContent,
    pub properties: HashMap<String, Value>,
    pub tags: Tags,
    pub requires: Vec<EntityUri>,
    pub advice_suppressed: Vec<EntityUri>,
}

/// Write ONE new node (node + STABLE_ID + content + properties + edge fields +
/// timestamps) into `tree`, returning the domain block and its `TreeID`.
///
/// Deliberately does NOT commit: the caller owns the commit boundary, which is
/// what lets a batch create N nodes in one commit while the single-block path
/// keeps its own. Sole writer of a new node's meta, so the two paths cannot
/// drift.
fn write_new_node(
    tree: &loro::LoroTree,
    id_cache: &Arc<Mutex<HashMap<String, loro::TreeID>>>,
    request: &NewBlockWithProperties,
    now: i64,
) -> anyhow::Result<(Block, loro::TreeID)> {
    let stable_id = request.id.id().to_string();
    let parent_tree_id =
        resolve_parent_tree_id_for_create(tree, id_cache, &request.parent_id, &request.id)?;

    let node = tree.create(parent_tree_id)?;
    let meta = tree.get_meta(node)?;
    meta.insert(STABLE_ID, loro::LoroValue::from(stable_id.as_str()))?;
    write_content_to_meta(&meta, &request.content)?;
    replace_properties_in_meta(&meta, &request.properties)?;
    // Tags are edge fields (block_tags), stored in Loro meta as a JSON list
    // under "tags" (mirrors `set_block_tags`). Carrying them in the create
    // commit is essential: the downstream projection reads them via
    // `read_block_from_tree` and writes `block_tags`. The `Page` tag in
    // particular makes a document resolvable — dropping it here orphans
    // every doc. `requires` and `advice_suppressed` (ADR 0021) mirror it.
    if !request.tags.is_empty() {
        let serialized = serde_json::to_string(&request.tags)
            .map_err(|e| anyhow::anyhow!("serialize tags: {e}"))?;
        meta.insert("tags", loro::LoroValue::from(serialized.as_str()))?;
    }
    if !request.requires.is_empty() {
        let serialized = serde_json::to_string(&request.requires)
            .map_err(|e| anyhow::anyhow!("serialize requires: {e}"))?;
        meta.insert("requires", loro::LoroValue::from(serialized.as_str()))?;
    }
    if !request.advice_suppressed.is_empty() {
        let serialized = serde_json::to_string(&request.advice_suppressed)
            .map_err(|e| anyhow::anyhow!("serialize advice_suppressed: {e}"))?;
        meta.insert(
            "advice_suppressed",
            loro::LoroValue::from(serialized.as_str()),
        )?;
    }
    meta.insert("created_at", loro::LoroValue::from(now))?;
    meta.insert("updated_at", loro::LoroValue::from(now))?;

    let parent_uri = match parent_tree_id {
        Some(pid) => block_uri_from_meta(&tree.get_meta(pid)?, pid),
        None => EntityUri::no_parent(),
    };
    let mut block = Block::from_block_content(
        EntityUri::block(&stable_id),
        parent_uri,
        request.content.clone(),
    );
    block.set_properties_map(request.properties.clone());
    block.tags = request.tags.clone();
    block.requires = request.requires.clone();
    block.advice_suppressed = request.advice_suppressed.clone();
    block.created_at = now;
    block.updated_at = now;
    Ok((block, node))
}

/// Get the parent TreeID of a node.
fn get_node_parent(tree: &loro::LoroTree, node: loro::TreeID) -> Option<loro::TreeID> {
    match tree.parent(node)? {
        loro::TreeParentId::Node(pid) => Some(pid),
        _ => None,
    }
}

/// Is this node deleted (or unknown) in the tree's CURRENT state? Used by the
/// snapshot readers to distinguish a torn walk — a concurrent commit deleted
/// the node between enumeration and the per-node reads — from a genuine
/// ordering-invariant violation on a live node. `Err` from `is_node_deleted`
/// means the tree no longer knows the node at all: the same torn shape.
fn node_deleted_now(tree: &loro::LoroTree, node: loro::TreeID) -> bool {
    tree.is_node_deleted(&node).unwrap_or(true)
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
/// sink row, which the next settled snapshot re-creates (an add/remove CDC
/// churn cycle — `inv-editable-text-has-draggable`). A genuinely deleted node
/// parents to `Deleted`/`Unexist` (the first `continue` below) and does **not**
/// flip `settled`, so real deletes still flow.
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
            // Torn walk: `get_nodes` is a point-in-time enumeration, but a
            // concurrent commit may delete this node (or an ancestor) before
            // the per-node reads above run. The enumerated `node.parent` is
            // then stale, so the sibling-group lookup misses. That is a benign
            // in-flight delete, NOT an ADR 0005 violation: withhold silently
            // (exactly like the missing-meta skip above) and let the writer's
            // own commit trigger the clean re-projection.
            if node_deleted_now(&tree, node.id) {
                tracing::debug!(
                    block_id = %block.id,
                    node = ?node.id,
                    "loro projection: node deleted mid-walk; withholding from \
                     this (unsettled) snapshot"
                );
                settled = false;
                continue;
            }
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
/// `ORDER BY sort_key, id` tiebreak — id-string order, random from the user's
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

/// Reverse-map a changed container (from `doc.diff`) to the tree node that owns
/// it — the deepest `Index::Node` on its container path. A block's content
/// (text) and properties (nested map) live in containers under the node's meta
/// map, so a content/property edit surfaces as a `Map`/`Text` diff on such a
/// container; this recovers the owning `TreeID`. Returns `None` for containers
/// with no tree-node ancestor (not part of the block tree).
fn owning_tree_node(doc: &loro::LoroDoc, cid: &loro::ContainerID) -> Option<loro::TreeID> {
    let path = doc.get_path_to_container(cid)?;
    path.iter().rev().find_map(|(_, idx)| match idx {
        loro::Index::Node(tid) => Some(*tid),
        _ => None,
    })
}

/// Children `TreeID`s of a `TreeParentId` scope in Loro child order (the same
/// order `snapshot_blocks_from_doc_settled` reads for sibling sort keys).
fn children_of_scope(tree: &loro::LoroTree, scope: loro::TreeParentId) -> Vec<loro::TreeID> {
    match scope {
        loro::TreeParentId::Node(pid) => tree.children(pid).unwrap_or_default(),
        loro::TreeParentId::Root => tree.roots(),
        loro::TreeParentId::Deleted | loro::TreeParentId::Unexist => Vec::new(),
    }
}

/// Read ONE live tree node into a `(stable_id, SnapshotBlock)`, sharing the
/// sibling tie-break recompute across a group via `group_keys` (parent scope →
/// {child → sort_key}). Returns `None` — WITHOUT faking an A0 key — when the
/// node is transiently incomplete (missing meta / stable id / fractional
/// index): identical withhold semantics to the full-snapshot reader
/// (`snapshot_blocks_from_doc_settled`), so the caller can mark the pass
/// unsettled and retry.
fn read_one_node_snapshot(
    tree: &loro::LoroTree,
    node: loro::TreeID,
    group_keys: &mut HashMap<loro::TreeParentId, HashMap<loro::TreeID, Option<String>>>,
) -> Option<(String, SnapshotBlock)> {
    let meta = tree.get_meta(node).ok()?; // ALLOW(ok): absence = node gone mid-batch; None withholds it
    let stable_id = read_stable_id(&meta)?;
    let parent_tid = get_node_parent(tree, node);
    let scope = match parent_tid {
        Some(p) => loro::TreeParentId::Node(p),
        None => loro::TreeParentId::Root,
    };
    let keys = group_keys.entry(scope).or_insert_with(|| {
        let siblings = children_of_scope(tree, scope);
        let ks = effective_sibling_sort_keys(tree, &siblings);
        siblings.into_iter().zip(ks).collect()
    });
    let Some(Some(sort_key)) = keys.get(&node).cloned() else {
        // Torn read: the node (or an ancestor) was deleted by a concurrent
        // commit between the caller's batch capture and this per-node read —
        // a benign in-flight delete, not an ADR 0005 violation. Returning
        // `None` withholds it; the caller's unsettled handling retries.
        if node_deleted_now(tree, node) {
            tracing::debug!(
                block_id = %stable_id,
                node = ?node,
                "loro incremental projection: node deleted mid-batch; withholding"
            );
            return None;
        }
        // A live, non-deleted node whose fractional index has not yet landed in
        // THIS O(changed) observation window. Returning `None` sets the pass
        // `settled=false`, which routes it to the full-snapshot reseed — a
        // DISCLOSED RETRY, not a swallowed failure. The reseed's full-walk reader
        // (`snapshot_blocks_from_doc_settled`) is the AUTHORITATIVE persistent
        // check: if the fi is genuinely missing (a real ADR-0005 violation) it
        // still logs ERROR there and fails `inv-no-observed-errors`. A transient
        // mid-mutation fi (the fi op commits microseconds after the node becomes
        // visible) resolves by the reseed and never reaches that ERROR. So WARN
        // here (visible, attributable) and let the reseed be the arbiter of
        // persistence — an incremental first-observation must not itself trip the
        // no-swallowed-errors gate on what is a normal retry window.
        tracing::warn!(
            block_id = %stable_id,
            ?scope,
            node = ?node,
            "loro incremental projection: live node has no fractional index in this \
             O(changed) window; withholding and reseeding (disclosed retry). The \
             full-snapshot reseed is the authoritative persistent-violation check."
        );
        return None;
    };
    let block = read_block_from_tree(tree, node, parent_tid);
    // Key by the SCHEMED id (`block:<stable>`), exactly as the full-snapshot
    // reader keys `blocks` — the `live`/SQL rows use the schemed id. Keying by
    // the bare `stable_id` here would make every update look like a create
    // (live grows unboundedly) and every delete miss its schemed sink row.
    Some((block.id.to_string(), SnapshotBlock { block, sort_key }))
}

/// Incremental block changes from a batch of `PendingChange` facts (drained
/// from the `subscribe_root` event stream), cost proportional to the number of
/// changed nodes, NOT the total tree size. This is the O(changed) replacement
/// for the full-document walk + full-map diff on every commit. It reads only
/// the CURRENT tree state for the named nodes — it never calls `doc.diff`,
/// which checked the shared live doc out and raced concurrent readers.
///
/// Returns `(changed, settled)` where `changed` maps each affected stable id to
/// its new state (`None` = the node was deleted). `settled` is `false` when a
/// touched live node was transiently incomplete (mid-mutation) — the caller
/// discards the incremental result and falls back to a full reseed for that
/// pass (which owns the delete-withhold gate), mirroring the full-snapshot
/// reader's unsettled handling.
///
/// Invariant preservation:
/// * **peer-sibling-order** — any Create/Move/Delete marks its parent scope(s)
///   dirty; every current member of a dirty scope is re-read so the
///   `effective_sibling_sort_keys` tie-break is recomputed for the WHOLE group
///   (a new/removed tied sibling shifts the `.<run_pos>` suffix of its peers).
///   A pure content/property edit touches no scope, so no sibling recompute.
/// * **delete-pass** — deletes come from the tree diff's `Delete` items; the
///   node's stable id is recovered from its (often still-present) meta or the
///   maintained `tid_index`, so a delete is never silently dropped.
///
/// `tid_index` (TreeID -> stable id) is maintained across calls so a deleted
/// node whose meta is already gone can still be mapped to the sink row.
/// A single owned "dirty fact" extracted from a Loro `subscribe_root` DiffEvent
/// on the committing thread. It names WHAT changed (a tree node create/move/
/// delete, or a content/property sub-container edit) without touching the doc —
/// so the subscribe callback appends these under a mutex with no `doc` access,
/// no re-entrant lock, and crucially no checkout. `project()` drains a batch
/// and reads the CURRENT tree state for the named nodes. This is the O(changed)
/// replacement for `doc.diff(from, to)`, which checked out the shared live doc
/// (to `from`, then `to`, then restored) and raced concurrent readers — the
/// root cause of the flaky `SplitBlock … Block not found`.
#[derive(Debug, Clone)]
pub enum PendingChange {
    Create {
        parent: loro::TreeParentId,
        target: loro::TreeID,
    },
    Move {
        parent: loro::TreeParentId,
        old_parent: loro::TreeParentId,
        target: loro::TreeID,
    },
    Delete {
        old_parent: loro::TreeParentId,
        target: loro::TreeID,
    },
    /// A content (text) or property (map) edit on a node's sub-container; the
    /// owning `TreeID` is recovered at drain time via `owning_tree_node`.
    Container(loro::ContainerID),
}

/// Extract the dirty facts from a `subscribe_root` DiffEvent. A pure function
/// of the event (no `doc` access), safe to call inside the subscribe callback.
///
/// **Checkout events are hard-filtered** (fail-loud): a checkout diff is a
/// backward delta that, consumed as facts, would corrupt `live`. Once the
/// projection stopped calling `doc.diff`, NOTHING checks the global live doc
/// out (all other `fork()`/`fork_at()` are on separate/shared docs), so a
/// Checkout firing here is an invariant breach — drop it and warn.
pub fn extract_pending_changes(event: &loro::event::DiffEvent) -> Vec<PendingChange> {
    if matches!(event.triggered_by, loro::EventTriggerKind::Checkout) {
        tracing::warn!(
            "[LoroProjection] unexpected Checkout DiffEvent on the global doc — ignoring;              no projection path should check the live doc out (would corrupt incremental live)"
        );
        return Vec::new();
    }
    let mut out: Vec<PendingChange> = Vec::new();
    for cd in event.events.iter() {
        match &cd.diff {
            loro::event::Diff::Tree(td) => {
                for item in td.diff.iter() {
                    match &item.action {
                        loro::TreeExternalDiff::Create { parent, .. } => {
                            out.push(PendingChange::Create {
                                parent: *parent,
                                target: item.target,
                            });
                        }
                        loro::TreeExternalDiff::Move {
                            parent, old_parent, ..
                        } => {
                            out.push(PendingChange::Move {
                                parent: *parent,
                                old_parent: *old_parent,
                                target: item.target,
                            });
                        }
                        loro::TreeExternalDiff::Delete { old_parent, .. } => {
                            out.push(PendingChange::Delete {
                                old_parent: *old_parent,
                                target: item.target,
                            });
                        }
                    }
                }
            }
            // A content (text) or property (map) edit on a node's sub-container.
            _ => out.push(PendingChange::Container(cd.target.clone())),
        }
    }
    out
}

pub fn incremental_block_changes(
    doc: &loro::LoroDoc,
    pending: &[PendingChange],
    tid_index: &mut HashMap<loro::TreeID, String>,
) -> anyhow::Result<(HashMap<String, Option<SnapshotBlock>>, bool)> {
    let tree = doc.get_tree(TREE_NAME);

    let mut reread: HashSet<loro::TreeID> = HashSet::new();
    let mut deleted: HashSet<loro::TreeID> = HashSet::new();
    let mut dirty_scopes: HashSet<loro::TreeParentId> = HashSet::new();

    // Structural facts are routed ORDER-AWARE, last fact per target wins: one
    // import event can carry `Delete(X)` followed by `Create(X)` for the SAME
    // TreeID — Loro's encoding of a sibling re-slot when a concurrent peer
    // create merges in front of an already-present node. Treating `deleted` as
    // terminal for the batch (the old `reread.retain(!deleted)`) projected that
    // still-alive node as a DELETE (peer-merge row loss), or — if it had never
    // been projected, so `tid_index` couldn't map it — silently dropped its
    // create while its children's creates flowed (deferred-FK batch reject).
    for change in pending {
        match change {
            PendingChange::Create { parent, target } => {
                dirty_scopes.insert(*parent);
                deleted.remove(target);
                reread.insert(*target);
            }
            PendingChange::Move {
                parent,
                old_parent,
                target,
            } => {
                dirty_scopes.insert(*parent);
                dirty_scopes.insert(*old_parent);
                deleted.remove(target);
                reread.insert(*target);
            }
            PendingChange::Delete { old_parent, target } => {
                dirty_scopes.insert(*old_parent);
                reread.remove(target);
                deleted.insert(*target);
            }
            PendingChange::Container(cid) => {
                if let Some(tid) = owning_tree_node(doc, cid) {
                    reread.insert(tid);
                }
            }
        }
    }

    // A structural change in a scope can shift the sibling tie-break key of
    // EVERY current member (peer-sibling-order) — re-read the whole group.
    for scope in &dirty_scopes {
        for child in children_of_scope(&tree, *scope) {
            reread.insert(child);
        }
    }
    // A node explicitly deleted this interval is handled by `deleted`, never
    // re-read as live.
    reread.retain(|t| !deleted.contains(t));
    // Fail-safe liveness recheck: a target whose LAST structural fact was a
    // delete but which is alive in the CURRENT tree was re-slotted (delete +
    // create split across drained batches), not removed — reread it instead of
    // emitting a false delete for a live block.
    deleted.retain(|t| {
        if is_node_alive(&tree, *t) {
            reread.insert(*t);
            false
        } else {
            true
        }
    });

    let mut group_keys: HashMap<loro::TreeParentId, HashMap<loro::TreeID, Option<String>>> =
        HashMap::new();
    let mut changed: HashMap<String, Option<SnapshotBlock>> = HashMap::new();
    let mut settled = true;

    for node in reread {
        // A node whose parent scope was dirtied may itself have been deleted in
        // the same interval (e.g. subtree prune): if it is no longer alive,
        // route it through the delete path instead of reading it as live.
        if !is_node_alive(&tree, node) {
            if let Some(sid) = tid_index.remove(&node) {
                changed.insert(sid, None);
            }
            continue;
        }
        match read_one_node_snapshot(&tree, node, &mut group_keys) {
            Some((sid, snap)) => {
                tid_index.insert(node, sid.clone());
                changed.insert(sid, Some(snap));
            }
            None => settled = false,
        }
    }

    for node in deleted {
        let sid = tree
            .get_meta(node)
            .ok() // ALLOW(ok): deleted node's meta may already be pruned; tid_index below recovers the id
            .and_then(|m| read_stable_id(&m))
            .map(|raw| EntityUri::block(&raw).to_string())
            .or_else(|| tid_index.get(&node).cloned());
        match sid {
            Some(sid) => {
                tid_index.remove(&node);
                changed.insert(sid, None);
            }
            None => {
                tracing::warn!(
                    node = ?node,
                    "loro incremental projection: deleted node has no known stable id \
                     (never projected) - nothing to delete"
                );
            }
        }
    }

    Ok((changed, settled))
}

/// Build a `TreeID -> stable id` index over all live nodes. (Re)seeds the
/// incremental projector's index on a full reseed pass so subsequent deletes
/// map even after Loro drops a deleted node's meta.
pub fn build_tid_index(doc: &loro::LoroDoc) -> HashMap<loro::TreeID, String> {
    let tree = doc.get_tree(TREE_NAME);
    let mut index = HashMap::new();
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
            index.insert(node.id, EntityUri::block(&sid).to_string());
        }
    }
    index
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

/// Collect all alive blocks from a shared tree, grafting them into the personal
/// tree hierarchy. Shared tree roots get `mount_parent` as their parent (the
/// mount node's parent in the personal tree), making them appear inline. Deeper
/// nodes keep their internal relationships.
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
/// the global doc silently no-ops (or fails `BlockNotFound`).
/// `resolve_write_target` routes each write to the doc that actually holds the
/// live node.
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
    /// shared subtree doc. Two writes land in the same doc iff their keys
    /// match.
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

    /// A cheap monotone version of this backend's tree: [`doc_lamport_height`]
    /// of the underlying doc, which advances on every applied op (local or
    /// imported). Pollers use it to skip a full tree walk while nothing has
    /// changed.
    ///
    /// `None` once a shared-tree store is attached: mounted subtrees carry
    /// their own oplogs, so this doc's height cannot see a change inside one
    /// and a caller must re-read instead of trusting it.
    pub fn change_version(&self) -> Option<u32> {
        if self.shared_trees.is_some() {
            return None;
        }
        Some(doc_lamport_height(&self.collab_doc.doc()))
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
    /// id is absent from the global tree (its subtree was pruned at share
    /// time), so the global resolver returns `None`; TreeIDs are globally
    /// unique (peer+counter), so a stale global TreeID is still a valid key
    /// to probe the shared docs with.
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
        // ALLOW(entity_uri_from_raw): backend string-id resolve surface (accepts both
        // id formats)
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

    /// Wrap the resolved target's doc in a `LoroDocument` for writing. Both
    /// arms use `from_existing` to reuse the already-configured inner
    /// `Arc<LoroDoc>` (the shared doc's text styles were latched at accept;
    /// re-`configure` via `LoroDocument::new` would corrupt them). A bare
    /// `doc.commit()` inside `with_write` fires the shared doc's
    /// already-attached save/sync/projection workers, so routed writes need
    /// no extra outbound plumbing.
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
                    "block {id} is a mount node (a pointer into a shared subtree); mounts are not \
                     editable content — unshare instead of editing"
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

    /// Write `content` onto a node the caller has established is content-less
    /// — an auto-created
    /// [`create_placeholder_root`](Self::create_placeholder_root)
    /// standing in for a parent reached before its own create.
    ///
    /// Goes through the same `write_content_to_meta` the create path uses, so
    /// every `BlockContent` variant (source language, image path, inline marks)
    /// lands exactly as it would have on a first-class create. Nothing is
    /// clobbered: the node held no content to lose.
    pub async fn complete_placeholder_content(
        &self,
        id: &str,
        content: &BlockContent,
    ) -> Result<(), ApiError> {
        let target = self.resolve_write_target_checked(id).await?;
        let (write_doc, tree_id) = self.target_doc(&target);

        write_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let meta = tree.get_meta(tree_id)?;
                write_content_to_meta(&meta, content)?;
                meta.insert("updated_at", loro::LoroValue::from(self.now_millis()))?;
                doc.commit();
                Ok(())
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to complete placeholder content: {}", e),
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
                let text = crate::mergeable_child::ensure_text(&meta, CONTENT_RAW)?;
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

                let text = crate::mergeable_child::ensure_text(&meta, CONTENT_RAW)?;
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

                let text = crate::mergeable_child::ensure_text(&meta, CONTENT_RAW)?;
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
                let text = crate::mergeable_child::ensure_text(&meta, CONTENT_RAW)?;
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

                let text = crate::mergeable_child::ensure_text(&meta, CONTENT_RAW)?;
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

                let text = crate::mergeable_child::ensure_text(&meta, CONTENT_RAW)?;
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
    // Grouping these into a params struct would ripple across the 13 call
    // sites in 4 other crates that construct this argument list positionally.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_block_with_properties(
        &self,
        parent_id: EntityUri,
        content: BlockContent,
        id: Option<EntityUri>,
        properties: &HashMap<String, Value>,
        tags: &Tags,
        requires: &[EntityUri],
        advice_suppressed: &[EntityUri],
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
        // The child's URI drives the typed `ParentNotFound` if the parent is
        // absent: use the caller-supplied id, else the freshly-minted stable id.
        let child_uri = id.clone().unwrap_or_else(|| EntityUri::block(&stable_id));
        let request = NewBlockWithProperties {
            parent_id: parent_id.clone(),
            id: child_uri.clone(),
            content: content.clone(),
            properties: properties.clone(),
            tags: tags.clone(),
            requires: requires.to_vec(),
            advice_suppressed: advice_suppressed.to_vec(),
        };
        let (created_block, tree_id) = write_doc
            .with_write(|doc| {
                let tree = doc.get_tree(TREE_NAME);
                let (block, node) = write_new_node(&tree, &id_cache, &request, now)?;
                doc.commit();
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

    /// [`create_block_with_properties`](Self::create_block_with_properties) for
    /// MANY blocks: one Loro commit per destination doc instead of one per
    /// block. Each node is written by the same
    /// [`write_new_node`] the single-block path uses, so the resulting meta is
    /// byte-identical; only the commit boundary and the id-cache timing differ.
    ///
    /// The id cache is populated INSIDE the loop, so a block whose parent was
    /// created earlier in the same batch resolves through the cache instead of
    /// the O(nodes) tree walk — without that, an intra-batch parent chain would
    /// re-introduce per-block quadratic resolution.
    ///
    /// Requests are grouped by destination doc (a Loro commit cannot span
    /// docs), preserving each group's request order. Returns the created blocks
    /// in REQUEST order.
    pub async fn create_blocks_with_properties(
        &self,
        requests: Vec<NewBlockWithProperties>,
    ) -> Result<Vec<Block>, ApiError> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let now = self.now_millis();
        // Group by destination doc, keeping each request's index so the caller
        // gets its blocks back in request order.
        let mut groups: Vec<(ParentWriteTarget, Vec<(usize, NewBlockWithProperties)>)> = Vec::new();
        // A request whose parent is created by THIS batch lands in the same doc
        // as that parent, by construction — so inherit its group. Resolving such
        // a parent against the tree would MISS (it does not exist yet) and pay a
        // full node walk per request: the very quadratic this batch removes.
        let mut group_of_id: HashMap<String, usize> = HashMap::new();
        for (idx, request) in requests.into_iter().enumerate() {
            if let Some(slot) = group_of_id.get(request.parent_id.id()).copied() {
                group_of_id.insert(request.id.id().to_string(), slot);
                groups[slot].1.push((idx, request));
                continue;
            }
            let target = self
                .resolve_write_target_for_parent(&request.parent_id)
                .await?;
            let slot = match groups
                .iter()
                .position(|(t, _)| t.doc_key() == target.doc_key())
            {
                Some(slot) => slot,
                None => {
                    groups.push((target, Vec::new()));
                    groups.len() - 1
                }
            };
            group_of_id.insert(request.id.id().to_string(), slot);
            groups[slot].1.push((idx, request));
        }

        let mut placed: Vec<Option<Block>> = Vec::new();
        let mut created_global: Vec<(String, loro::TreeID)> = Vec::new();
        for (target, members) in groups {
            let write_doc = self.parent_doc(&target);
            let is_global = matches!(target, ParentWriteTarget::Global);
            // Shared arm gets a throwaway cache: a shared TreeID must never
            // enter the global `id_cache` (its keys index the global tree).
            let id_cache = if is_global {
                self.id_cache.clone()
            } else {
                Arc::new(Mutex::new(HashMap::new()))
            };
            let written = write_doc
                .with_write(|doc| {
                    let tree = doc.get_tree(TREE_NAME);
                    let mut out: Vec<(usize, Block, loro::TreeID)> = Vec::new();
                    for (idx, request) in &members {
                        let (block, node) = write_new_node(&tree, &id_cache, request, now)?;
                        id_cache
                            .lock()
                            .unwrap()
                            .insert(block.id.id().to_string(), node);
                        out.push((*idx, block, node));
                    }
                    doc.commit();
                    Ok(out)
                })
                .map_err(|e| ApiError::InternalError {
                    message: format!("Failed to create {} block(s): {e:#}", members.len()),
                })?;
            for (idx, block, node) in written {
                if is_global {
                    created_global.push((block.id.id().to_string(), node));
                }
                if placed.len() <= idx {
                    placed.resize(idx + 1, None);
                }
                placed[idx] = Some(block);
            }
        }
        for (stable_id, node) in created_global {
            self.cache_stable_id(&stable_id, node);
        }

        let created: Vec<Block> = placed
            .into_iter()
            .map(|b| {
                b.ok_or_else(|| ApiError::InternalError {
                    message: "create_blocks_with_properties: a request produced no block".into(),
                })
            })
            .collect::<Result<_, ApiError>>()?;
        for block in &created {
            self.emit_change(Change::Created {
                data: block.clone(),
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            });
        }
        Ok(created)
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
        // ALLOW(entity_uri_from_raw): new_parent_id String from cell-registry field
        // write Value
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
                    "cross-boundary move of a shared subtree is not supported yet: block {id} \
                     lives in doc {:?} but new parent {new_parent_id} lives in doc {:?}",
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
        // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both
        // id formats)
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
                    "cross-boundary move of a shared subtree is not supported yet: block \
                     {target_id} lives in doc {:?} but new parent {new_parent_id} lives in doc \
                     {:?}",
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

    // -- Edge fields (set-valued, junction-projected) --

    /// Replace a set-valued **edge field** (`tags` / `requires` /
    /// `advice_suppressed`) on a tree node's meta under its own `key`, so the
    /// Loro→SQL projector reads it into the matching junction table. Values are
    /// stored as a JSON string array (the same shape every edge field uses —
    /// tag strings or id strings), so ONE generic writer serves every member of
    /// [`holon_api::EdgeField`] without a per-field branch. An empty set
    /// deletes the key entirely.
    pub async fn set_block_edge_field(
        &self,
        tree_id_str: &str,
        key: &str,
        values: &[String],
    ) -> anyhow::Result<()> {
        let target = self
            .resolve_write_target_checked(tree_id_str)
            .await
            .map_err(|e| anyhow::anyhow!("set_block_edge_field({key}): {e}"))?;
        let (write_doc, tree_id) = self.target_doc(&target);
        let serialized = serde_json::to_string(values)?;
        write_doc.with_write(|doc| {
            let tree = doc.get_tree(TREE_NAME);
            let meta = tree.get_meta(tree_id)?;
            if values.is_empty() {
                meta.delete(key)?;
            } else {
                meta.insert(key, loro::LoroValue::from(serialized.as_str()))?;
            }
            doc.commit();
            Ok(())
        })
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

    /// Set the `advice_suppressed` edge field (advice-suppression exclusion
    /// set, ADR 0021) on a block's Loro meta. Mirrors
    /// [`set_block_requires`](Self::set_block_requires): a dedicated
    /// `advice_suppressed` meta key holding a JSON list, read back by
    /// `read_block_from_tree`, projected to the `advice_suppressed` junction.
    pub async fn set_block_advice_suppressed(
        &self,
        tree_id_str: &str,
        advice_suppressed: &[EntityUri],
    ) -> anyhow::Result<()> {
        let target = self
            .resolve_write_target_checked(tree_id_str)
            .await
            .map_err(|e| anyhow::anyhow!("set_block_advice_suppressed: {e}"))?;
        let (write_doc, tree_id) = self.target_doc(&target);

        let serialized = serde_json::to_string(advice_suppressed)?;

        write_doc.with_write(|doc| {
            let tree = doc.get_tree(TREE_NAME);
            let meta = tree.get_meta(tree_id)?;
            if advice_suppressed.is_empty() {
                meta.delete("advice_suppressed")?;
            } else {
                meta.insert(
                    "advice_suppressed",
                    loro::LoroValue::from(serialized.as_str()),
                )?;
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

    /// Peek the stable-id cache WITHOUT the O(nodes) tree walk
    /// `find_tree_id_by_stable_id` performs on a miss.
    ///
    /// A miss here means "not cached", NOT "not in the tree" — only sound as an
    /// existence test right after
    /// [`warm_stable_id_cache`](Self::warm_stable_id_cache)
    /// with no concurrent writer, which is exactly the batched ingest's
    /// situation. Also asserts in tests that a shared child's id never leaks
    /// into the global `id_cache`.
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
    /// Call after `doc.import(delta)` to ensure newly imported nodes are
    /// resolvable.
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

    /// Snapshot all alive blocks keyed by stable ID. Call before
    /// `doc.import(delta)`.
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
    /// Call after `doc.import(delta)` with the snapshot from
    /// `snapshot_blocks()`.
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
            // Validate the cached TreeID is still alive. A delete → undo(create)
            // resurrects the SAME stable id under a NEW TreeID (delete+recreate,
            // not in-place un-delete), so a handle that cached the pre-delete
            // TreeID would otherwise resolve to the tombstoned node and report
            // the restored block as missing. On a dead hit, drop the stale entry
            // and fall through to the tree-walk below, which re-resolves and
            // re-caches the live TreeID.
            let alive = self
                .collab_doc
                .with_read(|doc| Ok(!node_deleted_now(&doc.get_tree(TREE_NAME), tid)))
                .unwrap_or(false);
            if alive {
                return Some(tid);
            }
            self.uncache_stable_id(stable_id);
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
    /// Accepts both `block:{peer}:{counter}` (TreeID format) and `block:{uuid}`
    /// (stable ID). Uses cache for stable ID lookups.
    pub async fn resolve_to_tree_id(&self, id_str: &str) -> Option<loro::TreeID> {
        // Fast path: try parsing as TreeID directly
        if let Some(tid) = str_to_tree_id(id_str) {
            return Some(tid);
        }
        // Slow path: resolve via stable ID
        // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both
        // id formats)
        let uri = EntityUri::from_raw(id_str);
        if uri.is_block() || uri.is_sentinel() {
            return self.find_tree_id_by_stable_id(uri.id()).await;
        }
        None
    }

    /// Resolve a block ID string to TreeID, returning ApiError::BlockNotFound
    /// on failure.
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
    /// Used by reverse sync to represent document blocks that aren't in the
    /// EventBus. The `stable_id` becomes the node's STABLE_ID and is
    /// returned as a `block:` URI.
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

                // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both
                // id formats)
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
                            let Some(sid) = read_stable_id(&meta) else {
                                warn_half_born(shared_root, parent_id);
                                continue;
                            };
                            result.push(EntityUri::block(&sid).to_string());
                        }
                        continue;
                    }
                    let meta = tree.get_meta(*tid)?;
                    // A live child with no `STABLE_ID` is an in-flight create,
                    // not a corrupt node: `tree.create()` and the meta insert
                    // are two doc-state steps and `with_write` does not exclude
                    // `with_read` (the same window
                    // `snapshot_blocks_from_doc_settled` withholds for). Panic
                    // here and the tokio worker that owns the org-writeback
                    // fold dies, leaving the file stale for the life of the
                    // process. Callers poll `children` until the ids they
                    // expect appear, so withholding is recoverable — an `Err`
                    // would abort their ingest instead.
                    let Some(sid) = read_stable_id(&meta) else {
                        warn_half_born(*tid, parent_id);
                        continue;
                    };
                    result.push(EntityUri::block(&sid).to_string());
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
                write_content_to_meta(&meta, &content)?;
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
                // A block's mergeable children are ROOT containers, which
                // `tree.delete` leaves alive holding the block's content. Name
                // them while the subtree still exists, purge them once it is
                // gone.
                let roots = crate::deleted_container_purge::subtree_roots(&tree, tree_id)?;
                match tree.delete(tree_id) {
                    Ok(()) => {
                        crate::deleted_container_purge::purge_roots(doc, &roots)?;
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
            // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both
            // id formats)
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
                    "block {id} is a mount node; moving a mount is an unshare concern, not a \
                     block move"
                ),
            });
        }
        let parent_target = self.resolve_write_target_for_parent(&new_parent).await?;
        if source_target.doc_key() != parent_target.doc_key() {
            return Err(ApiError::InvalidOperation {
                message: format!(
                    "cross-boundary move of a shared subtree is not supported yet: block {id} \
                     lives in doc {:?} but new parent {new_parent} lives in doc {:?}",
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
                            "create_blocks: batch straddles two docs ({:?} vs {:?}); cross-doc \
                             batch creation into a shared subtree is not supported",
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
                    let stable_id = match &new_block.id {
                        Some(uri) => uri.id().to_string(),
                        None => uuid::Uuid::new_v4().to_string(),
                    };
                    let child_uri = new_block
                        .id
                        .clone()
                        .unwrap_or_else(|| EntityUri::block(&stable_id));
                    let parent_tree_id = resolve_parent_tree_id_for_create(
                        &tree,
                        &id_cache,
                        &new_block.parent_id,
                        &child_uri,
                    )?;
                    let node = tree.create(parent_tree_id)?;
                    let meta = tree.get_meta(node)?;
                    meta.insert(STABLE_ID, loro::LoroValue::from(stable_id.as_str()))?;
                    write_content_to_meta(&meta, &new_block.content)?;
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
                // Name every doomed subtree's root containers before ANY delete:
                // one id in the batch may be an ancestor of another, and a
                // deleted node no longer names its roots.
                let mut roots = Vec::new();
                for tid in &resolved {
                    roots.extend(crate::deleted_container_purge::subtree_roots(&tree, *tid)?);
                }
                for tid in &resolved {
                    tree.delete(*tid)?;
                }
                crate::deleted_container_purge::purge_roots(doc, &roots)?;
                doc.commit();
                Ok(())
            })
            .map_err(|e| ApiError::InternalError {
                message: format!("Failed to delete blocks: {}", e),
            })?;

        for id in &unique_ids {
            // ALLOW(entity_uri_from_raw): id/parent_id &str backend API param (accepts both
            // id formats)
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
    use loro::ExportMode;
    use loro::LoroDoc;

    use super::*;

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
    /// LoroMap keys merge so both survive. The pre-H3 single-JSON-blob would
    /// have dropped one peer's whole change to blob-level LWW.
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
    /// correctly, and the first write migrates it to the nested map and drops
    /// the legacy key (self-healing — no lingering dual representation).
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

    /// `replace_properties_in_meta` is the EXACT-set writer: keys absent from
    /// the new set are deleted, down to the empty set.
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

    /// Regression (intent-divergence): reserved block COLUMNS (`updated_at`,
    /// `created_at`, `id`, …) must NEVER survive as generic properties. Org
    /// ingest lifts drawer keys like `:UPDATED_AT:` into `block.properties`,
    /// which land in the Loro PROPERTIES_MAP. Left in, the Loro-read block's
    /// `properties` carries `{"updated_at": …}` while the SQL-read block's
    /// stays `{}` (SQL routes each to its own column) — so `blocks_differ`
    /// fires, yet `block_diff_params` can't represent it (`updated_at`
    /// collides with the always-emitted bookkeeping `updated_at` via
    /// `or_insert`), producing a bookkeeping-only update that decodes to
    /// zero typed ops and trips the consolidator's `agrees_with_ops`
    /// divergence. The read boundary strips them so Loro's `properties`
    /// agrees with SQL's by construction.
    #[test]
    fn reserved_column_keys_are_stripped_from_properties() {
        let doc = LoroDoc::new();
        let node = seed_node(
            &doc,
            &HashMap::from([
                ("updated_at".to_string(), Value::Integer(1783768599379)),
                ("created_at".to_string(), Value::Integer(1783768500000)),
                ("id".to_string(), s("block:leaked")),
                ("sort_key".to_string(), s("A0")),
                // Edge fields leak in as empty `Array([])` and trip the same
                // divergence — they must be stripped too.
                ("tags".to_string(), Value::Array(vec![])),
                ("requires".to_string(), Value::Array(vec![])),
                ("advice_suppressed".to_string(), Value::Array(vec![])),
                // `collapsed` is deliberately NOT stripped here — it lives in
                // properties and `read_block_from_tree` lifts it to the typed slot.
                ("collapsed".to_string(), Value::Boolean(true)),
                // A genuine domain property must survive untouched.
                ("sequence".to_string(), Value::Integer(1)),
            ]),
        );
        let props = read_props(&doc, node);
        assert_eq!(
            props.get("updated_at"),
            None,
            "reserved `updated_at` stripped"
        );
        assert_eq!(
            props.get("created_at"),
            None,
            "reserved `created_at` stripped"
        );
        assert_eq!(props.get("id"), None, "reserved `id` stripped");
        assert_eq!(props.get("sort_key"), None, "reserved `sort_key` stripped");
        assert_eq!(props.get("tags"), None, "edge `tags` stripped");
        assert_eq!(props.get("requires"), None, "edge `requires` stripped");
        assert_eq!(
            props.get("advice_suppressed"),
            None,
            "edge `advice_suppressed` stripped"
        );
        assert_eq!(
            props.get("collapsed"),
            Some(&Value::Boolean(true)),
            "`collapsed` kept for read_block_from_tree to lift into the typed slot"
        );
        assert_eq!(
            props.get("sequence"),
            Some(&Value::Integer(1)),
            "genuine domain property survives"
        );
    }
}

#[cfg(test)]
mod diff_checkout_race_tests {
    //! RCA lock for the flaky keystone `SplitBlock … update_block_position:
    //! Block not found`. `LoroDoc::diff(a,b)` is NOT a pure read: it checks out
    //! the shared live doc to `a`, then `b`, then restores `old_frontiers`
    //! (loro-internal 1.12.0 `loro.rs`). A concurrent reader of the SAME
    //! `Arc<LoroDoc>` can therefore observe the doc at `a`, missing a node
    //! created after `a`. The OLD incremental projection called `doc.diff` on
    //! the global doc from a spawned worker while split/create ops read the
    //! same doc → the just-created node was transiently absent → `Block not
    //! found`.
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::LoroDocument;

    async fn make_backend() -> (Arc<LoroDocument>, Arc<LoroBackend>) {
        let doc = Arc::new(LoroDocument::new("diff-race".to_string()).unwrap());
        let backend = Arc::new(LoroBackend::from_document(doc.clone()));
        (doc, backend)
    }

    async fn seed_parent_and_child(backend: &LoroBackend) {
        let props = HashMap::new();
        let tags = Tags::default();
        backend
            .create_block_with_properties(
                EntityUri::no_parent(),
                BlockContent::text("parent"),
                Some(EntityUri::block("parent")),
                &props,
                &tags,
                &[],
                &[],
            )
            .await
            .unwrap();
    }

    /// REMOTE-DELETE STALE-PARENT HOLE (verifier finding d): a stable-id cache
    /// entry tombstoned by a delete that bypasses THIS backend's uncache — a
    /// remote / CRDT-merge delete, modelled here by deleting through a SECOND
    /// backend/handle on the same doc — must NOT be served as a live parent.
    /// loro 1.12's `get_meta` returns Ok for a tombstoned
    /// (deleted-but-existing) node, so the old `get_meta(tid).is_ok()`
    /// guard would attach a new child UNDER the dead node. The liveness
    /// (`node_deleted_now`) guard drops the stale entry and re-resolves;
    /// here the parent is truly gone, so the create must fail LOUD
    /// (ParentNotFound), never silently parent under a tombstone.
    #[tokio::test]
    async fn remote_delete_tombstoned_parent_never_served_from_stale_cache() {
        let doc = Arc::new(LoroDocument::new("remote-del".to_string()).unwrap());
        let backend_a = LoroBackend::from_document(doc.clone());
        let backend_b = LoroBackend::from_document(doc.clone());

        // A creates the parent — this caches "parent" → its live TreeID in A's
        // (per-backend) id_cache.
        backend_a
            .create_block_with_properties(
                EntityUri::no_parent(),
                BlockContent::text("parent"),
                Some(EntityUri::block("parent")),
                &HashMap::new(),
                &Tags::default(),
                &[],
                &[],
            )
            .await
            .unwrap();
        assert!(
            backend_a.peek_id_cache("parent").is_some(),
            "precondition: A cached the parent's live TreeID"
        );

        // A REMOTE delete: B removes the parent on the SHARED doc. `delete_block`
        // uncaches from B's own cache only — A's cache still points at the now-
        // tombstoned node, exactly the shape a CRDT-merge delete leaves locally.
        backend_b.delete_block("block:parent").await.unwrap();
        assert!(
            backend_a.peek_id_cache("parent").is_some(),
            "the remote delete must NOT touch A's cache — the stale entry is the \
             hole under test"
        );

        // A now creates a child under the (dead) parent. With the tombstone-blind
        // `get_meta().is_ok()` guard this SUCCEEDED, attaching the child under a
        // deleted node. With the liveness guard the stale entry is evicted, the
        // tree-walk finds no live parent, and the create fails loud.
        let result = backend_a
            .create_block_with_properties(
                EntityUri::block("parent"),
                BlockContent::text("child"),
                Some(EntityUri::block("child")),
                &HashMap::new(),
                &Tags::default(),
                &[],
                &[],
            )
            .await;
        assert!(
            result.is_err(),
            "create under a remotely-tombstoned parent must fail, not attach the \
             child under a dead node; got {result:?}"
        );
        assert!(
            backend_a.peek_id_cache("parent").is_none(),
            "the stale tombstone entry must have been evicted during re-resolution"
        );
    }

    /// PROJECTION-GAP PROBE (2026-07-13): a `set_block_tags` meta-map edit on
    /// an EXISTING node must be captured by the DiffEvent → PendingChange →
    /// `incremental_block_changes` path (the steady-state projector), carrying
    /// the new tags. If `owning_tree_node` can't map the meta-map container
    /// back to its TreeID, the edit is silently dropped and the tags never
    /// project — the composed-keystone `SetEdgeField{Tags}` divergence.
    #[tokio::test]
    async fn incremental_projection_captures_tags_meta_edit() {
        let (doc, backend) = make_backend().await;
        backend
            .create_block_with_properties(
                EntityUri::no_parent(),
                BlockContent::text("hello"),
                Some(EntityUri::block("b1")),
                &HashMap::new(),
                &Tags::default(),
                &[],
                &[],
            )
            .await
            .unwrap();

        let captured: Arc<std::sync::Mutex<Vec<PendingChange>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let cap2 = captured.clone();
        let raw = doc.doc();
        let _sub = raw.subscribe_root(Arc::new(move |event| {
            let mut facts = extract_pending_changes(&event);
            cap2.lock().unwrap().append(&mut facts);
        }));

        backend
            .set_block_tags("block:b1", &["task".to_string()])
            .await
            .unwrap();

        let pending = captured.lock().unwrap().clone();
        assert!(
            !pending.is_empty(),
            "tags meta edit must produce at least one PendingChange (got none)"
        );
        let mut tid_index = HashMap::new();
        let (changed, settled) = incremental_block_changes(&raw, &pending, &mut tid_index).unwrap();
        assert!(settled, "incremental pass must settle");
        let b1 = changed
            .get("block:b1")
            .and_then(|o| o.as_ref())
            .expect("b1 must be re-read by the incremental projector after a tags edit");
        assert_eq!(
            b1.block.tags.to_vec(),
            vec!["task".to_string()],
            "incremental projection must carry the tags meta edit; got {:?}",
            b1.block.tags
        );
    }

    /// Regression (keystone `inv-blocks-match-ref/org` RED, 2026-07-12): an
    /// image block created through the Loro backend must read back as
    /// `ContentType::Image`, not collapse to `Text`. Before the
    /// `BlockContent::Image` variant, `write_content_to_meta` stored the block
    /// as `content_type = "text"` (the ingest path built `BlockContent::text`)
    /// and `read_content_from_meta` re-hydrated `image` meta into
    /// `BlockContent::Text` — either loss turned an org `[[file:…]]` child into
    /// a plain Text headline on the disk round-trip (image-ness permanently
    /// lost).
    #[tokio::test]
    async fn image_block_survives_loro_create_read_round_trip() {
        let (_doc, backend) = make_backend().await;
        backend
            .create_block_with_properties(
                EntityUri::no_parent(),
                BlockContent::text("parent"),
                Some(EntityUri::block("parent")),
                &HashMap::new(),
                &Tags::default(),
                &[],
                &[],
            )
            .await
            .unwrap();

        let created = backend
            .create_block(
                EntityUri::block("parent"),
                BlockContent::image("attachments/foo.png"),
                Some(EntityUri::block("img1")),
            )
            .await
            .unwrap();
        assert_eq!(
            created.content_type,
            ContentType::Image,
            "create echo must carry Image, got {:?}",
            created.content_type
        );
        assert_eq!(created.content, "attachments/foo.png");

        let read = backend.get_block(created.id.as_str()).await.unwrap();
        assert_eq!(
            read.content_type,
            ContentType::Image,
            "Loro read must preserve Image (was collapsing to Text), got {:?}",
            read.content_type
        );
        assert_eq!(read.content, "attachments/foo.png");
    }

    /// MECHANISM PROOF (diagnostic, `#[ignore]`d in CI because it asserts a
    /// race *occurs* and is therefore timing-dependent): hammering
    /// `doc.diff` across an interval whose `from` predates the child checks
    /// the shared live doc back to before the child on every iteration; a
    /// concurrent resolve then intermittently fails with `Block not found`.
    /// Run manually with `--ignored` to observe the keystone flake
    /// mechanism deterministically.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "diagnostic: asserts a timing race occurs; run with --ignored"]
    async fn diff_checkout_races_concurrent_resolve() {
        let (doc, backend) = make_backend().await;
        seed_parent_and_child(&backend).await;
        let f_before_child = doc.doc().oplog_frontiers();
        let props = HashMap::new();
        let tags = Tags::default();
        backend
            .create_block_with_properties(
                EntityUri::block("parent"),
                BlockContent::text("child"),
                Some(EntityUri::block("child")),
                &props,
                &tags,
                &[],
                &[],
            )
            .await
            .unwrap();
        let f_after_child = doc.doc().oplog_frontiers();

        let raw = doc.doc();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let hammer = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                let _ = raw.diff(&f_before_child, &f_after_child);
            }
        });

        let mut spurious = 0usize;
        for _ in 0..3000 {
            if backend
                .update_block_position("block:child", "block:parent", None)
                .await
                .is_err()
            {
                spurious += 1;
            }
            tokio::task::yield_now().await;
        }
        stop.store(true, Ordering::Relaxed);
        hammer.join().unwrap();

        assert!(
            spurious > 0,
            "expected doc.diff checkout to race the concurrent resolve at least once"
        );
    }

    /// FIX-APPROACH GATE (stable): the event-driven projection reads the
    /// CURRENT tree state (`snapshot_blocks_from_doc_settled` / per-node
    /// reads) instead of `doc.diff`. A current-state read takes no
    /// checkout, so hammering it concurrently with a resolve must NEVER
    /// produce a spurious `Block not found`. Green on this proves the fix
    /// approach eliminates the race; it would be RED if the projection
    /// still checked out. Stable (asserts absence of a race, which holds
    /// deterministically for a checkout-free reader).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn current_state_read_never_races_concurrent_resolve() {
        let (doc, backend) = make_backend().await;
        seed_parent_and_child(&backend).await;
        let props = HashMap::new();
        let tags = Tags::default();
        backend
            .create_block_with_properties(
                EntityUri::block("parent"),
                BlockContent::text("child"),
                Some(EntityUri::block("child")),
                &props,
                &tags,
                &[],
                &[],
            )
            .await
            .unwrap();

        let raw = doc.doc();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let hammer = std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                // The projection's read shape under the fix: snapshot current
                // state. No checkout, so no interval that hides the child.
                let _ = snapshot_blocks_from_doc_settled(&raw);
            }
        });

        let mut spurious = 0usize;
        for _ in 0..3000 {
            if backend
                .update_block_position("block:child", "block:parent", None)
                .await
                .is_err()
            {
                spurious += 1;
            }
            tokio::task::yield_now().await;
        }
        stop.store(true, Ordering::Relaxed);
        hammer.join().unwrap();

        assert_eq!(
            spurious, 0,
            "current-state read must not race a concurrent resolve: {spurious}/3000 spurious \
             Block-not-found — the projection read is checking out the shared doc"
        );
    }
}

/// Unit tests for [`incremental_block_changes`] — the O(changed) fact-driven
/// diff the Loro→SQL projection's fast path drives. Each test builds a raw
/// `LoroDoc` tree (STABLE_ID meta + a fractional index, mirroring the prod
/// create path), enacts a structural batch, then asserts the `(changed,
/// settled)` result against the exact rows a projection pass would emit.
#[cfg(test)]
mod incremental_tests {
    use loro::ContainerTrait;
    use loro::LoroDoc;

    use super::*;

    fn schemed(sid: &str) -> String {
        EntityUri::block(sid).to_string()
    }

    /// A doc whose block tree assigns fractional indices (prod invariant, ADR
    /// 0005) so every live node projects a real sort key.
    fn new_fi_doc() -> LoroDoc {
        let doc = LoroDoc::new();
        doc.get_tree(TREE_NAME).enable_fractional_index(0);
        doc
    }

    /// Create a block node under `parent` (`None` = tree root) with STABLE_ID
    /// `sid` and text `content`, committing it. Mirrors the meta the prod
    /// create path writes (`create_block_with_properties`).
    fn create_node(
        doc: &LoroDoc,
        parent: Option<loro::TreeID>,
        sid: &str,
        content: &str,
    ) -> loro::TreeID {
        let tree = doc.get_tree(TREE_NAME);
        let node = tree.create(parent).unwrap();
        let meta = tree.get_meta(node).unwrap();
        meta.insert(STABLE_ID, loro::LoroValue::from(sid)).unwrap();
        write_content_to_meta(
            &meta,
            &BlockContent::Text {
                raw: content.to_string(),
            },
        )
        .unwrap();
        doc.commit();
        node
    }

    /// Case 1 — subtree delete during a dirtied scope. A batch that creates C
    /// under P and deletes A (a sibling) must: re-read C as live, tombstone A
    /// (via the pre-seeded `tid_index`, since Loro may drop a deleted node's
    /// meta), and re-read the untouched sibling B because a structural change
    /// in P's scope can shift every member's sibling tie-break key.
    #[test]
    fn subtree_delete_during_dirty_scope_reads_survivor_and_tombstones_gone() {
        let doc = new_fi_doc();
        let p = create_node(&doc, None, "P", "parent");
        let a = create_node(&doc, Some(p), "A", "child-a");
        let _b = create_node(&doc, Some(p), "B", "child-b");

        // The dirty interval: create C under P and delete A.
        let c = create_node(&doc, Some(p), "C", "child-c");
        doc.get_tree(TREE_NAME).delete(a).unwrap();
        doc.commit();

        // The projector indexed A on a prior reseed, so its delete resolves
        // even after Loro drops the deleted node's meta.
        let mut tid_index: HashMap<loro::TreeID, String> = HashMap::new();
        tid_index.insert(a, schemed("A"));

        let pending = vec![
            PendingChange::Create {
                parent: loro::TreeParentId::Node(p),
                target: c,
            },
            PendingChange::Delete {
                old_parent: loro::TreeParentId::Node(p),
                target: a,
            },
        ];

        let (changed, settled) = incremental_block_changes(&doc, &pending, &mut tid_index).unwrap();

        assert!(settled, "all survivors are meta-complete → settled");
        assert!(
            matches!(changed.get(&schemed("C")), Some(Some(_))),
            "newly created C is re-read as a live block; changed = {changed:?}"
        );
        assert!(
            matches!(changed.get(&schemed("A")), Some(None)),
            "deleted A is emitted as a tombstone (None); changed = {changed:?}"
        );
        assert!(
            matches!(changed.get(&schemed("B")), Some(Some(_))),
            "untouched sibling B is re-read (dirty-scope sibling-order recompute); changed = \
             {changed:?}"
        );
        assert!(
            !tid_index.contains_key(&a),
            "A's stale index entry is removed on delete"
        );
    }

    /// Case 3 — a content-container edit re-reads only the owning node, with no
    /// sibling churn. A `Container` fact dirties no tree scope, so a sibling of
    /// the edited node must NOT be re-read.
    #[test]
    fn container_only_edit_rereads_owner_no_sibling_churn() {
        let doc = new_fi_doc();
        let a = create_node(&doc, None, "A", "hello");
        let _b = create_node(&doc, None, "B", "sibling");

        let cid = {
            let tree = doc.get_tree(TREE_NAME);
            let meta = tree.get_meta(a).unwrap();
            let text = meta.ensure_mergeable_text(CONTENT_RAW).unwrap();
            text.insert(0, "x").unwrap();
            text.id()
        };
        doc.commit();

        let mut tid_index: HashMap<loro::TreeID, String> = HashMap::new();
        let pending = vec![PendingChange::Container(cid)];

        let (changed, settled) = incremental_block_changes(&doc, &pending, &mut tid_index).unwrap();

        assert!(settled, "the owning node is meta-complete → settled");
        assert!(
            matches!(changed.get(&schemed("A")), Some(Some(_))),
            "the container edit re-reads its owning node A; changed = {changed:?}"
        );
        assert!(
            !changed.contains_key(&schemed("B")),
            "a container edit dirties no scope → sibling B is NOT re-read; changed = {changed:?}"
        );
        assert_eq!(changed.len(), 1, "only the owning node is touched");
    }

    /// Case 5 — a live node that `read_one_node_snapshot` withholds (here:
    /// meta-incomplete, STABLE_ID not yet landed) under a dirtied scope marks
    /// the pass `settled == false`, so the projector reseeds/withholds deletes
    /// rather than under-reporting the live set.
    ///
    /// NOTE (deviation from the spec's "NO fractional index" framing): loro
    /// assigns a node position on `create` regardless of
    /// `enable_fractional_index` (`TreeStateNode::position` is `Some` from the
    /// create op), so an fi-absent LIVE node is a concurrent mid-commit
    /// transient, not deterministically constructible in a single-threaded raw
    /// doc. The meta-incomplete branch is the sibling `read_one_node_snapshot`
    /// withhold path and drives the identical `settled == false` caller
    /// contract, so it is the deterministic proof of the same behaviour.
    #[test]
    fn withheld_live_node_under_dirty_scope_is_unsettled() {
        let doc = new_fi_doc();
        let p = create_node(&doc, None, "P", "parent");

        // A live child of P with a position but NO STABLE_ID — the transient
        // "meta not yet landed" shape `read_one_node_snapshot` must withhold.
        let no_sid = {
            let tree = doc.get_tree(TREE_NAME);
            let node = tree.create(Some(p)).unwrap();
            doc.commit();
            node
        };
        assert!(
            doc.get_tree(TREE_NAME).fractional_index(no_sid).is_some(),
            "precondition: the withheld node still HAS a fractional index — the withhold is \
             driven by the missing STABLE_ID, not a missing fi"
        );

        // Dirty P's scope so the meta-incomplete child is pulled into `reread`.
        let c = create_node(&doc, Some(p), "C", "child-c");

        let mut tid_index: HashMap<loro::TreeID, String> = HashMap::new();
        let pending = vec![PendingChange::Create {
            parent: loro::TreeParentId::Node(p),
            target: c,
        }];

        let (_changed, settled) =
            incremental_block_changes(&doc, &pending, &mut tid_index).unwrap();

        assert!(
            !settled,
            "a live node the per-node reader withholds marks the pass unsettled"
        );
    }

    /// Regression — peer-merge re-slot loses the re-slotted block. A peer
    /// import whose concurrent sibling create merges IN FRONT of an
    /// already-present node arrives as ONE DiffEvent carrying `Delete(X)`
    /// followed by `Create(X)` for the SAME TreeID (observed fact stream of
    /// `peer_merge_sibling_order_sql_matches_loro`:
    /// `[Delete{X}, Create{Z}, Create{X}, Container…]`). Routing facts into
    /// unordered sets with delete-wins semantics projected live X as a DELETE
    /// (SQL row loss) — or, when X had never been projected (bulk import),
    /// silently dropped its create while its children's creates flowed into
    /// the batch (deferred-FK reject). Facts are ordered; the LAST structural
    /// fact per target must win.
    #[test]
    fn delete_then_recreate_same_target_in_one_batch_is_a_live_reread() {
        let doc = new_fi_doc();
        let p = create_node(&doc, None, "P", "parent");
        let x = create_node(&doc, Some(p), "X", "reslotted");
        let z = create_node(&doc, Some(p), "Z", "merged-in-front");

        let mut tid_index: HashMap<loro::TreeID, String> = HashMap::new();
        tid_index.insert(x, schemed("X"));

        let scope = loro::TreeParentId::Node(p);
        let pending = vec![
            PendingChange::Delete {
                old_parent: scope,
                target: x,
            },
            PendingChange::Create {
                parent: scope,
                target: z,
            },
            PendingChange::Create {
                parent: scope,
                target: x,
            },
        ];

        let (changed, settled) = incremental_block_changes(&doc, &pending, &mut tid_index).unwrap();

        assert!(settled, "all nodes are meta-complete → settled");
        assert!(
            matches!(changed.get(&schemed("X")), Some(Some(_))),
            "re-slotted X (delete + create of the same target) is a LIVE reread, never a delete; \
             changed = {changed:?}"
        );
        assert!(
            matches!(changed.get(&schemed("Z")), Some(Some(_))),
            "the merged-in-front create Z is read as live; changed = {changed:?}"
        );
        assert!(
            tid_index.contains_key(&x),
            "X stays indexed — it was never actually deleted"
        );
    }

    /// Regression (fail-safe half): a lone `Delete(X)` fact whose target is
    /// alive in the CURRENT tree (the matching re-create landed in a commit
    /// whose facts sit in a later drain) must be rerouted to a live reread —
    /// the tree at drain time is the authority, not the stale fact.
    #[test]
    fn stale_delete_fact_for_alive_node_rereads_instead_of_tombstoning() {
        let doc = new_fi_doc();
        let p = create_node(&doc, None, "P", "parent");
        let x = create_node(&doc, Some(p), "X", "alive");

        let mut tid_index: HashMap<loro::TreeID, String> = HashMap::new();
        tid_index.insert(x, schemed("X"));

        let pending = vec![PendingChange::Delete {
            old_parent: loro::TreeParentId::Node(p),
            target: x,
        }];

        let (changed, settled) = incremental_block_changes(&doc, &pending, &mut tid_index).unwrap();

        assert!(settled);
        assert!(
            matches!(changed.get(&schemed("X")), Some(Some(_))),
            "an alive node is never tombstoned off a stale delete fact; changed = {changed:?}"
        );
        assert!(tid_index.contains_key(&x));
    }
}

/// Semantic pin for lazy child-container creation under a tree-node `meta` key.
///
/// [`update_text_field`] and [`properties_map_container`] are the two places
/// prod creates a child container at a map key that a *concurrent* peer may
/// create at the same time. These tests drive those prod helpers, never the
/// loro API directly, so they hold the merge outcome fixed across any change to
/// which loro API the helpers call.
///
/// The pinned outcome is that BOTH peers' first writes survive: the helpers go
/// through [`crate::mergeable_child`], which gives concurrent creators one
/// deterministic child container instead of two competing ones.
///
/// State written before this migration holds legacy op-id children, which
/// mergeable creation refuses; [`crate::mergeable_child`] surfaces that refusal
/// as the delete-and-restart instruction rather than converting in place.
#[cfg(test)]
mod concurrent_child_creation_semantics {
    use loro::LoroDoc;

    use super::*;

    /// Two peers that both see a tree node whose child container at the key
    /// under test does not exist yet — the fork point of a first-creation race.
    fn forked_pair_on_shared_node() -> (LoroDoc, LoroDoc, loro::TreeID) {
        let a = LoroDoc::new();
        a.set_peer_id(1).unwrap();
        let node = a.get_tree(TREE_NAME).create(None).unwrap();
        a.commit();

        let b = LoroDoc::new();
        b.set_peer_id(2).unwrap();
        b.import(&a.export(loro::ExportMode::Snapshot).unwrap())
            .unwrap();

        (a, b, node)
    }

    fn sync(a: &LoroDoc, b: &LoroDoc) {
        b.import(&a.export(loro::ExportMode::updates(&b.oplog_vv())).unwrap())
            .unwrap();
        a.import(&b.export(loro::ExportMode::updates(&a.oplog_vv())).unwrap())
            .unwrap();
    }

    fn meta_of(doc: &LoroDoc, node: loro::TreeID) -> loro::LoroMap {
        doc.get_tree(TREE_NAME).get_meta(node).unwrap()
    }

    /// Both peers create the `CONTENT_RAW` text child for the first time while
    /// partitioned, each writing its own text. Both writes land in the *same*
    /// mergeable child container, so neither peer's text is lost — loro's RGA
    /// orders the two concurrent inserts at position 0 by peer id.
    #[test]
    fn concurrent_first_text_creation_merges_both_peers_text() {
        let (a, b, node) = forked_pair_on_shared_node();

        update_text_field(&meta_of(&a, node), CONTENT_RAW, "alpha").unwrap();
        a.commit();
        update_text_field(&meta_of(&b, node), CONTENT_RAW, "beta").unwrap();
        b.commit();

        sync(&a, &b);

        let seen_by_a = read_text_content(&meta_of(&a, node));
        let seen_by_b = read_text_content(&meta_of(&b, node));

        assert_eq!(
            seen_by_a, seen_by_b,
            "peers must converge on one text after syncing both ways"
        );
        assert_eq!(
            seen_by_a, "alphabeta",
            "both peers write into one mergeable text child; neither insert is dropped"
        );
    }

    /// Both peers create the `PROPERTIES_MAP` child for the first time while
    /// partitioned, each writing a *different* property key. Both writes land
    /// in the same mergeable child map, so the two non-colliding keys union.
    #[test]
    fn concurrent_first_properties_map_creation_merges_both_peers_keys() {
        let (a, b, node) = forked_pair_on_shared_node();

        properties_map_container(&meta_of(&a, node))
            .unwrap()
            .insert("from_a", encode_property_value(&Value::from(1)).unwrap())
            .unwrap();
        a.commit();
        properties_map_container(&meta_of(&b, node))
            .unwrap()
            .insert("from_b", encode_property_value(&Value::from(2)).unwrap())
            .unwrap();
        b.commit();

        sync(&a, &b);

        let seen_by_a =
            decode_properties_map(&properties_map_container(&meta_of(&a, node)).unwrap());
        let seen_by_b =
            decode_properties_map(&properties_map_container(&meta_of(&b, node)).unwrap());

        assert_eq!(
            seen_by_a, seen_by_b,
            "peers must converge on one property map after syncing both ways"
        );
        assert_eq!(
            seen_by_a
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            ["from_a".to_string(), "from_b".to_string()]
                .into_iter()
                .collect(),
            "both peers write into one mergeable child map; the keys union"
        );
    }

    /// A doc written before the migration holds a legacy op-id child at the
    /// key. The prod write path refuses it rather than clobbering it or
    /// silently falling back to the losing LWW shape — fresh start is the
    /// only migration.
    #[test]
    fn a_prod_write_onto_pre_migration_state_fails_loud() {
        let (a, _b, node) = forked_pair_on_shared_node();
        let meta = meta_of(&a, node);
        #[allow(deprecated)]
        let legacy: loro::LoroText = meta
            .get_or_create_container(CONTENT_RAW, loro::LoroText::new())
            .unwrap();
        legacy.insert(0, "written before the migration").unwrap();
        a.commit();

        let err = update_text_field(&meta, CONTENT_RAW, "new")
            .expect_err("prod must not write through a legacy op-id child");

        assert!(
            err.to_string().contains("predates the mergeable migration"),
            "the refusal must name the migration; got: {err}"
        );
    }
}

#[cfg(test)]
mod legacy_link_mark_payloads {
    use holon_api::EntityRef;
    use holon_api::InlineMark;

    use super::*;

    fn link_map(kind: &str, key: &str, target: &str) -> loro::LoroValue {
        let mut map = std::collections::HashMap::new();
        map.insert("label".to_string(), loro::LoroValue::from(target));
        map.insert("type".to_string(), loro::LoroValue::from(kind));
        map.insert(key.to_string(), loro::LoroValue::from(target));
        loro::LoroValue::from(map)
    }

    /// Loro marks written before the scheme merge carry `internal`/`id` or
    /// `unknown_scheme`/`uri`. The latter also holds targets that are NOT
    /// valid absolute URIs (a wiki path with a colon in a later segment), so a
    /// reader that pushes the payload through `EntityUri::from_raw` silently
    /// rewrites `Areas/cc-session:abc` to `block:Areas/cc-session:abc` — a
    /// silent edit of authored link text, worse than refusing to load.
    #[test]
    fn every_legacy_spelling_reads_back_verbatim() {
        let cases = [
            ("unknown_scheme", "uri", "Areas/cc-session:abc"),
            ("unknown_scheme", "uri", "Meeting/Notes:2026"),
            ("unknown_scheme", "uri", "t-widget:abc123"),
            ("internal", "id", "block:abc-123"),
            ("scheme", "raw", "tag:work"),
        ];
        for (kind, key, target) in cases {
            let value = link_map(kind, key, target);
            let mark = mark_from_loro_value("link", &value)
                .unwrap_or_else(|| panic!("legacy {kind} mark must read: {target}"));
            let InlineMark::Link {
                target: EntityRef::Scheme { raw },
                ..
            } = &mark
            else {
                panic!("expected a Scheme link for {kind}/{target}, got {mark:?}");
            };
            assert_eq!(raw, target, "{kind} payload altered the target text");
        }
    }

    /// The write half round-trips: what we emit today reads back
    /// byte-identical.
    #[test]
    fn scheme_marks_round_trip_through_loro() {
        for target in ["Areas/cc-session:abc", "t-widget:abc123", "block:abc-123"] {
            let mark = InlineMark::Link {
                target: EntityRef::Scheme {
                    raw: target.to_string(),
                },
                label: target.to_string(),
            };
            let value = mark_to_loro_value(&mark);
            let back = mark_from_loro_value("link", &value).expect("round trips");
            assert_eq!(back, mark, "loro round trip altered {target}");
        }
    }
}

#[cfg(test)]
mod half_born_node_tests {
    use std::sync::Arc;

    use super::*;
    use crate::LoroDocument;

    async fn create(backend: &LoroBackend, parent: EntityUri, id: &str) {
        backend
            .create_block_with_properties(
                parent,
                BlockContent::text(id),
                Some(EntityUri::block(id)),
                &HashMap::new(),
                &Tags::default(),
                &[],
                &[],
            )
            .await
            .unwrap();
    }

    /// The create window: `tree.create()` and the `STABLE_ID` insert are two
    /// doc-state steps, and `LoroDocument::with_write` does not exclude
    /// `with_read`, so a reader on another task observes a live child that has
    /// no `STABLE_ID` yet. `list_children` must withhold it, not panic —
    /// panicking kills the tokio worker that owns the org-writeback fold and
    /// the file stays stale for the life of the process.
    ///
    /// Withholding (not `Err`) is the required shape: `file_sync_controller`
    /// polls `ordering.children` until the expected ids appear and `?`-bails on
    /// `Err`, so absence is recoverable and an error is not.
    #[tokio::test]
    async fn list_children_withholds_a_child_whose_stable_id_has_not_landed() {
        let doc = Arc::new(LoroDocument::new("half-born".to_string()).unwrap());
        let backend = LoroBackend::from_document(doc.clone());
        create(&backend, EntityUri::no_parent(), "parent").await;
        create(&backend, EntityUri::block("parent"), "settled").await;

        let parent_tid = backend.resolve_to_tree_id("block:parent").await.unwrap();
        doc.with_write(|d| {
            d.get_tree(TREE_NAME).create(Some(parent_tid))?;
            Ok(())
        })
        .unwrap();

        let kids = backend.list_children("block:parent").await.unwrap();
        assert_eq!(
            kids,
            vec!["block:settled".to_string()],
            "a half-born child must be withheld, and its settled sibling must still be answered"
        );
    }
}
