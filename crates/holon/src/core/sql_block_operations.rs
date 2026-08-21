//! `SqlBlockOperations` — registers `BlockOperations` (`indent`, `outdent`,
//! `move_block`, `move_up`, `move_down`, `split_block`) as a separate
//! `OperationProvider` for the `"block"` entity.
//!
//! `SqlOperationProvider` advertises only the generic CRUD ops
//! (`set_field` / `create` / `update` / `delete` / `cycle_task_state`).
//! Without this provider, nothing in the dispatcher answers an `indent`
//! request — the keychord registered for Tab in
//! `holon-frontend/src/reactive.rs` cannot bind to any widget, and
//! `bubble_input` returns `false` ("Keychord did not match"). See the
//! comment on `BLOCK_TREE_KEYCHORD_OPS_ENABLED` in
//! `crates/holon-integration-tests/src/pbt/state_machine.rs` for the full
//! diagnosis of the production gap.
//!
//! This provider runs the trait default implementations from
//! `BlockOperations`, which decompose into a sequence of `set_field` calls.
//! In Loro mode block writes route through the `BlockCellRegistry` to Loro (the
//! authority) and the outbound projector emits the SQL row; in SqlOnly mode
//! they land in SQL directly via `SqlOperationProvider::execute_operation`.
//! There is no SQL→Loro reflection. Reads come from `QueryableCache<Block>` —
//! same backing store as the rest of the system.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OperationDescriptor;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::capability::Consolidator;
use holon_api::capability::SessionCapabilities;
use holon_core::BlockDataSourceHelpers;
use holon_core::BlockOperations;
use holon_core::BlockQueryHelpers;
use holon_core::CrudOperations;
use holon_core::DataSource;
use holon_core::EventOrigin;
use holon_core::OperationProvider;
use holon_core::OperationRegistry;
use holon_core::OperationResult;
use holon_core::OriginTaggedWrites;
use holon_core::Result;
use holon_core::SqlOnlyCellRegistry;
use holon_core::UnknownOperationError;
use holon_core::block_ordering::BlockOrdering;
use holon_core::block_ordering::MintedPosition;
use holon_core::block_ordering::OrderKeyMinting;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::fractional_index::default_sort_key;
use holon_core::fractional_index::gen_key_between;
use holon_core::fractional_index::gen_n_keys;
use holon_core::fractional_index::is_minted_key;
use holon_core::storage::types::StorageEntity;

use crate::core::queryable_cache::HasCache;
use crate::core::queryable_cache::QueryableCache;
use crate::core::sql_operation_provider::SqlOperationProvider;

/// The decision an order re-key makes, before anything is written.
struct RekeyPlan {
    /// The `siblings.len() + 1` target keys, in sibling order with the new
    /// block's slot spliced in.
    keys: Vec<String>,
    /// `(block id, new sort_key)` for the siblings that actually move.
    rekeys: Vec<(String, String)>,
}

pub struct SqlBlockOperations {
    sql_ops: Arc<SqlOperationProvider>,
    cache: Arc<QueryableCache<Block>>,
    /// Cell registry for block fields. Populated from DI at construction
    /// (`event_infra_module.rs`); chord-time ops route content reads/
    /// writes through this so the live Loro view is consulted before the
    /// lagging SQL `block.content` projection. `None`-equivalent
    /// behaviour for synthetic `MemStore` test impls is achieved by them
    /// inheriting the `BlockOperations::cells()` default of `None`
    /// rather than calling into this struct.
    cell_registry: Arc<dyn EntityCellRegistry>,
    /// The session's order/merge role, injected by the DI composition root —
    /// the SQL component is *told* whether it owns order, it does not probe a
    /// Loro-aware component to find out. This is the dependency inversion that
    /// keeps `SqlBlockOperations` ignorant of Loro: it acts on its capability
    /// (`Consolidator::Store` ⇒ mint keys / write directly; otherwise an
    /// upstream consolidator owns order and this component defers). Defaults to
    /// the direct-store profile so non-DI/test construction degrades safely.
    caps: SessionCapabilities,
}

impl SqlBlockOperations {
    /// Construct without a cell registry. Defaults to a `sql_only()`
    /// registry so chord ops degrade to direct SQL writes when no
    /// LoroModule is loaded. Production callers pair this with
    /// `with_cell_registry` in their DI factory closure.
    pub fn new(sql_ops: Arc<SqlOperationProvider>, cache: Arc<QueryableCache<Block>>) -> Self {
        Self {
            sql_ops,
            cache,
            cell_registry: Arc::new(SqlOnlyCellRegistry::new()),
            caps: SessionCapabilities::detect_and_pin(false),
        }
    }

    /// Attach a cell registry resolved from DI. Used by the
    /// `event_infra_module` factory so chord-time ops route content
    /// reads/writes through `BlockCellRegistry::live_field<String>` (and
    /// hence the live Loro `LoroText` view) instead of the SQL cache.
    pub fn with_cell_registry(mut self, registry: Arc<dyn EntityCellRegistry>) -> Self {
        self.cell_registry = registry;
        self
    }

    /// Pin the session's capability role (who owns order/merge). The DI
    /// composition root resolves this once from what is actually present and
    /// hands it in; this component never asks "is Loro present" itself.
    pub fn with_capabilities(mut self, caps: SessionCapabilities) -> Self {
        self.caps = caps;
        self
    }

    /// Ordered `(id, sort_key)` pairs for a parent, read straight from the
    /// internal `sort_key` column via the cache wrapper and sorted on it
    /// (`(sort_key, id)` lexicographic — Turso IVM matviews can't `ORDER BY`,
    /// so the wrapper sorts). Used by the SqlOnly key-minting writer; the
    /// `sort_key` encoding stays internal and never surfaces on the domain
    /// `Block` (ADR 0005).
    async fn sibling_keys(&self, parent_id: &str) -> Result<Vec<(String, String)>> {
        let rows = self
            .cache
            .query_raw("parent_id = ?", vec![Value::String(parent_id.to_string())])
            .await?;
        let mut pairs: Vec<(String, String)> = rows
            .into_iter()
            .filter_map(|r| {
                let id = r
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)?;
                let sk = r
                    .get("sort_key")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)?;
                Some((id, sk))
            })
            .collect();
        pairs.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        Ok(pairs)
    }

    /// True when `siblings` do not describe an insertable sequence: two share a
    /// key (no room between them), or one is not minted generator output and
    /// therefore is not a position at all. Either way the set must be re-keyed
    /// before anything can be placed relative to it.
    fn needs_rekey(siblings: &[(String, String)]) -> bool {
        siblings.windows(2).any(|w| w[0].1 == w[1].1)
            || siblings.iter().any(|(_, sk)| !is_minted_key(sk))
    }

    /// Order-preserving re-key of `siblings` into distinct minted keys, with
    /// one extra slot at `insert_idx`. Pure: it decides, it does not write.
    ///
    /// `keys` holds all `siblings.len() + 1` target keys so the caller can pick
    /// the slot it asked for; `rekeys` holds only the `(id, key)` pairs that
    /// actually move. Values are computed in one pass from one sibling read, so
    /// the chord-op projection race documented in MEMORY can't bite.
    ///
    /// This is the ONLY way an unkeyed row becomes a position: it is re-minted
    /// in the place it is already observed in, never skipped. Skipping one
    /// would silently place the new block on the wrong side of it.
    fn plan_rekey_with_slot(siblings: &[(String, String)], insert_idx: usize) -> Result<RekeyPlan> {
        let keys = gen_n_keys(siblings.len() + 1)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?;
        let rekeys = siblings
            .iter()
            .enumerate()
            .filter_map(|(i, (sib_id, sib_key))| {
                let target = if i < insert_idx {
                    &keys[i]
                } else {
                    &keys[i + 1]
                };
                (sib_key != target).then(|| (sib_id.clone(), target.clone()))
            })
            .collect();
        Ok(RekeyPlan { keys, rekeys })
    }

    /// The SqlOnly order owner's single mint path, over bare id strings.
    /// [`BlockOrdering::new_child_anchor`] and the `create` /
    /// `apply_ingest_batch` append sites all route through here so they agree
    /// on what a position is.
    ///
    /// Pure — it reads the sibling set and decides. Whatever re-keys the
    /// decision needs come back in the [`MintedPosition`] for the caller's
    /// firing transaction to write; minting them here would make a refused
    /// create leave a rewritten keyspace behind (ADR 0030 D1).
    async fn mint_child_key(
        &self,
        parent_id: &str,
        after_id: Option<&str>,
    ) -> Result<MintedPosition> {
        // P1 isolation: only the SqlOnly (no-Loro) order owner mints keys here.
        // In Loro mode the fractional index is authoritative — `apply_create`
        // sets it from `position_after_block_id` and this return value is
        // discarded — so short-circuit BEFORE the generator and its rebalance,
        // which would otherwise emit spurious sibling `set_field("sort_key")`
        // writes against the Loro-projected SQL view. The placeholder routes
        // through `default_sort_key()` (the single default owner), never a
        // stray literal.
        if matches!(self.consolidator(), Consolidator::Upstream) {
            return Ok(MintedPosition::alone(default_sort_key()));
        }
        // `(id, sort_key)` pairs already in `(sort_key, id)` order — matches
        // `OrgRenderer::render_entity_tree` so the new block's "after" slot is
        // interpreted in the same order the on-disk render uses.
        let siblings = self.sibling_keys(parent_id).await?;

        // Insertion index: where the new block lands. With no `after_id` it is
        // slot 0 (first child).
        let insert_idx = match after_id {
            None => 0usize,
            Some(after) => {
                siblings.iter().position(|(id, _)| id == after).ok_or_else(
                    || -> Box<dyn std::error::Error + Send + Sync> {
                        format!("mint_child_key: after block {after} missing").into()
                    },
                )? + 1
            }
        };

        if Self::needs_rekey(&siblings) {
            let plan = Self::plan_rekey_with_slot(&siblings, insert_idx)?;
            return Ok(MintedPosition::new(
                plan.keys[insert_idx].clone(),
                plan.rekeys,
            ));
        }

        let prev_key = insert_idx.checked_sub(1).map(|i| siblings[i].1.clone());
        let next_key = siblings.get(insert_idx).map(|(_, sk)| sk.clone());
        let sort_key = gen_key_between(prev_key.as_deref(), next_key.as_deref())
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?;
        Ok(MintedPosition::alone(sort_key))
    }
}

#[async_trait]
impl DataSource<Block> for SqlBlockOperations {
    async fn get_all(&self) -> Result<Vec<Block>> {
        self.cache.get_all().await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Block>> {
        self.cache.get_by_id(id).await
    }

    /// The override the default invites: one recursive-CTE read resolves the
    /// whole subtree, then each row is hydrated by id. The default walked
    /// level by level over `get_children`, whose own default is a full-table
    /// `get_all()` + in-memory filter — one unbounded read per tree level,
    /// every one of them the SAME binding.
    async fn get_descendants(&self, parent_id: &EntityUri) -> Result<Vec<Block>> {
        let ids = self.sql_ops.descendant_ids(parent_id.as_str()).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(block) = self.cache.get_by_id(&id).await? {
                out.push(block);
            }
        }
        Ok(out)
    }
}

impl HasCache<Block> for SqlBlockOperations {
    fn get_cache(&self) -> &QueryableCache<Block> {
        &self.cache
    }
}

