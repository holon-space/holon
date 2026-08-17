//! Slash command provider for the unified popup menu.
//!
//! Implements `PopupProvider` to show available operations filtered by typed
//! text. Handles the two-phase flow: command list → param collection.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use futures_signals::signal::Signal;
use futures_signals::signal::SignalExt;
use futures_signals::signal_vec::SignalVec;
use holon_api::Value;
use holon_api::render_types::OperationParam;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::TypeHint;
use holon_api::types::EntityName;

use crate::operation_matcher::MatchedOperation;
use crate::operation_matcher::{self};
use crate::popup_menu::HideSpan;
use crate::popup_menu::PopupItem;
use crate::popup_menu::PopupProvider;
use crate::popup_menu::PopupResult;
use crate::reactive::BuilderServices;
use crate::template_placement::BlockResolver;
use crate::template_placement::TargetBlock;
use crate::template_placement::TemplateChoice;
use crate::template_placement::TemplatePlacement;

/// Adapts a [`BuilderServices`] into a [`BlockResolver`] — resolves the picked
/// block's real content/parent from the projection (`resolve_block`), instead
/// of the editor's id-only `context_params`.
pub struct ServicesBlockResolver {
    services: Arc<dyn BuilderServices>,
}

impl ServicesBlockResolver {
    pub fn new(services: Arc<dyn BuilderServices>) -> Self {
        Self { services }
    }
}

impl BlockResolver for ServicesBlockResolver {
    fn resolve(&self, id: &str) -> Option<TargetBlock> {
        self.services
            .resolve_block(id)
            .map(|b| TargetBlock::from_block(&b))
    }
}

/// `PopupItem::id` prefix marking a per-template entry (as opposed to a plain
/// operation). The suffix is the template's block id.
const TEMPLATE_ITEM_PREFIX: &str = "__template__:";

/// `PopupItem::id` of the disclosed failure row shown when the param-collection
/// search errors. Selecting it surfaces the failure instead of embedding a
/// block whose id is an error message.
const SEARCH_ERROR_ID: &str = "__search_error__";

/// Internal state for param collection sub-phase.
#[derive(Debug, Clone)]
struct ParamCollectionState {
    operation: MatchedOperation,
    param: OperationParam,
    /// The command-list filter the user had typed when they picked the
    /// operation (`"enti"` for `/enti` → Embed Entity), for callers whose
    /// editor keeps that text: the popup's filter stays prefixed by it, so the
    /// search term for this phase is whatever the user types AFTER it. EMPTY
    /// for callers that hide the command text for the duration of the phase —
    /// their filter is the search term already.
    command_filter: String,
}

/// Slash command provider.
///
/// Shows available operations matching the filter. When an operation with
/// missing entity params is selected, transitions to param collection
/// sub-phase.
pub struct CommandProvider {
    operations: Vec<OperationWiring>,
    context_params: HashMap<String, Value>,
    /// Vault templates offered as per-template entries (the picker). The raw
    /// `instantiate_template` op is hidden from the command list in favour of
    /// these — each carries a concrete `template_id`, so selecting one executes
    /// directly with no second-phase param collection.
    templates: Vec<TemplateChoice>,
    /// Resolves the picked block's REAL content/parent from the projection.
    /// `None` for callers without a backend (headless mirror) — a template pick
    /// then fails loud rather than guessing placement from the id-only context.
    resolver: Option<Arc<dyn BlockResolver>>,
    /// Backs the entity search that fills an operation's missing `EntityId`
    /// param (`embed_entity`'s `target_uri`). `None` for callers without a
    /// backend — such an operation then has no reachable completion, so it
    /// fails loud on selection instead of picking a target out of thin air.
    services: Option<Arc<dyn BuilderServices>>,
    /// If Some, we're in param collection mode.
    param_state: Arc<Mutex<Option<ParamCollectionState>>>,
    /// Line-relative offset of the "/" trigger char (from
    /// `ViewEvent::TriggerFired.prefix_start`). Threaded into
    /// `PopupResult::Execute.strip_prefix_start` so the frontend removes
    /// the typed command text from the editor when the op fires. `None`
    /// for callers that manage editor text themselves (headless mirror).
    prefix_start: Option<usize>,
    /// Byte length of the trigger prefix that opened this menu. With the
    /// current filter it gives the exact span of typed command text, which is
    /// what a picker phase hides.
    trigger_len: usize,
}

