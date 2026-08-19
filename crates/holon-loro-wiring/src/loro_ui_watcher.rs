//! A Turso-free `watch_ui` that renders a block from a [`BlockQuerySource`]
//! (Loro) snapshot instead of the Turso CDC pipeline.
//!
//! This is the no-Turso counterpart of [`holon::api::ui_watcher::watch_ui`].
//! Where the Turso path subscribes to a structural CDC matview and re-renders
//! via [`holon::api::BlockDomain::render_entity`], this path takes one
//! [`BlockQuerySource::snapshot`] and synthesizes the same two [`UiEvent`]s the
//! reactive engine consumes:
//!
//! 1. [`UiEvent::Structure`] — the render expression for `block_id`.
//! 2. [`UiEvent::Data`] — the block plus its direct children, the same row set
//!    the structural query `… WHERE id = X OR parent_id = X` would surface.
//!
//! **Phase 0 (V2 paint-proof gate):** the render expression is a hard-coded
//! placeholder and the watcher is one-shot (no Loro re-snapshot on change). The
//! point this proves is the async `snapshot()` → sync `UiEvent` bridge — the
//! one load-bearing unknown of the slice. Phase 1 replaces the placeholder with
//! a profile-derived render expression and makes the watcher re-emit on Loro
//! change. See `~/.claude/plans/playful-waddling-flute.md`.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use holon::api::block_domain::BlockDomain;
use holon::entity_profile::LiveEntities;
use holon::entity_profile::ProfileResolver;
use holon::entity_profile::ProfileResolving;
use holon_api::EntityUri;
use holon_api::RenderExpr;
use holon_api::UiEvent;
use holon_api::Value;
use holon_api::WatchHandle;
use holon_api::block::Block;
use holon_api::entity::IntoEntity;
use holon_api::streaming::ActorAbortGuard;
use holon_api::streaming::Batch;
use holon_api::streaming::BatchMetadata;
use holon_api::streaming::Change;
use holon_api::streaming::ChangeOrigin;
use holon_api::streaming::WithMetadata;
use holon_core::storage::BlockQuery;
use holon_core::storage::BlockQuerySource;
use tokio::sync::mpsc;

/// Build the Turso-free [`ProfileResolving`] the Loro render path uses to
/// derive collection render expressions.
///
/// The production resolver is fed by a Turso matview watching `PROFILE_SQL`
/// (user-authored profile blocks); a no-Turso session has no such matview. We
/// seed a resolver from the **built-in** type profiles only (the same
/// `create_default_registry` → `profile_from_type_def` path the Turso engine
/// uses), backed by an empty profile source. The result carries the built-in
/// `collection` variants (tree/table/board) so `collection_render_from_profile`
/// resolves a real `view_mode_switcher`, minus user-defined profile overrides.
///
/// Its entity lookups (`query_source`, `rule_sibling`) come from `source`
/// instead of the Turso CDC matviews — without them every lookup-dependent
/// computed field (`has_query_source`, `is_program`) would sit at `Null` in a
/// Loro-only session.
///
/// Must be called from within a Tokio runtime:
/// `ProfileResolver::with_type_profiles` spawns a background actor that watches
/// the (empty) profile source, and the entity refresh below spawns its own.
pub fn build_turso_free_profile_resolver(
    source: Arc<dyn BlockQuerySource>,
) -> Arc<dyn ProfileResolving> {
    let type_registry = holon_profiles::create_default_registry().expect("default TypeRegistry");
    let type_profiles = holon_profiles::type_profiles_from_registry(&type_registry);

    let empty_profiles = holon_api::live_data::LiveData::new(
        Vec::new(),
        |_| Ok(String::new()),
        |_| anyhow::bail!("no-Turso session has no user profile source"),
    );

    let resolver = Arc::new(ProfileResolver::with_type_profiles(
        empty_profiles,
        holon_api::UiInfo::default(),
        LiveEntities::new(),
        HashMap::new(),
        type_profiles,
    ));
    spawn_live_entity_refresh(source, Arc::downgrade(&resolver));
    resolver
}

