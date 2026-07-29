//! [`EntityCellRegistry`] for the `block` entity type.
//!
//! Phase 1 wired `block.content` to a [`LoroTextCellBacking`] when a Loro
//! doc was configured (Full mode). Phase 2 extends this with a
//! [`BlockCellRegistry::write_field`] dispatcher: every block field write
//! is offered to the registry first, which routes it through the Loro
//! authority (LoroText for `content`, LoroTree `tree.mov` for `parent_id`,
//! LoroMap meta entries for everything else). On `Ok(false)` the caller
//! falls through to direct SQL — that branch only runs in SqlOnly mode or
//! for fields without a clean Loro encoding (`sort_key`, `depth`, ...).
//!
//! Caching for the `content` cell is delegated to
//! [`holon_core::cell_registry::CellCache`] — `Weak`-keyed natural eviction
//! plus an `on_entity_deleted` proactive prune so a same-id re-create
//! can't observe a stale cell wrapping an orphaned `LoroText` container.

use std::any::Any;
use std::any::TypeId;
use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::stream::StreamExt;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::live_data::LiveData;
use holon_api::repository::CoreOperations;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::cell::CellBacking;
use holon_core::cell::LwwScalarBacking;
use holon_core::cell::LwwTextCellBacking;
use holon_core::cell_registry::CellCache;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::cell_registry::EntityCellRegistryExt;
use loro::LoroDoc;
use loro::LoroText;

use crate::loro_backend::CONTENT_RAW;
use crate::loro_backend::LoroBackend;
use crate::loro_backend::NewBlockWithProperties;
use crate::loro_backend::STABLE_ID;
use crate::loro_backend::TREE_NAME;
use crate::loro_document::LoroDocument;
use crate::loro_meta_cell_backing::LoroMetaCellBacking;
use crate::loro_meta_cell_backing::LoroScalarField;
use crate::loro_text_cell_backing::LoroTextCellBacking;

/// Injected write path for SqlOnly cells: routes a `(uri, field, value)` scalar
/// write straight to the SQL `set_field` operation (the composition root builds
/// this in `event_infra_module` over `SqlOperationProvider`, bypassing the
/// registry's own `write_field` so there is no `Arc` cycle back through
/// `SqlBlockOperations`, which owns the registry).
pub type SqlScalarWriteFn =
    Arc<dyn Fn(EntityUri, String, Value) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// Deps that make SqlOnly cells resolve to a live `LwwScalarBacking` /
/// `LwwTextCellBacking` instead of erroring: the convergent `LiveData<Block>`
/// entity cache (sync `read()` for `current()`, `signal_map()` for the CDC
/// signal) plus the SQL `set_field` write path. Injected via the DI seam so
/// `holon-loro` never names the `holon`-side `SqlOperationProvider`.
struct SqlCellWiring {
    live: Arc<LiveData<Block>>,
    write: SqlScalarWriteFn,
}

/// Registry of [`Cell<T>`](holon_core::cell::Cell)s for `block` entity
/// fields.
///
/// Construction modes:
/// - `with_loro(doc)` — Full mode; `content` returns a Loro-backed cell.
/// - `sql_only()` — SqlOnly mode; ANY `live_field_any` call errors loudly
///   because Phase 1 has no editor in SqlOnly mode and synthetic test stores
///   bypass the registry entirely (they get `cells() == None` from the
///   `BlockOperations` default).
pub struct BlockCellRegistry {
    cache: CellCache,
    backing_source: BackingSource,
}

enum BackingSource {
    Loro {
        doc: Arc<LoroDoc>,
        backend: Arc<LoroBackend>,
    },
    /// SqlOnly mode. `wiring` is `Some` once the composition root injects the
    /// entity-cache read + `set_field` write seam (`sql_only_wired`); `None`
    /// for non-DI / synthetic-test construction (`sql_only`), where
    /// `live_field` keeps erroring loudly — a fake no-op backing would hide
    /// a genuinely-unavailable dependency.
    SqlOnly { wiring: Option<SqlCellWiring> },
}

impl BlockCellRegistry {
    /// Construct a Loro-backed registry from the shared `LoroDocument`. The
    /// registry walks the tree on demand to resolve `(block_id, "content")`
    /// to a `LoroText` container, and dispatches non-content writes through
    /// the wrapped [`LoroBackend`] (Phase 2 authority flip).
    pub fn with_loro(loro_doc: Arc<LoroDocument>) -> Self {
        let doc = loro_doc.doc();
        let backend = Arc::new(LoroBackend::from_document(loro_doc));
        Self {
            cache: CellCache::new(),
            backing_source: BackingSource::Loro { doc, backend },
        }
    }

    /// Convenience constructor that takes a raw `Arc<LoroDoc>`. Used by
    /// tests and integration test fixtures (`pbt/sut.rs`) that build a
    /// `LoroDoc` directly via the `loro` crate without going through
    /// `LoroDocumentStore`. Production callers should prefer
    /// [`Self::with_loro`].
    pub fn with_loro_doc(doc: Arc<LoroDoc>) -> Self {
        let loro_doc = LoroDocument::from_existing(doc.clone(), "test");
        let backend = Arc::new(LoroBackend::from_document(Arc::new(loro_doc)));
        Self {
            cache: CellCache::new(),
            backing_source: BackingSource::Loro { doc, backend },
        }
    }

    /// Construct a SqlOnly-mode registry with no injected cell seam. All
    /// `live_field_any` calls error with a clear message — used by non-DI /
    /// synthetic-test construction where the entity cache + `set_field` write
    /// path aren't available. DI callers use [`Self::sql_only_wired`].
    pub fn sql_only() -> Self {
        Self {
            cache: CellCache::new(),
            backing_source: BackingSource::SqlOnly { wiring: None },
        }
    }

    /// Construct a SqlOnly-mode registry wired to the convergent
    /// `LiveData<Block>` entity cache (read + CDC signal) and the SQL
    /// `set_field` write path. `live_field` then resolves the same
    /// `Cell<T>` surface a caller sees in Full (Loro) mode — content via
    /// [`LwwTextCellBacking`], scalars via [`LwwScalarBacking`] — so the two
    /// mode surfaces are symmetric. The composition root
    /// (`event_infra_module`) builds `write` over `SqlOperationProvider`.
    pub fn sql_only_wired(live: Arc<LiveData<Block>>, write: SqlScalarWriteFn) -> Self {
        Self {
            cache: CellCache::new(),
            backing_source: BackingSource::SqlOnly {
                wiring: Some(SqlCellWiring { live, write }),
            },
        }
    }

    fn loro_doc(&self) -> Result<Arc<LoroDoc>> {
        match &self.backing_source {
            BackingSource::Loro { doc, .. } => Ok(doc.clone()),
            BackingSource::SqlOnly { .. } => Err(anyhow!(
                "BlockCellRegistry is in SqlOnly mode; no Loro backing is available. SqlOnly \
                 cells are not wired yet (they need the entity-cache read + CDC signal injection; \
                 see LwwScalarBacking / LwwTextCellBacking)."
            )),
        }
    }

    /// Walk the Loro tree for the node whose stable id matches `block_id` and
    /// return its `meta` map — the read/write root shared by the content
    /// container and every scalar property. Errors loudly if the block isn't
    /// in the tree (inbound consumer hasn't applied the create yet, or it was
    /// never imported), never silently falling to SQL.
    fn resolve_node_meta(&self, block_id: &str) -> Result<loro::LoroMap> {
        let doc = self.loro_doc()?;
        let bare_id = block_id.strip_prefix("block:").unwrap_or(block_id);
        let tree = doc.get_tree(TREE_NAME);
        for node in tree.get_nodes(false) {
            if matches!(
                node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            ) {
                continue;
            }
            let meta = tree
                .get_meta(node.id)
                .map_err(|e| anyhow!("tree.get_meta({:?}) failed: {e:#}", node.id))?;
            let stable_id_matches = match meta.get(STABLE_ID) {
                Some(loro::ValueOrContainer::Value(v)) => {
                    v.as_string().is_some_and(|s| s.to_string() == bare_id)
                }
                _ => false,
            };
            if stable_id_matches {
                return Ok(meta);
            }
        }
        Err(anyhow!(
            "Block {block_id} not found in Loro tree (inbound consumer hasn't applied the create \
             event yet, or the block was never imported)"
        ))
    }

