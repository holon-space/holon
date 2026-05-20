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

use std::any::{Any, TypeId};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use loro::{LoroDoc, LoroText};

use crate::api::repository::CoreOperations;
use holon_api::{EntityUri, Tags, Value};
use holon_core::cell::CellBacking;
use holon_core::cell_registry::{CellCache, EntityCellRegistry, EntityCellRegistryExt};

use crate::api::loro_backend::{CONTENT_RAW, LoroBackend, STABLE_ID, TREE_NAME};
use crate::sync::loro_document::LoroDocument;
use crate::sync::loro_text_cell_backing::LoroTextCellBacking;

/// Registry of [`Cell<T>`](holon_core::cell::Cell)s for `block` entity
/// fields.
///
/// Construction modes:
/// - `with_loro(doc)` — Full mode; `content` returns a Loro-backed cell.
/// - `sql_only()` — SqlOnly mode; ANY `live_field_any` call errors loudly
///   because Phase 1 has no editor in SqlOnly mode and synthetic test
///   stores bypass the registry entirely (they get `cells() == None`
///   from the `BlockOperations` default).
pub struct BlockCellRegistry {
    cache: CellCache,
    backing_source: BackingSource,
}

enum BackingSource {
    Loro {
        doc: Arc<LoroDoc>,
        backend: Arc<LoroBackend>,
    },
    SqlOnly,
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

    /// Construct a SqlOnly-mode registry. All `live_field_any` calls
    /// error with a clear message — there's no editor in SqlOnly mode.
    pub fn sql_only() -> Self {
        Self {
            cache: CellCache::new(),
            backing_source: BackingSource::SqlOnly,
        }
    }

    fn resolve_loro_text_container(&self, block_id: &str) -> Result<(Arc<LoroDoc>, LoroText)> {
        let doc = match &self.backing_source {
            BackingSource::Loro { doc, .. } => doc.clone(),
            BackingSource::SqlOnly => {
                return Err(anyhow!(
                    "BlockCellRegistry is in SqlOnly mode; no Loro backing is available \
                     for ({block_id}, \"content\"). Phase 1 doesn't expect editor cells \
                     in SqlOnly mode."
                ));
            }
        };
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
                // Source blocks keep their content in the SOURCE_CODE
                // container (`write_content_to_meta`), text blocks in
                // CONTENT_RAW. Binding CONTENT_RAW unconditionally silently
                // forked source-block content: `set_field("content")` (org
                // re-ingest of an index.org swap) wrote a container that
                // `read_content_from_meta` never reads — the update was lost
                // and Loro kept serving the stale source text.
                let content_key = match meta.get(crate::api::loro_backend::CONTENT_TYPE) {
                    Some(loro::ValueOrContainer::Value(v))
                        if v.as_string().is_some_and(|s| s.as_str() == "source") =>
                    {
                        crate::api::loro_backend::SOURCE_CODE
                    }
                    _ => CONTENT_RAW,
                };
                let text = meta
                    .get_or_create_container(content_key, LoroText::new())
                    .map_err(|e| {
                        anyhow!("get_or_create_container({content_key}) for {block_id}: {e:?}")
                    })?;
                return Ok((doc, text));
            }
        }
        Err(anyhow!(
            "Block {block_id} not found in Loro tree (inbound consumer hasn't applied the \
             create event yet, or the block was never imported)"
        ))
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
    pub async fn write_field(&self, uri: &EntityUri, field: &str, value: Value) -> Result<bool> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly => return Ok(false),
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
        //   `update_block_text` / chord-time content create paths, not by
        //   `set_field` callers
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
                        "write_field(content) for {uri}: no Loro tree node — \
                         writing through the SQL path (Loro authority missing or \
                         unseeded for this block)"
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
            "tags" => {
                let arr = match &value {
                    Value::Array(a) => a.clone(),
                    other => {
                        return Err(anyhow!("write_field(tags): expected Array, got {other:?}"));
                    }
                };
                let tags: Vec<String> = arr
                    .into_iter()
                    .map(|v| {
                        v.as_string()
                            .map(String::from)
                            .ok_or_else(|| anyhow!("write_field(tags): entry not a string"))
                    })
                    .collect::<Result<_>>()?;
                backend
                    .set_block_tags(&id, &tags)
                    .await
                    .map_err(|e| anyhow!("set_block_tags({id}): {e:#}"))?;
                Ok(true)
            }
            "sort_key" => {
                // Sibling order is owned by `place()`/`tree.mov_after` and
                // projected to SQL from the Loro fractional index — a block's
                // `sort_key` is never written through `set_field`. The org sync
                // path explicitly omits it (`build_block_params`), and
                // `project_sort_keys` writes the projected key straight to SQL
                // (bypassing this registry). A `set_field("sort_key")` reaching
                // here is a bug, not a positional intent — fail loud rather than
                // silently mis-route it to the meta `properties` map (which
                // `read_block_from_tree` ignores in favour of the fractional
                // index).
                Err(anyhow!(
                    "write_field(sort_key) is unsupported: order is owned by \
                     place()/mov_after and projected from the fractional index; \
                     a set_field(\"sort_key\") reached the cell registry for {id} — bug"
                ))
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
            // created_at, updated_at, …) round-trip through the meta
            // `properties` map: written by `update_block_fields`, read back
            // into `block.properties` HashMap by `read_block_from_tree`,
            // and projected to SQL columns by `block_to_params`.
            _ => {
                backend
                    .update_block_fields(&id, &[(field.to_string(), Value::Null, value)])
                    .await
                    .map_err(|e| anyhow!("update_block_fields({field}) for {id}: {e:#}"))?;
                Ok(true)
            }
        }
    }
}

