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

use std::sync::Arc;

use holon_api::ReactiveRowProvider;
use holon_api::RenderExpr;
use holon_frontend::reactive_view::CollectionConfig;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;

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
            context_root_id: None,
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
/// (`mode_<default_mode>`, else the first mode `modes:` lists — see
/// `shadow_builders/view_mode_switcher.rs`). A naive "first layout call"
/// search picks whichever `mode_*` appears first (often `table`), giving the
/// wrong — and usually non-hierarchical — layout. This mirrors the switcher's
/// own resolution so the live tree matches prod's active view.
pub fn resolve_active_collection(expr: &RenderExpr) -> Option<&RenderExpr> {
    match expr {
        RenderExpr::FunctionCall { name, args } if name == "view_mode_switcher" => {
            let mode_key = format!("mode_{}", active_mode_name(args)?);
            let tmpl = args
                .iter()
                .find(|a| a.name.as_deref() == Some(mode_key.as_str()))
                .map(|a| &a.value)?;
            resolve_active_collection(tmpl)
        }
        RenderExpr::FunctionCall { name, .. }
            if holon_frontend::collection_layout::is_layout(name) =>
        {
            Some(expr)
        }
        RenderExpr::FunctionCall { args, .. } => args
            .iter()
            .find_map(|a| resolve_active_collection(&a.value)),
        _ => None,
    }
}

/// The active mode of a `view_mode_switcher`'s args: explicit `default_mode`
/// (the backend marks the `Predicate::Always` variant this way), else the
/// first mode `modes:` lists; `None` where the builder draws an error node or
/// where `default_mode`/`modes` is not a string literal this can read.
fn active_mode_name(args: &[holon_api::render_types::Arg]) -> Option<String> {
    let arg = |key: &str| {
        args.iter()
            .find(|a| a.name.as_deref() == Some(key))
            .map(|a| &a.value)
    };
    let string_literal = |key: &str| match arg(key)? {
        RenderExpr::Literal {
            value: holon_api::Value::String(s),
        } => Some(s.clone()),
        _ => None,
    };
    arg("entity_uri")?;
    let listed =
        holon_frontend::reactive_view_model::parse_view_modes(&string_literal("modes")?).ok()?;
    match arg("default_mode") {
        None => listed.into_iter().next().map(|m| m.name),
        Some(_) => {
            let mode = string_literal("default_mode")?;
            listed.iter().any(|m| m.name == mode).then_some(mode)
        }
    }
}

/// The `CollectionVariant` prod draws for `expr`'s *active* collection (see
/// [`resolve_active_collection`]), built through `services` in `ctx` the way
/// prod builds it; `Err` with the message of the error node prod draws in its
/// place.
pub fn extract_collection_variant(
    expr: &RenderExpr,
    services: &dyn holon_frontend::reactive::BuilderServices,
    ctx: &holon_frontend::RenderContext,
) -> Result<Option<CollectionVariant>, String> {
    let Some(collection) = resolve_active_collection(expr) else {
        return Ok(None);
    };
    let built = services.interpret(collection, ctx);
    if built.is_error() {
        return Err(built
            .prop_str("message")
            .expect("an error node carries a message"));
    }
    Ok(built.collection.as_ref().and_then(|view| view.layout()))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A source prod renders: `gap:` resolves against the row, and the
    /// collection is not the first call of the expression.
    #[test]
    fn a_row_resolved_gap_off_the_walk_reaches_the_active_collection() {
        holon_frontend::shadow_builders::register_render_dsl_widget_names();
        let expr = holon_api::render_dsl::parse_render_dsl(
            r#"column(text("h"), list(#{gap: col("n"), item_template: text("x")}))"#,
        )
        .expect("the source parses");
        let services = holon_frontend::StubBuilderServices::new();
        let ctx = holon_frontend::RenderContext::default().with_row(std::sync::Arc::new(
            holon_api::widget_spec::DataRow::from([(
                "n".to_string(),
                holon_api::Value::Integer(9),
            )]),
        ));
        let variant = extract_collection_variant(&expr, &services, &ctx)
            .expect("prod draws the collection")
            .expect("the source names a collection");
        assert_eq!((variant.name(), variant.gap), ("list", 9.0));
    }
}