    fn resolve_loro_text_container(&self, block_id: &str) -> Result<(Arc<LoroDoc>, LoroText)> {
        let doc = self.loro_doc()?;
        let meta = self.resolve_node_meta(block_id)?;
        // Source blocks keep their content in the SOURCE_CODE container
        // (`write_content_to_meta`), text blocks in CONTENT_RAW. Binding
        // CONTENT_RAW unconditionally silently forked source-block content:
        // `set_field("content")` (org re-ingest of an index.org swap) wrote a
        // container that `read_content_from_meta` never reads — the update was
        // lost and Loro kept serving the stale source text.
        let content_key = match meta.get(crate::loro_backend::CONTENT_TYPE) {
            Some(loro::ValueOrContainer::Value(v))
                if v.as_string().is_some_and(|s| s.as_str() == "source") =>
            {
                crate::loro_backend::SOURCE_CODE
            }
            _ => CONTENT_RAW,
        };
        // replacement changes CRDT child-creation semantics — pending Martin ruling
        #[allow(deprecated)]
        let text = meta
            .get_or_create_container(content_key, LoroText::new())
            .map_err(|e| anyhow!("get_or_create_container({content_key}) for {block_id}: {e:?}"))?;
        Ok((doc, text))
    }

    /// Write a single block field through the Loro authority. Returns
    /// `Ok(true)` when the write landed via Loro; `Ok(false)` when this
    /// registry can't handle the (uri, field) pair (SqlOnly mode, or a
    /// field whose Loro encoding doesn't round-trip cleanly to SQL today —
    /// `sort_key` lives only in SQL; `depth` is derived from tree
    /// structure; the various `_expected_*` watermark fields produced by
    /// the outbound projector are control metadata, not field writes).
    /// On `Ok(false)` the caller falls through to the SQL `set_field`
    /// path. On `Err` the error is loud and the SQL path is NOT tried.
    /// Copy-on-write seed refresh. For each `(id, content)` whose block already
    /// exists in the Loro authority with DIFFERENT content, rewrite the content
    /// to the current shipped-asset value via `update_block_text` (which routes
    /// Source → `SOURCE_CODE`, Text → `CONTENT_RAW` by the block's stored
    /// content_type). Compares against the authority FIRST, so re-seeding an
    /// UNCHANGED default layout emits NO Loro op — boot re-seed is churn-free.
    /// A block with no tree node yet is skipped (the create pass seeds it
    /// fresh). Returns how many blocks were refreshed. SqlOnly: `Ok(0)` — there
    /// is no separate Loro authority to reconcile against.
    pub async fn reseed_content(&self, blocks: &[(EntityUri, String)]) -> Result<usize> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(0),
        };
        let mut refreshed = 0usize;
        for (id, content) in blocks {
            if backend.resolve_to_tree_id(id.id()).await.is_none() {
                continue;
            }
            let current = backend
                .get_block(id.id())
                .await
                .map_err(|e| anyhow!("reseed_content get_block({id}): {e:#}"))?;
            if &current.content != content {
                backend
                    .update_block_text(id.id(), content)
                    .await
                    .map_err(|e| anyhow!("reseed_content update_block_text({id}): {e:#}"))?;
                tracing::info!(
                    "[seed] copy-on-write refresh: {id} default-layout content updated from the                      shipped asset ({} -> {} chars)",
                    current.content.len(),
                    content.len()
                );
                refreshed += 1;
            }
        }
        Ok(refreshed)
    }

    pub async fn write_field(&self, uri: &EntityUri, field: &str, value: Value) -> Result<bool> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(false),
        };

        // Watermark / control fields produced by `LoroSyncController`'s
        // outbound diff. They're not field writes; they're WHERE-clause
        // hints for the SQL UPDATE the projector emits. Pass straight
        // through to SQL.
        if field.starts_with("_expected_") {
            return Ok(false);
        }

        // Fields without a clean Loro encoding today. Keep them on the
        // SQL path; the inbound consumer (when it sees a non-Loro origin
        // CDC event) reflects them back into Loro where applicable.
        // - `id`: the row's primary key, never reassigned
        // - `depth`: derived from the tree structure on snapshot
        // - `content_type`, `source_name`: stored in Loro but written by
        //   `update_block_text` / chord-time content create paths, not by `set_field`
        //   callers
        // (`source_language` IS handled below: the org re-ingest of an
        // `index.org` swap legitimately changes a src block's language via
        // `set_field`, and an SQL-direct write would fork the authority.)
        if matches!(field, "id" | "depth" | "content_type" | "source_name") {
            return Ok(false);
        }

        let id = uri.to_string();
        match field {
            "content" => {
                let s = value.as_string().map(String::from).ok_or_else(|| {
                    anyhow!("write_field(content): expected String, got {value:?}")
                })?;
                // A block with no Loro tree node has its content authority in
                // SQL (unseeded vault, SQL-origin block the inbound consumer
                // hasn't mirrored). Fall through to the direct SQL write the
                // `set_field` contract documents instead of erroring — the
                // inbound consumer reflects the SQL CDC event into Loro when
                // the block eventually exists there. ALLOW(fallback):
                // disclosed degraded mode, same rationale as `create_entity`'s
                // anchor guard.
                if backend.resolve_to_tree_id(uri.id()).await.is_none() {
                    tracing::warn!(
                        "write_field(content) for {uri}: no Loro tree node — writing through the \
                         SQL path (Loro authority missing or unseeded for this block)"
                    );
                    return Ok(false);
                }
                let cell =
                    (self as &dyn EntityCellRegistry).live_field::<String>(uri, "content")?;
                let debug = std::env::var("HOLON_CELL_WRITE_DEBUG").is_ok();
                let before = debug.then(|| cell.current());
                cell.set(s.clone()).await?;
                if debug {
                    tracing::warn!(
                        "[CELL_WRITE] content {uri}: {:?} -> {:?} (now {:?})",
                        before,
                        s,
                        cell.current()
                    );
                }
                Ok(true)
            }
            "source_language" => {
                let s = value.as_string().map(String::from).ok_or_else(|| {
                    anyhow!("write_field(source_language): expected String, got {value:?}")
                })?;
                backend
                    .set_source_language(&id, &s)
                    .await
                    .map_err(|e| anyhow!("set_source_language({id}): {e:#}"))?;
                Ok(true)
            }
            "parent_id" => {
                let s = value.as_string().map(String::from).ok_or_else(|| {
                    anyhow!("write_field(parent_id): expected String, got {value:?}")
                })?;
                backend
                    .update_parent_id(&id, s)
                    .await
                    .map_err(|e| anyhow!("update_parent_id({id}): {e:#}"))?;
                Ok(true)
            }
            _ if holon_api::EdgeField::is_edge_column(field) => {
                // Set-valued edge field (`tags`/`requires`/`advice_suppressed`):
                // write the tree node's dedicated meta key so the Loro→SQL
                // projector reads it into the matching junction table. Generic
                // over every `EdgeField` member — routing to `properties` (the
                // `_` cell arm) would drop it from the junction.
                let targets = Self::parse_edge_string_targets(field, &value)?;
                backend
                    .set_block_edge_field(&id, field, &targets)
                    .await
                    .map_err(|e| anyhow!("set_block_edge_field({id}, {field}): {e:#}"))?;
                Ok(true)
            }
            "sort_key" => {
                // Sibling order is owned by `place()`/`tree.mov_after` and
                // projected to SQL from the Loro fractional index by the outbound
                // snapshot projection — a block's `sort_key` is never written
                // through `set_field`. The org sync path explicitly omits it
                // (`build_block_params`). A `set_field("sort_key")` reaching
                // here is a bug, not a positional intent — fail loud rather than
                // silently mis-route it to the meta `properties` map (which
                // `read_block_from_tree` ignores in favour of the fractional
                // index).
                Err(anyhow!(
                    "write_field(sort_key) is unsupported: order is owned by place()/mov_after \
                     and projected from the fractional index; a set_field(\"sort_key\") reached \
                     the cell registry for {id} — bug"
                ))
            }
            "task_state" => {
                // `task_state` travels with its `task_state_category` sidecar:
                // the org parse boundary (`Block::set_task_state`) writes BOTH
                // keys and `Block::task_state()` reads the pair back into a
                // `TaskState`. The widget click intent (`state_toggle` →
                // `set_field("task_state", next)`) carries only the keyword, so
                // this boundary derives and writes the sidecar alongside —
                // otherwise every UI cycle dropped/staled the category (a DONE
                // keyword could read back as Active). Both keys land in ONE
                // `update_block_properties` commit (per-key LWW merge, H3).
                let category = match &value {
                    Value::Null => Value::Null,
                    Value::String(kw) => Value::String(
                        holon_api::TaskState::category_str_for_keyword(kw).to_string(),
                    ),
                    other => {
                        return Err(anyhow!(
                            "write_field(task_state): expected String or Null, got {other:?}"
                        ));
                    }
                };
                let mut props = std::collections::HashMap::new();
                props.insert("task_state".to_string(), value);
                props.insert("task_state_category".to_string(), category);
                backend
                    .update_block_properties(&id, &props)
                    .await
                    .map_err(|e| anyhow!("update_block_properties(task_state) for {id}: {e:#}"))?;
                Ok(true)
            }
            "marks" => {
                // Phase 3.2: marks go through the Peritext write path
                // (`update_block_marked`). The marks live on the LoroText
                // container; we re-assert the current text alongside the new
                // mark set so the existing helper's "wholesale replace" stays
                // tight. `Value::Null` clears all marks.
                let marks: Vec<holon_api::MarkSpan> = match &value {
                    Value::Null => Vec::new(),
                    Value::String(s) => holon_api::marks_from_json(s)
                        .map_err(|e| anyhow!("write_field(marks): marks JSON parse error: {e}"))?,
                    other => {
                        return Err(anyhow!(
                            "write_field(marks): expected String (JSON) or Null, got {other:?}"
                        ));
                    }
                };
                let current = backend
                    .get_block(&id)
                    .await
                    .map_err(|e| anyhow!("write_field(marks): get_block({id}): {e:#}"))?;
                backend
                    .update_block_marked(&id, &current.content, &marks)
                    .await
                    .map_err(|e| anyhow!("update_block_marked({id}): {e:#}"))?;
                Ok(true)
            }
            // Other scalars (completed, collapsed, block_type, properties,
            // created_at, updated_at, …) resolve a `LoroMetaCellBacking` cell
            // and write through it (invariant 12). The cell's `apply_replace`
            // still lands via `update_block_fields` — touching only this key
            // (per-key LWW, H3) and bumping `updated_at` — so the projection is
            // unchanged; the difference is the write now shares the exact
            // backing a live reader observes, instead of a parallel dispatch.
            _ => {
                let cell = (self as &dyn EntityCellRegistry).live_field::<Value>(uri, field)?;
                cell.set(value).await?;
                Ok(true)
            }
        }
    }

    /// Parse an edge-field write value (`Value::Array` of strings, or
    /// `Value::Null` for the empty set) into owned strings — the on-the-wire
    /// shape every edge field shares (tag strings or id strings), stored as a
    /// JSON string array. Fails loud on any non-string entry.
    fn parse_edge_string_targets(field: &str, value: &Value) -> Result<Vec<String>> {
        match value {
            Value::Array(items) => items
                .iter()
                .map(|v| {
                    v.as_string().map(String::from).ok_or_else(|| {
                        anyhow!("write_field({field}): edge target entry not a string: {v:?}")
                    })
                })
                .collect(),
            Value::Null => Ok(Vec::new()),
            other => Err(anyhow!(
                "write_field({field}): expected Array of strings, got {other:?}"
            )),
        }
    }
}