/// Keep a Turso-free resolver's entity lookups in step with the Loro tree.
///
/// The Turso arm keeps these live off CDC; a no-Turso session has no CDC, so
/// liveness is poll-based exactly like [`loro_watch_ui`] itself — a session
/// boots before its content is loaded, so a one-shot read would answer "no
/// query source" forever. The task holds only a `Weak`, so it ends when the
/// session drops its resolver.
fn spawn_live_entity_refresh(
    source: Arc<dyn BlockQuerySource>,
    resolver: std::sync::Weak<ProfileResolver>,
) {
    tokio::spawn(async move {
        let mut previous: Option<SourceBlockKey> = None;
        let mut polled_version: Option<u64> = None;
        loop {
            let Some(resolver) = resolver.upgrade() else {
                return;
            };
            // Two gates, cheapest first: the substrate's own version skips the
            // whole tree walk while the doc is idle, the key skips the engine
            // rebuild when the walk found nothing the lookups read.
            let version = source.change_version();
            if version.is_none() || version != polled_version {
                match source.snapshot().await {
                    Ok(snapshot) => {
                        let current = source_block_key(&snapshot);
                        if previous.as_ref() != Some(&current) {
                            let entities = holon_profiles::LiveEntitySpec::ALL
                                .iter()
                                .map(|spec| {
                                    (
                                        spec.entity_name(),
                                        spec.live_data_from_blocks(snapshot.iter_blocks()),
                                    )
                                })
                                .collect();
                            resolver.set_live_entities(entities);
                            previous = Some(current);
                        }
                        polled_version = version;
                    }
                    Err(e) => tracing::error!(
                        "[LoroLiveEntities] snapshot failed; entity lookups are stale: {e:#}"
                    ),
                }
            }
            drop(resolver);
            tokio::time::sleep(LORO_WATCH_POLL).await;
        }
    });
}

/// Every source block's id, parent, content type and language — sorted.
type SourceBlockKey = Vec<(String, String, String, String)>;

/// Exactly what the lookups read:
/// [`LiveEntitySpec`](holon_profiles::LiveEntitySpec) selects on `content_type`
/// AND `source_language`, and keys on `parent_id`, so two snapshots agreeing
/// here cannot disagree on any lookup. The refresh above rebuilds only when
/// this changes.
fn source_block_key(snapshot: &holon_core::storage::BlockSnapshot) -> SourceBlockKey {
    let mut key: SourceBlockKey = snapshot
        .iter_blocks()
        .filter_map(|b| b.source_language.as_ref().map(|lang| (b, lang)))
        .map(|(b, lang)| {
            (
                b.id.as_str().to_string(),
                b.parent_id.as_str().to_string(),
                b.content_type.to_string(),
                lang.to_string(),
            )
        })
        .collect();
    key.sort();
    key
}

/// How often the Loro watcher re-snapshots to detect tree changes. The
/// [`BlockQuerySource`] trait exposes only `snapshot()` (no push signal), so
/// liveness here is poll-based — the same shape as the PBT settle barriers.
const LORO_WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// Build a [`WatchHandle`] that renders `block_id` from a pure-Loro
/// [`BlockQuerySource`], **re-emitting** whenever the Loro tree changes.
///
/// The watcher polls `source.snapshot()` and emits a fresh `UiEvent::Structure`
/// when the derived render expression changes and a `UiEvent::Data` delta
/// (Created / Updated / Deleted) whenever the structural row set changes. The
/// PBT/slice drives mutations into the Loro tree, so without this re-emit a
/// window would go stale after the first edit (ADR 0004 Phase 9, part (a) —
/// re-emit-on-change was deferred from b3's one-shot).
///
/// The command channel is wired but inert (no variant switching in the Loro
/// path yet). Dropping the returned handle aborts the poll task.
pub async fn loro_watch_ui(
    source: Arc<dyn BlockQuerySource>,
    block_id: EntityUri,
    advice_status: holon_advice::AdviceRuleStatusHandle,
) -> Result<WatchHandle> {
    let (output_tx, output_rx) = mpsc::channel(64);
    let (command_tx, _command_rx) = mpsc::channel(16);
    let mut aborts = ActorAbortGuard::new();

    let task = tokio::spawn(async move {
        run_watch_loop(source, block_id, advice_status, output_tx).await;
    });
    aborts.push(task.abort_handle());

    Ok(WatchHandle::with_aborts(output_rx, command_tx, aborts))
}

