//! Reusable SUT **write** caps backed by the production operation dispatcher.
//!
//! The block-tree write cap ([`SutBlockTreeWrite`]) is *not* a per-component
//! concern: `split_block`/`indent`/`outdent`/`move_up`/`move_down`/`join_block`
//! are the production `block` operations dispatched through
//! [`BackendEngine::execute_operation`] — the same path the keychord handler
//! drives. So instead of hand-forwarding these on every storage component
//! (`SqlProjectionComponent`, the headless frontend, `E2ESut`, …), we define
//! the cap **once** on a thin newtype over the engine and let every component
//! register the same impl.
//!
//! This is the "don't reinvent the wheel" form the γ design intends: a write
//! cap is a thin shim over a *production* operation, single-sourced. As more
//! op-dispatch-backed write caps are decomposed off `SutHandle` (the mutation
//! family, `cycle_task_state`, create/delete), they land here too — one writer,
//! many components.
//!
//! Orphan/layering note: `SutBlockTreeWrite` lives in `holon-pbt-core` (which
//! deliberately depends only on `holon-api`) and
//! `OperationProvider`/`BackendEngine` live in `holon`/`holon-core`, so a
//! blanket `impl<T: OperationProvider> SutBlockTreeWrite for T` is impossible
//! (both foreign). A **local newtype** (`OpDispatchWriter`) carrying a foreign
//! trait impl is the orphan-legal way to single-source it here in
//! `holon-integration-tests`.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon::api::BackendEngine;
use holon_api::EdgeFieldUpdate;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutEdgeFieldWrite;

/// True for the ADR 0028 D1 page-boundary refusal of `outdent` — the op engine
/// declining to move a direct page child out of its page container.
///
/// Every `SutBlockTreeWrite` realization must treat this refusal as a NO-OP,
/// not as a driver failure: it is the correct production outcome (prod shows a
/// `CommandFailed` toast and leaves the tree unchanged) and
/// `outdent_apply_to_ref` models the same no-op. Swallowing it does not blind
/// the oracle — the reference decides page-ness independently, so an engine
/// that refused an outdent the reference DID apply still diverges the tree
/// comparison.
pub fn is_page_boundary_outdent_refusal(msg: &str) -> bool {
    msg.contains("escape its page container")
}

/// Shared oracle-synthetic → SUT-real id map. The composed runner accumulates
/// split reconciliations into it; the writer resolves every incoming id through
/// it before dispatching — exactly `E2ESut::resolve_uri`. A re-export, not a
/// parallel alias: a SUT adapter's `doc_uri_map` IS this map.
pub type IdResolver = holon_pbt_core::types::DocUriMap;

/// A `SutBlockTreeWrite` realization that dispatches the production `block`
/// structural operations through a real [`BackendEngine`]. `&self` (the engine
/// is `Arc`-shared), so it hosts on `CapMap` via the cap's `#[capmap_adapter]`
/// like any read cap. Unlike `MemoryBackendComponent`'s synchronous mirror,
/// `split_block` mints a fresh **real** id (production `uuid::Uuid`), so a
/// runner driving this must reconcile the oracle's synthetic `block::split-N`
/// against the minted id (the EXP-2/3 `ComposedRunner`).
pub struct OpDispatchWriter {
    sink: DispatchSink,
    /// Synthetic→real id map. Empty (`new`) ⇒ identity resolution (every id
    /// passes through), which is correct for fixed-id slices. A multi-tick
    /// composed runner over an id-minting backend shares a populated map
    /// (`with_resolver`) so a transition referencing an earlier split's
    /// oracle id resolves to the real id.
    resolver: IdResolver,
}

/// Which production dispatch seam the writer's ops travel.
///
/// The two are NOT interchangeable for the structural focus movers: only the
/// frontend seam runs `apply_structural_focus`, which reads `split_block` /
/// `join_block`'s focus response (new block, caret 0) and moves the in-memory
/// focus authority — the handoff the desktop app performs on every Enter and
/// the one `SplitBlock::apply_to_ref` mirrors. A writer on the storage seam
/// leaves `focused_block` on the pre-split block, so the next keystroke lands
/// somewhere the oracle never sent it.
enum DispatchSink {
    /// Storage rung: no frontend is booted, so there is no focus authority to
    /// move and the op engine is the whole system under test.
    Storage(Arc<BackendEngine>),
    /// Frontend rung: `dispatch_intent_sync`, the seam GPUI's keychord handler
    /// and MCP `send_key_chord` dispatch through. (MCP's raw-op tools
    /// `execute_operation`/`execute_command` go through `HolonService` below
    /// this seam and deliberately do not move UI focus.)
    Frontend(Arc<ReactiveEngine>),
}