/// Build the wired-SqlOnly `content` cell: an LWW `Cell<String>` reading the
/// block's text from the entity cache and writing via `set_field("content")`.
/// The storage-agnostic twin of the Full-mode Loro `content` cell.
fn build_sql_content_cell(wiring: &SqlCellWiring, uri: &EntityUri) -> Arc<dyn CellBacking<String>> {
    let live = wiring.live.clone();
    let key = uri.to_string();

    let read_live = live.clone();
    let read_key = key.clone();
    let read = Arc::new(move || -> String {
        read_live
            .read()
            .get(&read_key)
            .map(|b| b.content_text().to_string())
            .unwrap_or_default()
    });

    let write_fn = wiring.write.clone();
    let write_uri = uri.clone();
    let write = Arc::new(move |v: String| {
        let write_fn = write_fn.clone();
        let uri = write_uri.clone();
        Box::pin(async move { (write_fn)(uri, "content".to_string(), Value::String(v)).await })
            as BoxFuture<'static, Result<()>>
    });

    let sig_live = live;
    let sig_key = key;
    let signal_factory = Arc::new(move || -> BoxStream<'static, String> {
        use futures_signals::signal::SignalExt;
        use futures_signals::signal_map::SignalMapExt;
        Box::pin(
            sig_live
                .signal_map()
                .key_cloned(sig_key.clone())
                .to_stream()
                .map(|opt: Option<Arc<Block>>| {
                    opt.map(|b| b.content_text().to_string())
                        .unwrap_or_default()
                }),
        )
    });

    Arc::new(LwwTextCellBacking::new(read, write, signal_factory)) as Arc<dyn CellBacking<String>>
}

/// Dispatch a wired-SqlOnly scalar `live_field` to a typed `LwwScalarBacking`.
/// The type set mirrors the Loro twin's `live_field_any` dispatch so the two
/// mode surfaces are symmetric: `T ∈ {bool, i64, String, Value}`.
fn build_sql_scalar_cell(
    cache: &CellCache,
    wiring: &SqlCellWiring,
    uri: &EntityUri,
    field: &str,
    type_id: TypeId,
) -> Result<Arc<dyn Any + Send + Sync>> {
    if type_id == TypeId::of::<bool>() {
        cache.get_or_construct::<bool, _>(uri, field, || {
            Ok(sql_scalar_backing::<bool>(wiring, uri, field))
        })
    } else if type_id == TypeId::of::<i64>() {
        cache.get_or_construct::<i64, _>(uri, field, || {
            Ok(sql_scalar_backing::<i64>(wiring, uri, field))
        })
    } else if type_id == TypeId::of::<String>() {
        cache.get_or_construct::<String, _>(uri, field, || {
            Ok(sql_scalar_backing::<String>(wiring, uri, field))
        })
    } else if type_id == TypeId::of::<Value>() {
        cache.get_or_construct::<Value, _>(uri, field, || {
            Ok(sql_scalar_backing::<Value>(wiring, uri, field))
        })
    } else {
        Err(anyhow!(
            "BlockCellRegistry::live_field_any: SqlOnly scalar field {field:?} has no cell for \
             the requested type (supported: bool, i64, String, Value)"
        ))
    }
}

