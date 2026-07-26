//! `DirectUserDriver` — legacy PBT driver that bypasses FrontendSession and
//! calls `BackendEngine::execute_operation` directly. Used by backend PBTs
//! that don't need the reactive/UI pipeline.
//!
//! The `UserDriver` trait and `ReactiveEngineDriver` now live in
//! `holon_frontend::user_driver` so they can be shared across all
//! frontends (including MCP's channel-based `GpuiUserDriver`). This module
//! re-exports them for backcompat with existing test code.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use holon::api::backend_engine::BackendEngine;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::KeyChord;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_frontend::ReactiveViewModel;
use holon_frontend::operations::OperationIntent;
pub use holon_frontend::user_driver::ReactiveEngineDriver;
pub use holon_frontend::user_driver::UserDriver;
use holon_pbt_core::capabilities::SutBlockCreate;
use holon_pbt_core::capabilities::SutBlockToPage;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutTemplateInstantiate;

use crate::pbt::op_write_cap::IdResolver;

/// Dispatches mutations directly via `BackendEngine::execute_operation`.
/// Legacy driver — bypasses FrontendSession and ReactiveEngine.
///
/// This is also the **dispatch floor** of the layer-localization driver ladder
/// (§8.11): the bottom rung that applies a structural op directly to the
/// engine, below the interaction (geometry / view-model) layers. Its
/// `SutBlockTreeWrite` impl is what a storage-only composed config (no
/// ViewModel/UI → no higher driver) installs, so "the floor is just another
/// `UserDriver`" rather than a bespoke cap.
pub struct DirectUserDriver {
    engine: Arc<BackendEngine>,
    /// Synthetic-oracle→SUT-real id map (the `SutBlockTreeWrite` floor resolves
    /// every id through it, exactly like `OpDispatchWriter`). Empty =
    /// identity (the legacy `new` callers that dispatch fixed ids).
    resolver: IdResolver,
}

impl DirectUserDriver {
    /// Identity resolution (legacy backend PBTs that pass fixed ids).
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self {
            engine,
            resolver: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Share the composed runner's id map so the floor `SutBlockTreeWrite`
    /// resolves an oracle synthetic id (`block::split-N`) to the
    /// engine-minted id before dispatch.
    pub fn with_resolver(engine: Arc<BackendEngine>, resolver: IdResolver) -> Self {
        Self { engine, resolver }
    }

    fn resolve(&self, id: &EntityUri) -> EntityUri {
        self.resolver
            .lock()
            .expect("resolver lock")
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.clone())
    }

    /// Dispatch a `block` op directly to the engine — the floor below
    /// interaction resolution (`synthetic_dispatch` ==
    /// `BackendEngine::execute_operation`).
    async fn dispatch_block(&self, op: &str, params: StorageEntity) {
        let params: HashMap<String, Value> = params
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        self.synthetic_dispatch("block", op, params)
            .await
            .unwrap_or_else(|e| panic!("[DirectUserDriver floor] block/{op} failed: {e:#}"));
    }

    fn id_only(&self, id: &EntityUri) -> StorageEntity {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(self.resolve(id).to_string()));
        params
    }
}

/// The dispatch-floor `SutBlockTreeWrite` (§8.11 LL-2). Each structural op is
/// applied directly to the engine via `synthetic_dispatch` (==
/// `OpDispatchWriter`'s no-focus-sink path), below the geometry/view-model
/// interaction layers. UI-only gestures (click→focus, expand/collapse, slash)
/// have NO floor — they are view-model concepts and correctly bottom out at the
/// VM rung (§8.11 care-point 3), so this floor provides
/// only the structural-write cap, not the gesture caps.
#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for DirectUserDriver {
    async fn apply_split_block(&self, id: &EntityUri, position: usize) {
        let mut params = self.id_only(id);
        params.insert("position".into(), Value::Integer(position as i64));
        self.dispatch_block("split_block", params).await;
    }

    async fn apply_join_block(&self, id: &EntityUri) {
        let mut params = self.id_only(id);
        params.insert("position".into(), Value::Integer(0));
        self.dispatch_block("join_block", params).await;
    }

    async fn apply_indent(&self, id: &EntityUri) {
        self.dispatch_block("indent", self.id_only(id)).await;
    }

    async fn apply_outdent(&self, id: &EntityUri) {
        self.dispatch_block("outdent", self.id_only(id)).await;
    }

    async fn apply_move_up(&self, id: &EntityUri) {
        self.dispatch_block("move_up", self.id_only(id)).await;
    }

    async fn apply_move_down(&self, id: &EntityUri) {
        self.dispatch_block("move_down", self.id_only(id)).await;
    }
}