#[async_trait]
impl BlockQueryHelpers<Block> for SqlBlockOperations {
    // Route the sibling/child lookups through `BlockOrdering` instead of
    // filtering blocks by `Block::sort_key()` in-memory. Same source
    // (the SQL cache) — the indirection just keeps `sort_key` reads
    // confined to the ordering adapter so a future backing without a
    // fractional-index string only has to change `BlockOrdering`.
    async fn get_prev_sibling(&self, block_id: &EntityUri) -> Result<Option<Block>> {
        match <Self as BlockOrdering>::prev_sibling(self, block_id).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn get_next_sibling(&self, block_id: &EntityUri) -> Result<Option<Block>> {
        match <Self as BlockOrdering>::next_sibling(self, block_id).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn get_first_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<Block>> {
        let Some(pid) = parent_id else {
            return Ok(None);
        };
        match <Self as BlockOrdering>::first_child(self, pid).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn get_last_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<Block>> {
        let Some(pid) = parent_id else {
            return Ok(None);
        };
        match <Self as BlockOrdering>::last_child(self, pid).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn children_ordered(&self, parent_id: &EntityUri) -> Result<Vec<Block>> {
        // The ordering authority is `BlockOrdering::children` (Loro live tree
        // in Loro mode, else the SQL cache ordered by the internal sort_key
        // column). Resolve ids there, then hydrate the domain blocks.
        let ids = <Self as BlockOrdering>::children(self, parent_id).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(b) = self.get_by_id(id.as_str()).await? {
                out.push(b);
            }
        }
        Ok(out)
    }
}
#[async_trait]
impl BlockDataSourceHelpers<Block> for SqlBlockOperations {
    /// Read the `Page` tag from the write authority (`block_tags`) instead of
    /// the `block`-matview-projected `Block::tags`, which trails the edge write
    /// via CDC. Closes the read-snapshot window that let a day-page's child
    /// escape into `journals` during tag-propagation lag (journals-phantom).
    async fn is_page_authoritative(&self, id: &holon_api::EntityUri) -> Result<bool> {
        self.sql_ops.block_is_page(id.as_str()).await
    }

    /// The SQL order owner CAN displace siblings, so it overrides the default
    /// `create_at` to apply the position's re-keys atomically with the row via
    /// `create_row` — the typed create seam (no `_order_rekeys` params key).
    async fn create_at(
        &self,
        fields: holon_api::StorageEntity,
        position: MintedPosition,
    ) -> Result<(String, OperationResult)> {
        let id = fields
            .get("id")
            .and_then(|v| v.as_string())
            .map(String::from)
            .ok_or_else(|| "SqlBlockOperations::create_at: missing 'id'".to_string())?;
        let result = self.sql_ops.create_row(fields, Some(position)).await?;
        Ok((id, result))
    }
}
impl BlockOperations<Block> for SqlBlockOperations {
    fn cells(&self) -> Option<&dyn EntityCellRegistry> {
        Some(&*self.cell_registry)
    }

    fn ordering(&self) -> Option<&dyn BlockOrdering> {
        Some(self as &dyn BlockOrdering)
    }

    fn order_key_minter(&self) -> Option<&dyn OrderKeyMinting> {
        Some(self as &dyn OrderKeyMinting)
    }
}

/// Pure monotonic relabel: given `ordered_ids` (the intended order) and the key
/// each currently carries (`cur_keys`, aligned by index; the default sentinel
/// `default_sort_key()` means "unkeyed"), return the key each id should
/// have so that lexical `sort_key` order reproduces `ordered_ids`. Keys already
/// strictly above their predecessor's final key are kept verbatim; only
/// violators (out-of-place, or carrying the default sentinel) are re-minted
/// strictly between their predecessor's final key and the next keepable anchor.
///
/// A key that is not minted output (`is_minted_key`) is never "keepable": it
/// lands arbitrarily in the lexical keyspace (the default `"A0"` sorts *above*
/// real indices like `"80"`) and cannot be anchored on, so such a block must
/// always receive a real minted key (projection totality). Idempotent: a
/// fully-keyed, already-ordered input returns its input unchanged.
fn relabel_order(ordered_ids: &[&str], cur_keys: &[String]) -> Result<Vec<String>> {
    let keepable =
        |k: &str, prev: Option<&str>| -> bool { is_minted_key(k) && prev.is_none_or(|p| k > p) };
    let mut out: Vec<String> = Vec::with_capacity(ordered_ids.len());
    let mut prev_final: Option<String> = None;
    for i in 0..ordered_ids.len() {
        if keepable(&cur_keys[i], prev_final.as_deref()) {
            prev_final = Some(cur_keys[i].clone());
            out.push(cur_keys[i].clone());
            continue;
        }
        let upper: Option<&str> = ((i + 1)..ordered_ids.len())
            .map(|j| cur_keys[j].as_str())
            .find(|kj| keepable(kj, prev_final.as_deref()));
        let new_key = gen_key_between(prev_final.as_deref(), upper)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?;
        prev_final = Some(new_key.clone());
        out.push(new_key);
    }
    Ok(out)
}

#[async_trait]
impl OrderKeyMinting for SqlBlockOperations {
    /// Returns the position to persist for a new block placed under
    /// `parent_id` after `after_id`. Real minting happens only
    /// when this store is the `Store` consolidator (SqlOnly); in Upstream
    /// (Loro) mode the body short-circuits to `default_sort_key()` because the
    /// tree owns the fractional index — `apply_create` overwrites the value
    /// after `Event::position_after_block_id` drives `tree.mov_after`. That
    /// residual Upstream reachability exists because this hybrid store's
    /// `place()` falls through to minting for blocks not yet in the Loro tree
    /// (the disclosed SQL-path carve-out for blocks absent from the tree); the
    /// pure Loro ordering seam has no minting method at all (`OrderKeyMinting`,
    /// Replication.md §5).
    ///
    /// Sibling sets that do not describe an insertable sequence — ties, or a
    /// row carrying the SQL default `"A0"` or a legacy value — are re-keyed
    /// first; see `mint_child_key`.
    async fn new_child_anchor(
        &self,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<MintedPosition> {
        self.mint_child_key(parent_id.as_str(), after_id.map(|u| u.as_str()))
            .await
    }
}

#[async_trait]
impl BlockOrdering for SqlBlockOperations {
    /// Loro mode → `write_position` (tree.mov_after). SqlOnly mode →
    /// `new_child_anchor` + paired `set_field("parent_id") +
    /// set_field("sort_key")` via the underlying `OperationProvider`.
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<()> {
        if self
            .cell_registry
            .write_position(uri, parent_id.as_str(), after_id.map(|u| u.as_str()))
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?
        {
            return Ok(());
        }
        // SqlOnly idempotency guard: skip if current parent_id and predecessor
        // already match the requested placement. Without this, new_child_anchor's
        // neighbor scan includes the target itself as the next sibling, so
        // gen_key_between mints a fresh sort_key on every call — causing a loop.
        // Use uri.as_str() (full URI like "block:abc") for cache lookups — the
        // DB stores full URIs, not bare IDs.
        {
            let current_block_opt = self.cache.get_by_id(uri.as_str()).await.map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("place: cache get_by_id {}: {e:#}", uri.as_str()).into()
                },
            )?;
            if let Some(current_block) = current_block_opt {
                let current_prev = self.prev_sibling(uri).await.map_err(
                    |e| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("place: prev_sibling {}: {e:#}", uri.as_str()).into()
                    },
                )?;
                if &current_block.parent_id == parent_id && current_prev.as_ref() == after_id {
                    return Ok(());
                }
            }
        }
        let position = self.new_child_anchor(parent_id, after_id).await?;
        // Parent, key and the sibling re-keys the key is expressed against are
        // ONE placement, so they go in ONE transaction (ADR 0030 D1). SQL
        // stores the full URI form ("block:..."); the UPDATE's `WHERE id = ?`
        // does a literal string match, so passing a bare id would silently
        // match zero rows.
        self.sql_ops
            .place_row(uri.as_str(), parent_id.as_str(), position)
            .await?;
        Ok(())
    }

    /// Total, minimal-diff re-key for the SQL (no-Loro) order owner.
    ///
    /// The org re-ingest hands us the file's complete line order for a parent;
    /// SQL is the sole order owner in this configuration, so we must make
    /// `ORDER BY sort_key` reproduce `ordered_ids` exactly. The incremental
    /// `place` loop can't converge a full reorder, but a naïve full rewrite
    /// (`gen_n_keys` for every child on every change) is O(N) key churn — the
    /// thing `Replication.md` §5 warns against — and floods CDC.
    ///
    /// So we do a **monotonic relabel**: walk `ordered_ids` left→right keeping
    /// the running maximum assigned key; a block whose current key is already
    /// strictly greater than its predecessor's final key is left untouched, and
    /// only an out-of-place block is re-minted with `gen_key_between` strictly
    /// between its predecessor's final key and the next key we will keep.
    /// Result is a strictly-increasing total order (correct) that touches
    /// only the blocks that actually moved (low churn). Idempotent: an
    /// already-ordered parent makes zero writes.
    ///
    /// Only under `Consolidator::Store`. When an upstream consolidator owns
    /// order, the tree holds the fractional index and the outbound projector
    /// writes `sort_key` FROM it, so a relabel here would be overwritten
    /// unread — the whole call would silently do nothing. There we restate the
    /// order through [`place`](Self::place), which routes to the tree.
    async fn place_all(&self, parent_id: &EntityUri, ordered_ids: &[EntityUri]) -> Result<()> {
        if ordered_ids.is_empty() {
            return Ok(());
        }
        if matches!(self.consolidator(), Consolidator::Upstream) {
            let mut prev: Option<&EntityUri> = None;
            for id in ordered_ids {
                self.place(id, parent_id, prev).await?;
                prev = Some(id);
            }
            return Ok(());
        }
        let parent_str = parent_id.as_str();
        // (parent_id, sort_key) per block, read from the raw rows — the
        // internal `sort_key` column is no longer on the domain `Block`.
        let rows = self.cache.query_raw("", vec![]).await?;
        let by_id: HashMap<String, (String, String)> = rows
            .into_iter()
            .filter_map(|r| {
                let id = r
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)?;
                let parent = r
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
                    .unwrap_or_default();
                let sk = r
                    .get("sort_key")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
                    .unwrap_or_else(default_sort_key);
                Some((id, (parent, sk)))
            })
            .collect();
        let entity = EntityName::new(Block::entity_name());
        let default_key = default_sort_key();

        // Resolve current keys in target order, fixing parent_id where it
        // diverges (rare: a re-parent into this parent). A missing block is
        // treated as unkeyed (default sentinel) so a not-yet-projected row
        // still gets a real key rather than panicking the scan.
        let mut cur_keys: Vec<String> = Vec::with_capacity(ordered_ids.len());
        for id in ordered_ids {
            match by_id.get(id.as_str()) {
                Some((parent, sk)) => {
                    if parent != parent_str {
                        let mut parent_params: StorageEntity = HashMap::new();
                        parent_params.insert("id".into(), Value::String(id.as_str().to_string()));
                        parent_params
                            .insert("field".into(), Value::String("parent_id".to_string()));
                        parent_params.insert("value".into(), Value::String(parent_str.to_string()));
                        self.sql_ops
                            .execute_operation(&entity, "set_field", parent_params)
                            .await?;
                    }
                    cur_keys.push(sk.clone());
                }
                None => cur_keys.push(default_key.clone()),
            }
        }

        // Compute the total order key for each child (keep already-ordered keys,
        // re-mint only violators) and write only the ones that changed.
        // Idempotent: a parent whose children carry born-correct keys makes
        // zero writes — so the common org-ingest path (blocks created already
        // carrying their order key, see `order_keys`) leaves no half-applied
        // window for a concurrent reader.
        let ordered_strs: Vec<&str> = ordered_ids.iter().map(|u| u.as_str()).collect();
        let target = relabel_order(&ordered_strs, &cur_keys)?;
        for (i, key) in target.iter().enumerate() {
            if *key != cur_keys[i] {
                let mut sort_params: StorageEntity = HashMap::new();
                sort_params.insert(
                    "id".into(),
                    Value::String(ordered_ids[i].as_str().to_string()),
                );
                sort_params.insert("field".into(), Value::String("sort_key".to_string()));
                sort_params.insert("value".into(), Value::String(key.clone()));
                self.sql_ops
                    .execute_operation(&entity, "set_field", sort_params)
                    .await?;
            }
        }
        Ok(())
    }

