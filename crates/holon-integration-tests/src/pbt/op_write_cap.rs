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
use std::time::Duration;

use holon::api::BackendEngine;
use holon_api::{EntityUri, StorageEntity, Value};
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::user_driver::UserDriver;
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

/// VM-rung **keystroke-driven** `SutBlockTreeWrite` (LL-3, §8.11). Structural
/// mutations are driven through the production [`UserDriver`]'s editor-keystroke
/// pipeline — the UI-adjacent interaction layer — NOT raw op dispatch. This is the
/// "drive interactions UI-adjacent, even headless" directive applied to structural
/// edits: the construction-time-installed driver (here a `ReactiveEngineDriver`
/// over the booted frontend) IS what performs the split, so a bug in the
/// keystroke→intent→reducer path reproduces here and localizes to the VM layer.
///
/// `apply_split_block` = focus the block's editor (`click_entity`, exactly the
/// production focus-on-click — `HeadlessFrontendComponent::apply_focus_editable_text`
/// IS `click_entity`) + `home` + N×`right` + `Enter`. The `HeadlessEditorMirror`
/// maps `Enter` (no slash match) to `split_block` at the live cursor, so the split
/// lands at the same byte the user's caret sits on — the same physical sequence
/// `E2ESut`'s windowed `apply_split_block_input_pipeline` performs, minus the
/// geometry/window-focus prechecks (no platform window headless).
///
/// Ops not yet on the keystroke path (`join`/`indent`/`outdent`/`move_*`) delegate
/// to the inner [`OpDispatchWriter`] — the rebind is incremental (Split first).
pub struct KeystrokeBlockTreeWriter {
    driver: Arc<dyn UserDriver>,
    /// The frontend `ReactiveEngine` (as `BuilderServices`) — read the block's live
    /// `MutableText` content cell for the byte→keystroke conversion (the same source
    /// `editor_live_text` reads; populated by rendering, not the router cache that
    /// `displayed_text` consults).
    reactive: Arc<ReactiveEngine>,
    resolver: IdResolver,
    fallback: OpDispatchWriter,
}

impl KeystrokeBlockTreeWriter {
    /// `driver` drives the keystrokes; `reactive` reads live editor content; `resolver`
    /// translates oracle synthetic ids to the SUT-minted ids (shared with `fallback`'s
    /// resolver); `fallback` dispatches the not-yet-converted ops over the same engine.
    pub fn new(
        driver: Arc<dyn UserDriver>,
        reactive: Arc<ReactiveEngine>,
        resolver: IdResolver,
        fallback: OpDispatchWriter,
    ) -> Self {
        Self {
            driver,
            reactive,
            resolver,
            fallback,
        }
    }

    fn resolve(&self, id: &EntityUri) -> EntityUri {
        self.resolver
            .lock()
            .expect("resolver lock")
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.clone())
    }

    /// Focus/open the block's editor — the production focus-on-click path
    /// (`HeadlessFrontendComponent::apply_focus_editable_text` IS `click_entity`),
    /// so a subsequent keystroke routes to THIS block's editor.
    async fn focus_editor(&self, resolved: &EntityUri, ctx: &str) {
        self.driver
            .click_entity(resolved, "main")
            .await
            .unwrap_or_else(|e| panic!("[{ctx}/keystroke] focus {resolved} failed: {e:#}"));
    }

    /// Send one raw keystroke through the driver, fail-loud with the gesture context.
    async fn key(&self, keystroke: &str, modifiers: &[&str], ctx: &str) {
        self.driver
            .send_raw_keystroke(keystroke, modifiers)
            .await
            .unwrap_or_else(|e| {
                panic!("[{ctx}/keystroke] {keystroke} {modifiers:?} failed: {e:#}")
            });
    }
}

#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for KeystrokeBlockTreeWriter {
    async fn apply_split_block(&self, id: &EntityUri, position: usize) {
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "SplitBlock").await;
        // `position` is a byte offset; each `right` advances one CHAR. Convert against
        // the block's live `MutableText` content cell (the pre-split text the caret
        // walks — the same source `editor_live_text` reads). Populated by rendering, so
        // it is present for any rendered text block (unlike `displayed_text`'s router
        // cache, which is only warm for router-touched blocks).
        let services: &dyn BuilderServices = self.reactive.as_ref();
        // Poll for the content cell: a freshly-created target (e.g. split-of-a-split)
        // may still be landing its Loro `content_raw`. Fail loud at the deadline rather
        // than convert against an absent cell.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let text = loop {
            match services.editable_text(&resolved, "content") {
                Ok(cell) => break cell.current(),
                Err(e) => {
                    if tokio::time::Instant::now() >= deadline {
                        panic!(
                            "[SplitBlock/keystroke] no editable content cell for {resolved} \
                             within 2s — cannot convert byte position {position} to \
                             keystrokes: {e:#}"
                        );
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(
            text.is_char_boundary(position),
            "[SplitBlock/keystroke] position {position} is not a char boundary of {text:?}"
        );
        let right_presses = text[..position].chars().count();
        // Caret to `position`, then Enter → `HeadlessEditorMirror` splits at the caret.
        self.key("home", &[], "SplitBlock").await;
        for _ in 0..right_presses {
            self.key("right", &[], "SplitBlock").await;
        }
        self.key("enter", &[], "SplitBlock").await;
    }

    async fn apply_join_block(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        // Backspace at caret 0 → `join_block` (merge into previous sibling) in the
        // `HeadlessEditorMirror`. Focus the block, home to pin the caret at 0, Backspace.
        self.focus_editor(&resolved, "JoinBlock").await;
        self.key("home", &[], "JoinBlock").await;
        self.key("backspace", &[], "JoinBlock").await;
    }

    async fn apply_indent(&self, id: &EntityUri) {
        // Tab → `indent` in the `HeadlessEditorMirror` (block-level; caret-independent).
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "Indent").await;
        self.key("tab", &[], "Indent").await;
    }

    async fn apply_outdent(&self, id: &EntityUri) {
        // Shift+Tab → `outdent` (BackTab) in the `HeadlessEditorMirror`.
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "Outdent").await;
        self.key("tab", &["shift"], "Outdent").await;
    }

    // `move_up`/`move_down` are block-reorder ops with NO editor-mirror keystroke (they
    // ride the chord path, not text editing). Kept on the dispatch fallback until the
    // chord-resolution rebind (`send_key_chord`) lands.
    async fn apply_move_up(&self, id: &EntityUri) {
        self.fallback.apply_move_up(id).await;
    }

    async fn apply_move_down(&self, id: &EntityUri) {
        self.fallback.apply_move_down(id).await;
    }
}