impl CommandProvider {
    pub fn new(operations: Vec<OperationWiring>, context_params: HashMap<String, Value>) -> Self {
        Self {
            operations,
            context_params,
            templates: Vec::new(),
            resolver: None,
            services: None,
            param_state: Arc::new(Mutex::new(None)),
            prefix_start: None,
            trigger_len: 0,
        }
    }

    /// Where the trigger that opened this menu sits on its line, and how long
    /// the trigger char(s) are.
    pub fn with_trigger_span(mut self, prefix_start: usize, trigger_len: usize) -> Self {
        self.prefix_start = Some(prefix_start);
        self.trigger_len = trigger_len;
        self
    }

    /// Offer these templates as per-template picker entries.
    pub fn with_templates(mut self, templates: Vec<TemplateChoice>) -> Self {
        self.templates = templates;
        self
    }

    /// Supply the block resolver used to read the picked block's real
    /// content/parent at execute time. Narrow seam for callers that have no
    /// `BuilderServices` to hand (unit tests); production uses
    /// [`Self::with_services`].
    pub fn with_resolver(mut self, resolver: Arc<dyn BlockResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Supply the backend capabilities: block resolution for template
    /// placement, and entity search for param collection.
    pub fn with_services(mut self, services: Arc<dyn BuilderServices>) -> Self {
        self.resolver = Some(Arc::new(ServicesBlockResolver::new(services.clone())));
        self.services = Some(services);
        self
    }

    /// Per-template picker entries matching `filter`. Each carries the concrete
    /// `template_id` in its `PopupItem::id` (behind [`TEMPLATE_ITEM_PREFIX`]).
    /// The filter matches the full `Template: <name>` label, so typing
    /// `/template` surfaces every template while `/<name>` narrows to one.
    fn build_template_items(templates: &[TemplateChoice], filter: &str) -> Vec<PopupItem> {
        let filter_lower = filter.to_lowercase();
        templates
            .iter()
            .map(|t| PopupItem {
                id: format!("{TEMPLATE_ITEM_PREFIX}{}", t.template_id),
                label: format!("Template: {}", t.name),
                icon: None,
            })
            .filter(|item| filter.is_empty() || item.label.to_lowercase().contains(&filter_lower))
            .collect()
    }

    /// Build the `instantiate_template` Execute for a picked template,
    /// resolving the empty-vs-non-empty placement (USER RULING "Option B").
    ///
    /// The target block's content/parent come from the RESOLVER (a live read of
    /// the projection), NOT from `context_params`: the editor's live DataRow
    /// carries only the block `id`, so trusting it made every non-empty block
    /// look empty and bail (live-drive regression). Only the `id` is taken from
    /// context; everything placement depends on is re-resolved.
    fn template_execute(
        &self,
        template_id: &str,
        filter: &str,
    ) -> Result<PopupResult, anyhow::Error> {
        let id = self
            .context_params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow::anyhow!("instantiate_template: no focused block id in context"))?
            .to_string();
        let resolver = self.resolver.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "instantiate_template: no block resolver wired — cannot read the target block's \
                 placement (backend not available in this session)"
            )
        })?;
        let target = resolver.resolve(&id).ok_or_else(|| {
            anyhow::anyhow!("instantiate_template: focused block '{id}' not found in projection")
        })?;
        // The resolved content still contains the "/<filter>" the user typed to
        // open the menu — strip it so a bullet whose whole content IS the
        // command counts as empty (path-B: otherwise in-place never fires).
        // Command bytes = "/" (1) + the filter text.
        let target = match self.prefix_start {
            Some(start) => target.without_typed_command(start, 1 + filter.len()),
            None => target,
        };

        let placement = TemplatePlacement::decide(&target)?;

        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("template_id".into(), Value::String(template_id.to_string()));
        params.insert(
            "target_parent".into(),
            Value::String(placement.target_parent().to_string()),
        );
        // Manual invocation → fresh context_key so every instantiation is a new
        // instance (rules pass their firing key instead, to converge on re-fire).
        // `uuid` is a native-only dep (see Cargo.toml); on wasm (the browser
        // worker) a process-monotonic counter gives an equally-fresh key.
        #[cfg(not(target_arch = "wasm32"))]
        let context_key = format!("manual:{}", uuid::Uuid::new_v4());
        #[cfg(target_arch = "wasm32")]
        let context_key = {
            use std::sync::atomic::AtomicU64;
            use std::sync::atomic::Ordering;
            static MANUAL_CTX_SEQ: AtomicU64 = AtomicU64::new(0);
            format!(
                "manual:wasm-{}",
                MANUAL_CTX_SEQ.fetch_add(1, Ordering::Relaxed)
            )
        };
        params.insert("context_key".into(), Value::String(context_key));
        if let Some(replaced) = placement.block_to_replace() {
            params.insert("replace_block".into(), Value::String(replaced.to_string()));
        }

        Ok(PopupResult::Execute {
            entity_name: EntityName::new("block"),
            op_name: holon_api::INSTANTIATE_TEMPLATE_OP.to_string(),
            params,
            strip_prefix_start: self.prefix_start,
        })
    }

    pub fn build_command_items(
        operations: &[OperationWiring],
        context_params: &HashMap<String, Value>,
        filter: &str,
    ) -> Vec<PopupItem> {
        let all_matches: Vec<MatchedOperation> =
            operation_matcher::find_satisfiable(operations, context_params)
                .into_iter()
                // The slash command list shows exactly the ops CLASSIFIED as
                // `Listed` at their descriptor (parse-don't-validate). This
                // subsumes the old bespoke `instantiate_template` exclusion —
                // that op is `PickerBacked` (surfaced as per-template picker
                // entries, `build_template_items`), so it is not `Listed`. It
                // also keeps gesture/navigation/internal ops (move_block,
                // split_block, set_field, create_page_from_link, …) out of the
                // menu instead of leaking every id-resolvable provider op.
                .filter(|m| {
                    matches!(
                        m.descriptor.menu_exposure,
                        holon_api::MenuExposure::Listed { surfaces } if surfaces.slash_menu
                    )
                })
                .collect();

        let filtered: Vec<MatchedOperation> = if filter.is_empty() {
            all_matches
        } else {
            let filter_lower = filter.to_lowercase();
            all_matches
                .into_iter()
                .filter(|m| {
                    m.descriptor.name.to_lowercase().contains(&filter_lower)
                        || m.descriptor
                            .display_name
                            .to_lowercase()
                            .contains(&filter_lower)
                })
                .collect()
        };

        // Presentation dedup (dogfood-round3 B1): the operation catalog is the
        // dispatcher's UNIONED provider set, advertised WITHOUT dedup (see
        // `OperationDispatcher::operations` — structural block ops are knowingly
        // double-advertised by `SqlBlockOperations` + `LoroBlockOperations`
        // under Loro authority, tolerated by `STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST`).
        // A duplicated op name would therefore render as two identical menu
        // rows. Collapse to ONE entry per operation name, keeping the FIRST
        // occurrence so the visible row corresponds to the same first-wins
        // provider the dispatcher routes to — this is presentation-only, it does
        // NOT touch dispatch (both registrations remain; `find_matched_operation`
        // resolves independently). Keyed on the operation name (the `id`), never
        // the label: two genuinely different ops may legitimately share a
        // display label and must both survive.
        let mut seen_op_names = std::collections::HashSet::new();
        filtered
            .iter()
            .filter(|m| seen_op_names.insert(m.operation_name().to_string()))
            .map(|m| PopupItem {
                id: m.operation_name().to_string(),
                label: m.descriptor.display_name.clone(),
                icon: None,
            })
            .collect()
    }

    fn find_matched_operation(
        operations: &[OperationWiring],
        context_params: &HashMap<String, Value>,
        op_name: &str,
    ) -> Option<MatchedOperation> {
        operation_matcher::find_satisfiable(operations, context_params)
            .into_iter()
            .find(|m| m.operation_name() == op_name)
    }
}