    /// Loro mode → `create_entity` (LoroBackend::create_block, synchronous).
    /// SqlOnly mode → `false`, so the caller keeps its SQL create path.
    async fn create_in_tree(
        &self,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
        new_id: &EntityUri,
        content: holon_api::BlockContent,
        properties: &HashMap<String, Value>,
        edges: &holon_api::BlockEdges,
    ) -> Result<bool> {
        self.cell_registry
            .create_entity(parent_id, after_id, new_id, content, properties, edges)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    /// One warm + one commit for the whole chunk (see
    /// `BlockCellRegistry::create_entities`) instead of the default's
    /// per-block create — the cold-boot ingest's dominant term.
    async fn create_in_tree_batch(
        &self,
        requests: &[holon_core::block_ordering::BlockCreateRequest],
    ) -> Result<Vec<bool>> {
        self.cell_registry
            .create_entities(requests)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    async fn in_tree(&self, id: &EntityUri) -> Result<Option<bool>> {
        self.cell_registry
            .live_in_tree(id.as_str())
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    /// Apply a block update intent — the single org→block mutation seam (no
    /// command bus behind it).
    ///
    /// Loro mode: route each changed field through
    /// [`set_field`](CrudOperations::set_field) (→ Loro via the cell
    /// registry; the outbound projector writes the SQL row) and the
    /// position through [`place`](Self::place). SqlOnly mode: write the SQL
    /// row directly, picking `create`/`update` by the row's prior
    /// presence so the emitted CDC event kind matches (the cache subscriber
    /// distinguishes `Created`/`Updated`). A block is either a create or an
    /// update within a single org scan, never both, so the cache read is a
    /// reliable presence test.
    async fn update_in_tree(&self, mut params: holon_api::StorageEntity) -> Result<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                "update_in_tree: missing 'id' param".into()
            })?;
        let after = params
            .remove(holon_api::POSITION_AFTER_BLOCK_ID_PARAM)
            .and_then(|v| v.as_string().map(str::to_string));

        if matches!(self.consolidator(), Consolidator::Upstream) {
            let parent_id = params
                .get("parent_id")
                .and_then(|v| v.as_string())
                .map(str::to_string);
            // Content / edge / scalar fields → Loro via set_field. Skip the
            // primary key, the routing hint (not a field), and parent_id +
            // position which `place` owns.
            for (field, value) in params.into_iter() {
                if &*field == "id"
                    || &*field == "parent_id"
                    || &*field == holon_api::ROUTING_DOC_URI_KEY
                {
                    continue;
                }
                self.set_field(&id, &field, value).await?;
            }
            // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
            let block_uri = EntityUri::from_raw(&id);
            match (parent_id, after) {
                (Some(parent), Some(after_id)) => {
                    // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
                    let parent_uri = EntityUri::from_raw(&parent);
                    // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
                    let after_uri = EntityUri::from_raw(&after_id);
                    self.place(&block_uri, &parent_uri, Some(&after_uri))
                        .await?;
                }
                // No recorded predecessor: set the parent; the org reconciler's
                // disk-order replay loop finalises first-child ordering.
                (Some(parent), None) => {
                    self.set_field(&id, "parent_id", Value::String(parent))
                        .await?;
                }
                (None, _) => {}
            }
        } else {
            let op = if self.cache.get_by_id(&id).await?.is_some() {
                "update"
            } else {
                "create"
            };
            // SQL is the order owner: a freshly created block is born carrying
            // its real order key — minted between its file predecessor and the
            // next sibling — so the create and its keying are a single write and
            // the row is never observable at the default `"A0"`. This closes the
            // create-default-then-rekey window that let a concurrent reader (or
            // the PBT invariant) see a half-applied sibling order. Updates keep
            // their key; position changes route through `place`.
            let position = if op == "create"
                && let Some(parent) = params
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            {
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
                let parent_uri = EntityUri::from_raw(&parent);
                let after_uri = after.as_deref().map(EntityUri::from_raw);
                // The minted position (key + sibling re-keys) travels TYPED into
                // create_row's transaction; never a `_order_rekeys` params key.
                Some(
                    self.new_child_anchor(&parent_uri, after_uri.as_ref())
                        .await?,
                )
            } else {
                None
            };
            if let Some(after_id) = after {
                params.insert(
                    holon_api::POSITION_AFTER_BLOCK_ID_PARAM.into(),
                    Value::String(after_id),
                );
            }
            let entity = EntityName::new(Block::entity_name());
            if op == "create" {
                self.sql_ops.create_row(params, position).await?;
            } else {
                self.sql_ops
                    .execute_operation_with_origin(&entity, op, params, EventOrigin::Org)
                    .await?;
            }
        }
        Ok(())
    }

    /// Apply a block delete intent.
    ///
    /// Loro mode: delete from the Loro tree (the authority) via the cell
    /// registry; the outbound projector emits the SQL DELETE. This mirrors
    /// `create_in_tree` (creates go to Loro, the projector writes SQL).
    /// Deleting only from SQL would race the armed projection, which
    /// re-creates the still-present Loro node back into SQL — the block
    /// resurrects (observed as `inv-backend-blocks-match-ref` spurious
    /// `bulk-*` rows).
    ///
    /// SqlOnly mode: the registry returns `false`; delete straight from SQL via
    /// the operation provider, preserving the `ROUTING_DOC_URI_KEY` hint so
    /// `prepare_delete` skips the recursive document walk.
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> Result<()> {
        let id = params.get("id").and_then(|v| v.as_string()).ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                "delete_in_tree: missing 'id' param".into()
            },
        )?;
        // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
        let uri = EntityUri::from_raw(id);
        if self
            .cell_registry
            .delete_entity(&uri)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?
        {
            // Loro-backed: the outbound projector writes the SQL DELETE.
            return Ok(());
        }
        // ALLOW(fallback): authority-first delete guard (Task 3) — the SQL delete
        // path here is only reachable for unseeded blocks and SqlOnly mode. In Loro
        // mode, log a warning so we observe how often the transitional path fires.
        if self.cell_registry.has_loro_backing() {
            tracing::warn!(
                block_id = %id, // ALLOW(fallback): disclosed degraded-mode warning, transitional path
                "SQL delete on the degraded path in Loro mode — block was unseeded. \
                 Transitional; re-seed adoption eliminates unseeded blocks."
            );
        }
        // ALLOW(sole_block_writer) ALLOW(fallback): SQL delete fallback for unseeded
        // blocks. Transitional — after sole-writer, all blocks originate in
        // Loro, so this fires only for pre-existing unseeded vaults the re-seed
        // adoption pass eliminates. NOT dead code: re-seed can fail mid-adoption.
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation_with_origin(&entity, "delete", params, EventOrigin::Org)
            .await?;
        Ok(())
    }

    async fn reseed_content(&self, blocks: &[(EntityUri, String)]) -> Result<usize> {
        self.cell_registry
            .reseed_content(blocks)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    /// Apply a whole file's ingest ops in ONE transaction (BugFunnel row 32).
    ///
    /// The historic boot loop applied each block through
    /// [`update_in_tree`](Self::update_in_tree) / `delete_in_tree`, so every
    /// single-row write drove a matview IVM maintenance pass over the live
    /// watch-views — cost scaling with the accumulated block table (O(N²) cold
    /// boot). SqlOnly is the SQL store's own keyspace, so the whole file's ops
    /// route through `SqlOperationProvider::execute_batch_with_origin` — ONE
    /// `db_handle.transaction()`, hence ONE maintenance pass per file.
    ///
    /// Creates are born carrying a strictly-increasing per-parent
    /// document-order `sort_key`, minted in-memory (seeded once per parent from
    /// the current sibling set — empty on cold boot). That matches what the
    /// downstream SqlOnly `place_all` totalizer expects, so it finds every new
    /// child already ordered and issues ZERO single-row `set_field("sort_key")`
    /// rewrites — otherwise the per-block cost merely migrates into
    /// `place_all`.
    ///
    /// In Upstream (Loro) mode field writes route through the cell registry
    /// (Loro owns order), NOT this SQL batch sink, so the per-op default is
    /// kept verbatim — the O(N²) this fixes was measured SqlOnly only.
    async fn apply_ingest_batch(&self, ops: Vec<(String, StorageEntity)>) -> Result<()> {
        if matches!(self.consolidator(), Consolidator::Upstream) {
            for (op, params) in ops {
                match op.as_str() {
                    "create" | "update" => self.update_in_tree(params).await?,
                    "delete" => self.delete_in_tree(params).await?,
                    other => {
                        return Err(format!("apply_ingest_batch: unknown op {other:?}").into());
                    }
                }
            }
            return Ok(());
        }

        let entity = EntityName::new(Block::entity_name());
        let mut batch: Vec<holon_core::BatchOp> = Vec::with_capacity(ops.len());
        // Per-parent last-assigned sort_key cursor, seeded lazily from the DB
        // sibling set the first time a parent is touched.
        let mut parent_cursor: HashMap<String, Option<String>> = HashMap::new();
        for (op, params) in ops {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .map(str::to_string)
                .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                    "apply_ingest_batch: op missing 'id' param".into()
                })?;
            match op.as_str() {
                "create" | "update" => {
                    // Re-derive create vs update from the SQL cache — the same
                    // classification `update_in_tree` makes, so the CDC op kind
                    // matches the row's prior presence.
                    let real_op = if self.cache.get_by_id(&id).await?.is_some() {
                        "update"
                    } else {
                        "create"
                    };
                    let position = if real_op == "create"
                        && let Some(parent) = params
                            .get("parent_id")
                            .and_then(|v| v.as_string())
                            .map(str::to_string)
                    {
                        // Sibling re-keys are minted by US over the projected
                        // sibling set, on the FIRST create under each parent —
                        // never sourced from the op's own params. They travel
                        // TYPED on this op's position into the batch's single
                        // transaction (ADR 0030 D1), so a peer property can
                        // never become a re-key (Ruling B).
                        let mut rekeys: Vec<(String, String)> = Vec::new();
                        if !parent_cursor.contains_key(&parent) {
                            let existing = self.sibling_keys(&parent).await?;
                            // The batch appends after the LAST sibling. If the
                            // set is not an insertable sequence, re-key it in
                            // place first (order-preserving) and seed from the
                            // last sibling's NEW key — skipping an unkeyed row
                            // instead would silently place every ingested block
                            // before it.
                            let seed = if Self::needs_rekey(&existing) {
                                let plan = Self::plan_rekey_with_slot(&existing, existing.len())?;
                                rekeys = plan.rekeys.clone();
                                existing.len().checked_sub(1).map(|i| plan.keys[i].clone())
                            } else {
                                existing.last().map(|(_, k)| k.clone())
                            };
                            parent_cursor.insert(parent.clone(), seed);
                        }
                        let cursor = parent_cursor.get_mut(&parent).ok_or_else(
                            || -> Box<dyn std::error::Error + Send + Sync> {
                                format!("apply_ingest_batch: cursor for {parent} vanished").into()
                            },
                        )?;
                        // SqlOnly `Store` order owner — the sanctioned mint site
                        // (Replication.md §5): batched form of `new_child_anchor`'s
                        // append path with an in-memory per-parent cursor.
                        // ALLOW(order_minting): SqlOnly order owner, batched mint.
                        let key = gen_key_between(cursor.as_deref(), None).map_err(
                            |e| -> Box<dyn std::error::Error + Send + Sync> {
                                format!("apply_ingest_batch mint for {id}: {e:#}").into()
                            },
                        )?;
                        *cursor = Some(key.clone());
                        Some(MintedPosition::new(key, rekeys))
                    } else {
                        None
                    };
                    match position {
                        Some(p) => batch.push(holon_core::BatchOp::placed(real_op, params, p)),
                        None => batch.push(holon_core::BatchOp::data(real_op, params)),
                    }
                }
                "delete" => batch.push(holon_core::BatchOp::data("delete", params)),
                other => {
                    return Err(format!("apply_ingest_batch: unknown op {other:?}").into());
                }
            }
        }
        self.sql_ops
            .execute_batch_with_origin(&entity, batch, EventOrigin::Org)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("apply_ingest_batch execute_batch_with_origin: {e:#}").into()
            })?;
        Ok(())
    }

    fn has_upstream_consolidator(&self) -> bool {
        self.caps.profile().has_downstream_projection()
    }

    fn consolidator(&self) -> Consolidator {
        self.caps.consolidator()
    }

    async fn prev_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let id = id.as_str();
        let Some(block) = self.cache.get_by_id(id).await? else {
            return Err(format!("prev_sibling: block {id} missing").into());
        };
        if !block.parent_id.is_block() {
            return Ok(None);
        }
        let siblings = self.sibling_keys(block.parent_id.as_str()).await?;
        let pos = siblings.iter().position(|(sid, _)| sid == id);
        Ok(pos
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| siblings.get(i))
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(sid, _)| EntityUri::from_raw(sid)))
    }

    async fn next_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let id = id.as_str();
        let Some(block) = self.cache.get_by_id(id).await? else {
            return Err(format!("next_sibling: block {id} missing").into());
        };
        if !block.parent_id.is_block() {
            return Ok(None);
        }
        let siblings = self.sibling_keys(block.parent_id.as_str()).await?;
        let pos = siblings.iter().position(|(sid, _)| sid == id);
        Ok(pos
            .and_then(|i| siblings.get(i + 1))
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(sid, _)| EntityUri::from_raw(sid)))
    }

    async fn first_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(self
            .sibling_keys(parent_id.as_str())
            .await?
            .into_iter()
            .next()
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(id, _)| EntityUri::from_raw(&id)))
    }

    async fn last_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(self
            .sibling_keys(parent_id.as_str())
            .await?
            .into_iter()
            .last()
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(id, _)| EntityUri::from_raw(&id)))
    }

    async fn children(&self, parent_id: &EntityUri) -> Result<Vec<EntityUri>> {
        let parent_id = parent_id.as_str();
        // Loro mode: the Loro tree is the order authority. Read it directly so
        // the org-scan place loop sees `create_in_tree` blocks immediately —
        // the outbound projector that fills the SQL cache is not running during
        // the initial scan, so a cache read returns `[]` for freshly-created
        // blocks and the poll times out. Steady-state, the cache is just a
        // projection of this same tree, so the answer is identical.
        if let Some(kids) = self.cell_registry.live_children(parent_id).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("children({parent_id}): {e:#}").into()
            },
        )? {
            // ALLOW(entity_uri_from_raw): child id String from
            // cell_registry.live_children() (Loro output)
            return Ok(kids.iter().map(|k| EntityUri::from_raw(k)).collect());
        }
        Ok(self
            .sibling_keys(parent_id)
            .await?
            .into_iter()
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(id, _)| EntityUri::from_raw(&id))
            .collect())
    }
}

