//! Reusable SUT **write** caps backed by the production operation dispatcher.
//!
//! The block-tree write cap ([`SutBlockTreeWrite`]) is *not* a per-component
//! concern: `split_block`/`indent`/`outdent`/`move_up`/`move_down`/`join_block`
//! are the production `block` operations dispatched through
//! [`BackendEngine::execute_operation`] — the same path the keychord handler
//! drives. So instead of hand-forwarding these on every storage component
//! (`SqlProjectionComponent`, the headless frontend, `E2ESut`, …), we define the
//! cap **once** on a thin newtype over the engine and let every component register
//! the same impl.
//!
//! This is the "don't reinvent the wheel" form the γ design intends: a write cap
//! is a thin shim over a *production* operation, single-sourced. As more
//! op-dispatch-backed write caps are decomposed off `SutHandle` (the mutation
//! family, `cycle_task_state`, create/delete), they land here too — one writer,
//! many components.
//!
//! Orphan/layering note: `SutBlockTreeWrite` lives in `holon-pbt-core` (which
//! deliberately depends only on `holon-api`) and `OperationProvider`/`BackendEngine`
//! live in `holon`/`holon-core`, so a blanket `impl<T: OperationProvider>
//! SutBlockTreeWrite for T` is impossible (both foreign). A **local newtype**
//! (`OpDispatchWriter`) carrying a foreign trait impl is the orphan-legal way to
//! single-source it here in `holon-integration-tests`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use holon::api::BackendEngine;
use holon_api::{EntityUri, StorageEntity, Value};
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_pbt_core::capabilities::SutBlockTreeWrite;

/// Shared oracle-synthetic → SUT-real id map (the `doc_uri_map` analog). The
/// composed runner accumulates split reconciliations into it; the writer resolves
/// every incoming id through it before dispatching — exactly `E2ESut::resolve_uri`.
pub type IdResolver = Arc<Mutex<BTreeMap<EntityUri, EntityUri>>>;

/// A `SutBlockTreeWrite` realization that dispatches the production `block`
/// structural operations through a real [`BackendEngine`]. `&self` (the engine is
/// `Arc`-shared), so it hosts on `CapMap` via the cap's `#[capmap_adapter]` like
/// any read cap. Unlike `MemoryBackendComponent`'s synchronous mirror,
/// `split_block` mints a fresh **real** id (production `uuid::Uuid`), so a runner
/// driving this must reconcile the oracle's synthetic `block::split-N` against the
/// minted id (the EXP-2/3 `ComposedRunner`).
pub struct OpDispatchWriter {
    engine: Arc<BackendEngine>,
    /// Synthetic→real id map. Empty (`new`) ⇒ identity resolution (every id passes
    /// through), which is correct for fixed-id slices. A multi-tick composed runner
    /// over an id-minting backend shares a populated map (`with_resolver`) so a
    /// transition referencing an earlier split's oracle id resolves to the real id.
    resolver: IdResolver,
    /// Optional frontend focus sink. When present (a *booted frontend* config), the
    /// structural ops that production focus-hands-off — `split_block`/`join_block` —
    /// dispatch through the engine's production `dispatch_intent_sync`
    /// (`execute_operation` + `apply_structural_focus`), so the new/merged block
    /// becomes the engine's `focused_block` (and armed caret seed) exactly as the
    /// GPUI/TUI keychord handler does. That mirrors the reference's `set_focus` +
    /// `open_active_editor` on the split target, so a subsequent `TypeChars` types
    /// into the right block (true split-then-type) — no blur workaround needed.
    /// Absent (memory/turso storage-only configs, fixed-id slices) ⇒ the raw
    /// `engine.execute_operation` path, since no frontend focus-handoff exists there.
    focus_sink: Option<Arc<ReactiveEngine>>,
}