impl PopupProvider for CommandProvider {
    fn source(&self) -> &str {
        "command_menu"
    }

    fn candidates(
        &self,
        filter: Pin<Box<dyn Signal<Item = String> + Send + Sync>>,
    ) -> Pin<Box<dyn SignalVec<Item = PopupItem> + Send>> {
        let operations = self.operations.clone();
        let context_params = self.context_params.clone();
        let templates = self.templates.clone();
        let param_state = self.param_state.clone();
        let services = self.services.clone();

        let signal = filter.map_future(move |f| {
            let operations = operations.clone();
            let context_params = context_params.clone();
            let templates = templates.clone();
            let param_state = param_state.clone();
            let services = services.clone();
            async move {
                // Snapshot the phase, then release the lock — the search below
                // awaits, and this Mutex is also taken by `on_select`.
                let collecting = param_state
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|ps| ps.command_filter.clone());

                let Some(command_filter) = collecting else {
                    let mut items = Self::build_command_items(&operations, &context_params, &f);
                    items.extend(Self::build_template_items(&templates, &f));
                    return items;
                };

                // The editor still holds the text that opened this phase, so
                // the search term is what the user typed after it.
                let term = f.strip_prefix(&command_filter).unwrap_or(&f).trim();
                if term.is_empty() {
                    return Vec::new();
                }

                let Some(services) = services else {
                    return vec![PopupItem {
                        id: SEARCH_ERROR_ID.to_string(),
                        label: "Search unavailable (no backend)".to_string(),
                        icon: None,
                    }];
                };
                let handle = services.runtime_handle();
                let term = term.to_string();
                let search = handle.spawn({
                    let services = services.clone();
                    let term = term.clone();
                    async move { services.search_link_candidates(&term).await }
                });

                match search.await {
                    Ok(Ok(candidates)) => candidates
                        .iter()
                        .map(|c| PopupItem {
                            id: c.id.to_string(),
                            label: c.label.clone(),
                            icon: None,
                        })
                        .collect(),
                    Ok(Err(e)) => {
                        tracing::warn!("[CommandProvider] target search failed ({term:?}): {e}");
                        vec![PopupItem {
                            id: SEARCH_ERROR_ID.to_string(),
                            label: format!("Search failed: {e}"),
                            icon: None,
                        }]
                    }
                    Err(e) => {
                        tracing::warn!("[CommandProvider] target search task panicked: {e}");
                        vec![PopupItem {
                            id: SEARCH_ERROR_ID.to_string(),
                            label: "Search failed".to_string(),
                            icon: None,
                        }]
                    }
                }
            }
        });

