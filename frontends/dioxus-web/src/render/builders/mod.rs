//! Widget builder registry for the dioxus-web snapshot renderer.
//!
//! Uses `holon_macros::builder_registry!` in `node_dispatch` mode, pointing
//! at the static `view_model::ViewKind` enum (not the live
//! `reactive_view_model::ReactiveViewKind` that GPUI uses — see the
//! `node_type` / `kind_type` args below).
//!
//! Each file in this directory exports `pub fn render(...) -> Element`.
//! The macro parses that signature and generates a `render_node` function
//! that destructures each `ViewKind` variant into its fields and calls the
//! matching builder.

pub mod prelude;
pub mod util;

use dioxus::prelude::*;
pub use holon_frontend::view_model::ViewModel;

pub use super::DioxusRenderContext;

// ── Macro-generated dispatch ──────────────────────────────────────────────
//
// `render_node(node, ctx) -> Element` is produced by walking the directory
// for `*.rs` files other than `mod` / `prelude` and emitting one match arm
// per file. Variants not covered by a file hit the fallback (empty element
// with a `tracing::warn!`).
holon_macros::builder_registry!(
    "src/render/builders",
    skip: [prelude, util],
    node_dispatch: Element,
    context: DioxusRenderContext,
    node_type: holon_frontend::view_model::ViewModel,
    kind_type: holon_frontend::view_model::ViewKind,
);

/// Top-level render entry point. A thin `#[component]` wrapper around the
/// macro-generated `render_node` so rsx! consumers can write
/// `RenderNode { node: child.clone() }` idiomatically.
#[component]
pub fn RenderNode(node: ViewModel) -> Element {
    render_node(&node, &DioxusRenderContext)
}

/// Fallback arm emitted by `builder_registry!` when a `ViewKind` variant
/// has no matching builder file. `Empty` / `Loading` / `DropZone` fall
/// through here by design — they render as nothing.
pub fn render_unsupported(_name: &str, _ctx: &DioxusRenderContext) -> Element {
    rsx! {}
}