impl OpDispatchWriter {
    /// Identity resolution (fixed-id slices: oracle id == store id).
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self {
            engine,
            resolver: Arc::new(Mutex::new(BTreeMap::new())),
            focus_sink: None,
        }
    }

    /// Share a populated id map with the composed runner (id-minting backends).
    pub fn with_resolver(engine: Arc<BackendEngine>, resolver: IdResolver) -> Self {
        Self {
            engine,
            resolver,
            focus_sink: None,
        }
    }

    /// Resolver-sharing writer that ALSO drives the production frontend focus-handoff
    /// for `split_block`/`join_block` through `reactive` (a booted-frontend config), so
    /// the split/merge target becomes the engine's focused block — the composed-write
    /// realization of the frontend split focus-handoff (`apply_structural_focus`).
    pub fn with_resolver_and_focus(
        engine: Arc<BackendEngine>,
        resolver: IdResolver,
        reactive: Arc<ReactiveEngine>,
    ) -> Self {
        Self {
            engine,
            resolver,
            focus_sink: Some(reactive),
        }
    }

    /// Resolve an oracle-space id to its SUT-space id (identity if unmapped).
    fn resolve(&self, id: &EntityUri) -> EntityUri {
        self.resolver
            .lock()
            .expect("resolver lock")
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.clone())
    }

    async fn execute(&self, op: &str, params: StorageEntity) {
        let entity = "block".to_string().into();
        self.engine
            .execute_operation(&entity, op, params)
            .await
            .unwrap_or_else(|e| panic!("block/{op} operation failed: {e}"));
    }

    /// Dispatch a focus-handing-off structural op (`split_block`/`join_block`). With a
    /// frontend focus sink this goes through the production `dispatch_intent_sync`
    /// (`execute_operation` + `apply_structural_focus`), so the op-response focus result
    /// moves the engine's `focused_block`/caret-seed onto the new/merged block — the
    /// exact in-process projection the GPUI/TUI frontends apply. Without a sink it is the
    /// plain backend execute (storage-only configs have no frontend focus to hand off).
    async fn execute_structural(&self, op: &str, params: StorageEntity) {
        match &self.focus_sink {
            Some(reactive) => {
                // `OperationIntent::params` is keyed by `String`; `StorageEntity` by
                // `Arc<str>`. Convert at this boundary.
                let intent_params: HashMap<String, Value> = params
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect();
                let intent = OperationIntent::new("block".into(), op.to_string(), intent_params);
                reactive
                    .dispatch_intent_sync(intent)
                    .await
                    .unwrap_or_else(|e| panic!("block/{op} dispatch_intent_sync failed: {e:#}"));
            }
            None => self.execute(op, params).await,
        }
    }

    fn id_only(&self, id: &EntityUri) -> StorageEntity {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(self.resolve(id).to_string()));
        params
    }
}

#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for OpDispatchWriter {
    async fn apply_split_block(&self, id: &EntityUri, position: usize) {
        let mut params = self.id_only(id);
        params.insert("position".into(), Value::Integer(position as i64));
        self.execute_structural("split_block", params).await;
    }

    async fn apply_join_block(&self, id: &EntityUri) {
        // `join_block(id, position)` merges into the previous sibling; the cap trait
        // carries no position, so position 0 is a documented default. Fails loud via
        // `execute` if the op is genuinely dispatched and unregistered.
        let mut params = self.id_only(id);
        params.insert("position".into(), Value::Integer(0));
        self.execute_structural("join_block", params).await;
    }

    async fn apply_indent(&self, id: &EntityUri) {
        self.execute("indent", self.id_only(id)).await;
    }

    async fn apply_outdent(&self, id: &EntityUri) {
        self.execute("outdent", self.id_only(id)).await;
    }

    async fn apply_move_up(&self, id: &EntityUri) {
        self.execute("move_up", self.id_only(id)).await;
    }

    async fn apply_move_down(&self, id: &EntityUri) {
        self.execute("move_down", self.id_only(id)).await;
    }
}