/// One typed wired-SqlOnly scalar backing. `read`/signal decode the field from
/// the entity cache (a present-but-wrong-shape value is corruption → panic,
/// exactly as the Loro twin's `read` does); `write` encodes and routes through
/// the injected `set_field` path.
fn sql_scalar_backing<T: LoroScalarField>(
    wiring: &SqlCellWiring,
    uri: &EntityUri,
    field: &str,
) -> Arc<dyn CellBacking<T>> {
    let live = wiring.live.clone();
    let key = uri.to_string();

    let read_live = live.clone();
    let read_key = key.clone();
    let read_field = field.to_string();
    let read = Arc::new(move || -> T {
        let snap = read_live.read();
        let stored = snap
            .get(&read_key)
            .and_then(|b| b.get_property(&read_field));
        T::decode(stored)
            .unwrap_or_else(|e| panic!("SqlOnly scalar read ({read_key}, {read_field}): {e:#}"))
    });

    let write_fn = wiring.write.clone();
    let write_uri = uri.clone();
    let write_field = field.to_string();
    let write = Arc::new(move |v: T| {
        let write_fn = write_fn.clone();
        let uri = write_uri.clone();
        let field = write_field.clone();
        let value = v.encode();
        Box::pin(async move { (write_fn)(uri, field, value).await })
            as BoxFuture<'static, Result<()>>
    });

    let sig_live = live;
    let sig_key = key;
    let sig_field = field.to_string();
    let signal_factory = Arc::new(move || -> BoxStream<'static, T> {
        use futures_signals::signal::SignalExt;
        use futures_signals::signal_map::SignalMapExt;
        let field = sig_field.clone();
        Box::pin(
            sig_live
                .signal_map()
                .key_cloned(sig_key.clone())
                .to_stream()
                .map(move |opt: Option<Arc<Block>>| {
                    let stored = opt.and_then(|b| b.get_property(&field));
                    T::decode(stored)
                        .unwrap_or_else(|e| panic!("SqlOnly scalar signal ({field}): {e:#}"))
                }),
        )
    });

    Arc::new(LwwScalarBacking::<T>::new(read, write, signal_factory)) as Arc<dyn CellBacking<T>>
}

/// Resolve `parent_id` to a node that exists in the Loro tree, standing up a
/// placeholder root for it when it does not — a child reached before its own
/// parent's create, which the org scan does routinely.
///
/// The placeholder carries NO content and NO tags, so its outbound projection
/// writes an empty, untagged row over whatever the parent already had in SQL.
/// That is why it is disclosed here and why the create path COMPLETES such a
/// node (content + home) the moment the real create for it arrives.
async fn resolve_parent_or_placeholder(
    backend: &Arc<LoroBackend>,
    parent_id: &EntityUri,
    child_id: &EntityUri,
) -> Result<EntityUri> {
    if parent_id.is_no_parent()
        || parent_id.is_sentinel()
        || backend.resolve_to_tree_id(parent_id.id()).await.is_some()
    {
        return Ok(parent_id.clone());
    }
    tracing::warn!(
        child = %child_id,
        parent = %parent_id,
        "standing up an EMPTY placeholder root for a parent not yet in the Loro tree"
    );
    let placeholder = backend
        .create_placeholder_root(parent_id.id())
        .await
        .map_err(|e| anyhow!("create_placeholder_root({parent_id}): {e:#}"))?;
    // ALLOW(entity_uri_from_raw): placeholder id String from
    // backend.create_placeholder_root() (Loro adapter output)
    Ok(EntityUri::from_raw(&placeholder))
}

#[async_trait::async_trait]
impl EntityCellRegistry for BlockCellRegistry {
    fn live_field_any(
        &self,
        uri: &EntityUri,
        field: &str,
        type_id: TypeId,
    ) -> Result<Arc<dyn Any + Send + Sync>> {
        let block_id_owned = uri.id().to_string();

        // `content` is rich text on its own LoroText container.
        if field == "content" {
            if type_id != TypeId::of::<String>() {
                return Err(anyhow!(
                    "BlockCellRegistry::live_field_any: field \"content\" requires T=String \
                     (caller asked for a different type)"
                ));
            }
            return self.cache.get_or_construct::<String, _>(uri, field, || {
                // Wired SqlOnly mode presents the same `content` cell surface as
                // Full mode, but LWW (no rich-text ops) — the storage-agnostic
                // twin of the Loro `LoroText` cell.
                if let BackingSource::SqlOnly {
                    wiring: Some(wiring),
                } = &self.backing_source
                {
                    return Ok(build_sql_content_cell(wiring, uri));
                }
                let (doc, text) = self.resolve_loro_text_container(&block_id_owned)?;
                let backing = LoroTextCellBacking::new(doc, text)?;
                Ok(Arc::new(backing) as Arc<dyn CellBacking<String>>)
            });
        }

        // Every other field is a scalar. Full mode resolves it on the node meta
        // map; wired SqlOnly mode resolves an `LwwScalarBacking` over the
        // entity cache + `set_field`; unwired SqlOnly errors loudly.
        let (doc, backend) = match &self.backing_source {
            BackingSource::Loro { doc, backend } => (doc.clone(), backend.clone()),
            BackingSource::SqlOnly {
                wiring: Some(wiring),
            } => {
                return build_sql_scalar_cell(&self.cache, wiring, uri, field, type_id);
            }
            BackingSource::SqlOnly { wiring: None } => {
                return Err(anyhow!(
                    "BlockCellRegistry::live_field_any: SqlOnly mode has no scalar cell for field \
                     {field:?}; the entity-cache read + set_field write seam was not injected \
                     (use BlockCellRegistry::sql_only_wired)."
                ));
            }
        };
        let meta = self.resolve_node_meta(&block_id_owned)?;
        let schemed_id = uri.to_string();
        let make = |m: loro::LoroMap| (doc.clone(), backend.clone(), m, schemed_id.clone());

        if type_id == TypeId::of::<bool>() {
            let mk = make(meta);
            self.cache.get_or_construct::<bool, _>(uri, field, || {
                Ok(Arc::new(LoroMetaCellBacking::<bool>::new(
                    mk.0,
                    mk.1,
                    mk.2,
                    mk.3,
                    field.to_string(),
                )?) as Arc<dyn CellBacking<bool>>)
            })
        } else if type_id == TypeId::of::<i64>() {
            let mk = make(meta);
            self.cache.get_or_construct::<i64, _>(uri, field, || {
                Ok(Arc::new(LoroMetaCellBacking::<i64>::new(
                    mk.0,
                    mk.1,
                    mk.2,
                    mk.3,
                    field.to_string(),
                )?) as Arc<dyn CellBacking<i64>>)
            })
        } else if type_id == TypeId::of::<String>() {
            let mk = make(meta);
            self.cache.get_or_construct::<String, _>(uri, field, || {
                Ok(Arc::new(LoroMetaCellBacking::<String>::new(
                    mk.0,
                    mk.1,
                    mk.2,
                    mk.3,
                    field.to_string(),
                )?) as Arc<dyn CellBacking<String>>)
            })
        } else if type_id == TypeId::of::<Value>() {
            let mk = make(meta);
            self.cache.get_or_construct::<Value, _>(uri, field, || {
                Ok(Arc::new(LoroMetaCellBacking::<Value>::new(
                    mk.0,
                    mk.1,
                    mk.2,
                    mk.3,
                    field.to_string(),
                )?) as Arc<dyn CellBacking<Value>>)
            })
        } else {
            Err(anyhow!(
                "BlockCellRegistry::live_field_any: scalar field {field:?} has no cell for the \
                 requested type (supported: bool, i64, String, Value)"
            ))
        }
    }

    fn on_entity_deleted(&self, uri: &EntityUri) {
        self.cache.evict_uri(uri);
    }