/// The no-Turso arm of the [`UiWatcher`](holon::api::ui_watcher::UiWatcher)
/// capability: renders a block's UI from a [`BlockQuerySource`] snapshot via
/// [`loro_watch_ui`]. The Turso arm is `BackendEngine`'s own `UiWatcher` impl
/// (CDC pipeline). Both are present in their respective sessions so `watch_ui`
/// dispatches through the capability with no backend branch.
pub struct LoroUiWatcher {
    source: Arc<dyn BlockQuerySource>,
    /// Advice-rule status surface (ADR 0022). Empty in a no-Turso session (no
    /// advice reconciler synthesizes matviews there), but wired for parity
    /// with the Turso watcher so a rule block's error would render in place
    /// if one is ever recorded.
    advice_status: holon_advice::AdviceRuleStatusHandle,
}

impl LoroUiWatcher {
    pub fn new(source: Arc<dyn BlockQuerySource>) -> Self {
        Self {
            source,
            advice_status: holon_advice::AdviceRuleStatusHandle::new(),
        }
    }

    /// Wire a shared advice-rule status handle (the reader side of ADR 0022's
    /// surface).
    pub fn with_advice_status(mut self, status: holon_advice::AdviceRuleStatusHandle) -> Self {
        self.advice_status = status;
        self
    }
}

#[async_trait::async_trait]
impl holon::api::ui_watcher::UiWatcher for LoroUiWatcher {
    async fn watch_ui(self: Arc<Self>, block_id: EntityUri) -> Result<WatchHandle> {
        loro_watch_ui(
            Arc::clone(&self.source),
            block_id,
            self.advice_status.clone(),
        )
        .await
    }
}

fn local_origin() -> ChangeOrigin {
    ChangeOrigin::Local {
        operation_id: None,
        trace_id: None,
    }
}

/// Poll the Loro source and (re)emit Structure + Data deltas until the output
/// channel closes (the consumer dropped the handle) or the task is aborted.
async fn run_watch_loop(
    source: Arc<dyn BlockQuerySource>,
    block_id: EntityUri,
    advice_status: holon_advice::AdviceRuleStatusHandle,
    output_tx: mpsc::Sender<UiEvent>,
) {
    let mut generation: u64 = 0;
    let mut seq: u64 = 0;
    let mut prev_expr: Option<RenderExpr> = None;
    // Last emitted structural rows, keyed by id (for delta computation).
    let mut prev_rows: HashMap<EntityUri, holon_api::StorageEntity> = HashMap::new();

    loop {
        // A snapshot failure is surfaced as an error render expr that flows
        // through the same diff path, so the watcher recovers automatically
        // when the underlying block is fixed (matches the Turso watcher's
        // error-recovery semantics) — it never silently stalls.
        // Advice-rule status surface (ADR 0022): a NON-Active rule block renders its
        // error in place. Inert in a no-Turso session (the map is empty there).
        let (expr, ordered) = match advice_status.get(block_id.as_str()) {
            Some(status) if !status.is_active() => (
                error_render_expr(&format!("advice rule: {status}")),
                Vec::new(),
            ),
            _ => match render_state(&source, &block_id).await {
                Ok(state) => state,
                Err(e) => {
                    tracing::warn!("[LoroWatcher] render of '{block_id}' failed: {e}");
                    (error_render_expr(&format!("{e:#}")), Vec::new())
                }
            },
        };

        let structure_changed = prev_expr.as_ref() != Some(&expr);
        if structure_changed {
            generation += 1;
            if output_tx
                .send(UiEvent::Structure {
                    render_expr: expr.clone(),
                    candidates: Vec::new(),
                    generation,
                })
                .await
                .is_err()
            {
                return;
            }
        }

        // Compute the Data delta in canonical order (block first, then
        // `children_ordered`). On a structure change the generation bumped, so
        // re-assert the full current set under the new generation; otherwise
        // emit only what changed.
        let new_ids: std::collections::HashSet<&EntityUri> =
            ordered.iter().map(|(id, _)| id).collect();
        let mut items: Vec<Change<HashMap<String, Value>>> = Vec::new();
        for id in prev_rows.keys() {
            if !new_ids.contains(id) {
                items.push(Change::Deleted {
                    id: id.to_string(),
                    origin: local_origin(),
                });
            }
        }
        for (id, row) in &ordered {
            // UiEvent batches are an frb surface (String-keyed rows) — re-key
            // the Arc<str> storage row at the emit boundary.
            let data_row = || {
                row.iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect()
            };
            let change = if structure_changed || !prev_rows.contains_key(id) {
                Some(Change::Created {
                    data: data_row(),
                    origin: local_origin(),
                })
            } else if prev_rows.get(id) != Some(row) {
                Some(Change::Updated {
                    id: id.to_string(),
                    data: data_row(),
                    origin: local_origin(),
                })
            } else {
                None
            };
            if let Some(change) = change {
                items.push(change);
            }
        }

        if !items.is_empty() {
            seq += 1;
            let batch = WithMetadata {
                inner: Batch { items },
                metadata: BatchMetadata {
                    relation_name: "loro_structural".to_string(),
                    trace_context: None,
                    linked_contexts: Vec::new(),
                    sync_token: None,
                    seq,
                    degraded: None,
                },
            };
            if output_tx
                .send(UiEvent::Data { batch, generation })
                .await
                .is_err()
            {
                return;
            }
        }

        prev_expr = Some(expr);
        prev_rows = ordered.into_iter().collect();

        if output_tx.is_closed() {
            return;
        }
        tokio::time::sleep(LORO_WATCH_POLL).await;
    }
}