        // `map_future` yields None while the future is pending.
        let signal = signal.map(|opt| opt.unwrap_or_default());
        Box::pin(signal.to_signal_vec())
    }

    fn on_select(&self, item: &PopupItem, filter: &str) -> PopupResult {
        let mut state = self.param_state.lock().unwrap();

        if let Some(ps) = state.take() {
            // We're in param collection — the selected item is an entity
            if item.id == SEARCH_ERROR_ID {
                return PopupResult::Failed {
                    message: format!("Cannot pick a {} — the search failed", ps.param.name),
                    strip_prefix_start: self.prefix_start,
                };
            }
            let selected_id = item.id.clone();
            let mut params = ps.operation.resolved_params.clone();
            params.insert(ps.param.name.clone(), Value::String(selected_id));

            return PopupResult::Execute {
                entity_name: ps.operation.entity_name().clone(),
                op_name: ps.operation.operation_name().to_string(),
                params,
                strip_prefix_start: self.prefix_start,
            };
        }

        // A per-template picker entry: resolve placement + execute directly.
        if let Some(template_id) = item.id.strip_prefix(TEMPLATE_ITEM_PREFIX) {
            return match self.template_execute(template_id, filter) {
                Ok(result) => result,
                Err(e) => {
                    // Fail loud AND visible: the selection consumed the Enter,
                    // so returning `NotActive` (→ EditorAction::None) would let
                    // it fall through to `split_block` — a silent block-split
                    // masquerading as success. `Failed` strips the typed
                    // command, surfaces a toast, and stops propagation.
                    tracing::error!("instantiate_template placement failed: {e}");
                    PopupResult::Failed {
                        message: format!("Template insert failed: {e}"),
                        strip_prefix_start: self.prefix_start,
                    }
                }
            };
        }

        // Command list phase — find the matched operation
        let matched =
            match Self::find_matched_operation(&self.operations, &self.context_params, &item.id) {
                Some(m) => m,
                None => return PopupResult::NotActive,
            };

        if matched.is_fully_satisfied() {
            return PopupResult::Execute {
                entity_name: matched.entity_name().clone(),
                op_name: matched.operation_name().to_string(),
                params: matched.resolved_params,
                strip_prefix_start: self.prefix_start,
            };
        }

        // Has missing entity params — transition to param collection, where
        // `candidates` searches for a target on every further keystroke.
        let entity_params = matched.entity_params_needed();
        if !entity_params.is_empty() {
            let first_missing = matched.missing_params[0].clone();
            // When the frontend owns the editor text (`prefix_start` is Some)
            // it HIDES the typed command for this phase (ruling D1.b), so the
            // filter it reports from here on is already only the search term —
            // there is no command prefix left to strip.
            let command_filter = match self.prefix_start {
                Some(_) => String::new(),
                None => filter.to_string(),
            };
            *state = Some(ParamCollectionState {
                operation: matched,
                param: first_missing,
                command_filter,
            });
            return PopupResult::PhaseAdvanced {
                hide: self.prefix_start.map(|prefix_start| HideSpan {
                    prefix_start,
                    // The span the MENU matched on. Deriving it from the caret
                    // instead would under-hide whenever the caret had been
                    // moved back into the command before picking.
                    len: self.trigger_len + filter.len(),
                }),
            };
        }

        // Nothing can complete this operation. Returning `NotActive` here would
        // read to the frontends as "no popup was open" and let the Enter fall
        // through to `split_block` — a mangled block masquerading as a command
        // (Martin, GPUI dogfooding 2026-08-17). Fail visibly instead.
        let missing: Vec<&str> = matched
            .missing_params
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        tracing::error!(
            "slash command {:?} has no reachable completion; missing {missing:?}",
            matched.operation_name()
        );
        PopupResult::Failed {
            message: format!(
                "\"{}\" needs {} and there is no way to supply {} here",
                matched.descriptor.display_name,
                missing.join(", "),
                if missing.len() == 1 { "it" } else { "them" }
            ),
            strip_prefix_start: self.prefix_start,
        }
    }
}