    /// Item 4 phase 1: typed positional write. Routes a (parent, after_id)
    /// positional intent straight to `LoroBackend::update_block_position`,
    /// bypassing the legacy `set_field("sort_key", gen_key_between(...))`
    /// string round-trip. In SqlOnly mode, returns `Ok(false)` so the
    /// caller falls back to the gen_key_between + `set_field` shape that
    /// still persists the fractional-index value in the SQL column.
    async fn write_position(
        &self,
        uri: &EntityUri,
        parent_id: &str,
        after_id: Option<&str>,
    ) -> Result<bool> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(false),
        };
        // Synthetic SQL-only blocks (render artifacts like `<parent>::src::0` /
        // `::render::0`) have no Loro node — their order lives only in SQL. Fall
        // through to the SQL sort_key path (`Ok(false)`) instead of letting
        // `update_block_position` error "Block not found", which propagated up
        // through `update_in_tree` and aborted the org scan's update pass
        // *before* the place loop ran — scrambling sibling order
        // (`inv-live-children-match-ref`). Mirrors the resolve-first guard in
        // `create_entity`.
        if backend.resolve_to_tree_id(uri.id()).await.is_none() {
            return Ok(false);
        }
        backend
            .update_block_position(uri.id(), parent_id, after_id)
            .await
            .map_err(|e| anyhow!("update_block_position({}): {e:#}", uri.id()))?;
        Ok(true)
    }

    /// Authoritative block create through `LoroBackend::create_block` +
    /// `update_block_position`. The chord-op (`split_block`) drives this
    /// instead of `BlockOperations::create` so the new block lands in the
    /// Loro tree first; the outbound projector then emits the SQL INSERT
    /// tagged `EventOrigin::Loro`, which the inbound gate `EchoSuppress`es
    /// rather than dropping as an unmigrated SQL-direct write. SqlOnly
    /// mode returns `Ok(false)` so the caller falls back to the SQL path.
    async fn create_entity(
        &self,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
        new_id: &EntityUri,
        content: holon_api::BlockContent,
        properties: &std::collections::HashMap<String, holon_api::Value>,
        tags: &Tags,
        requires: &[EntityUri],
        advice_suppressed: &[EntityUri],
    ) -> Result<bool> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(false),
        };
        // The positional anchor must already be under Loro authority. When
        // the after-block has no tree node (unseeded vault, synthetic
        // SQL-only row), positioning through Loro is impossible — fall
        // through to the SQL path BEFORE touching the tree. The pre-guard
        // order matters: erroring after `create_block` poisoned the tree
        // with placeholder roots + empty-text nodes whose "" content then
        // shadowed the real SQL content on later reads ("Split position N
        // exceeds content length 0"). Mirrors the resolve-first guard in
        // `write_position`. ALLOW(fallback): disclosed degraded mode — the
        // new block stays in the same (SQL-only) store as its anchor.
        if let Some(after) = after_id
            && backend.resolve_to_tree_id(after.id()).await.is_none()
        {
            tracing::warn!(
                "create_entity({new_id}): after-block {after} has no Loro tree node — falling \
                 back to the SQL create path (Loro authority missing or unseeded for this \
                 block family)"
            );
            return Ok(false);
        }
        // Idempotent: if the node already exists in the tree (e.g. the org
        // initial scan calls this for a block a prior scan/seed already
        // placed), skip the create — `create_block` would mint a duplicate
        // node for the same stable id. Still apply the requested position.
        if backend.resolve_to_tree_id(new_id.id()).await.is_some() {
            if let Some(after) = after_id {
                backend
                    .update_block_position(new_id.id(), parent_id.as_str(), Some(after.id()))
                    .await
                    .map_err(|e| anyhow!("update_block_position({new_id}): {e:#}"))?;
            }
            // Reconcile the requested edge fields against the existing node — but
            // only WRITE when the tree's current value differs from the request.
            //
            // The existing node may be a tagless placeholder root, auto-created
            // (below) when a child's `create_in_tree` reached this id before its
            // own create call: reconciling its tags keeps a page document's `Page`
            // marker in Loro (otherwise the outbound projector diffs Loro(no tag)
            // against SQL(Page) and wipes the SQL tag). But re-asserting a value
            // the node ALREADY carries still emits a Loro op → DiffEvent → SQL
            // junction DELETE+INSERT: gratuitous churn on every boot re-seed and
            // org re-scan, and — on a restart, where the persisted matview keeps a
            // tag its emptied base table no longer holds — the delta that DOUBLES
            // the matview tag row. Comparing against the Loro tree (the authority,
            // fully loaded before the seed runs) makes the skip deterministic,
            // unlike diffing the lagging SQL projection during boot.
            let current = backend
                .get_block(new_id.id())
                .await
                .map_err(|e| anyhow!("get_block({new_id}) for edge reconcile: {e:#}"))?;
            // Content half of the same placeholder reconcile. A placeholder root
            // is created with NO content, and its outbound projection writes that
            // "" over the parent's real SQL row — permanently, because no later
            // call ever gave the node its content. Completing it here is
            // clobber-free by construction (an empty node has nothing to lose)
            // and re-homes it off the tree root, where it was parked. The
            // re-home resolves its own parent through the SAME
            // placeholder-standing-up path, so a whole ancestor chain reached
            // bottom-up (the folder-companion vault shape) still lands homed
            // instead of stranding pages at the root — where write-back would
            // RELOCATE their org files out of their folders.
            let requested_is_empty = matches!(
                &content, holon_api::BlockContent::Text { raw } if raw.trim().is_empty()
            );
            if !requested_is_empty && current.content.trim().is_empty() {
                tracing::warn!(
                    id = %new_id,
                    "completing an empty placeholder root with this create's content"
                );
                backend
                    .complete_placeholder_content(new_id.id(), &content)
                    .await
                    .map_err(|e| anyhow!("complete_placeholder_content({new_id}): {e:#}"))?;
                let home = resolve_parent_or_placeholder(&backend, parent_id, new_id).await?;
                backend
                    .update_block_position(new_id.id(), home.as_str(), after_id.map(|a| a.id()))
                    .await
                    .map_err(|e| anyhow!("update_block_position({new_id}) placeholder: {e:#}"))?;
            }
            if !tags.is_empty() && current.tags != *tags {
                backend
                    .set_block_tags(new_id.id(), &tags.to_vec())
                    .await
                    .map_err(|e| anyhow!("set_block_tags({new_id}): {e:#}"))?;
            }
            // Skipped when empty to avoid clobbering deps set elsewhere with an
            // empty list; otherwise written only when the request differs.
            if !requires.is_empty() && current.requires.as_slice() != requires {
                backend
                    .set_block_requires(new_id.id(), requires)
                    .await
                    .map_err(|e| anyhow!("set_block_requires({new_id}): {e:#}"))?;
            }
            if !advice_suppressed.is_empty()
                && current.advice_suppressed.as_slice() != advice_suppressed
            {
                backend
                    .set_block_advice_suppressed(new_id.id(), advice_suppressed)
                    .await
                    .map_err(|e| anyhow!("set_block_advice_suppressed({new_id}): {e:#}"))?;
            }
            return Ok(true);
        }
        let resolved_parent = resolve_parent_or_placeholder(&backend, parent_id, new_id).await?;
        backend
            .create_block_with_properties(
                resolved_parent,
                content,
                Some(new_id.clone()),
                properties,
                tags,
                requires,
                advice_suppressed,
            )
            .await
            .map_err(|e| anyhow!("create_block({new_id}): {e:#}"))?;
        if let Some(after) = after_id {
            backend
                .update_block_position(new_id.id(), parent_id.as_str(), Some(after.id()))
                .await
                .map_err(|e| anyhow!("update_block_position({new_id}): {e:#}"))?;
        }
        Ok(true)
    }

    /// Authoritative block delete through `LoroBackend::delete_block`. Mirrors
    /// [`create_entity`](EntityCellRegistry::create_entity): the block leaves
    /// the Loro tree first and the outbound projector emits the SQL DELETE.
    /// Drivers: the org reconciler and `join_block`'s merged-away-block
    /// delete. SqlOnly mode returns `Ok(false)` so the caller falls back to
    /// the direct SQL delete path. Loro mode: checks tree membership first
    /// and returns `Ok(false)` for unseeded blocks (caller falls through to the
    /// direct SQL delete path — transitional; after sole-writer all blocks
    /// originate in Loro). `delete_block` is idempotent on the tree side, so
    /// the TOCTOU between the resolve_ check and the call is harmless.
    async fn delete_entity(&self, uri: &EntityUri) -> Result<bool> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(false),
        };
        let in_tree = backend.resolve_to_tree_id(uri.id()).await.is_some();
        if in_tree {
            backend
                .delete_block(uri.id())
                .await
                .map_err(|e| anyhow!("delete_block({uri}): {e:#}"))?;
            self.cache.evict_uri(uri);
        }
        Ok(in_tree)
    }
}