impl OpDispatchWriter {
    /// Identity resolution (fixed-id slices: oracle id == store id).
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self {
            sink: DispatchSink::Storage(engine),
            resolver: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Share a populated id map with the composed runner (id-minting backends).
    pub fn with_resolver(engine: Arc<BackendEngine>, resolver: IdResolver) -> Self {
        Self {
            sink: DispatchSink::Storage(engine),
            resolver,
        }
    }

    /// The frontend-rung writer: same ops, dispatched through the booted
    /// `ReactiveEngine` so the structural focus handoff happens. Used wherever
    /// a frontend exists but the keystroke writer cannot be — SqlOnly, where
    /// `KeystrokeBlockTreeWriter` has no `MutableText` to press against.
    pub fn with_frontend(reactive: Arc<ReactiveEngine>, resolver: IdResolver) -> Self {
        Self {
            sink: DispatchSink::Frontend(reactive),
            resolver,
        }
    }

    /// Resolve an oracle-space id to its SUT-space id.
    fn resolve(&self, id: &EntityUri) -> EntityUri {
        holon_pbt_core::types::resolve_sut_id(&self.resolver, id)
    }

    async fn execute(&self, op: &str, params: StorageEntity) {
        if let Err(e) = self.try_execute(op, params).await {
            panic!("block/{op} operation failed: {e}");
        }
    }

    /// Dispatch without the fail-loud wrapper, for the one caller that has to
    /// inspect a DOCUMENTED refusal ([`is_page_boundary_outdent_refusal`]).
    async fn try_execute(&self, op: &str, params: StorageEntity) -> anyhow::Result<()> {
        let entity: holon_api::EntityName = "block".to_string().into();
        match &self.sink {
            DispatchSink::Storage(engine) => engine
                .execute_operation(&entity, op, params, holon_api::OpOrigin::User)
                .await
                .map(|_| ()),
            DispatchSink::Frontend(reactive) => {
                reactive
                    .dispatch_intent_sync(holon_frontend::operations::OperationIntent::new(
                        entity,
                        op.to_string(),
                        params
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), v))
                            .collect(),
                    ))
                    .await
            }
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
        self.execute("split_block", params).await;
    }

    async fn apply_join_block(&self, id: &EntityUri) {
        // `join_block(id, position)` merges into the previous sibling; the cap trait
        // carries no position, so position 0 is a documented default. Fails loud via
        // `execute` if the op is genuinely dispatched and unregistered.
        let mut params = self.id_only(id);
        params.insert("position".into(), Value::Integer(0));
        self.execute("join_block", params).await;
    }

    async fn apply_indent(&self, id: &EntityUri) {
        self.execute("indent", self.id_only(id)).await;
    }

    async fn apply_outdent(&self, id: &EntityUri) {
        // The dispatch floor sees the ADR 0028 D1 page-boundary refusal RAW (its
        // keystroke twin below sees the same refusal through the driver). It is a
        // modelled no-op, not a failure — see `is_page_boundary_outdent_refusal`.
        // Any OTHER dispatch error stays fail-loud.
        if let Err(e) = self.try_execute("outdent", self.id_only(id)).await {
            let msg = format!("{e:#}");
            assert!(
                is_page_boundary_outdent_refusal(&msg),
                "block/outdent operation failed: {msg}"
            );
        }
    }

    async fn apply_move_up(&self, id: &EntityUri) {
        self.execute("move_up", self.id_only(id)).await;
    }

    async fn apply_move_down(&self, id: &EntityUri) {
        self.execute("move_down", self.id_only(id)).await;
    }
}