/// Check if the provider is currently in param collection phase.
pub fn is_collecting_params(provider: &CommandProvider) -> bool {
    provider.param_state.lock().unwrap().is_some()
}

/// Get the entity name being searched for during param collection.
pub fn param_search_entity(provider: &CommandProvider) -> Option<String> {
    let state = provider.param_state.lock().unwrap();
    state.as_ref().and_then(|ps| match &ps.param.type_hint {
        TypeHint::EntityId { entity_name } => Some(entity_name.to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use holon_api::render_types::OperationDescriptor;
    use holon_api::render_types::OperationParam;
    use holon_api::render_types::TypeHint;
    use holon_api::types::EntityName;

    use super::*;

    fn make_op(name: &str, display: &str, params: Vec<OperationParam>) -> OperationWiring {
        OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                name: name.into(),
                display_name: display.into(),
                required_params: params,
                id_column: "id".to_string(),
                description: String::new(),
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::Listed {
                    surfaces: holon_api::SurfaceSet {
                        slash_menu: true,
                        action_bar: false,
                    },
                },
                trigger: None,
                bound_params: Default::default(),
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            },
        }
    }

    fn param(name: &str, hint: TypeHint) -> OperationParam {
        OperationParam {
            name: name.into(),
            type_hint: hint,
            description: String::new(),
        }
    }

    fn test_ops() -> Vec<OperationWiring> {
        vec![
            make_op(
                "set_field",
                "Set Field",
                vec![
                    param("id", TypeHint::String),
                    param("field", TypeHint::String),
                    param("value", TypeHint::String),
                ],
            ),
            make_op(
                "embed_entity",
                "Embed",
                vec![
                    param("id", TypeHint::String),
                    param(
                        "target_uri",
                        TypeHint::EntityId {
                            entity_name: EntityName::new("block"),
                        },
                    ),
                ],
            ),
            make_op("delete", "Delete", vec![param("id", TypeHint::String)]),
        ]
    }

    fn context() -> HashMap<String, Value> {
        HashMap::from([("id".into(), Value::String("block-1".into()))])
    }

    #[test]
    fn builds_filtered_items() {
        let items = CommandProvider::build_command_items(&test_ops(), &context(), "");
        // set_field needs 3 params, only id available → not fully matchable but still
        // shows delete + embed_entity show
        assert!(items.len() >= 2);
    }

    #[test]
    fn filter_narrows_items() {
        let items = CommandProvider::build_command_items(&test_ops(), &context(), "emb");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Embed");
    }

    #[test]
    fn select_fully_satisfied_executes() {
        let provider = CommandProvider::new(test_ops(), context());
        let item = PopupItem {
            id: "delete".into(),
            label: "Delete".into(),
            icon: None,
        };
        let result = provider.on_select(&item, "del");
        match result {
            PopupResult::Execute {
                op_name, params, ..
            } => {
                assert_eq!(op_name, "delete");
                assert_eq!(params["id"], Value::String("block-1".into()));
            }
            other => panic!("Expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn select_with_missing_params_enters_collection() {
        let provider = CommandProvider::new(test_ops(), context());
        let item = PopupItem {
            id: "embed_entity".into(),
            label: "Embed".into(),
            icon: None,
        };
        let result = provider.on_select(&item, "emb");
        assert!(matches!(result, PopupResult::PhaseAdvanced { .. }));
        assert!(is_collecting_params(&provider));
        assert_eq!(param_search_entity(&provider), Some("block".to_string()));
    }

    #[test]
    fn param_collection_select_executes() {
        let provider = CommandProvider::new(test_ops(), context());

        // First: select embed (enters param collection)
        let item = PopupItem {
            id: "embed_entity".into(),
            label: "Embed".into(),
            icon: None,
        };
        provider.on_select(&item, "emb");

        // Select a search result
        let entity_item = PopupItem {
            id: "target-block".into(),
            label: "(untitled)".into(),
            icon: None,
        };
        let result = provider.on_select(&entity_item, "");
        match result {
            PopupResult::Execute { params, .. } => {
                assert_eq!(params["target_uri"], Value::String("target-block".into()));
            }
            other => panic!("Expected Execute, got {:?}", other),
        }
        assert!(!is_collecting_params(&provider));
    }

    fn templates() -> Vec<TemplateChoice> {
        vec![TemplateChoice {
            template_id: "block:tpl".into(),
            name: "Daily Journal".into(),
        }]
    }

    fn ctx(fields: &[(&str, &str)]) -> HashMap<String, Value> {
        fields
            .iter()
            .map(|(k, v)| ((*k).into(), Value::String((*v).into())))
            .collect()
    }

    #[test]
    fn template_entries_listed_and_filtered() {
        let items = CommandProvider::build_template_items(&templates(), "");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "Template: Daily Journal");
        assert_eq!(items[0].id, "__template__:block:tpl");
        // `/template` surfaces all; `/<name>` narrows; miss → none.
        assert_eq!(
            CommandProvider::build_template_items(&templates(), "template").len(),
            1
        );
        assert_eq!(
            CommandProvider::build_template_items(&templates(), "daily").len(),
            1
        );
        assert_eq!(
            CommandProvider::build_template_items(&templates(), "zzz").len(),
            0
        );
    }

    #[test]
    fn raw_instantiate_op_is_hidden_from_command_list() {
        // `instantiate_template` is `PickerBacked` (surfaced via per-template
        // picker entries), so the `Listed`-only command filter hides the bare
        // op — a corollary of the registry↔menu correspondence, not a bespoke
        // name check.
        let mut wiring = make_op(
            "instantiate_template",
            "Instantiate template",
            vec![param("template_id", TypeHint::String)],
        );
        wiring.descriptor.menu_exposure = holon_api::MenuExposure::PickerBacked {
            picker: holon_api::PickerKind::Template,
        };
        let items = CommandProvider::build_command_items(&[wiring], &context(), "");
        assert!(
            items.iter().all(|i| i.id != "instantiate_template"),
            "the bare instantiate_template op (PickerBacked) must not appear as a command"
        );
    }

    #[test]
    fn convert_block_to_page_surfaces_as_turn_into_page() {
        // Mirrors the engine-synthetic `convert_block_to_page` descriptor:
        // required `target`, mapped from the focused block's `id`. Once the
        // descriptor is in a block's resolved profile operations (DI wiring in
        // `create_profile_resolver`), the slash menu must surface it as the
        // "Turn into page" command — resolving `target` from the context `id`.
        let convert = OperationWiring {
            modified_param: String::new(),
            descriptor: OperationDescriptor {
                entity_name: EntityName::new("block"),
                entity_short_name: "block".into(),
                name: "convert_block_to_page".into(),
                display_name: "Turn into page".into(),
                required_params: vec![param("target", TypeHint::String)],
                param_mappings: vec![holon_api::render_types::ParamMapping {
                    from: "id".into(),
                    provides: vec!["target".into()],
                    defaults: Default::default(),
                }],
                id_column: "id".to_string(),
                description: String::new(),
                affected_fields: vec![],
                target_scope: holon_api::TargetScope::Block,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::Listed {
                    surfaces: holon_api::SurfaceSet {
                        slash_menu: true,
                        action_bar: false,
                    },
                },
                trigger: None,
                bound_params: Default::default(),
                guard: holon_api::pattern::OpGuard::None,
                arcs: holon_api::arcs::TransitionArcs::Undeclared,
            },
        };
        let items = CommandProvider::build_command_items(&[convert], &context(), "");
        assert!(
            items
                .iter()
                .any(|i| i.id == "convert_block_to_page" && i.label == "Turn into page"),
            "convert_block_to_page must surface as the 'Turn into page' command"
        );
    }

    /// A resolver stub returning canned target blocks by id — stands in for the
    /// real projection read (`ServicesBlockResolver`).
    struct FakeResolver(HashMap<String, TargetBlock>);
    impl BlockResolver for FakeResolver {
        fn resolve(&self, id: &str) -> Option<TargetBlock> {
            self.0.get(id).cloned()
        }
    }

    fn resolver(entries: &[(&str, &str, Option<&str>)]) -> Arc<dyn BlockResolver> {
        let map = entries
            .iter()
            .map(|(id, content, parent)| {
                (
                    id.to_string(),
                    TargetBlock::from_parts(id, content, *parent),
                )
            })
            .collect();
        Arc::new(FakeResolver(map))
    }

    // The LIVE editor DataRow carries ONLY the block `id` — content/parent must
    // come from the resolver, not context_params. All picker tests build the
    // id-only context to lock in that contract (they would have been RED against
    // the old context_params-trusting code, which saw empty content and bailed).
    fn live_ctx(id: &str) -> HashMap<String, Value> {
        ctx(&[("id", id)])
    }

    #[test]
    fn pick_template_on_empty_block_replaces_in_place() {
        // LIVE-shaped: the user typed "/journal" INTO an empty bullet, so the
        // resolved content is the command text itself. Stripping it must leave
        // the block empty → in-place. (RED before path-B fix: the "/journal"
        // made it look non-empty → AsChildren, never InPlace.)
        let provider = CommandProvider::new(vec![], live_ctx("block:child"))
            .with_trigger_span(0, 1)
            .with_templates(templates())
            .with_resolver(resolver(&[(
                "block:child",
                "/journal",
                Some("block:parent"),
            )]));
        let item = PopupItem {
            id: "__template__:block:tpl".into(),
            label: "Template: Daily Journal".into(),
            icon: None,
        };
        match provider.on_select(&item, "journal") {
            PopupResult::Execute {
                op_name, params, ..
            } => {
                assert_eq!(op_name, "instantiate_template");
                assert_eq!(params["template_id"], Value::String("block:tpl".into()));
                // Empty → in place: instantiate under the PARENT, delete the
                // empty block.
                assert_eq!(
                    params["target_parent"],
                    Value::String("block:parent".into())
                );
                assert_eq!(params["replace_block"], Value::String("block:child".into()));
                assert!(params.contains_key("context_key"));
            }
            other => panic!("Expected Execute, got {other:?}"),
        }
    }

    #[test]
    fn pick_template_on_nonempty_block_nests_as_children() {
        // LIVE-shaped: real content "Weekly sync" with "/journal" typed at the
        // end. Stripping the command leaves non-empty content → children (never
        // bail "empty page root", never split).
        let provider = CommandProvider::new(vec![], live_ctx("block:meeting"))
            .with_trigger_span(11, 1)
            .with_templates(templates())
            .with_resolver(resolver(&[(
                "block:meeting",
                "Weekly sync/journal",
                Some("block:parent"),
            )]));
        let item = PopupItem {
            id: "__template__:block:tpl".into(),
            label: "Template: Daily Journal".into(),
            icon: None,
        };
        match provider.on_select(&item, "journal") {
            PopupResult::Execute { params, .. } => {
                // Non-empty → children of the current block; content untouched.
                assert_eq!(
                    params["target_parent"],
                    Value::String("block:meeting".into())
                );
                assert!(
                    !params.contains_key("replace_block"),
                    "non-empty target must never be deleted"
                );
            }
            other => panic!("Expected Execute, got {other:?}"),
        }
    }

    #[test]
    fn pick_template_without_resolver_fails_loud_not_silent_split() {
        // No resolver wired → the pick must FAIL (visible), NOT return NotActive
        // (which the editor maps to None → split_block fall-through).
        let provider = CommandProvider::new(vec![], live_ctx("block:x"))
            .with_trigger_span(3, 1)
            .with_templates(templates());
        let item = PopupItem {
            id: "__template__:block:tpl".into(),
            label: "Template: Daily Journal".into(),
            icon: None,
        };
        match provider.on_select(&item, "") {
            PopupResult::Failed {
                strip_prefix_start, ..
            } => assert_eq!(strip_prefix_start, Some(3)),
            other => panic!("Expected Failed (never NotActive), got {other:?}"),
        }
    }

    #[test]
    fn pick_template_when_block_missing_fails_loud() {
        // Resolver present but the focused id isn't in the projection.
        let provider = CommandProvider::new(vec![], live_ctx("block:ghost"))
            .with_templates(templates())
            .with_resolver(resolver(&[("block:other", "x", None)]));
        let item = PopupItem {
            id: "__template__:block:tpl".into(),
            label: "Template: Daily Journal".into(),
            icon: None,
        };
        assert!(
            matches!(provider.on_select(&item, ""), PopupResult::Failed { .. }),
            "missing block must fail loud, not split"
        );
    }
}