/// The op-floor `SutBlockCreate` (`CreateBlockUnderFocus`). Unlike the headless
/// UI's creation-slot gesture, this dispatches `block.create` straight to the
/// engine under the ref-resolved `parent` — deterministic, no dependency on a
/// live rendered slot rowset. This is what makes `CreateBlockUnderFocus` run
/// under a no-UI (storage-only) pin. The `parent` is resolved through the
/// shared id map (a minted/synthetic parent → its real id); the born-equal
/// `id`, when present, is passed verbatim so oracle and SUT share it. When `id`
/// is `None` the `id` key is OMITTED — exercising the provider's
/// mint-when-absent path.
#[async_trait::async_trait(?Send)]
impl SutBlockCreate for DirectUserDriver {
    async fn apply_create_under_focus(
        &self,
        parent: &EntityUri,
        content: &str,
        id: Option<&EntityUri>,
    ) {
        let mut params: StorageEntity = HashMap::new();
        params.insert(
            "parent_id".into(),
            Value::String(self.resolve(parent).to_string()),
        );
        params.insert("content".into(), Value::String(content.to_string()));
        if let Some(uri) = id {
            params.insert("id".into(), Value::String(uri.to_string()));
        }
        self.dispatch_block("create", params).await;
    }
}

#[async_trait::async_trait]
impl UserDriver for DirectUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        let params: holon_api::StorageEntity = params
            .into_iter()
            .map(|(k, v)| (std::sync::Arc::from(k.as_str()), v))
            .collect();
        self.engine
            .execute_operation(
                &EntityName::new(entity),
                op,
                params.into_iter().map(|(k, v)| (k.into(), v)).collect(),
                holon_api::OpOrigin::User,
            )
            .await
            .map(|_| ())
            .context(format!("execute_operation({entity}, {op}) failed"))
    }

    /// Drag&drop has no faithful direct-engine equivalent — `DirectUserDriver`
    /// bypasses the reactive layer where draggable / drop_zone widgets live.
    /// Tests that need drag&drop must install a driver with widget-tree
    /// access (e.g. `ReactiveEngineDriver` or `GpuiUserDriver`).
    async fn drop_entity(&self, _: &EntityUri, _: &EntityUri, _: &EntityUri) -> Result<bool> {
        anyhow::bail!(
            "DirectUserDriver does not implement drop_entity — install ReactiveEngineDriver or a \
             native frontend driver to exercise drag&drop transitions"
        )
    }

    // ── Action verbs ────────────────────────────────────────────────────
    //
    // Backend-direct equivalents of the user actions. These keep the
    // historical "fast PBT" behavior for tests that don't need the
    // reactive pipeline — they're the same bodies the trait defaults used
    // to provide, just made explicit so screen drivers can't accidentally
    // inherit them.

    async fn send_key_chord(
        &self,
        _: &EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &EntityUri,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        use holon_frontend::input::InputAction;
        use holon_frontend::input::WidgetInput;
        let input = WidgetInput::KeyChord {
            keys: chord.0.clone(),
        };
        let action = holon_frontend::focus_path::bubble_input_oneshot(root_tree, entity_id, &input);
        match action {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                let mut params = HashMap::new();
                params.insert("id".into(), Value::String(entity_id.to_string()));
                params.extend(extra_params);
                self.synthetic_dispatch(entity_name.as_str(), &operation.name, params)
                    .await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Click-to-focus is a frontend concern: since ADR 0010 editor focus is
    /// pure in-memory UI state (`UiState.focused_block`), not a backend op.
    /// `DirectUserDriver` only has a `BackendEngine`, so it can't set focus —
    /// bail loudly, like `drop_entity` and the observation verbs. Tests that
    /// need click-to-focus must install `ReactiveEngineDriver`.
    async fn click_entity(&self, _: &EntityUri, _: &str) -> Result<()> {
        anyhow::bail!(
            "DirectUserDriver cannot click-to-focus: editor focus is frontend in-memory state \
             (ADR 0010), not a backend op. Install ReactiveEngineDriver to exercise focus \
             transitions"
        )
    }

    async fn click_entity_with_tree(
        &self,
        _: &EntityUri,
        root_tree: &ReactiveViewModel,
        entity_id: &EntityUri,
        region: &str,
    ) -> Result<bool> {
        if let Some(intent) =
            holon_frontend::focus_path::find_click_intent_oneshot(root_tree, entity_id)
        {
            self.apply_intent(intent).await?;
            return Ok(true);
        }
        self.click_entity(entity_id, region).await?;
        Ok(false)
    }

    // ── Observation verbs ───────────────────────────────────────────────
    //
    // DirectUserDriver has no reactive state, so it can't faithfully
    // answer "what's visible" or "what's clickable". Bail loudly. Tests
    // that need observation must install ReactiveEngineDriver.

    fn is_widget_visible(&self, _: &EntityUri) -> bool {
        false
    }

    fn is_in_region(&self, _: &EntityUri, _: holon_api::Region) -> bool {
        false
    }

    fn entities_in_region(&self, _: holon_api::Region) -> Vec<holon_api::EntityUri> {
        Vec::new()
    }

    fn reachable_entities_in_region(&self, _: holon_api::Region) -> Vec<holon_api::EntityUri> {
        Vec::new()
    }

    async fn scroll_to_entity(&self, _: &EntityUri) -> Result<()> {
        Ok(())
    }

    fn click_intent_of(&self, _: &EntityUri) -> Option<OperationIntent> {
        None
    }

    fn displayed_text(&self, _: &EntityUri) -> Option<String> {
        None
    }
}

const TPL_CHILD: &str = "block:tpl-c1";

/// Op-floor `SutTemplateInstantiate`: seeds the canned template blocks
/// (idempotent `block.create`), then dispatches
/// `block.instantiate_template` through the production engine.
#[async_trait::async_trait(?Send)]
impl SutTemplateInstantiate for DirectUserDriver {
    async fn instantiate_template(
        &self,
        template_id: &EntityUri,
        target_parent: &EntityUri,
        context_key: &str,
        bindings: &[(String, String)],
    ) {
        // Seed the template blocks idempotent (UPSERT).
        let mut root_params: HashMap<String, Value> = HashMap::new();
        root_params.insert("id".to_string(), Value::String(template_id.to_string()));
        root_params.insert("content".to_string(), Value::String("{{date}}".to_string()));
        root_params.insert("template".to_string(), Value::String("t".to_string()));
        root_params.insert(
            "template_vars".to_string(),
            Value::String("date, mood=neutral".to_string()),
        );
        self.synthetic_dispatch("block", "create", root_params)
            .await
            .unwrap_or_else(|e| {
                panic!("[DirectUserDriver floor] seed tpl root {template_id} failed: {e:#}")
            });

        let child_id = TPL_CHILD.to_string();
        let mut child_params: HashMap<String, Value> = HashMap::new();
        child_params.insert("id".to_string(), Value::String(child_id.clone()));
        child_params.insert(
            "parent_id".to_string(),
            Value::String(template_id.to_string()),
        );
        child_params.insert(
            "content".to_string(),
            Value::String("see {{date}} now".to_string()),
        );
        child_params.insert(
            "marks".to_string(),
            Value::String(r#"[{"start":0,"end":3,"kind":"Bold"}]"#.to_string()),
        );
        self.synthetic_dispatch("block", "create", child_params)
            .await
            .unwrap_or_else(|e| {
                panic!("[DirectUserDriver floor] seed tpl child {child_id} failed: {e:#}")
            });

        // Dispatch instantiate_template.
        let bindings_obj: HashMap<String, Value> = bindings
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect();
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert(
            "template_id".to_string(),
            Value::String(template_id.to_string()),
        );
        params.insert(
            "target_parent".to_string(),
            Value::String(self.resolve(target_parent).to_string()),
        );
        params.insert(
            "context_key".to_string(),
            Value::String(context_key.to_string()),
        );
        params.insert("bindings".to_string(), Value::Object(bindings_obj));
        self.synthetic_dispatch("block", "instantiate_template", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[DirectUserDriver floor] block/instantiate_template failed: {e:#}")
            });
    }
}

/// Op-floor `SutBlockToPage`: dispatch `block.convert_block_to_page` through
/// the production engine (BlockToPageTransform Option B). An empty
/// `destination_path` is OMITTED so the op takes its `destination_path`-absent
/// branch — defaulting to the origin's nearest page ancestor — which is exactly
/// what the reference effect models, so the born-equal page id agrees.
#[async_trait::async_trait(?Send)]
impl SutBlockToPage for DirectUserDriver {
    async fn convert_block_to_page(&self, target: &EntityUri, destination_path: &str) {
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert(
            "target".to_string(),
            Value::String(self.resolve(target).to_string()),
        );
        // Empty ⇒ omit, so the backend defaults to the nearest page ancestor
        // (the ref model's destination). A non-empty path is passed verbatim.
        if !destination_path.is_empty() {
            params.insert(
                "destination_path".to_string(),
                Value::String(destination_path.to_string()),
            );
        }
        self.synthetic_dispatch("block", "convert_block_to_page", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[DirectUserDriver floor] block/convert_block_to_page failed: {e:#}")
            });
    }
}

/// Op-floor `SutPageIdentity`: the two production ops the page-identity
/// property needs, dispatched straight to the engine.
///
/// * `rename_page` → `block.set_field("content")`. This is the SAME op the
///   editor's on-blur write takes, so a page rename is journaled for undo and
///   is an ordinary edit to the existing entity — the id does not re-mint
///   (`docs/Plans/PageIdentityDeterminism.md` §5.3).
/// * `create_page_from_link` → `block.create_page_from_link(target)`, the lazy
///   page-creation path a click on a dangling `[[Target]]` takes. It mints each
///   missing segment's id as `PageId::for_path(accumulated_path)`.
///
/// `target` is a page PATH, not an id, so it is passed verbatim — there is no
/// synthetic id to resolve.
#[async_trait::async_trait(?Send)]
impl holon_pbt_core::capabilities::SutPageIdentity for DirectUserDriver {
    async fn rename_page(&self, page: &EntityUri, new_title: &str) {
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert(
            "id".to_string(),
            Value::String(self.resolve(page).to_string()),
        );
        params.insert("field".to_string(), Value::String("content".to_string()));
        params.insert("value".to_string(), Value::String(new_title.to_string()));
        self.synthetic_dispatch("block", "set_field", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[DirectUserDriver floor] block/set_field(content) on {page} failed: {e:#}")
            });
    }

    async fn create_page_from_link(&self, target: &str) {
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("target".to_string(), Value::String(target.to_string()));
        if let Err(e) = self
            .synthetic_dispatch("block", "create_page_from_link", params)
            .await
        {
            // Interim identity policy (plan §5): re-creating a page at a path a
            // `RenamePage` FREED is REFUSED fail-loud — the derived id is still
            // held by the renamed page. That refusal is the SPECIFIED behaviour,
            // NOT a driver failure: model it as a disclosed no-op (the reference
            // mirrors it in `apply_create_page_at_path`). Any OTHER failure is a
            // real defect — panic loud. Recognised by the stable marker because
            // the concrete `IdentityCollision` type is erased by the dispatch
            // chain's string-enriching wrappers.
            let msg = format!("{e:#}");
            assert!(
                msg.contains(holon_api::IDENTITY_COLLISION_MARKER),
                "[DirectUserDriver floor] block/create_page_from_link({target:?}) failed: {msg}"
            );
        }
    }
}