/// VM-rung **keystroke-driven** `SutBlockTreeWrite` (LL-3, §8.11). Structural
/// mutations are driven through the production [`UserDriver`]'s
/// editor-keystroke pipeline — the UI-adjacent interaction layer — NOT raw op
/// dispatch. This is the "drive interactions UI-adjacent, even headless"
/// directive applied to structural edits: the construction-time-installed
/// driver (here a `ReactiveEngineDriver` over the booted frontend) IS what
/// performs the split, so a bug in the keystroke→intent→reducer path reproduces
/// here and localizes to the VM layer.
///
/// `apply_split_block` = focus the block's editor (`click_entity`, exactly the
/// production focus-on-click —
/// `HeadlessFrontendComponent::apply_focus_editable_text` IS `click_entity`) +
/// `home` + N×`right` + `Enter`. The `HeadlessEditorMirror` maps `Enter` (no
/// slash match) to `split_block` at the live cursor, so the split lands at the
/// same byte the user's caret sits on — the same physical sequence `E2ESut`'s
/// windowed `apply_split_block_input_pipeline` performs, minus the
/// geometry/window-focus prechecks (no platform window headless).
///
/// The whole structural family rides `driver`:
/// `split`/`join`/`indent`/`outdent` via editor keystrokes, and
/// `move_up`/`move_down` via the production chord-resolution path
/// (`send_key_chord`, C-3 mechanism 3) — the reorder ops have no editor-mirror
/// keystroke, so they resolve their bound chord (Alt+Up / Alt+Down, `reactive.
/// rs`) through `bubble_input` → `ExecuteOperation` exactly as the GPUI
/// page-level chord pump does. No op is left on raw op dispatch.
pub struct KeystrokeBlockTreeWriter {
    driver: Arc<dyn UserDriver>,
    /// The frontend `ReactiveEngine` (as `BuilderServices`) — read the block's
    /// live `MutableText` content cell for the byte→keystroke conversion
    /// (the same source `editor_live_text` reads; populated by rendering,
    /// not the router cache that `displayed_text` consults). Also the
    /// source of truth for the chord registry (`key_bindings()`) and the
    /// reactive root snapshot the chord path needs.
    reactive: Arc<ReactiveEngine>,
    resolver: IdResolver,
}