/// Capture one snapshot and derive the render expression plus the structural
/// row set (block + its direct children, in canonical sibling order — exactly
/// what `… WHERE id = X OR parent_id = X` produces under Turso).
async fn render_state(
    source: &Arc<dyn BlockQuerySource>,
    block_id: &EntityUri,
) -> Result<(RenderExpr, Vec<(EntityUri, holon_api::StorageEntity)>)> {
    let snapshot = source
        .snapshot()
        .await
        .map_err(|e| anyhow::anyhow!("loro_watch_ui: snapshot failed: {e}"))?;

    let render_expr = derive_render_expr(&snapshot, block_id);

    let mut blocks: Vec<Block> = Vec::new();
    if let Some(block) = snapshot.block_by_id(block_id) {
        blocks.push(block);
    }
    blocks.extend(snapshot.children_ordered(block_id));

    let ordered = blocks
        .iter()
        .map(|block| (block.id.clone(), block_to_row(block)))
        .collect();

    Ok((render_expr, ordered))
}

/// Derive the render expression for `block_id` from the snapshot — the
/// Turso-free counterpart of [`BlockDomain::render_entity`]'s template
/// derivation.
///
/// The Turso path reads the query-source / render-source children via
/// `block_with_query_source.sql`; here we read the same children straight from
/// the snapshot (those joins are just "children of `block_id` that are source
/// blocks").
///
/// **Capability-driven degradation (ADR 0004 Phase 9).** The data-rendering
/// view modes (tree/table/board) render *query results*, which only the Turso
/// query engine can produce. A no-Turso session has no query engine, so those
/// modes are not offered: a query-source block degrades to the one view mode
/// that needs no engine — `source`, the raw query text — rendered bare (no
/// view-mode-switcher chrome, since it is the only mode). The Turso path keeps
/// the full tree/table/board + source switcher via
/// [`BlockDomain::render_entity`].
///
/// A block with no query-source child is a leaf, rendered via `render_entity()`
/// (mirrors `BlockDomain::render_leaf_block`).
fn derive_render_expr(
    snapshot: &holon_core::storage::BlockSnapshot,
    block_id: &EntityUri,
) -> RenderExpr {
    // ROOT display slot: resolved via a query over data (the
    // active-perspective pointer is the degenerate slot query) — the no-Turso
    // counterpart of `BlockDomain::render_root_slot`.
    if *block_id == holon_api::root_layout_block_uri() {
        return derive_root_slot_expr(snapshot, block_id);
    }

    let children = snapshot.children_ordered(block_id);

    let query_child = children.iter().find_map(|child| {
        child
            .source_language
            .as_ref()
            .and_then(|lang| lang.as_query())
            .map(|query_language| (child, query_language))
    });

    let Some((query_src, query_language)) = query_child else {
        // Leaf block: no query-source child to drive a collection render.
        return RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };
    };

    // No query engine in this wiring → offer only the `source` view mode.
    BlockDomain::source_editor_expr(&query_src.content, query_language)
}