#[async_trait::async_trait]
impl EntityCellRegistry for BlockCellRegistry {
    fn live_field_any(
        &self,
        uri: &EntityUri,
        field: &str,
        type_id: TypeId,
    ) -> Result<Arc<dyn Any + Send + Sync>> {
        if field != "content" {
            return Err(anyhow!(
                "BlockCellRegistry::live_field_any: field {field:?} not registered \
                 (Phase 1 only handles \"content\"); other fields land in Phase 2"
            ));
        }
        if type_id != TypeId::of::<String>() {
            return Err(anyhow!(
                "BlockCellRegistry::live_field_any: field \"content\" requires T=String \
                 (caller asked for a different type)"
            ));
        }
        let block_id = uri.id();
        let block_id_owned = block_id.to_string();
        self.cache.get_or_construct::<String, _>(uri, field, || {
            let (doc, text) = self.resolve_loro_text_container(&block_id_owned)?;
            let backing = LoroTextCellBacking::new(doc, text)?;
            Ok(Arc::new(backing) as Arc<dyn CellBacking<String>>)
        })
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
            BackingSource::SqlOnly => return Ok(false),
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
        requires: &[String],
    ) -> Result<bool> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly => return Ok(false),
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
        if let Some(after) = after_id {
            if backend.resolve_to_tree_id(after.id()).await.is_none() {
                tracing::warn!(
                    "create_entity({new_id}): after-block {after} has no Loro tree node — \
                     falling back to the SQL create path (Loro authority missing or \
                     unseeded for this block family)"
                );
                return Ok(false);
            }
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
            // The existing node may be a tagless placeholder root, auto-created
            // (below) when a child's `create_in_tree` reached this id before its
            // own create call. Reconcile the requested tags so a page document
            // keeps its `Page` marker in Loro — otherwise the outbound projector
            // diffs Loro(no tag) against SQL(Page) and wipes the SQL tag.
            if !tags.is_empty() {
                backend
                    .set_block_tags(new_id.id(), &tags.to_vec())
                    .await
                    .map_err(|e| anyhow!("set_block_tags({new_id}): {e:#}"))?;
            }
            // Reconcile the `requires` edge field too (mirrors tags): a re-scan
            // of an already-placed block must re-assert its org-edna deps so the
            // projection writes `block_requires`. Skipped when empty to avoid
            // clobbering deps set elsewhere with an empty list.
            if !requires.is_empty() {
                backend
                    .set_block_requires(new_id.id(), requires)
                    .await
                    .map_err(|e| anyhow!("set_block_requires({new_id}): {e:#}"))?;
            }
            return Ok(true);
        }
        // Resolve the parent in the tree; if it's a real block not yet
        // present (a child reached before its parent during the org scan),
        // stand up a placeholder root — mirrors `apply_create` so the create
        // is order-independent. The placeholder reconciles when the real
        // parent arrives.
        let resolved_parent = if parent_id.is_no_parent()
            || parent_id.is_sentinel()
            || backend.resolve_to_tree_id(parent_id.id()).await.is_some()
        {
            parent_id.clone()
        } else {
            let placeholder = backend
                .create_placeholder_root(parent_id.id())
                .await
                .map_err(|e| anyhow!("create_placeholder_root({parent_id}): {e:#}"))?;
            // ALLOW(entity_uri_from_raw): placeholder id String from backend.create_placeholder_root() (Loro adapter output)
            EntityUri::from_raw(&placeholder)
        };
        backend
            .create_block_with_properties(
                resolved_parent,
                content,
                Some(new_id.clone()),
                properties,
                tags,
                requires,
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
    /// and returns `Ok(false)` for unseeded blocks (caller falls through to
    /// SQL fallback — transitional; after sole-writer all blocks originate in
    /// Loro). `delete_block` is idempotent on the tree side, so the TOCTOU
    /// between the resolve_ check and the call is harmless.
    async fn delete_entity(&self, uri: &EntityUri) -> Result<bool> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly => return Ok(false),
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
    /// outbound projector writes to SQL `sort_key`. Returns `None` in SqlOnly
    /// mode, where SQL itself owns `sort_key`. Used by the org-scan order
    /// writeback (`BlockOrdering::project_sort_keys`) to project blocks that
    /// were created but never repositioned: those emit no Loro mov delta, so
    /// the projector never writes their fi and they would keep the default
    /// `"A0"` and mis-sort against moved siblings.
    pub async fn live_sort_key(&self, id: &str) -> Result<Option<String>> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly => return Ok(None),
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
            BackingSource::SqlOnly => return Ok(None),
        };
        Ok(Some(backend.resolve_to_tree_id(id).await.is_some()))
    }

    pub async fn live_children(&self, parent_id: &str) -> Result<Option<Vec<String>>> {
        let backend = match &self.backing_source {
            BackingSource::Loro { backend, .. } => backend.clone(),
            BackingSource::SqlOnly => return Ok(None),
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
        // ALLOW(entity_uri_from_raw): parent_id &str backend API param (accepts both id formats)
        let parent_uri = EntityUri::from_raw(parent_id);
        if !parent_uri.is_no_parent()
            && !parent_uri.is_sentinel()
            && backend.resolve_to_tree_id(parent_id).await.is_none()
        {
            tracing::warn!(
                parent_id,
                "live_children: parent has no Loro tree node (unseeded vault) — \
                 SQL cache owns this subtree's order"
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
    use super::*;
    use holon_core::cell::Cell;
    use holon_core::cell_registry::EntityCellRegistryExt;

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
    fn loro_mode_unknown_field_errs() {
        let doc = make_loro_doc_with_block("abc");
        let registry: Box<dyn EntityCellRegistry> = Box::new(BlockCellRegistry::with_loro_doc(doc));
        let uri = EntityUri::block("abc");
        let res = registry.as_ref().live_field::<String>(&uri, "completed");
        let err = res.err().expect("expected an error for unknown field");
        let msg = format!("{:#}", err);
        assert!(msg.contains("not registered"), "msg = {msg}");
    }

    #[test]
    fn sql_only_mode_errs_loudly() {
        let registry: Box<dyn EntityCellRegistry> = Box::new(BlockCellRegistry::sql_only());
        let uri = EntityUri::block("abc");
        let res = registry.as_ref().live_field::<String>(&uri, "content");
        assert!(res.is_err());
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
