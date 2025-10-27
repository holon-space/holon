//! Tree walker for value-fn provider invariants (inv11/inv12/inv13).
//!
//! Walks a `ReactiveViewModel` and surfaces every `Reactive` node that is
//! backed by a live `ReactiveRowProvider` — the providers produced by
//! value functions like `focus_chain()`, `ops_of(uri)`, `chain_ops(level)`
//! (or by data-bound `ReactiveQueryResults`). The downstream invariants
//! use the collected `ProviderEntry` list to check identity stability,
//! arg variance, and flicker resistance.

use std::sync::Arc;

use holon_api::render_types::RenderExpr;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// One streaming `Reactive` node surfaced during tree walking.
///
/// `cache_identity` is the `ReactiveRowProvider::cache_identity()` for
/// the backing provider; same Arc → same identity. `item_template` is
/// the per-row render expression, useful for grouping providers by the
/// template that produced them. `rows_snapshot_len` records how many
/// rows the provider currently carries — lets inv11 assert "provider
/// for URI A produced rows ≠ provider for URI B".
pub struct ProviderEntry {
    pub cache_identity: u64,
    pub item_template_debug: String,
    pub rows_snapshot_len: usize,
}

pub fn collect_providers(tree: &ReactiveViewModel) -> Vec<ProviderEntry> {
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

fn walk(node: &ReactiveViewModel, out: &mut Vec<ProviderEntry>) {
    // Check collection (streaming reactive data source)
    if let Some(ref view) = node.collection {
        if let Some(ds) = view.data_source() {
            let rows_len = ds.rows_snapshot().len();
            let template_debug = view
                .item_template()
                .map(render_expr_shape)
                .unwrap_or_else(|| "<no-template>".to_string());
            out.push(ProviderEntry {
                cache_identity: ds.cache_identity(),
                item_template_debug: template_debug,
                rows_snapshot_len: rows_len,
            });
        }
        for item_arc in view.items.lock_ref().iter() {
            walk(item_arc, out);
        }
    }

    // Walk static children
    for child in &node.children {
        walk(child, out);
    }

    // Walk slot content
    if let Some(ref slot) = node.slot {
        let content = slot.content.lock_ref();
        walk(&content, out);
    }
}

/// Compact, cache-key-friendly string for a `RenderExpr`. Using the
/// `Debug` impl directly keeps this alignment-free and good enough for
/// grouping identical templates in inv12.
fn render_expr_shape(expr: &RenderExpr) -> String {
    format!("{expr:?}")
}

/// Count `BottomDock` nodes in a reactive tree. Used by `inv_bar` to
/// assert that a render_expr mentioning `bottom_dock` produces at least
/// one structural `BottomDock` in the interpreted tree.
pub fn count_bottom_docks(node: &ReactiveViewModel) -> usize {
    let mut n = if node.widget_name().as_deref() == Some("bottom_dock") {
        1
    } else {
        0
    };

    // Walk static children
    for child in &node.children {
        n += count_bottom_docks(child);
    }

    // Walk collection items
    if let Some(ref view) = node.collection {
        for item in view.items.lock_ref().iter() {
            n += count_bottom_docks(item);
        }
    }

    // Walk slot content
    if let Some(ref slot) = node.slot {
        let content = slot.content.lock_ref();
        n += count_bottom_docks(&content);
    }

    n
}

/// Return `true` when `expr` (or any of its sub-exprs) is a
/// `FunctionCall` with the given name. Used by inv11 to pick render
/// expressions that drive `focus_chain()` / `ops_of()` / `chain_ops()`
/// without having to re-parse the Rhai source.
pub fn rhai_mentions(expr: &RenderExpr, fn_name: &str) -> bool {
    use holon_api::render_types::Arg;
    fn walk_args(args: &[Arg], target: &str) -> bool {
        args.iter().any(|a| rhai_mentions_inner(&a.value, target))
    }
    fn rhai_mentions_inner(expr: &RenderExpr, target: &str) -> bool {
        match expr {
            RenderExpr::FunctionCall { name, args, .. } => {
                name == target || walk_args(args, target)
            }
            RenderExpr::BinaryOp { left, right, .. } => {
                rhai_mentions_inner(left, target) || rhai_mentions_inner(right, target)
            }
            RenderExpr::Array { items } => items.iter().any(|i| rhai_mentions_inner(i, target)),
            RenderExpr::Object { fields } => {
                fields.iter().any(|(_, v)| rhai_mentions_inner(v, target))
            }
            _ => false,
        }
    }
    rhai_mentions_inner(expr, fn_name)
}

/// Silence unused-import warning when this module is trimmed down —
/// the generic `Arc` reference keeps rustc happy regardless of the
/// variants we match on.
#[allow(dead_code)]
const _: fn() = || {
    fn _assert<T: ?Sized>() {}
    _assert::<Arc<dyn holon_api::ReactiveRowProvider>>();
};