/// Resolve the ROOT display slot from the snapshot: follow the
/// active-perspective pointer on the root-layout block and synthesize the
/// layout from the resolved perspective's panels. The poll loop re-derives on
/// every Loro change, so a pointer `set_field` (or a panel edit) re-fires
/// this naturally.
///
/// A snapshot without a root-layout block renders as a leaf (mirrors the
/// Turso arm's disclosed degraded arm); a broken pointer/perspective fails loud
/// as a red `error(...)` node.
fn derive_root_slot_expr(
    snapshot: &holon_core::storage::BlockSnapshot,
    root_id: &EntityUri,
) -> RenderExpr {
    use holon_api::perspective;

    let Some(root_block) = snapshot.block_by_id(root_id) else {
        tracing::warn!(
            "[derive_root_slot_expr] no {root_id} block in this snapshot — rendering the root \
             slot as a plain leaf (no layout to resolve)"
        );
        return RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };
    };

    let active =
        match perspective::active_perspective_id(root_id, std::slice::from_ref(&root_block)) {
            Ok(active) => active,
            Err(e) => return error_render_expr(&format!("root slot: {e:#}")),
        };

    let mut blocks = vec![root_block];
    if let Some(persp) = snapshot.block_by_id(&active)
        && active != *root_id
    {
        blocks.push(persp);
    }
    blocks.extend(snapshot.descendants_ordered(&active));

    match perspective::resolve_active_perspective(root_id, &blocks)
        .and_then(|spec| spec.layout_expr())
    {
        Ok(expr) => expr,
        Err(e) => error_render_expr(&format!("root slot: {e:#}")),
    }
}

/// Convert a [`Block`] into the row map the reactive engine consumes.
///
/// `Block::to_entity().fields` is the canonical field→`Value` map (the same one
/// the storage layer writes to `block_raw`), so it cannot drift from the column
/// shape the renderer expects. `properties` is flattened to top-level keys to
/// match [`holon_api::widget_spec::EnrichedRow`]'s `flatten_properties`.
fn block_to_row(block: &Block) -> HashMap<Arc<str>, Value> {
    let mut row: HashMap<Arc<str>, Value> = block.to_entity().fields;
    if let Some(Value::Object(props)) = row.get("properties").cloned() {
        for (key, value) in props {
            row.entry(Arc::from(key)).or_insert(value);
        }
    }
    row
}

/// Mirror of `ui_watcher::error_render_expr` for the Loro path.
fn error_render_expr(message: &str) -> RenderExpr {
    use holon_api::render_types::Arg;
    RenderExpr::FunctionCall {
        name: "error".to_string(),
        args: vec![Arg {
            name: Some("message".to_string()),
            value: RenderExpr::Literal {
                value: Value::String(message.to_string()),
            },
        }],
    }
}

#[cfg(test)]
mod tests {
    use holon::api::repository::CoreOperations;
    use holon::api::repository::Lifecycle;
    use holon_api::BlockContent;
    use holon_loro::LoroBackend;

    use super::*;
    use crate::loro_block_query_source::LoroBlockQuerySource;

