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
    pub fn new(
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        services: Arc<dyn holon_frontend::reactive::BuilderServices>,
        rt: &tokio::runtime::Handle,
    ) -> Self {
        let config = CollectionConfig {
            layout: CollectionVariant::List { gap: 0.0 },
            item_template,
            sort_key: None,
            virtual_child: None,
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

/// Extract the `item_template` argument from a render expression.
/// Searches named args recursively — handles `table(#{item_template: row(...)})`,
/// `list(#{item_template: ...})`, and nested wrappers like
/// `view_mode_switcher(table(#{item_template: ...}))`.
pub fn extract_item_template(expr: &RenderExpr) -> Option<RenderExpr> {
    match expr {
        RenderExpr::FunctionCall { args, .. } => {
            for arg in args {
                if arg.name.as_deref() == Some("item_template")
                    || arg.name.as_deref() == Some("item")
                {
                    return Some(arg.value.clone());
                }
            }
            for arg in args {
                if let Some(tmpl) = extract_item_template(&arg.value) {
                    return Some(tmpl);
                }
            }
            None
        }
        _ => None,
    }
}
