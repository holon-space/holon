//! Widget builder registry for the dioxus desktop snapshot renderer.
//!
//! Uses `holon_macros::builder_registry!` in `node_dispatch` mode, pointing
//! at the static `view_model::ViewKind` enum (not the live
//! `reactive_view_model::ReactiveViewModel` that GPUI uses — see the
//! `node_type` arg below).
//!
//! Each file in this directory exports `pub fn render(node, ctx) -> Element`
//! and matches its own `ViewKind` variant. The macro generates a `render_node`
//! that dispatches on `node.widget_name()` to the matching builder.

pub mod dispatch;
pub mod prelude;
pub mod util;

use dioxus::prelude::*;
pub use holon_frontend::view_model::ViewModel;

pub use super::DioxusRenderContext;

// ── Macro-generated dispatch ──────────────────────────────────────────────
//
// `render_node(node, ctx) -> Element` is produced by walking the directory
// for `*.rs` files other than `mod` / `prelude` / `util` and emitting one
// match arm per file. The `empty:` arg supplies the empty/None expression
// (the macro defaults that arm to a GPUI element, which is wrong here).
holon_macros::builder_registry!(
    "src/render/builders",
    skip: [prelude, util, dispatch],
    node_dispatch: Element,
    context: DioxusRenderContext,
    node_type: holon_frontend::view_model::ViewModel,
    empty: rsx! {},
);

/// Top-level render entry point. A thin `#[component]` wrapper around the
/// macro-generated `render_node` so rsx! consumers can write
/// `RenderNode { node: child.clone() }` idiomatically.
#[component]
pub fn RenderNode(node: ViewModel) -> Element {
    render_node(&node, &DioxusRenderContext)
}

/// Default arm emitted by `builder_registry!` when a widget name has no
/// matching builder file. `Empty` / `Loading` route here by design — they
/// render as nothing.
pub fn render_unsupported(_: &str, _: &DioxusRenderContext) -> Element {
    rsx! {}
}
