//! Headless live tree — a persistent ReactiveViewModel collection backed by
//! the engine's live CDC data, mirroring what the GPUI frontend sees.
//!
//! The fresh tree (from `snapshot_reactive` / `interpret_pure`) always
//! re-interprets from current data — masking bugs where `set_data` doesn't
//! propagate to child widgets. This module creates a persistent tree that
//! receives CDC updates through the collection driver's `set_data` path,
//! exactly like the GPUI frontend.
//!
//! Usage in PBT:
//! ```text
//! let live = HeadlessLiveTree::new(engine, block_id);
//! // ... perform transitions ...
//! let live_items = live.items();
//! let fresh_items = interpret_pure(&expr, &rows, &services);
//! let diffs = tree_diff(&live_items, &fresh_items);
//! assert!(diffs.is_empty(), "live tree diverges from fresh");
//! ```

use holon_api::{ReactiveRowProvider, RenderExpr};
use holon_frontend::reactive_view::{CollectionConfig, ReactiveView};
use holon_frontend::reactive_view_model::{CollectionVariant, ReactiveViewModel};
use std::sync::Arc;

pub struct HeadlessLiveTree {
    view: ReactiveView,
}

impl HeadlessLiveTree {
    /// Build a persistent live collection mirroring prod's main panel.
    ///
    /// `layout` MUST be the variant prod actually renders (derived from the
    /// real render expr via [`extract_collection_variant`]). A hierarchical
    /// main panel routes through `create_tree_driver` and its *targeted*
    /// focus driver — the path where the rendered_text↔editable_text variant
    /// swap can silently freeze. Forcing a flat `list` here (the old default)
    /// exercised only `create_flat_driver`, whose focus driver does a full
    /// rebuild and so masked tree-only focus bugs.
    pub fn new(
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        layout: CollectionVariant,
        services: Arc<dyn holon_frontend::reactive::BuilderServices>,
        rt: &tokio::runtime::Handle,
    ) -> Self {
        let config = CollectionConfig {
            layout,
            item_template,
            sort_key: None,
            virtual_child: None,
            rules: Vec::new(),
        };
        let view = ReactiveView::new_collection(config, data_source, None, None);
        view.start(services, rt);
        Self { view }
    }

    pub fn items(&self) -> Vec<Arc<ReactiveViewModel>> {
        self.view.items.lock_ref().iter().cloned().collect()
    }

    pub fn item_count(&self) -> usize {
        self.view.items.lock_ref().len()
    }
}

/// Resolve `expr` to the collection call prod *actually renders*, descending
/// through wrappers. The crucial case is `view_mode_switcher(...)`: it offers
/// several `mode_*` templates but interprets only the **active** one
/// (`mode_<default_mode>`, default `"tree"` — see
/// `shadow_builders/view_mode_switcher.rs`). A naive "first layout call"
/// search picks whichever `mode_*` appears first (often `table`), giving the
/// wrong — and usually non-hierarchical — layout. This mirrors the switcher's
/// own resolution so the live tree matches prod's active view.
pub fn resolve_active_collection(expr: &RenderExpr) -> Option<&RenderExpr> {
    match expr {
        RenderExpr::FunctionCall { name, args } if name == "view_mode_switcher" => {
            let active = active_mode_name(args);
            let mode_key = format!("mode_{active}");
            let tmpl = args
                .iter()
                .find(|a| a.name.as_deref() == Some(mode_key.as_str()))
                .or_else(|| {
                    args.iter()
                        .find(|a| a.name.as_deref().is_some_and(|k| k.starts_with("mode_")))
                })
                .map(|a| &a.value)?;
            resolve_active_collection(tmpl)
        }
        _ if holon_frontend::reactive_view_model::collection_variant_of(expr).is_some() => {
            Some(expr)
        }
        RenderExpr::FunctionCall { args, .. } => args
            .iter()
            .find_map(|a| resolve_active_collection(&a.value)),
        _ => None,
    }
}

/// The active mode of a `view_mode_switcher`'s args: explicit `default_mode`
/// (the backend marks the `Predicate::Always` variant this way), else `"tree"`.
fn active_mode_name(args: &[holon_api::render_types::Arg]) -> String {
    args.iter()
        .find(|a| a.name.as_deref() == Some("default_mode"))
        .and_then(|a| match &a.value {
            RenderExpr::Literal {
                value: holon_api::Value::String(s),
            } => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "tree".to_string())
}

/// Derive the `CollectionVariant` prod renders for `expr` — the *active* view
/// mode's layout (see [`resolve_active_collection`]).
pub fn extract_collection_variant(expr: &RenderExpr) -> Option<CollectionVariant> {
    holon_frontend::reactive_view_model::collection_variant_of(resolve_active_collection(expr)?)
}

/// Extract the `item_template` of the *active* collection (see
/// [`resolve_active_collection`]) — the template prod actually renders, not
/// some inactive sibling mode's.
pub fn extract_item_template(expr: &RenderExpr) -> Option<RenderExpr> {
    let collection = resolve_active_collection(expr)?;
    match collection {
        RenderExpr::FunctionCall { args, .. } => args
            .iter()
            .find(|a| {
                a.name.as_deref() == Some("item_template") || a.name.as_deref() == Some("item")
            })
            .map(|a| a.value.clone()),
        _ => None,
    }
}