#[async_trait]
impl CrudOperations<Block> for SqlBlockOperations {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        // Phase 2 authority flip: route block field writes through the
        // `BlockCellRegistry`, which writes to Loro (LoroText for `content`,
        // `tree.mov` for `parent_id`, LoroMap meta for the rest). The
        // `LoroSyncController.on_loro_changed` outbound projector then
        // emits the SQL UPDATE — there's no SQL write on this path. The
        // registry returns `Ok(false)` for fields it can't handle (SqlOnly
        // mode, or fields like `sort_key`/`marks`/`depth` whose Loro
        // encoding doesn't round-trip cleanly today); on `Ok(false)` we
        // fall through to the legacy SQL `set_field` path so existing
        // behaviour is preserved for those fields.
        // `id` arrives in mixed forms: already-schemed (`block:foo`) from the
        // org update path (`build_block_params` → `block.id.to_string()`), or
        // bare (`foo`) from some `LoroBlockOperations` callers. `from_raw`
        // normalizes both (parses a schemed id as-is, treats a bare id as a
        // block) — `EntityUri::block(id)` double-prefixed a schemed id to
        // `block:block:foo`, which `write_field`/`update_block_fields` then
        // failed to resolve ("Block not found"), aborting the org scan's update
        // pass *before* the place loop ran (the BulkExternalAdd sibling-order
        // scramble, `inv-live-children-match-ref`).
        // ALLOW(entity_uri_from_raw): set_field id &str from CrudOperations API surface
        let uri = EntityUri::from_raw(id);
        let routed = self
            .cell_registry
            .write_field(&uri, field, value.clone())
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("BlockCellRegistry::write_field({field}): {e:#}").into()
            })?;
        if routed {
            // The Loro outbound projector emits the SQL UPDATE and the resulting
            // CDC event produces the FieldDelta; there is no synchronous change
            // to surface here. This is the org-reingest / structural seam — user
            // CRUD `set_field` (which needs an undo inverse) is served by the
            // Loro CRUD authority (`LoroBlockOperations`) under Loro authority,
            // and by `SqlOperationProvider` in SqlOnly mode.
            return Ok(OperationResult::irreversible(Vec::new()));
        }

        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("field".into(), Value::String(field.to_string()));
        params.insert("value".into(), value);
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "set_field", params)
            .await
    }

    async fn create(&self, fields: holon_api::StorageEntity) -> Result<(String, OperationResult)> {
        let id = fields
            .get("id")
            .and_then(|v| v.as_string())
            .map(String::from)
            .ok_or_else(|| "SqlBlockOperations::create: missing 'id'".to_string())?;

        // This path writes `block_raw` directly — it never goes through
        // `BlockOrdering::place`/`OrderKeyMinting`, so a caller that omits
        // `sort_key` (creation-slot commit, a bare `block.create` Rhai/MCP
        // action, page create) would otherwise fall through to the SQL
        // column's literal default `"A0"` for EVERY such create. Two
        // consecutive id-less creates under the same parent then collide on
        // the identical key, leaving sibling order ambiguous until some
        // later op (e.g. `split_block`'s tie-detected rebalance) re-mints
        // distinct keys. Mint a real key here — strictly after the current
        // last sibling — using the same `gen_key_between` fractional-index
        // generator `new_child_anchor` uses, so a caller-supplied `sort_key`
        // still wins.
        //
        // Gated on consolidator exactly like `new_child_anchor`: only the
        // SqlOnly order owner mints. In Upstream (Loro) mode the tree is
        // authoritative and its outbound projector is the sole `sort_key`
        // writer (see the UPSERT comment in `prepare_create`), so minting
        // here too would mix `gen_key_between` values with Loro-fi values in
        // the same column — the exact keyspace-mixing bug class invariant 10
        // warns about.
        let fields = fields;
        let position = if !fields.contains_key("sort_key")
            && matches!(self.consolidator(), Consolidator::Store)
            && let Some(parent_id) = fields
                .get("parent_id")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        {
            // Append = anchor after the LAST sibling, whatever its key looks
            // like. Routing through `mint_child_key` means an unkeyed sibling
            // gets re-minted in place and the new block still lands after it;
            // anchoring on the greatest minted key instead would skip the
            // unkeyed row and silently place the new block BEFORE it. Positional
            // (after-a-specific-block) creates come through `create_at`, not
            // here — `split_block` / `restore_split` pre-mint and call that.
            let last_id = self.sibling_keys(&parent_id).await?.pop().map(|(id, _)| id);
            // ALLOW(order_minting): sanctioned SqlOnly order-owner mint site
            // (Replication.md §5), same file/gate as `new_child_anchor`.
            let minted = self
                .mint_child_key(&parent_id, last_id.as_deref())
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("SqlBlockOperations::create: mint sort_key: {e:#}").into()
                })?;
            // The key AND its sibling re-keys travel TYPED into create_row's
            // transaction, so a refused create leaves the keyspace untouched
            // (ADR 0030 D1) and no `_order_rekeys` params key ever exists.
            Some(minted)
        } else {
            None
        };

        let result = self.sql_ops.create_row(fields, position).await?;
        Ok((id, result))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        // Fail-closed on NON-LEAF (destructive-delete ruling 2026-07-21): a bare
        // `delete` NEVER cascades a subtree. The caller must opt in explicitly
        // via `delete_subtree` or `delete_keep_children`. Mirrors the Loro
        // authority's guard so both providers refuse identically.
        // ALLOW(entity_uri_from_raw): op-dispatch id string → EntityUri at the edge
        let uri = EntityUri::from_raw(id);
        let children = self.children_ordered(&uri).await?;
        if !children.is_empty() {
            return Err(format!(
                "delete: block {id} has {} child(ren); refusing to cascade. Use \
                 `delete_subtree` to delete the whole subtree, or \
                 `delete_keep_children` to reparent the children first.",
                children.len()
            )
            .into());
        }

        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "delete", params)
            .await
    }
}