    /// The V2 paint-proof gate, headless at the `watch_ui` seam: a pure-Loro
    /// `BlockQuerySource` snapshot drives the same `UiEvent::Structure` +
    /// `UiEvent::Data` the reactive engine consumes — with no Turso engine.
    ///
    /// With no query engine in this wiring, a query-source block degrades to
    /// the `source` view mode only (ADR 0004 Phase 9): the Structure is a
    /// bare `source_editor` (the raw query text), not a tree/table/board
    /// switcher — those modes render query *results*, which need the Turso
    /// engine.
    #[tokio::test]
    async fn loro_watch_ui_emits_source_render_then_children_data() {
        let backend = LoroBackend::create_new("loro-watch-ui-test".to_string())
            .await
            .unwrap();
        let root = backend
            .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
            .await
            .unwrap();
        let first = backend
            .create_block(root.id.clone(), BlockContent::text("first"), None)
            .await
            .unwrap();
        let second = backend
            .create_block(root.id.clone(), BlockContent::text("second"), None)
            .await
            .unwrap();
        // A query-source child makes `root` a query block. With no query engine,
        // derive_render_expr degrades to the `source` view mode only.
        backend
            .create_block(
                root.id.clone(),
                BlockContent::source("holon_prql", "from blocks"),
                None,
            )
            .await
            .unwrap();

        let source: Arc<dyn BlockQuerySource> =
            Arc::new(LoroBlockQuerySource::new(Arc::new(backend)));

        let mut handle = loro_watch_ui(
            source,
            root.id.clone(),
            holon_advice::AdviceRuleStatusHandle::new(),
        )
        .await
        .unwrap();

        // First event: the source-only degradation — a bare `source_editor`
        // showing the query text, not a tree/table/board switcher.
        let structure = handle.recv().await.expect("expected a Structure event");
        match structure {
            UiEvent::Structure { render_expr, .. } => match render_expr {
                RenderExpr::FunctionCall { name, args } => {
                    assert_eq!(
                        name, "source_editor",
                        "a query block with no engine must degrade to the bare source view"
                    );
                    let content = args.iter().find_map(|a| {
                        (a.name.as_deref() == Some("content")).then(|| match &a.value {
                            RenderExpr::Literal { value } => value.as_string().map(String::from),
                            _ => None,
                        })
                    });
                    assert_eq!(
                        content.flatten().as_deref(),
                        Some("from blocks"),
                        "source view must carry the raw query text"
                    );
                }
                other => panic!("expected a FunctionCall render expr, got {other:?}"),
            },
            other => panic!("expected Structure first, got {other:?}"),
        }

        // Second event: Data with the block + its direct children (the structural
        // `id = X OR parent_id = X` row set), in canonical sibling order.
        let data = handle.recv().await.expect("expected a Data event");
        let UiEvent::Data { batch, .. } = data else {
            panic!("expected Data second, got {data:?}");
        };
        let ids: Vec<String> = batch
            .inner
            .items
            .iter()
            .filter_map(|c| match c {
                Change::Created { data, .. } => {
                    data.get("id").and_then(|v| v.as_string()).map(String::from)
                }
                _ => None,
            })
            .collect();
        // root first, then its children (text children + the query-source child).
        assert_eq!(ids.first().map(String::as_str), Some(root.id.as_str()));
        assert!(
            ids.contains(&first.id.as_str().to_string())
                && ids.contains(&second.id.as_str().to_string()),
            "Data rows must include the block's direct children; got {ids:?}"
        );

        // Row shape fidelity: content + parent_id survive the Block→row mapping.
        let first_row = batch
            .inner
            .items
            .iter()
            .find_map(|c| match c {
                Change::Created { data, .. }
                    if data.get("id").and_then(|v| v.as_string()) == Some(first.id.as_str()) =>
                {
                    Some(data.clone())
                }
                _ => None,
            })
            .expect("first child row present");
        assert_eq!(
            first_row.get("content").and_then(|v| v.as_string()),
            Some("first")
        );
        assert_eq!(
            first_row.get("parent_id").and_then(|v| v.as_string()),
            Some(root.id.as_str())
        );
    }

    /// The PBT/slice drives mutations into the Loro tree; the watcher must
    /// **re-emit** so the UI tracks them (re-emit-on-change, deferred from b3's
    /// one-shot). Seed `{root, first}`, then add `second` and delete `first`,
    /// asserting the watcher's Data deltas converge the live row set each time.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loro_watch_ui_reemits_on_loro_churn() {
        use std::collections::HashSet;
        use std::time::Duration;
        use std::time::Instant;