impl KeystrokeBlockTreeWriter {
    /// `driver` drives the keystrokes and chords; `reactive` reads live editor
    /// content AND the keybinding registry; `resolver` translates oracle
    /// synthetic ids to the SUT-minted ids.
    pub fn new(
        driver: Arc<dyn UserDriver>,
        reactive: Arc<ReactiveEngine>,
        resolver: IdResolver,
    ) -> Self {
        Self {
            driver,
            reactive,
            resolver,
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
    /// (`HeadlessFrontendComponent::apply_focus_editable_text` IS
    /// `click_entity`), so a subsequent keystroke routes to THIS block's
    /// editor.
    async fn focus_editor(&self, resolved: &EntityUri, ctx: &str) {
        self.driver
            .click_entity(resolved, "main")
            .await
            .unwrap_or_else(|e| panic!("[{ctx}/keystroke] focus {resolved} failed: {e:#}"));
    }

    /// Send one raw keystroke through the driver, fail-loud with the gesture
    /// context.
    async fn key(&self, keystroke: &str, modifiers: &[&str], ctx: &str) {
        self.driver
            .send_raw_keystroke(keystroke, modifiers)
            .await
            .unwrap_or_else(|e| {
                panic!("[{ctx}/keystroke] {keystroke} {modifiers:?} failed: {e:#}")
            });
    }

    /// Drive a block-reorder op (`move_up`/`move_down`) through the production
    /// chord-resolution path — the SAME `find_keybinding_for_op` +
    /// `send_key_chord` binding the keystone `E2ESut` uses
    /// (`sut_capabilities.rs`). The chord is read from the live
    /// `key_bindings` registry (prod's Alt+Up / Alt+Down), so the test
    /// follows whatever prod binds; `send_key_chord` clicks-to-focus then
    /// resolves the chord via `bubble_input` → `ExecuteOperation`. The
    /// driver decides HOW (headless router vs window `PlatformInput`), so
    /// both rungs exercise the real chord→intent binding. Fail loud if the
    /// op has no registered chord (a prod regression) or the chord fails to
    /// dispatch.
    async fn send_block_chord(&self, resolved: &EntityUri, op: &str, ctx: &str) {
        let chord = self
            .reactive
            .key_bindings()
            .lock_ref()
            .get(op)
            .cloned()
            .unwrap_or_else(|| panic!("[{ctx}/chord] no keybinding registered for op {op:?}"));
        let root_id = holon_api::root_layout_block_uri();
        let root_tree = self.reactive.snapshot_reactive(&root_id);
        let dispatched = self
            .driver
            .send_key_chord(&root_id, &root_tree, resolved, &chord, HashMap::new())
            .await
            .unwrap_or_else(|e| {
                panic!("[{ctx}/chord] send_key_chord {chord:?} on {resolved} failed: {e:#}")
            });
        assert!(
            dispatched,
            "[{ctx}/chord] chord {chord:?} did not dispatch op {op:?} on {resolved}"
        );
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
                             within 2s — cannot convert byte position {position} to keystrokes: \
                             {e:#}"
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
        // `HeadlessEditorMirror`. Focus the block, home to pin the caret at 0,
        // Backspace.
        self.focus_editor(&resolved, "JoinBlock").await;
        self.key("home", &[], "JoinBlock").await;
        self.key("backspace", &[], "JoinBlock").await;
    }

    async fn apply_indent(&self, id: &EntityUri) {
        // Tab → `indent` in the `HeadlessEditorMirror` (block-level;
        // caret-independent).
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "Indent").await;
        self.key("tab", &[], "Indent").await;
    }

    async fn apply_outdent(&self, id: &EntityUri) {
        // Shift+Tab → `outdent` (BackTab) in the `HeadlessEditorMirror`.
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "Outdent").await;
        // The ADR 0028 D1 page-boundary refusal is a modelled no-op, not a driver
        // failure — see `is_page_boundary_outdent_refusal`. Any OTHER keystroke
        // error stays fail-loud.
        if let Err(e) = self.driver.send_raw_keystroke("tab", &["shift"]).await {
            let msg = format!("{e:#}");
            assert!(
                is_page_boundary_outdent_refusal(&msg),
                "[Outdent/keystroke] tab [\"shift\"] failed: {msg}"
            );
        }
    }

    // `move_up`/`move_down` are block-reorder ops with NO editor-mirror keystroke
    // (they move siblings, they don't edit text). Prod binds them to a key
    // chord (Alt+Up / Alt+Down, `reactive.rs` `key_bindings`), so they ride the
    // SAME chord-resolution path the GPUI page-level chord pump uses —
    // `send_key_chord` clicks-to-focus then dispatches the chord through
    // `bubble_input` → `ExecuteOperation`. This is the C-3 mechanism-3 rebind:
    // the driver (headless `ReactiveEngineDriver` in the base,
    // window `GpuiUserDriver`/`SimUserDriver` in the overlay) resolves the chord
    // itself, so BOTH rungs exercise prod's chord→intent binding — no op left
    // on op dispatch.
    async fn apply_move_up(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.send_block_chord(&resolved, "move_up", "MoveBlockUp")
            .await;
    }

    async fn apply_move_down(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.send_block_chord(&resolved, "move_down", "MoveBlockDown")
            .await;
    }
}

/// `SutEdgeFieldWrite` realization for a Loro-authority composed config (the
/// `full_headless` frontend). Dispatches the edge-field write as a production
/// `block` `set_field` op through the real [`BackendEngine`] — the SAME path
/// content/structural writes take (and the same `OpDispatchWriter` uses) — so
/// the write is journaled on the engine's undo stack and `UndoLastMutation` can
/// retract it. In Loro-authority mode `set_field` routes the edge value to the
/// production `set_block_{tags,requires,advice_suppressed}` setters over the
/// authority doc → `project()` → SQL, exactly as before, PLUS the undo entry
/// (whole-set-restore inverse) the raw-`LoroBackend` path could never record.
///
/// Resolves oracle ids through the shared [`IdResolver`] (like
/// [`OpDispatchWriter`]) so the write — and each `requires` dependency target —
/// hits the real (split-reconciled) block, not a synthetic oracle id.
pub struct EdgeFieldWriter {
    engine: Arc<BackendEngine>,
    resolver: IdResolver,
}

impl EdgeFieldWriter {
    pub fn new(engine: Arc<BackendEngine>, resolver: IdResolver) -> Self {
        Self { engine, resolver }
    }

    fn resolve(&self, id: &EntityUri) -> EntityUri {
        holon_pbt_core::types::resolve_sut_id(&self.resolver, id)
    }