#[async_trait]
impl OperationProvider for SqlBlockOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        use holon_core::__operations_block_operations;
        let entity_name = Block::entity_name();
        let short_name = Block::short_name().expect("Block must have short_name");
        let id_column = "id";
        __operations_block_operations::block_operations(
            entity_name,
            short_name,
            entity_name,
            id_column,
        )
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use holon_core::__operations_block_operations;

        if entity_name.as_str() != Block::entity_name() {
            return Err(format!(
                "SqlBlockOperations: expected entity '{}', got '{}'",
                Block::entity_name(),
                entity_name
            )
            .into());
        }

        match __operations_block_operations::dispatch_operation::<_, Block>(self, op_name, &params)
            .await
        {
            Ok(op) => Ok(op),
            Err(err) => {
                if UnknownOperationError::is_unknown(err.as_ref()) {
                    Err(format!("SqlBlockOperations: unknown block operation '{}'", op_name).into())
                } else {
                    Err(err)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_api::block::Block;
    use holon_api::entity_uri::EntityUri;
    use holon_core::__operations_block_operations;
    use holon_core::OperationRegistry;
    use holon_core::block_ordering::BlockOrdering;
    use holon_core::fractional_index::is_minted_key;
    use holon_core::storage::types::StorageEntity;
    use holon_turso::schema_modules::BlockMatviewSchemaModule;
    use holon_turso::schema_modules::BlockSchemaModule;

    use super::SqlBlockOperations;
    use crate::core::queryable_cache::QueryableCache;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::storage::BLOCK_WRITE_TABLE;
    use crate::storage::schema_module::SchemaModule;
    use crate::storage::turso::TursoBackend;

    /// Sanity check: the macro-generated `block_operations()` descriptor
    /// list — what `SqlBlockOperations::operations` returns — advertises
    /// indent / outdent / move_block / move_up / move_down. On `main`
    /// (before this provider was registered), the dispatcher had no entry
    /// for `("block", "indent")`. See the diagnostic comment on
    /// `BLOCK_TREE_KEYCHORD_OPS_ENABLED` in
    /// crates/holon-integration-tests/src/pbt/state_machine.rs.
    #[test]
    fn block_operations_advertise_indent_and_outdent() {
        let entity_name = Block::entity_name();
        let short_name = Block::short_name().expect("Block must have short_name");
        let names: Vec<String> = __operations_block_operations::block_operations(
            entity_name,
            short_name,
            entity_name,
            "id",
        )
        .into_iter()
        .map(|d| d.name)
        .collect();
        for op in ["indent", "outdent", "move_block", "move_up", "move_down"] {
            assert!(names.iter().any(|n| n == op), "{op} missing: {names:?}");
        }
    }

    async fn setup_sql_block_ops() -> (
        TursoBackend,
        Arc<SqlBlockOperations>,
        crate::storage::turso::DbHandle,
    ) {
        let (backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        handle
            .execute_ddl("PRAGMA foreign_keys = ON")
            .await
            .expect("FK pragma");
        // Minimal schema: block_raw + junction tables + block matview + the
        // link junction every `delete` cleans up.
        holon_turso::schema_modules::CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("CoreSchemaModule");
        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("BlockSchemaModule");
        BlockMatviewSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("BlockMatviewSchemaModule");
        holon_turso::schema_modules::LinkSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("LinkSchemaModule");

        let descriptors = BlockSchemaModule.edge_fields();
        let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
            handle.clone(),
            BLOCK_WRITE_TABLE.to_string(),
            "block".to_string(),
            "block".to_string(),
            descriptors,
        ));
        // Point the cache at `block_raw` (not the `block` matview) so that
        // test INSERTs into block_raw are immediately visible via get_by_id.
        let mut block_raw_type_def = Block::type_definition();
        block_raw_type_def.name = "block_raw".to_string();
        let cache = Arc::new(
            QueryableCache::<Block>::new(handle.clone(), block_raw_type_def)
                .await
                .expect("cache"),
        );
        let ops = Arc::new(SqlBlockOperations::new(sql_ops, cache));
        (backend, ops, handle)
    }

    async fn read_sort_key(handle: &crate::storage::turso::DbHandle, bare_id: &str) -> String {
        let rows = handle
            .query(
                &format!(
                    "SELECT sort_key FROM block_raw WHERE id = '{}'",
                    bare_id.replace('\'', "''")
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("read sort_key");
        rows.into_iter()
            .next()
            .and_then(|r| {
                r.get("sort_key")
                    .and_then(|v| v.as_string().map(str::to_string))
            })
            .unwrap_or_default()
    }

    /// Test 6: `SqlBlockOperations::place` is idempotent in SqlOnly mode.
    ///
    /// Insert two siblings A, B (B sort_key > A). Call `place(B, parent,
    /// Some(A.id))` twice. Assert the sort_key of B does NOT change after the
    /// second call — the idempotency guard short-circuits when the current
    /// predecessor already matches the requested placement.
    #[tokio::test]
    async fn place_is_idempotent_in_sql_only_mode() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;

        // Use a real block as parent so the is_block() guard in prev_sibling
        // doesn't short-circuit — the idempotency check relies on prev_sibling.
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let parent = EntityUri::from_raw("block:test-parent");
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let a_id = EntityUri::from_raw("block:test-a");
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let b_id = EntityUri::from_raw("block:test-b");

        // IDs stored in block_raw use full URI form (e.g. "block:test-a") so
        // EntityUri deserialization round-trips via QueryableCache.
        // Sort keys use valid fractional-index format (hex strings); "a0" < "a1".
        for (id, sort_key, content) in [
            (parent.as_str(), "V", "parent"),
            (a_id.as_str(), "a0", "A"),
            (b_id.as_str(), "a1", "B"),
        ] {
            let parent_val = if id == parent.as_str() {
                "sentinel:no_parent"
            } else {
                parent.as_str()
            };
            handle
                .execute(
                    &format!(
                        "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                         created_at, updated_at) VALUES ('{}', '{}', '{}', '{}', 'text', 0, 0)",
                        id, parent_val, sort_key, content
                    ),
                    vec![],
                )
                .await
                .unwrap_or_else(|e| panic!("insert {id}: {e}"));
        }

        // First placement: B is already after A (sort_key "a1" > "a0").
        // The idempotency guard detects this via prev_sibling and skips the write.
        // Pass full URI strings — place() compares against EntityUri::as_str().
        ops.place(&b_id, &parent, Some(&a_id))
            .await
            .expect("place B after A (first call)");

        let sort_key_after_first = read_sort_key(&handle, b_id.as_str()).await;

        // Second call — must be a no-op (same guard fires again).
        ops.place(&b_id, &parent, Some(&a_id))
            .await
            .expect("place B after A (second call)");

        let sort_key_after_second = read_sort_key(&handle, b_id.as_str()).await;

        assert_eq!(
            sort_key_after_first, sort_key_after_second,
            "place() called twice with identical args must not change sort_key (idempotency guard \
             regression)"
        );
    }

    /// Bug (dogfood 2026-07-10): `block.create` without an explicit `sort_key`
    /// always minted the SQL column default `"A0"`, so two consecutive
    /// id-less creates under the same parent collided on the identical key —
    /// sibling order stayed ambiguous until some later op (e.g.
    /// `split_block`'s tie-detected rebalance) re-minted distinct keys.
    /// `SqlBlockOperations::create` must instead mint a real, strictly
    /// increasing key for each create when the caller omits `sort_key`.
    #[tokio::test]
    async fn create_without_sort_key_mints_strictly_increasing_keys() {
        use std::collections::HashMap;

        use holon_core::CrudOperations;
        use holon_core::storage::types::StorageEntity;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let parent = EntityUri::from_raw("block:test-parent");
        handle
            .execute(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                     created_at, updated_at) VALUES ('{}', 'sentinel:no_parent', 'V', 'parent', \
                     'text', 0, 0)",
                    parent.as_str()
                ),
                vec![],
            )
            .await
            .expect("insert parent");

        let mut fields1: StorageEntity = HashMap::new();
        fields1.insert(
            "id".into(),
            holon_api::Value::String("block:child-1".to_string()),
        );
        fields1.insert(
            "parent_id".into(),
            holon_api::Value::String(parent.as_str().to_string()),
        );
        fields1.insert(
            "content".into(),
            holon_api::Value::String("first".to_string()),
        );
        fields1.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        ops.create(fields1).await.expect("create child 1");

        let mut fields2: StorageEntity = HashMap::new();
        fields2.insert(
            "id".into(),
            holon_api::Value::String("block:child-2".to_string()),
        );
        fields2.insert(
            "parent_id".into(),
            holon_api::Value::String(parent.as_str().to_string()),
        );
        fields2.insert(
            "content".into(),
            holon_api::Value::String("second".to_string()),
        );
        fields2.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        ops.create(fields2).await.expect("create child 2");

        let key1 = read_sort_key(&handle, "block:child-1").await;
        let key2 = read_sort_key(&handle, "block:child-2").await;

        assert_ne!(
            key1, "A0",
            "id-less create must not fall back to the literal SQL default"
        );
        assert_ne!(
            key2, "A0",
            "id-less create must not fall back to the literal SQL default"
        );
        assert_ne!(
            key1, key2,
            "two consecutive id-less creates must not collide"
        );
        assert!(
            key1 < key2,
            "second create must sort strictly after the first: {key1:?} vs {key2:?}"
        );
    }

    /// Insert a parent row directly and return its full URI.
    async fn seed_parent(handle: &crate::storage::turso::DbHandle, bare: &str) -> String {
        let id = format!("block:{bare}");
        handle
            .execute(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                     created_at, updated_at) VALUES ('{id}', 'sentinel:no_parent', 'V', 'parent', \
                     'text', 0, 0)"
                ),
                vec![],
            )
            .await
            .expect("insert parent");
        id
    }

    fn child_create_op(parent: &str, i: usize, after: Option<&str>) -> (String, StorageEntity) {
        use holon_api::Value;
        let mut p: StorageEntity = std::collections::HashMap::new();
        p.insert("id".into(), Value::String(format!("block:c{i}")));
        p.insert("parent_id".into(), Value::String(parent.to_string()));
        p.insert("content".into(), Value::String(format!("child {i}")));
        p.insert("content_type".into(), Value::String("text".to_string()));
        if let Some(a) = after {
            p.insert(
                holon_api::POSITION_AFTER_BLOCK_ID_PARAM.into(),
                Value::String(a.to_string()),
            );
        }
        ("create".to_string(), p)
    }

    /// Count matview-maintenance passes = broadcast batches on the `block`
    /// matview CDC channel. Turso emits CDC per matview per transaction commit
    /// (base tables emit none — see `cdc_base_vs_matview_repro`), so one batch
    /// == one IVM maintenance pass. Waits up to `first` for the first pass,
    /// then drains until the channel stays quiet for `quiet`.
    async fn count_matview_passes(
        cdc: &mut tokio::sync::broadcast::Receiver<
            holon_api::streaming::WithMetadata<
                holon_api::streaming::Batch<crate::storage::turso::RowChange>,
                holon_api::streaming::BatchMetadata,
            >,
        >,
        first: std::time::Duration,
        quiet: std::time::Duration,
    ) -> usize {
        let mut n = 0usize;
        if tokio::time::timeout(first, cdc.recv()).await.is_ok() {
            n += 1;
        } else {
            return n;
        }
        while tokio::time::timeout(quiet, cdc.recv()).await.is_ok() {
            n += 1;
        }
        n
    }

    const FIRST_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
    const QUIET: std::time::Duration = std::time::Duration::from_millis(300);

    /// BugFunnel row 32 fix. Applying a whole file's ingest through
    /// `apply_ingest_batch` drives the live watch-view matview IVM maintenance
    /// exactly ONCE — one transaction commit per file — regardless of the block
    /// count, instead of once per block (the O(N²) cold-boot cost whose
    /// per-block price scaled with the accumulated table). The born-correct
    /// per-parent sort_keys are strictly increasing, so the downstream
    /// `place_all` totalizer would rewrite nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn batched_ingest_runs_matview_maintenance_once_per_file() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;
        let parent = seed_parent(&handle, "p").await;

        let mut cdc = handle
            .subscribe_cdc("block")
            .await
            .expect("subscribe block matview cdc");
        // Clear the parent-insert pass so the count reflects only the ingest.
        let _ = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;

        let n = 16usize;
        let mut prev: Option<String> = None;
        let ops_vec: Vec<(String, StorageEntity)> = (0..n)
            .map(|i| {
                let op = child_create_op(&parent, i, prev.as_deref());
                prev = Some(format!("block:c{i}"));
                op
            })
            .collect();

        ops.apply_ingest_batch(ops_vec)
            .await
            .expect("batched ingest");

        let passes = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;
        assert_eq!(
            passes, 1,
            "one matview IVM maintenance pass per FILE (row 32 O(N²) fix); got {passes} passes for \
             {n} blocks in one batch"
        );

        // Born-correct order: strictly increasing per-parent keys ⇒ place_all
        // no-op (never re-mints a single row).
        let mut keys = Vec::new();
        for i in 0..n {
            keys.push(read_sort_key(&handle, &format!("block:c{i}")).await);
        }
        for w in keys.windows(2) {
            assert!(
                w[0] < w[1],
                "batched creates must be born strictly increasing so place_all rewrites nothing: \
                 {keys:?}"
            );
        }
    }

    /// Baseline that the fix removes: the historic per-op apply drove ONE
    /// matview maintenance pass per block (N passes for N blocks).
    /// Documents that the batch count above is a real collapse, not an
    /// artifact of the metric.
    #[tokio::test(flavor = "multi_thread")]
    async fn per_op_ingest_runs_one_matview_pass_per_block() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;
        let parent = seed_parent(&handle, "p").await;

        let mut cdc = handle
            .subscribe_cdc("block")
            .await
            .expect("subscribe block matview cdc");
        let _ = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;

        let n = 8usize;
        let mut prev: Option<String> = None;
        for i in 0..n {
            let (_op, params) = child_create_op(&parent, i, prev.as_deref());
            ops.update_in_tree(params).await.expect("per-op create");
            prev = Some(format!("block:c{i}"));
        }

        let passes = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;
        assert_eq!(
            passes, n,
            "the per-op path runs one matview maintenance pass per block ({n} expected); this is \
             the O(N²) cost the batch path collapses to 1"
        );
    }

    /// Insert a block row directly into `block_raw` (bypassing the ops layer)
    /// so a test can arrange states the write path would reject.
    async fn insert_raw(
        handle: &crate::storage::turso::DbHandle,
        id: &str,
        parent_id: &str,
        sort_key: &str,
    ) {
        handle
            .execute(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, sort_key, content, \
                     content_type, created_at, updated_at) VALUES ('{id}', '{parent_id}', \
                     '{sort_key}', '{id}', 'text', 0, 0)"
                ),
                vec![],
            )
            .await
            .unwrap_or_else(|e| panic!("insert {id}: {e}"));
    }

    async fn read_parent(handle: &crate::storage::turso::DbHandle, id: &str) -> String {
        handle
            .query(
                &format!("SELECT parent_id FROM block_raw WHERE id = '{id}'"),
                std::collections::HashMap::new(),
            )
            .await
            .expect("read parent_id")
            .into_iter()
            .next()
            .and_then(|r| {
                r.get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(String::from)
            })
            .unwrap_or_else(|| panic!("no row for {id}"))
    }

    async fn row_exists(handle: &crate::storage::turso::DbHandle, id: &str) -> bool {
        !handle
            .query(
                &format!("SELECT id FROM block_raw WHERE id = '{id}'"),
                std::collections::HashMap::new(),
            )
            .await
            .expect("probe block_raw")
            .is_empty()
    }

    /// A parent cycle reaching the walk's own root must FAIL LOUD, not answer
    /// a list containing that root.
    ///
    /// `move_block` has no reparent-under-own-descendant guard and is
    /// dispatchable (MCP `execute_operation`, Rhai), so this state is
    /// reachable. A caller that walks the returned set (`get_descendants`,
    /// `delete_subtree`) would otherwise treat the root as its own descendant
    /// and act on it twice. The predecessor BFS hung instead, which is bad but
    /// loud; degrading loud to silent is the forbidden direction.
    #[tokio::test]
    async fn descendant_ids_fails_loud_on_a_parent_cycle_through_its_own_root() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;

        // root -> x -> y -> z, then close the loop: root's parent becomes z.
        insert_raw(&handle, "block:root", "sentinel:no_parent", "a0").await;
        insert_raw(&handle, "block:x", "block:root", "a1").await;
        insert_raw(&handle, "block:y", "block:x", "a2").await;
        insert_raw(&handle, "block:z", "block:y", "a3").await;
        handle
            .execute(
                "UPDATE block_raw SET parent_id = 'block:z' WHERE id = 'block:root'",
                vec![],
            )
            .await
            .expect("close the cycle");

        let err =
            ops.sql_ops.descendant_ids("block:root").await.expect_err(
                "a cycle through the walk root must be an Err, not a list with the root",
            );
        let msg = err.to_string();
        assert!(
            msg.contains("cycle") && msg.contains("block:root"),
            "error must name the cycle and the offending block, got: {msg}"
        );
    }

    /// `indent` reparents through `move_block` and writes NO depth: the column
    /// it used to maintain does not exist, and the tree is the only authority
    /// on nesting level.
    ///
    /// This replaces the characterization test that pinned the old arithmetic's
    /// wrong output (`parent.depth()` read a hardcoded `0`, so every move wrote
    /// `depth = 1` and shifted the subtree by a cumulative `+1`). The schema
    /// assertion is the teeth: re-adding the column reds this test rather than
    /// silently reviving a value nothing writes authoritatively.
    #[tokio::test]
    async fn indent_reparents_and_block_raw_has_no_depth_column() {
        use holon_core::BlockOperations;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:page", "sentinel:no_parent", "a0").await;
        insert_raw(&handle, "block:a", "block:page", "a1").await;
        insert_raw(&handle, "block:b", "block:page", "a2").await;
        insert_raw(&handle, "block:c", "block:b", "a3").await;

        let columns: Vec<String> = handle
            .query(
                "SELECT name FROM pragma_table_info('block_raw')",
                std::collections::HashMap::new(),
            )
            .await
            .expect("read block_raw columns")
            .into_iter()
            .filter_map(|r| r.get("name").and_then(|v| v.as_string()).map(String::from))
            .collect();
        assert!(
            !columns.is_empty(),
            "pragma_table_info must answer the real column set, got none"
        );
        assert!(
            !columns.contains(&"depth".to_string()),
            "block_raw must have no depth column; got {columns:?}"
        );

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let b = EntityUri::from_raw("block:b");
        ops.indent(&b).await.expect("indent b under a");

        assert_eq!(
            read_parent(&handle, "block:b").await,
            "block:a",
            "indent reparents b under its previous sibling"
        );
        assert_eq!(
            read_parent(&handle, "block:c").await,
            "block:b",
            "the subtree rides along untouched"
        );
    }

    /// The CTE-backed `get_descendants` override answers the whole subtree.
    ///
    /// The override is NOT dead code, though a grep for `get_descendants`
    /// suggests it is: the only production call is virtual, from the
    /// `delete_subtree` trait default, and `delete_subtree` is a registered
    /// dispatchable op (operation_dispatcher.rs, beside indent/outdent/
    /// split_block). This test pins the override's own contract; the one below
    /// pins what that caller currently does with it.
    #[tokio::test]
    async fn get_descendants_override_answers_the_whole_subtree_in_one_cte() {
        use holon_core::DataSource;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:page", "sentinel:no_parent", "a0").await;
        insert_raw(&handle, "block:keep", "block:page", "a1").await;
        insert_raw(&handle, "block:sub", "block:page", "a2").await;
        insert_raw(&handle, "block:sub-c", "block:sub", "a3").await;
        insert_raw(&handle, "block:sub-d", "block:sub-c", "a4").await;

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let sub = EntityUri::from_raw("block:sub");
        let mut found: Vec<String> = ops
            .get_descendants(&sub)
            .await
            .expect("CTE descendants")
            .into_iter()
            .map(|b| b.id.to_string())
            .collect();
        found.sort();
        assert_eq!(
            found,
            vec!["block:sub-c".to_string(), "block:sub-d".to_string()],
            "both levels below `sub`, and neither `sub` itself nor its sibling"
        );
    }

    /// `delete_subtree` deletes a subtree of ARBITRARY depth: its deepest-first
    /// order is derived from `parent_id` within the descendant set, so every
    /// `delete` sees a leaf and the fail-closed non-leaf guard never fires.
    ///
    /// Red-for-the-right-reason before the fix (the ordering sorted by
    /// `BlockEntity::depth()`, hardcoded `0`, so the sort was a no-op):
    /// `refusing to cascade` — a registered dispatchable op that could not
    /// delete anything deeper than one level.
    #[tokio::test]
    async fn delete_subtree_deletes_a_three_level_subtree() {
        use holon_core::BlockOperations;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:page", "sentinel:no_parent", "a0").await;
        insert_raw(&handle, "block:keep", "block:page", "a1").await;
        insert_raw(&handle, "block:sub", "block:page", "a2").await;
        insert_raw(&handle, "block:sub-c", "block:sub", "a3").await;
        insert_raw(&handle, "block:sub-d", "block:sub-c", "a4").await;

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let sub = EntityUri::from_raw("block:sub");
        ops.delete_subtree(&sub).await.expect("delete the subtree");

        for gone in ["block:sub", "block:sub-c", "block:sub-d"] {
            assert!(
                !row_exists(&handle, gone).await,
                "{gone} must be deleted with its subtree"
            );
        }
        for kept in ["block:page", "block:keep"] {
            assert!(
                row_exists(&handle, kept).await,
                "{kept} is outside the subtree and must survive"
            );
        }
    }

    /// The keyspace `siblings` becomes once `position`'s re-keys are written —
    /// what the firing transaction will make durable, re-sorted the way the
    /// projection reads it.
    fn apply_rekeys(
        siblings: Vec<(String, String)>,
        position: &holon_core::block_ordering::MintedPosition,
    ) -> Vec<(String, String)> {
        let mut out = siblings;
        for (id, key) in position.rekeys() {
            let row = out
                .iter_mut()
                .find(|(sid, _)| sid == id)
                .unwrap_or_else(|| panic!("re-key names {id}, which is not a sibling"));
            row.1 = key.clone();
        }
        out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// Ordered `(id, sort_key)` for a parent, straight off `block_raw` in the
    /// order the projection reads it.
    async fn ordered_siblings(
        handle: &crate::storage::turso::DbHandle,
        parent_id: &str,
    ) -> Vec<(String, String)> {
        handle
            .query(
                &format!(
                    "SELECT id, sort_key FROM block_raw WHERE parent_id = '{parent_id}' ORDER BY \
                     sort_key, id"
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("read siblings")
            .into_iter()
            .map(|r| {
                let get = |k: &str| {
                    r.get(k)
                        .and_then(|v| v.as_string().map(str::to_string))
                        .unwrap_or_default()
                };
                (get("id"), get("sort_key"))
            })
            .collect()
    }

    /// The SqlOnly arm of a position-0 split of a PARENTLESS block. Such a
    /// block's predecessor is `None` (`get_prev_sibling` returns `None` for a
    /// null `parent_id`), so `split_block` anchors the minted empty block with
    /// `new_child_anchor(sentinel:no_parent, None)` — the first slot among the
    /// ROOTS. No keystone draw reaches this (nothing splits a parentless
    /// block), so the ordering is pinned directly: the minted key must sort
    /// strictly BEFORE the existing root, or the "empty block above the text"
    /// contract inverts at the vault's top level.
    #[tokio::test]
    async fn root_slot_anchor_sorts_before_the_first_root() {
        use holon_core::block_ordering::OrderKeyMinting;

        let (_backend, ops, handle) = setup_sql_block_ops().await;
        insert_raw(&handle, "block:first-root", "sentinel:no_parent", "80").await;
        insert_raw(&handle, "block:second-root", "sentinel:no_parent", "81").await;
        let before = ordered_siblings(&handle, "sentinel:no_parent").await;

        let position = ops
            .new_child_anchor(&EntityUri::no_parent(), None)
            .await
            .expect("anchor the minted empty block first among the roots");
        let projected = apply_rekeys(before.clone(), &position);
        // The anchor DECIDES; the create's transaction writes (ADR 0030 D1).
        assert_eq!(
            ordered_siblings(&handle, "sentinel:no_parent").await,
            before,
            "anchoring must not write: the re-key belongs to the firing transaction"
        );

        let (new_key, _rekeys) = position.into_parts();
        for (id, key) in &projected {
            assert!(
                new_key < *key,
                "the minted key {new_key:?} must sort before every existing root ({id} at {key:?})"
            );
        }
    }

    /// The SqlOnly `split_block` failure: `new_child_anchor` anchored on the
    /// raw column value of the sibling that follows the split origin. When
    /// that sibling still carries the SQL default `"A0"` — one unkeyed row,
    /// which ties with nothing, so the tied-key rebalance never fired — the
    /// pair handed to the generator was `("80", "A0")`. Neither is separable
    /// from the other (both are one byte; the generator only searches the
    /// bytes before the last), so the op failed outright with "Failed to
    /// generate fractional index between given keys".
    ///
    /// A single unkeyed sibling is exactly what a legacy vault row and a
    /// create that bypassed the minter both look like, so the anchor scan must
    /// classify keys, not read them.
    #[tokio::test]
    async fn split_anchors_past_a_single_unkeyed_sibling() {
        use holon_core::block_ordering::OrderKeyMinting;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        // The split origin, carrying a real minted key.
        insert_raw(&handle, "block:origin", "block:parent", "80").await;
        // One never-positioned sibling: the SQL column default. It ties with
        // nothing, so the pre-existing tied-key rebalance does not fire.
        insert_raw(&handle, "block:unkeyed", "block:parent", "A0").await;

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let parent = EntityUri::from_raw("block:parent");
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let origin = EntityUri::from_raw("block:origin");

        let position = ops
            .new_child_anchor(&parent, Some(&origin))
            .await
            .expect("anchor the new half of a split after its origin");
        // The key is reachable only by consuming the position (typed) — the way
        // production does when it hands the whole `MintedPosition` to the writer.
        let projected = apply_rekeys(ordered_siblings(&handle, "block:parent").await, &position);
        let (new_key, _rekeys) = position.into_parts();

        // The anchor DECIDES; the create's transaction writes (ADR 0030 D1).
        assert_eq!(
            ordered_siblings(&handle, "block:parent").await,
            vec![
                ("block:origin".to_string(), "80".to_string()),
                ("block:unkeyed".to_string(), "A0".to_string()),
            ],
            "anchoring must not write: the re-key belongs to the firing transaction"
        );

        // Every sibling then holds a real position, and the new block's key
        // sits strictly between the origin's and the next sibling's.
        let siblings = projected;
        for (id, key) in &siblings {
            assert!(
                is_minted_key(key),
                "{id} still holds the unkeyed value {key:?} after the anchor scan"
            );
        }
        let origin_key = &siblings
            .iter()
            .find(|(id, _)| id == "block:origin")
            .expect("origin row")
            .1;
        let unkeyed_key = &siblings
            .iter()
            .find(|(id, _)| id == "block:unkeyed")
            .expect("unkeyed row")
            .1;
        assert!(
            origin_key < &new_key && &new_key < unkeyed_key,
            "new key {new_key} must land between {origin_key} and {unkeyed_key}"
        );
        // Order is preserved, not reshuffled, by the re-key.
        assert_eq!(
            siblings
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["block:origin", "block:unkeyed"],
        );
    }

    /// An append must land LAST even when a sibling is unkeyed. Anchoring on
    /// the greatest MINTED key instead skips the unkeyed row — the new block
    /// then sorts BEFORE a sibling it must follow, silently. With the parent's
    /// only child carrying the default, there is no minted key at all, so the
    /// append anchors on nothing and the new block sorts FIRST.
    #[tokio::test]
    async fn create_appends_after_a_single_unkeyed_sibling() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        insert_raw(&handle, "block:only", "block:parent", "A0").await;

        create_child(&ops, "block:appended", "block:parent").await;

        assert_appended_last(&handle, &["block:only", "block:appended"]).await;
    }

    /// Same defect with a minted sibling present, so the skip is not merely
    /// "no anchor at all": anchoring on `"80"` and ignoring the `"A0"` row
    /// drops the new block into the MIDDLE of the sequence.
    #[tokio::test]
    async fn create_appends_after_a_mixed_keyed_and_unkeyed_sibling_set() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        insert_raw(&handle, "block:keyed", "block:parent", "80").await;
        insert_raw(&handle, "block:unkeyed", "block:parent", "A0").await;

        create_child(&ops, "block:appended", "block:parent").await;

        assert_appended_last(&handle, &["block:keyed", "block:unkeyed", "block:appended"]).await;
    }

    async fn create_child(ops: &Arc<SqlBlockOperations>, id: &str, parent_id: &str) {
        use holon_core::CrudOperations;

        let mut fields: StorageEntity = std::collections::HashMap::new();
        fields.insert("id".into(), holon_api::Value::String(id.to_string()));
        fields.insert(
            "parent_id".into(),
            holon_api::Value::String(parent_id.to_string()),
        );
        fields.insert("content".into(), holon_api::Value::String(id.to_string()));
        fields.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        ops.create(fields)
            .await
            .unwrap_or_else(|e| panic!("create {id}: {e}"));
    }

    async fn assert_appended_last(handle: &crate::storage::turso::DbHandle, expected: &[&str]) {
        let siblings = ordered_siblings(handle, "block:parent").await;
        assert_eq!(
            siblings
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "the appended block must sort LAST, after every existing sibling"
        );
        for (id, key) in &siblings {
            assert!(
                is_minted_key(key),
                "{id} still holds the unkeyed value {key:?} — an append must re-mint the rows it \
                 passes, never skip them"
            );
        }
    }

    /// The same defect without a split, and for the LEGACY shape rather than
    /// the column default: a persisted `sort_key` that is not minted output
    /// (here `"a0"` — lower-case and unterminated, so not a position) sorts
    /// arbitrarily against real keys, and anchoring on it cannot mint. Only
    /// excluding the literal `"A0"` sentinel is not enough — `place_all`, the
    /// SqlOnly order owner's total re-key driven by the Org authority's line
    /// order (ADR 0007), must classify every key and re-mint the ones that are
    /// not positions.
    #[tokio::test]
    async fn place_all_re_keys_a_legacy_unkeyed_block_into_its_requested_slot() {
        use holon_core::block_ordering::BlockOrdering;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        insert_raw(&handle, "block:first", "block:parent", "80").await;
        // Belongs in the middle, but this legacy value sorts above every
        // minted key — and it is not the `"A0"` sentinel, so a check that only
        // excludes the column default reads it as a real position.
        insert_raw(&handle, "block:middle", "block:parent", "a0").await;
        insert_raw(&handle, "block:last", "block:parent", "8180").await;

        assert_eq!(
            ordered_siblings(&handle, "block:parent")
                .await
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
            ["block:first", "block:last", "block:middle"],
            "precondition: the unkeyed row sorts last"
        );

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let parent = EntityUri::from_raw("block:parent");
        let ordered: Vec<EntityUri> = ["block:first", "block:middle", "block:last"]
            .iter()
            // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
            .map(|id| EntityUri::from_raw(id))
            .collect();
        ops.place_all(&parent, &ordered)
            .await
            .expect("re-key the parent to the authority's order");

        let siblings = ordered_siblings(&handle, "block:parent").await;
        assert_eq!(
            siblings
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["block:first", "block:middle", "block:last"],
        );
        for (id, key) in &siblings {
            assert!(is_minted_key(key), "{id} kept the unkeyed value {key:?}");
        }
    }

    /// ADR 0030 D1: a create is total or it is refused with ZERO side effects.
    ///
    /// The sibling re-key a minted position needs is part of the create's
    /// firing, so it must land in the create's transaction. Refuse the create
    /// — here through identity recognition, the derived-id collision a page
    /// create hits when the id's holder carries a different title — and the
    /// keyspace must look exactly as it did before the attempt.
    #[tokio::test]
    async fn a_refused_create_leaves_no_sibling_rekey_behind() {
        use holon_core::CrudOperations;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        // Not an insertable sequence (the SQL column default), so placing a new
        // last child re-keys it first.
        insert_raw(&handle, "block:unkeyed", "block:parent", "A0").await;
        // The derived id the create carries is already held by a DIFFERENT
        // title, so recognition refuses the create before any row is written.
        insert_raw(&handle, "block:held", "sentinel:no_parent", "80").await;

        let mut fields: StorageEntity = std::collections::HashMap::new();
        fields.insert(
            "id".into(),
            holon_api::Value::String("block:held".to_string()),
        );
        fields.insert(
            "parent_id".into(),
            holon_api::Value::String("block:parent".to_string()),
        );
        fields.insert(
            "content".into(),
            holon_api::Value::String("a rival title".to_string()),
        );
        fields.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        let err = ops
            .create(fields)
            .await
            .err()
            .expect("the create must be refused: its id is held under another title");
        assert!(
            err.to_string().contains("block:held"),
            "the refusal must name the contested id, got: {err}"
        );

        assert_eq!(
            ordered_siblings(&handle, "block:parent").await,
            vec![("block:unkeyed".to_string(), "A0".to_string())],
            "a refused create must leave the sibling keyspace byte-identical — the re-key is \
             part of the create's firing, not a durable prelude to it"
        );
    }

    /// The `prove_rekeys_are_siblings` backstop still refuses a re-key that
    /// targets a block outside the placed block's own parent — now on the TYPED
    /// channel. Re-keys reach `create_row` as a `MintedPosition`, never a
    /// params map key, so this guards a MINTING bug (the only way a wrong
    /// target can still arise once the data namespace cannot carry
    /// re-keys).
    #[tokio::test]
    async fn a_typed_create_rekey_cannot_target_a_non_sibling() {
        use holon_core::block_ordering::MintedPosition;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        insert_raw(&handle, "block:mine", "block:parent", "80").await;
        // A root block under a DIFFERENT parent — nothing to do with the create.
        insert_raw(&handle, "block:victim", "sentinel:no_parent", "80").await;

        let mut fields: StorageEntity = std::collections::HashMap::new();
        fields.insert(
            "id".into(),
            holon_api::Value::String("block:attacker".to_string()),
        );
        fields.insert(
            "parent_id".into(),
            holon_api::Value::String("block:parent".to_string()),
        );
        fields.insert("content".into(), holon_api::Value::String("hi".to_string()));
        fields.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );

        // A minted position whose re-key names a block under a DIFFERENT parent.
        let position = MintedPosition::new(
            "M0".to_string(),
            vec![("block:victim".to_string(), "ZZZZ".to_string())],
        );
        let err = ops
            .sql_ops
            .create_row(fields, Some(position))
            .await
            .err()
            .expect("a typed re-key must not touch a block outside the placed block's parent");
        let msg = err.to_string();
        assert!(
            msg.contains("block:victim") && msg.contains("sibling"),
            "the refusal must name the offending target and the rule, got: {msg}"
        );

        assert_eq!(
            read_sort_key(&handle, "block:victim").await,
            "80",
            "the unrelated block's order key must be untouched"
        );
        assert!(
            !row_exists(&handle, "block:attacker").await,
            "the refused create must leave no row behind either"
        );
    }

    /// STRUCTURAL IMPOSSIBILITY (Ruling B): a `_order_rekeys` key sitting in
    /// the caller-supplied params map is INERT — the writer never reads it,
    /// so it cannot re-key anyone.
    ///
    /// RED before this lane: the writer decoded the key and REFUSED the create
    /// (naming block:victim). GREEN after: the key is ordinary ignored data
    /// (the boundary filters still strip it as defense-in-depth, but even
    /// reaching the writer it has no effect), the create succeeds, and the
    /// victim is untouched.
    #[tokio::test]
    async fn a_params_map_order_rekeys_key_is_inert_at_the_writer() {
        use holon_core::CrudOperations;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        insert_raw(&handle, "block:mine", "block:parent", "80").await;
        insert_raw(&handle, "block:victim", "sentinel:no_parent", "80").await;

        let mut fields: StorageEntity = std::collections::HashMap::new();
        fields.insert(
            "id".into(),
            holon_api::Value::String("block:attacker".to_string()),
        );
        fields.insert(
            "parent_id".into(),
            holon_api::Value::String("block:parent".to_string()),
        );
        fields.insert("content".into(), holon_api::Value::String("hi".to_string()));
        fields.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        // Exactly the shape `Value::from_json_value` produces for an MCP
        // `properties` entry naming a NON-sibling — inert, the writer has no
        // params re-key channel.
        fields.insert(
            holon_core::block_ordering::ORDER_REKEYS_PARAM.into(),
            holon_api::Value::Object(std::collections::HashMap::from([(
                "block:victim".to_string(),
                holon_api::Value::String("ZZZZ".to_string()),
            )])),
        );

        ops.create(fields)
            .await
            .expect("the create succeeds; the map key is inert, not interpreted");
        assert_eq!(
            read_sort_key(&handle, "block:victim").await,
            "80",
            "the params-map _order_rekeys must have NO effect — the writer never reads it"
        );
        assert!(
            row_exists(&handle, "block:attacker").await,
            "the create landed normally despite the inert key"
        );
    }

    /// The same backstop guards a PLACEMENT on the typed channel: `place_row`
    /// takes a `MintedPosition`, and a re-key naming a block outside the target
    /// parent is refused.
    #[tokio::test]
    async fn a_typed_placement_rekey_cannot_target_a_non_sibling() {
        use holon_core::block_ordering::MintedPosition;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:pa", "sentinel:no_parent", "1000").await;
        insert_raw(&handle, "block:pb", "sentinel:no_parent", "2000").await;
        insert_raw(&handle, "block:mover", "block:pa", "80").await;
        // A block under the OLD parent — not a sibling of the mover's new home.
        insert_raw(&handle, "block:victim", "block:pa", "8180").await;

        // The placement moves block:mover under block:pb, but its position's
        // re-key names block:victim, which lives under block:pa.
        let position = MintedPosition::new(
            "80".to_string(),
            vec![("block:victim".to_string(), "ZZZZ".to_string())],
        );
        let err = ops
            .sql_ops
            .place_row("block:mover", "block:pb", position)
            .await
            .err()
            .expect("a placement must not re-key a block outside its target sibling set");
        assert!(
            err.to_string().contains("block:victim"),
            "the refusal must name the offending target, got: {err}"
        );

        assert_eq!(
            read_sort_key(&handle, "block:victim").await,
            "8180",
            "the unrelated block's order key must be untouched"
        );
        assert_eq!(
            read_parent(&handle, "block:mover").await,
            "block:pa",
            "a refused placement writes nothing at all"
        );
    }

    /// Re-key rights require the op to name its parent EXPLICITLY, on the typed
    /// channel too: a batch op that carries re-keys (a `BatchOp` with a
    /// `position`) but no `parent_id` names no placement, so
    /// `prove_rekeys_are_siblings` refuses it — no op inherits root re-key
    /// rights over the ENTIRE top-level set.
    #[tokio::test]
    async fn an_update_carrying_rekeys_but_no_parent_is_refused() {
        use holon_core::OriginTaggedWrites;
        use holon_core::block_ordering::MintedPosition;

        let (_backend, ops, handle) = setup_sql_block_ops().await;
        insert_raw(&handle, "block:page", "sentinel:no_parent", "80").await;
        insert_raw(&handle, "block:target", "sentinel:no_parent", "8180").await;

        let mut params: StorageEntity = std::collections::HashMap::new();
        params.insert(
            "id".into(),
            holon_api::Value::String("block:target".to_string()),
        );
        params.insert(
            "content".into(),
            holon_api::Value::String("edited".to_string()),
        );
        // No parent_id — the op names no placement, yet its position carries a
        // re-key of a top-level page.
        let position = MintedPosition::new(
            "M0".to_string(),
            vec![("block:page".to_string(), "ZZZZ".to_string())],
        );

        let entity = holon_api::EntityName::new(Block::entity_name());
        let err = ops
            .sql_ops
            .execute_batch_with_origin(
                &entity,
                vec![holon_core::BatchOp::placed("update", params, position)],
                holon_core::EventOrigin::Org,
            )
            .await
            .err()
            .expect("an op naming no parent must not inherit root re-key rights");
        assert!(
            err.to_string().contains("names no parent"),
            "the refusal must explain that no parent was named, got: {err}"
        );
        assert_eq!(
            read_sort_key(&handle, "block:page").await,
            "80",
            "the top-level page's order key must be untouched"
        );
    }

    /// LOAD-BEARING (Ruling B, #2): the batch writer ignores a `_order_rekeys`
    /// key sitting in an op's params when its typed `position` is `None` — the
    /// exact shape the Loro→SQL projection produces for an untrusted PEER block
    /// (the consolidator always builds `BatchOp::data`, position `None`).
    ///
    /// RED before this lane: the batch decoded the params key and, because the
    /// peer create is root-level and so is the victim, the re-key PASSED the
    /// same-parent proof and rewrote block:victim to "ZZZZ" — the round-2
    /// root-rekey exploit. GREEN after: the key is inert, the victim untouched.
    #[tokio::test]
    async fn a_projection_batchop_ignores_a_peer_supplied_order_rekeys_key() {
        use holon_core::OriginTaggedWrites;

        let (_backend, ops, handle) = setup_sql_block_ops().await;
        insert_raw(&handle, "block:victim", "sentinel:no_parent", "80").await;

        // A create op as the Loro projection builds it: params may carry ANY
        // peer property (incl. a smuggled `_order_rekeys`); position is None.
        let mut params: StorageEntity = std::collections::HashMap::new();
        params.insert(
            "id".into(),
            holon_api::Value::String("block:peerblock".to_string()),
        );
        params.insert("parent_id".into(), holon_api::Value::Null);
        params.insert(
            "content".into(),
            holon_api::Value::String("peer".to_string()),
        );
        params.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        params.insert(
            "sort_key".into(),
            holon_api::Value::String("90".to_string()),
        );
        params.insert(
            holon_core::block_ordering::ORDER_REKEYS_PARAM.into(),
            holon_api::Value::Object(std::collections::HashMap::from([(
                "block:victim".to_string(),
                holon_api::Value::String("ZZZZ".to_string()),
            )])),
        );

        let entity = holon_api::EntityName::new(Block::entity_name());
        ops.sql_ops
            .execute_batch_with_origin(
                &entity,
                vec![holon_core::BatchOp::data("create", params)],
                holon_core::EventOrigin::Loro,
            )
            .await
            .expect("the projection batch applies; the peer _order_rekeys key is inert");
        assert_eq!(
            read_sort_key(&handle, "block:victim").await,
            "80",
            "a peer-supplied _order_rekeys must NEVER re-key the victim on the projection path"
        );
    }

    /// The guard's non-existent-target arm returns `Err`, never a silent skip:
    /// a re-key naming a row that does not exist is a caller bug, and a
    /// placement that silently proceeded would land in a keyspace it did not
    /// actually re-key.
    #[tokio::test]
    async fn a_typed_rekey_naming_a_nonexistent_target_is_refused() {
        use holon_core::block_ordering::MintedPosition;

        let (_backend, ops, handle) = setup_sql_block_ops().await;
        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        insert_raw(&handle, "block:mine", "block:parent", "80").await;

        let mut fields: StorageEntity = std::collections::HashMap::new();
        fields.insert(
            "id".into(),
            holon_api::Value::String("block:new".to_string()),
        );
        fields.insert(
            "parent_id".into(),
            holon_api::Value::String("block:parent".to_string()),
        );
        fields.insert("content".into(), holon_api::Value::String("hi".to_string()));
        fields.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );

        let position = MintedPosition::new(
            "M0".to_string(),
            vec![("block:ghost".to_string(), "ZZZZ".to_string())],
        );
        let err = ops
            .sql_ops
            .create_row(fields, Some(position))
            .await
            .err()
            .expect("a re-key naming a non-existent target must be refused, not silently skipped");
        assert!(
            err.to_string().contains("not a row in"),
            "the refusal must say the target is not a row, got: {err}"
        );
        assert!(
            !row_exists(&handle, "block:new").await,
            "the refused create must leave no row behind"
        );
    }

    /// ADR 0030 D1: one placement is one transaction.
    ///
    /// `place` writes `parent_id` and `sort_key`. Split across two
    /// transactions, a failure of the second leaves the block re-parented but
    /// still carrying the key it was minted in its OLD parent's sequence — a
    /// position that means nothing among its new siblings.
    ///
    /// The second write is refused at the storage seam by a test-only UNIQUE
    /// index on `sort_key` plus a decoy row squatting on exactly the key the
    /// generator will mint.
    #[tokio::test]
    async fn a_refused_placement_leaves_neither_half_of_the_move_behind() {
        use holon_core::block_ordering::BlockOrdering;
        use holon_core::fractional_index::gen_key_between;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        // The key `place` will mint for "last child of block:pb, after
        // block:sib" — computed here so a decoy can occupy it.
        let minted = gen_key_between(Some("80"), None).expect("mint the target key");
        assert_ne!(minted, "9000", "the decoy key must differ from the mover's");

        insert_raw(&handle, "block:pa", "sentinel:no_parent", "1000").await;
        insert_raw(&handle, "block:pb", "sentinel:no_parent", "2000").await;
        insert_raw(&handle, "block:mover", "block:pa", "9000").await;
        insert_raw(&handle, "block:sib", "block:pb", "80").await;
        insert_raw(&handle, "block:decoy", "sentinel:no_parent", &minted).await;
        handle
            .execute_ddl("CREATE UNIQUE INDEX fault_unique_sort_key ON block_raw(sort_key)")
            .await
            .expect("the storage-seam fault: the second placement write is refused");

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let mover = EntityUri::from_raw("block:mover");
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let pb = EntityUri::from_raw("block:pb");
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let sib = EntityUri::from_raw("block:sib");
        ops.place(&mover, &pb, Some(&sib))
            .await
            .err()
            .expect("the placement must be refused: its key write collides");

        assert_eq!(
            read_parent(&handle, "block:mover").await,
            "block:pa",
            "a refused placement must not leave the block re-parented — parent and position are \
             one placement"
        );
        assert_eq!(
            read_sort_key(&handle, "block:mover").await,
            "9000",
            "a refused placement must leave the original position intact"
        );
    }

    /// The re-key plan for `parent`'s current children, appending one slot.
    async fn rekey_plan_for(ops: &Arc<SqlBlockOperations>, parent: &str) -> Vec<(String, String)> {
        let siblings = ops
            .sibling_keys(parent)
            .await
            .expect("read the sibling keyspace");
        let plan = SqlBlockOperations::plan_rekey_with_slot(&siblings, siblings.len())
            .expect("plan the re-key");
        assert!(
            plan.rekeys.len() >= 2,
            "the fixture must re-key at least two rows for a crash window to exist, got {:?}",
            plan.rekeys
        );
        plan.rekeys
    }

    /// ADR 0030 validation V3, half one: the crash window is NOT benign.
    ///
    /// Applies the re-key plan one row at a time — the durable residue a crash
    /// between two per-row writes leaves — over a sibling set whose existing
    /// keys sort BELOW the freshly minted ones. The first partial write alone
    /// re-orders the parent.
    #[tokio::test]
    async fn a_partially_applied_rekey_misorders_siblings() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        // Minted keys at the bottom of the space, with a tie to force a re-key.
        // `gen_n_keys` spreads its output around the middle, so the new keys
        // land ABOVE these — which is what makes a partial application invert.
        insert_raw(&handle, "block:alpha", "block:parent", "7080").await;
        insert_raw(&handle, "block:beta", "block:parent", "7080").await;
        insert_raw(&handle, "block:gamma", "block:parent", "7180").await;

        let before: Vec<String> = ordered_siblings(&handle, "block:parent")
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            before,
            ["block:alpha", "block:beta", "block:gamma"],
            "precondition: the tie still reads in document order via the id tiebreak"
        );

        let rekeys = rekey_plan_for(&ops, "block:parent").await;
        let (id, key) = &rekeys[0];
        handle
            .execute(
                &format!("UPDATE block_raw SET sort_key = '{key}' WHERE id = '{id}'"),
                vec![],
            )
            .await
            .unwrap_or_else(|e| panic!("apply the first re-key ({id} -> {key}): {e}"));

        let after = ordered_siblings(&handle, "block:parent").await;
        let order: Vec<&str> = after.iter().map(|(id, _)| id.as_str()).collect();
        assert_ne!(
            order,
            before.iter().map(String::as_str).collect::<Vec<_>>(),
            "V3: one of {} re-keys applied and the order is still {after:?} — the fixture no \
             longer exercises the inverting case",
            rekeys.len()
        );
    }

    /// ADR 0030 validation V3, half two: and it is not ALWAYS malignant either.
    ///
    /// The same crash over the common shape — siblings carrying the unkeyed
    /// sentinel `"A0"`, which sorts above every minted key — leaves the order
    /// intact at every prefix. So the mis-ordering is a property of the
    /// keyspaces involved, not of partial application as such; "crash windows
    /// degrade benignly" is not a defence available to the writer.
    #[tokio::test]
    async fn a_partially_applied_rekey_over_unkeyed_siblings_keeps_the_order() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;

        insert_raw(&handle, "block:parent", "sentinel:no_parent", "8180").await;
        // Document order first, second, third — expressed in a keyspace that is
        // not an insertable sequence (two ties, one unkeyed), which is exactly
        // what makes a re-key fire.
        insert_raw(&handle, "block:first", "block:parent", "A0").await;
        insert_raw(&handle, "block:second", "block:parent", "A0").await;
        insert_raw(&handle, "block:third", "block:parent", "A1").await;

        let before: Vec<String> = ordered_siblings(&handle, "block:parent")
            .await
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            before,
            ["block:first", "block:second", "block:third"],
            "precondition: the ties still read in document order via the id tiebreak"
        );

        let rekeys = rekey_plan_for(&ops, "block:parent").await;

        // Every crash point: the writer applies its re-keys in sibling order,
        // so the durable residue of a crash is exactly a PREFIX of the plan.
        for cut in 1..rekeys.len() {
            let (id, key) = &rekeys[cut - 1];
            handle
                .execute(
                    &format!("UPDATE block_raw SET sort_key = '{key}' WHERE id = '{id}'"),
                    vec![],
                )
                .await
                .unwrap_or_else(|e| panic!("apply re-key {cut} ({id} -> {key}): {e}"));

            let after: Vec<(String, String)> = ordered_siblings(&handle, "block:parent").await;
            let order: Vec<&str> = after.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(
                order,
                before.iter().map(String::as_str).collect::<Vec<_>>(),
                "V3: after {cut} of {} re-keys the sibling order is {after:?} — this shape was \
                 expected to survive partial application",
                rekeys.len()
            );
        }
    }
}