impl BlockCellRegistry {
    /// [`create_entity`](EntityCellRegistry::create_entity) for a whole chunk
    /// of creates, in ONE Loro commit — the cold-boot ingest's dominant cost.
    ///
    /// Per-block `create_entity` pays an existence probe
    /// (`resolve_to_tree_id`) that MISSES for every genuinely-new block and
    /// therefore walks all live nodes: O(nodes) per create, i.e. quadratic in
    /// one file's block count. Here the id cache is warmed ONCE per chunk, so
    /// the same existence question is answered from the cache and only the
    /// blocks that really do exist take the per-block reconcile path (which is
    /// `create_entity` verbatim — no second implementation of it).
    ///
    /// Returns one `persisted` flag per request, in request order, with the
    /// same meaning as `create_entity`: `false` = the caller owns the create
    /// (SqlOnly mode).
    pub async fn create_entities(&self, requests: &[BlockCreateRequest]) -> Result<Vec<bool>> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(vec![false; requests.len()]),
        };
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        // One tree walk for the whole chunk instead of one per block: after
        // this, a cache miss IS absence (the ingest is the sole writer while it
        // runs), so the batched arm below never re-walks.
        backend.warm_stable_id_cache().await;

        let mut out = vec![false; requests.len()];
        let mut fresh: Vec<(usize, NewBlockWithProperties)> = Vec::new();
        // Ids this chunk is about to create. A parent in here is NOT absent —
        // it is created earlier in this same batch (requests arrive in document
        // order, parents first) and the write loop resolves it from the cache
        // it populates as it goes. Standing up a placeholder root for one would
        // mint a SECOND node for the same stable id, which is how a batched
        // ingest lost blocks.
        let will_create: std::collections::HashSet<&str> =
            requests.iter().map(|r| r.id.id()).collect();
        for (idx, request) in requests.iter().enumerate() {
            if backend.peek_id_cache(request.id.id()).is_some() {
                // Already in the tree: the idempotent reconcile path (placeholder
                // completion, edge-field reconcile) is subtle and rare on a cold
                // boot — run the single-block seam unchanged.
                out[idx] = self
                    .create_entity(
                        &request.parent_id,
                        None,
                        &request.id,
                        request.content.clone(),
                        &request.properties,
                        &request.tags,
                        &request.requires,
                        &request.advice_suppressed,
                    )
                    .await?;
                continue;
            }
            let resolved_parent = if will_create.contains(request.parent_id.id()) {
                request.parent_id.clone()
            } else {
                resolve_parent_or_placeholder(&backend, &request.parent_id, &request.id).await?
            };
            fresh.push((
                idx,
                NewBlockWithProperties {
                    parent_id: resolved_parent,
                    id: request.id.clone(),
                    content: request.content.clone(),
                    properties: request.properties.clone(),
                    tags: request.tags.clone(),
                    requires: request.requires.clone(),
                    advice_suppressed: request.advice_suppressed.clone(),
                },
            ));
        }
        if !fresh.is_empty() {
            let payload: Vec<NewBlockWithProperties> =
                fresh.iter().map(|(_, r)| r.clone()).collect();
            let n = payload.len();
            backend
                .create_blocks_with_properties(payload)
                .await
                .map_err(|e| anyhow!("create_blocks_with_properties({n} block(s)): {e:#}"))?;
            for (idx, _) in fresh {
                out[idx] = true;
            }
        }
        Ok(out)
    }

    /// True when this registry is backed by a Loro doc (the outbound projector
    /// owns the SQL `block_raw` row). This is the concrete capability-detection
    /// boundary: the composition root feeds it to
    /// [`CapabilityProfile::detect`](holon_api::capability::CapabilityProfile::detect)
    /// to resolve the mechanism profile. The one place "Loro" is named in the
    /// order/consolidator axis — everything downstream branches on
    /// `Consolidator`. False in the direct-store mode.
    pub fn has_loro_backing(&self) -> bool {
        matches!(self.backing_source, BackingSource::Loro { .. })
    }
    /// Read a block's authoritative Loro fractional index — the value the
    /// outbound snapshot projection writes to SQL `sort_key`. Returns `None` in
    /// SqlOnly mode, where SQL itself owns `sort_key`. A read accessor for
    /// diagnostics / order-verification (e.g. comparing the live fi against the
    /// projected `block_raw.sort_key`); the projection itself writes every
    /// sibling's key each pass, so no separate writeback pass is needed.
    pub async fn live_sort_key(&self, id: &str) -> Result<Option<String>> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(None),
        };
        backend
            .block_sort_key(id)
            .await
            .map_err(|e| anyhow!("live_sort_key({id}): {e:#}"))
    }
    /// Children of `parent_id` in authoritative Loro tree order (full-URI
    /// form, e.g. `"block:foo"`). Returns `None` in SqlOnly mode, where the
    /// SQL cache is the order authority. Used by `BlockOrdering::children`
    /// so the org-scan place loop can observe blocks the instant they enter
    /// the Loro tree via `create_in_tree` — during the initial scan the
    /// outbound projector is not running yet, so the SQL cache is empty for
    /// freshly-created blocks and a cache read would spuriously time out.
    /// Whether `id` has a node in the authoritative Loro tree. `None` in
    /// SqlOnly mode (no separate tree to ask). `Some(false)` is the
    /// pre-Loro-vault upgrade signal consumed by `BlockOrdering::in_tree`.
    pub async fn live_in_tree(&self, id: &str) -> Result<Option<bool>> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(None),
        };
        Ok(Some(backend.resolve_to_tree_id(id).await.is_some()))
    }

    pub async fn live_children(&self, parent_id: &str) -> Result<Option<Vec<String>>> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => return Ok(None),
        };
        // Unseeded-vault guard (same family as the `create_entity`
        // after-anchor and `write_field` guards): a parent present in SQL but
        // absent from the Loro tree is a pre-Loro vault opened without a seed
        // pass. Loro has no opinion on that subtree's order, so answer `None`
        // and let the SQL cache own it — ALLOW(fallback): disclosed via warn;
        // erroring here aborted the whole OrgMode initial scan ("Cannot
        // resolve parent_id to TreeID") and the app never started on
        // upgraded vaults. Sentinel/no-parent parents read `tree.roots()`
        // and need no node, so they go straight through.
        // ALLOW(entity_uri_from_raw): parent_id &str backend API param (accepts both id
        // formats)
        let parent_uri = EntityUri::from_raw(parent_id);
        if !parent_uri.is_no_parent()
            && !parent_uri.is_sentinel()
            && backend.resolve_to_tree_id(parent_id).await.is_none()
        {
            tracing::warn!(
                parent_id,
                "live_children: parent has no Loro tree node (unseeded vault) — SQL cache owns \
                 this subtree's order"
            );
            return Ok(None);
        }
        let kids = backend
            .list_children(parent_id)
            .await
            .map_err(|e| anyhow!("live_children({parent_id}): {e:#}"))?;
        Ok(Some(kids))
    }
}