        let backend = Arc::new(
            LoroBackend::create_new("loro-watch-churn-test".to_string())
                .await
                .unwrap(),
        );
        let root = backend
            .create_block(EntityUri::no_parent(), BlockContent::text("root"), None)
            .await
            .unwrap();
        let first = backend
            .create_block(root.id.clone(), BlockContent::text("first"), None)
            .await
            .unwrap();

        let source: Arc<dyn BlockQuerySource> =
            Arc::new(LoroBlockQuerySource::new(backend.clone()));
        let mut handle = loro_watch_ui(
            source,
            root.id.clone(),
            holon_advice::AdviceRuleStatusHandle::new(),
        )
        .await
        .unwrap();

        // Fold the watcher's generation-gated Structure/Data deltas into a live
        // id set, exactly as `ReactiveRowSet` does, until `want` holds.
        let mut current_gen: u64 = 0;
        let mut ids: HashSet<String> = HashSet::new();
        async fn drain_until(
            handle: &mut WatchHandle,
            current_gen: &mut u64,
            ids: &mut HashSet<String>,
            label: &str,
            want: impl Fn(&HashSet<String>) -> bool,
        ) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if want(ids) {
                    return;
                }
                match tokio::time::timeout(Duration::from_millis(100), handle.recv()).await {
                    Ok(Some(UiEvent::Structure { generation, .. })) => *current_gen = generation,
                    Ok(Some(UiEvent::Data { batch, generation })) => {
                        if generation == *current_gen {
                            for change in batch.inner.items {
                                match change {
                                    Change::Created { data, .. } | Change::Updated { data, .. } => {
                                        if let Some(id) = data.get("id").and_then(|v| v.as_string())
                                        {
                                            ids.insert(id.to_string());
                                        }
                                    }
                                    Change::Deleted { id, .. } => {
                                        ids.remove(&id);
                                    }
                                    Change::FieldsChanged { .. } => {}
                                }
                            }
                        }
                    }
                    Ok(None) => break, // channel closed
                    Err(_) => {}       // poll timeout — re-check `want`
                }
            }
            assert!(
                want(ids),
                "[{label}] watcher never converged the row set; got {ids:?}"
            );
        }

        let r = root.id.as_str().to_string();
        let f = first.id.as_str().to_string();
        drain_until(&mut handle, &mut current_gen, &mut ids, "initial", |ids| {
            ids.contains(&r) && ids.contains(&f) && ids.len() == 2
        })
        .await;

        // Churn 1: add a child — the watcher must re-emit it.
        let second = backend
            .create_block(root.id.clone(), BlockContent::text("second"), None)
            .await
            .unwrap();
        let s = second.id.as_str().to_string();
        let (r1, f1, s1) = (r.clone(), f.clone(), s.clone());
        drain_until(
            &mut handle,
            &mut current_gen,
            &mut ids,
            "after-add",
            move |ids| {
                ids.contains(&r1) && ids.contains(&f1) && ids.contains(&s1) && ids.len() == 3
            },
        )
        .await;

        // Churn 2: delete a child — the watcher must emit a Deleted delta.
        backend.delete_block(first.id.as_str()).await.unwrap();
        let (r2, f2, s2) = (r.clone(), f.clone(), s.clone());
        drain_until(
            &mut handle,
            &mut current_gen,
            &mut ids,
            "after-delete",
            move |ids| {
                ids.contains(&r2) && ids.contains(&s2) && !ids.contains(&f2) && ids.len() == 2
            },
        )
        .await;
    }

    /// The user-chosen render fidelity: the Turso-free resolver must actually
    /// carry the built-in `collection` variants, so
    /// `collection_render_from_profile` resolves a real multi-mode switcher
    /// rather than degrading to bare `table()`.
    #[tokio::test]
    async fn turso_free_resolver_has_collection_variants() {
        let source = Arc::new(holon_core::storage::from_sync(|| {
            Ok(holon_core::storage::BlockSnapshot::from_ordered(
                Vec::new(),
                Vec::new(),
            ))
        })) as Arc<dyn BlockQuerySource>;
        let resolver = build_turso_free_profile_resolver(source);
        let variants = resolver.resolve_collection_variants();
        assert!(
            !variants.is_empty(),
            "Turso-free resolver must seed the built-in collection variants (tree/table/board); \
             got none — render would degrade to table()"
        );
    }

    // ── Root display slot resolution (no-Turso arm) ────────────────────────

    use holon_core::storage::BlockSnapshot;

    fn snap_block(id: &str, parent: &EntityUri) -> holon_api::Block {
        holon_api::Block::new_text(EntityUri::block(id), parent.clone(), id)
    }

    fn sql_source(id: &str, parent: &str, query: &str) -> holon_api::Block {
        let mut b =
            holon_api::Block::new_text(EntityUri::block(id), EntityUri::block(parent), query);
        b.source_language = Some(holon_api::SourceLanguage::Query(
            holon_api::QueryLanguage::HolonSql,
        ));
        b
    }

    fn expr_debug(expr: &RenderExpr) -> String {
        format!("{expr:?}")
    }

    /// Default (no pointer): the root slot resolves to the root-layout block's
    /// own panels — the degenerate perspective — and synthesizes the columns
    /// layout from them.
    #[test]
    fn root_slot_default_synthesizes_layout_from_root_children() {
        let root_uri = holon_api::root_layout_block_uri();
        let root = holon_api::Block::new_text(root_uri.clone(), EntityUri::no_parent(), "layout");
        let main = snap_block("default-main-panel", &root_uri);
        let src = sql_source("main-src", "default-main-panel", "SELECT 1");
        let snapshot = BlockSnapshot::from_ordered(vec![root, main, src], vec![]);

        let expr = derive_render_expr(&snapshot, &root_uri);
        let dbg = expr_debug(&expr);
        assert!(
            dbg.contains("if_space") && dbg.contains("block:default-main-panel"),
            "expected synthesized layout with the main panel, got: {dbg}"
        );
    }

    /// Pointer set: the slot resolves the pointed-to perspective's panels
    /// instead of the root-layout's own children — switching is pure data.
    #[test]
    fn root_slot_follows_active_perspective_pointer() {
        let root_uri = holon_api::root_layout_block_uri();
        let mut root =
            holon_api::Block::new_text(root_uri.clone(), EntityUri::no_parent(), "layout");
        holon_api::perspective::set_active_perspective(&mut root, &EntityUri::block("tasks"));

        let old_main = snap_block("default-main-panel", &root_uri);
        let old_src = sql_source("old-src", "default-main-panel", "SELECT 1");

        let tasks =
            holon_api::Block::new_text(EntityUri::block("tasks"), EntityUri::no_parent(), "Tasks");
        let tasks_main = snap_block("tasks-main-panel", &EntityUri::block("tasks"));
        let tasks_src = sql_source("tasks-src", "tasks-main-panel", "SELECT 2");

        let snapshot = BlockSnapshot::from_ordered(
            vec![root, old_main, old_src, tasks, tasks_main, tasks_src],
            vec![],
        );

        let expr = derive_render_expr(&snapshot, &root_uri);
        let dbg = expr_debug(&expr);
        assert!(
            dbg.contains("block:tasks-main-panel"),
            "expected the pointed-to perspective's panel, got: {dbg}"
        );
        assert!(
            !dbg.contains("block:default-main-panel"),
            "root-layout's own panels must NOT render when the pointer targets another \
             perspective, got: {dbg}"
        );
    }

    /// A dangling pointer fails loud as a visible error node, never a silent
    /// degrade to the default layout.
    #[test]
    fn root_slot_dangling_pointer_renders_error_node() {
        let root_uri = holon_api::root_layout_block_uri();
        let mut root =
            holon_api::Block::new_text(root_uri.clone(), EntityUri::no_parent(), "layout");
        holon_api::perspective::set_active_perspective(&mut root, &EntityUri::block("gone"));
        let main = snap_block("default-main-panel", &root_uri);
        let src = sql_source("main-src", "default-main-panel", "SELECT 1");
        let snapshot = BlockSnapshot::from_ordered(vec![root, main, src], vec![]);

        let expr = derive_render_expr(&snapshot, &root_uri);
        let RenderExpr::FunctionCall { name, .. } = &expr else {
            panic!("expected error FunctionCall, got: {expr:?}");
        };
        assert_eq!(name, "error", "dangling pointer must render loud: {expr:?}");
    }
}