    /// Edge targets travel as a `Value::Array` of id strings — the shape the
    /// `set_field` edge-field partition (SQL provider) and the Loro cell
    /// registry both consume.
    fn resolved_targets(&self, targets: &[EntityUri]) -> Value {
        Value::Array(
            targets
                .iter()
                .map(|t| Value::String(self.resolve(t).to_string()))
                .collect(),
        )
    }
}

#[async_trait::async_trait(?Send)]
impl SutEdgeFieldWrite for EdgeFieldWriter {
    async fn apply_set_edge_field(&self, id: &EntityUri, update: &EdgeFieldUpdate) {
        let rid = self.resolve(id);
        let (field, value) = match update {
            EdgeFieldUpdate::Tags(tags) => (
                "tags",
                Value::Array(tags.to_vec().into_iter().map(Value::String).collect()),
            ),
            EdgeFieldUpdate::Requires(reqs) => ("requires", self.resolved_targets(reqs)),
            EdgeFieldUpdate::AdviceSuppressed(reqs) => {
                ("advice_suppressed", self.resolved_targets(reqs))
            }
        };
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(rid.to_string()));
        params.insert("field".into(), Value::String(field.to_string()));
        params.insert("value".into(), value);
        let entity = "block".to_string().into();
        self.engine
            .execute_operation(&entity, "set_field", params, holon_api::OpOrigin::User)
            .await
            .unwrap_or_else(|e| panic!("block/set_field({field}) on {rid} failed: {e:#}"));
    }
}

#[cfg(test)]
mod split_focus_handoff_tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use holon_pbt_core::capabilities::SutBackend;

    use super::*;
    use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;

    const SETTLE: Duration = Duration::from_millis(400);

    async fn ids_and_contents(comp: &HeadlessFrontendComponent) -> BTreeMap<EntityUri, String> {
        comp.block_raw_snapshot()
            .await
            .into_iter()
            .map(|b| (b.id.clone(), b.content.clone()))
            .collect()
    }

    fn id_of(rows: &BTreeMap<EntityUri, String>, content: &str) -> EntityUri {
        rows.iter()
            .find(|(_, c)| c.as_str() == content)
            .unwrap_or_else(|| panic!("no block with content {content:?} in {rows:?}"))
            .0
            .clone()
    }

    /// Both rungs of [`OpDispatchWriter`], over ONE SqlOnly frontend (Loro off
    /// — the shipped default), on the op whose result carries a focus
    /// target.
    ///
    /// The storage rung must NOT move focus (it dispatches below the frontend
    /// that owns it); the frontend rung MUST — `split_block` hands the caret to
    /// the TEXT-bearing block at offset 0, and that handoff is the whole reason
    /// a following keystroke lands where the user is looking. The position-0
    /// identity routing is asserted alongside it: the text (and therefore the
    /// caret) stays on the ORIGINAL id and the minted block is the empty one.
    #[tokio::test(flavor = "multi_thread")]
    async fn split_hands_focus_to_the_text_bearing_block_only_on_the_frontend_rung() {
        let comp = HeadlessFrontendComponent::new(
            &[("doc0.org", "#+ID: ref-doc-0\n* Alpha\n* Beta\n")],
            SETTLE,
        )
        .await;
        let before = ids_and_contents(&comp).await;
        let alpha = id_of(&before, "Alpha");
        let beta = id_of(&before, "Beta");
        let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

        let focus_before = comp.reactive().focused_block();
        OpDispatchWriter::with_resolver(comp.engine(), resolver.clone())
            .apply_split_block(&alpha, 0)
            .await;
        tokio::time::sleep(SETTLE).await;
        assert_eq!(
            comp.reactive().focused_block(),
            focus_before,
            "the storage rung dispatches below the frontend: it has no focus authority to move"
        );

        let mid = ids_and_contents(&comp).await;
        OpDispatchWriter::with_frontend(comp.reactive(), resolver)
            .apply_split_block(&beta, 0)
            .await;
        tokio::time::sleep(SETTLE).await;
        let after = ids_and_contents(&comp).await;

        let minted: BTreeSet<EntityUri> = after
            .keys()
            .filter(|id| !mid.contains_key(*id))
            .cloned()
            .collect();
        assert_eq!(minted.len(), 1, "one block minted by the split: {minted:?}");
        let new_block = minted.into_iter().next().expect("one minted id");

        assert_eq!(
            comp.reactive().focused_block(),
            Some(beta.clone()),
            "the frontend rung must apply split_block's focus response, which at position 0 names \
             the ORIGINAL block — the one that still holds the text"
        );
        assert_eq!(
            comp.reactive().peek_caret_seed(&beta),
            Some(0),
            "the caret sits at offset 0 of the text-bearing block"
        );
        assert_eq!(
            after.get(&beta).map(String::as_str),
            Some("Beta"),
            "a split at position 0 keeps the whole text on the original id"
        );
        assert_eq!(
            after.get(&new_block).map(String::as_str),
            Some(""),
            "the minted block is the empty one inserted above"
        );
    }
}