#[cfg(test)]
mod tests {
    use holon_core::cell::Cell;
    use holon_core::cell_registry::EntityCellRegistryExt;

    use super::*;

    fn make_loro_doc_with_block(block_id: &str) -> Arc<LoroDoc> {
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        meta.insert(STABLE_ID, block_id.to_string()).unwrap();
        meta.insert_container(CONTENT_RAW, LoroText::new()).unwrap();
        doc.commit();
        doc
    }

    #[test]
    fn loro_mode_resolves_content_cell() -> Result<()> {
        let doc = make_loro_doc_with_block("abc");
        let registry: Box<dyn EntityCellRegistry> = Box::new(BlockCellRegistry::with_loro_doc(doc));
        let uri = EntityUri::block("abc");
        let cell: Cell<String> = registry.as_ref().live_field::<String>(&uri, "content")?;
        assert_eq!(cell.current(), "");
        Ok(())
    }

    #[test]
    fn loro_mode_resolves_scalar_completed_cell() -> Result<()> {
        // Phase 2 (invariant 12): scalar block fields now resolve a cell in
        // Full mode. This inverts the old pin that asserted `completed` FAILED.
        let doc = make_loro_doc_with_block("abc");
        let registry: Box<dyn EntityCellRegistry> = Box::new(BlockCellRegistry::with_loro_doc(doc));
        let uri = EntityUri::block("abc");
        let cell: Cell<bool> = registry.as_ref().live_field::<bool>(&uri, "completed")?;
        assert!(!cell.current(), "absent property decodes to false");
        Ok(())
    }

    #[tokio::test]
    async fn write_field_completed_round_trips_through_cell() -> Result<()> {
        let doc = make_loro_doc_with_block("abc");
        let registry = BlockCellRegistry::with_loro_doc(doc);
        let uri = EntityUri::block("abc");
        let routed = registry
            .write_field(&uri, "completed", Value::Boolean(true))
            .await?;
        assert!(routed, "scalar write must route through the Loro cell");
        let cell: Cell<bool> =
            (&registry as &dyn EntityCellRegistry).live_field::<bool>(&uri, "completed")?;
        assert!(cell.current(), "the write is visible through the cell");
        Ok(())
    }

    #[test]
    fn loro_mode_unsupported_scalar_type_errs() {
        let doc = make_loro_doc_with_block("abc");
        let registry: Box<dyn EntityCellRegistry> = Box::new(BlockCellRegistry::with_loro_doc(doc));
        let uri = EntityUri::block("abc");
        let res = registry.as_ref().live_field::<f64>(&uri, "completed");
        let err = res
            .err()
            .expect("expected an error for an unsupported scalar type");
        let msg = format!("{:#}", err);
        assert!(
            msg.contains("no cell for the requested type"),
            "msg = {msg}"
        );
    }

    #[test]
    fn sql_only_mode_errs_loudly() {
        let registry: Box<dyn EntityCellRegistry> = Box::new(BlockCellRegistry::sql_only());
        let uri = EntityUri::block("abc");
        let res = registry.as_ref().live_field::<String>(&uri, "content");
        assert!(res.is_err());
    }

    /// Spec 0008 §2.2: wired SqlOnly mode presents the same scalar cell surface
    /// as Full (Loro) mode. The write callback emulates `set_field` → CDC by
    /// updating the entity cache, proving `live_field::<bool>` round-trips a
    /// write and observes it via the cell — no Loro doc involved.
    #[tokio::test]
    async fn sql_only_wired_scalar_round_trips_via_entity_cache() -> Result<()> {
        use holon_api::StorageEntity;
        use holon_api::block::Block;
        use holon_api::live_data::LiveData;

        let live: Arc<LiveData<Block>> = LiveData::new(
            Vec::new(),
            |row: &StorageEntity| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow!("row missing id"))
            },
            |row: &StorageEntity| Block::try_from(row.clone()),
        );

        let uri = EntityUri::block("abc");
        let key = uri.to_string();
        let block = Block::new_text(uri.clone(), EntityUri::block("root"), "hello");
        live.insert(key.clone(), Arc::new(block));

        // set_field write path: emulate the SQL write + CDC reflection by
        // updating the entity cache with the encoded property.
        let live_for_write = live.clone();
        let write: SqlScalarWriteFn =
            Arc::new(move |uri: EntityUri, field: String, value: Value| {
                let live = live_for_write.clone();
                Box::pin(async move {
                    let key = uri.to_string();
                    let mut b = live
                        .read()
                        .get(&key)
                        .map(|b| (**b).clone())
                        .ok_or_else(|| anyhow!("block {key} absent from entity cache"))?;
                    b.set_property(field, value);
                    live.insert(key, Arc::new(b));
                    Ok(())
                }) as BoxFuture<'static, Result<()>>
            });

        let registry = BlockCellRegistry::sql_only_wired(live.clone(), write);
        let cell: Cell<bool> =
            (&registry as &dyn EntityCellRegistry).live_field::<bool>(&uri, "completed")?;
        assert!(!cell.current(), "absent property decodes to false");
        cell.set(true).await?;
        assert!(
            cell.current(),
            "the write is visible through the cell via the entity cache"
        );
        Ok(())
    }

    /// Splitting a block that has no Loro tree node (unseeded vault, synthetic
    /// SQL-only row) must fall back to the SQL create path WITHOUT mutating
    /// the tree. The pre-guard regression: `create_entity` used to mint a
    /// placeholder parent + the new node first and only then fail on the
    /// unresolvable after-anchor — poisoning the tree with empty-text nodes
    /// whose "" content shadowed the real SQL content on later reads.
    #[test]
    fn create_entity_missing_after_anchor_falls_back_without_tree_mutation() -> Result<()> {
        let doc = make_loro_doc_with_block("parent");
        let registry = BlockCellRegistry::with_loro_doc(doc.clone());
        let rt = tokio::runtime::Runtime::new()?;
        let wrote = rt.block_on(registry.create_entity(
            &EntityUri::block("parent"),
            Some(&EntityUri::block("missing-after")),
            &EntityUri::block("new-block"),
            holon_api::BlockContent::text("x"),
            &std::collections::HashMap::new(),
            &Tags::default(),
            &[],
            &[],
        ))?;
        assert!(
            !wrote,
            "expected Ok(false) — the disclosed SQL route for an anchor outside Loro authority"
        );
        let tree = doc.get_tree(TREE_NAME);
        assert_eq!(
            tree.get_nodes(false).len(),
            1,
            "tree must be untouched — no placeholder root or new node minted"
        );
        Ok(())
    }

    #[test]
    fn loro_mode_block_not_in_tree_errs() {
        let doc = make_loro_doc_with_block("present");
        let registry: Box<dyn EntityCellRegistry> = Box::new(BlockCellRegistry::with_loro_doc(doc));
        let uri = EntityUri::block("missing");
        let res = registry.as_ref().live_field::<String>(&uri, "content");
        let err = res.err().expect("expected an error for missing block");
        let msg = format!("{:#}", err);
        assert!(msg.contains("not found in Loro tree"), "msg = {msg}");
    }

    /// Task #65 (boot re-seed churn): re-running `create_entity` for a block
    /// that already exists with byte-identical edge fields must emit NO
    /// Loro op. The existing-node reconcile guards each `set_block_*`
    /// behind a compare against the tree's current value; an unconditional
    /// re-assert would commit an op (`set_block_tags` always `meta.insert`
    /// + `doc.commit()`) that projects to a junction DELETE+INSERT — the
    /// gratuitous boot/org-rescan churn, and the delta that doubles a
    /// persisted matview tag on restart. Observed at the Loro
    /// authority (deterministic) via the oplog frontier watermark.
    #[tokio::test]
    async fn create_entity_is_idempotent_for_unchanged_edge_fields() -> Result<()> {
        use std::collections::HashMap;

        let doc = make_loro_doc_with_block("parent");
        let registry = BlockCellRegistry::with_loro_doc(doc.clone());
        let parent = EntityUri::block("parent");
        let child = EntityUri::block("journals");
        let page_tags = Tags::from(vec!["Page".to_string()]);

        // First create: mints the node and sets the `Page` tag.
        let wrote = registry
            .create_entity(
                &parent,
                None,
                &child,
                holon_api::BlockContent::text("Journals"),
                &HashMap::new(),
                &page_tags,
                &[],
                &[],
            )
            .await?;
        assert!(wrote, "loro-mode create must persist");

        // Re-seed with byte-identical edge fields → no new Loro op.
        let before = doc.oplog_frontiers();
        let wrote2 = registry
            .create_entity(
                &parent,
                None,
                &child,
                holon_api::BlockContent::text("Journals"),
                &HashMap::new(),
                &page_tags,
                &[],
                &[],
            )
            .await?;
        assert!(wrote2, "existing-node reconcile still returns true");
        assert_eq!(
            before,
            doc.oplog_frontiers(),
            "re-seeding an unchanged block must emit NO Loro op (boot re-seed churn)"
        );

        // Control: a genuinely different tag set must still be written.
        let changed_tags = Tags::from(vec!["Page".to_string(), "Task".to_string()]);
        let after_noop = doc.oplog_frontiers();
        let wrote3 = registry
            .create_entity(
                &parent,
                None,
                &child,
                holon_api::BlockContent::text("Journals"),
                &HashMap::new(),
                &changed_tags,
                &[],
                &[],
            )
            .await?;
        assert!(wrote3);
        assert_ne!(
            after_noop,
            doc.oplog_frontiers(),
            "a changed tag set must still be written through"
        );
        Ok(())
    }

    /// A batch whose blocks parent EACH OTHER (the org shape: a headline and
    /// its children ingested together) must land as ONE node per stable id,
    /// homed under the real parent.
    ///
    /// Resolving a parent that the same batch is about to create reports it
    /// absent — it does not exist yet — and the placeholder-root path then
    /// mints a SECOND node for that id. The keystone caught it as INGEST DATA
    /// LOSS (blocks parsed from disk missing from the projection), so the
    /// observable asserted here is the placeholder count and the child's home,
    /// not just "the call returned true".
    #[tokio::test]
    async fn create_entities_batch_homes_children_under_a_parent_from_the_same_batch() -> Result<()>
    {
        use std::collections::HashMap;

        let doc = make_loro_doc_with_block("root");
        let registry = BlockCellRegistry::with_loro_doc(doc.clone());
        let root = EntityUri::block("root");
        let parent = EntityUri::block("headline");
        let child = EntityUri::block("child");
        let request = |id: &EntityUri, parent_id: &EntityUri| BlockCreateRequest {
            parent_id: parent_id.clone(),
            id: id.clone(),
            content: holon_api::BlockContent::text(id.id()),
            properties: HashMap::new(),
            tags: Tags::default(),
            requires: Vec::new(),
            advice_suppressed: Vec::new(),
        };

        let flags = registry
            .create_entities(&[request(&parent, &root), request(&child, &parent)])
            .await?;
        assert_eq!(flags, vec![true, true], "both creates must persist");

        // ONE tree node per stable id. A placeholder stood up for `headline`
        // and the batch's own create of `headline` are two live nodes carrying
        // the same STABLE_ID: reads resolve to one of them and the blocks under
        // the other vanish from the doc walk — the data loss the keystone saw.
        use crate::loro_backend::LoroMapExt;
        let tree = doc.get_tree(TREE_NAME);
        let mut nodes_per_id: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for node in tree.get_nodes(false) {
            if matches!(
                node.parent,
                loro::TreeParentId::Deleted | loro::TreeParentId::Unexist
            ) {
                continue;
            }
            if let Ok(meta) = tree.get_meta(node.id)
                && let Some(sid) =
                    meta.get_typed(STABLE_ID, |v| v.as_string().map(|s| s.to_string()))
            {
                *nodes_per_id.entry(sid).or_default() += 1;
            }
        }
        assert_eq!(
            nodes_per_id.get(parent.id()).copied(),
            Some(1),
            "one node per stable id; got {nodes_per_id:?}"
        );

        let backend = match &registry.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly { .. } => unreachable!("built with_loro_doc"),
        };
        let stored_parent = backend.get_block(parent.id()).await?;
        assert_eq!(
            stored_parent.content, "headline",
            "the parent node must carry its own content — a placeholder stood up for it is empty"
        );
        Ok(())
    }

    /// Copy-on-write auto-update (F4 stale-seed remedy): `reseed_content`
    /// refreshes a virtual seed-layout block whose PERSISTED content drifted
    /// from the current bundled asset, is a genuine NO-OP (no Loro op) when the
    /// content already matches, and skips a block with no tree node. This is
    /// the engine behind `seed_default_layout`'s default-layout auto-update
    /// over a stale persisted Loro snapshot.
    #[tokio::test]
    async fn reseed_content_refreshes_drift_and_is_churn_free() -> Result<()> {
        use std::collections::HashMap;

        let doc = make_loro_doc_with_block("parent");
        let registry = BlockCellRegistry::with_loro_doc(doc.clone());
        let parent = EntityUri::block("parent");
        let child = EntityUri::block("root-layout");

        // First boot seeded the layout block with the THEN-current asset text.
        let wrote = registry
            .create_entity(
                &parent,
                None,
                &child,
                holon_api::BlockContent::text("OLD layout"),
                &HashMap::new(),
                &Tags::default(),
                &[],
                &[],
            )
            .await?;
        assert!(wrote, "loro-mode create must persist");
        let seeded = (&registry as &dyn EntityCellRegistry)
            .live_field::<String>(&child, "content")?
            .current();
        assert_eq!(seeded, "OLD layout", "create must write the seed content");

        // A NEWER shipped asset carries different content → reseed refreshes it.
        let refreshed = registry
            .reseed_content(&[(child.clone(), "NEW layout".to_string())])
            .await?;
        assert_eq!(
            refreshed, 1,
            "drifted seed block must refresh from the asset"
        );
        let now = (&registry as &dyn EntityCellRegistry)
            .live_field::<String>(&child, "content")?
            .current();
        assert_eq!(now, "NEW layout", "content updated to the current asset");

        // Idempotent: re-seeding the SAME content emits NO Loro op (boot churn).
        let before = doc.oplog_frontiers();
        let refreshed2 = registry
            .reseed_content(&[(child.clone(), "NEW layout".to_string())])
            .await?;
        assert_eq!(refreshed2, 0, "unchanged content must not be refreshed");
        assert_eq!(
            before,
            doc.oplog_frontiers(),
            "re-seeding unchanged content must emit NO Loro op (churn-free boot)"
        );

        // A block absent from the tree is skipped — the create pass owns it.
        let refreshed3 = registry
            .reseed_content(&[(EntityUri::block("not-seeded"), "x".to_string())])
            .await?;
        assert_eq!(refreshed3, 0, "untracked block must be skipped");
        Ok(())
    }

    #[test]
    fn on_entity_deleted_prunes_cache() -> Result<()> {
        let doc = make_loro_doc_with_block("zzz");
        let registry: Arc<dyn EntityCellRegistry> = Arc::new(BlockCellRegistry::with_loro_doc(doc));
        let uri = EntityUri::block("zzz");
        let _cell = registry.as_ref().live_field::<String>(&uri, "content")?;
        registry.on_entity_deleted(&uri);
        // The cache entry was pruned; a fresh lookup constructs again.
        // We can't directly observe construction without instrumentation,
        // but we can confirm the resolve still succeeds and returns a
        // working cell.
        let cell: Cell<String> = registry.as_ref().live_field::<String>(&uri, "content")?;
        assert_eq!(cell.current(), "");
        Ok(())
    }
}
