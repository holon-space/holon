//! Dioxus renderer for `holon_frontend::view_model::ViewModel`.
//!
//! Mirrors `frontends/gpui/src/render/` in structure: one file per widget under
//! [`builders`], dispatched by the shared `holon_macros::builder_registry!`
//! macro. Unlike GPUI, which renders from the live `ReactiveViewModel` tree,
//! dioxus-web receives a serialized `ViewModel` snapshot from the worker over
//! postMessage and dispatches on the static `ViewKind`.
//!
//! The dioxus-only plumbing (worker bridge, contenteditable cell, per-`LiveBlock`
//! subscription) stays in `crate::{bridge, editor}`. Everything here is pure
//! `ViewModel → Element`.

pub mod builders;

pub use builders::RenderNode;

/// Dioxus context carrying the entity URI (block id) that should own
/// any `editable_text` rendered inside the current subtree.
///
/// Provided by `LiveBlockNode`; consumed by `EditableTextNode`. When an
/// `editable_text` appears outside any `live_block`, consumers fall back to
/// an empty id and log a warning — writes to an empty id are rejected at
/// the worker boundary.
#[derive(Clone, PartialEq)]
pub struct EntityContext(pub String);

/// Dioxus render context.
///
/// Deliberately empty: Dioxus doesn't need a bounds registry, focus scope, or
/// live GPUI handles at render time. Kept as a struct so the
/// `builder_registry!` macro has a stable type to thread through each
/// builder, and so we have a single place to add cross-cutting state
/// (theme, preferences, …) later without touching every builder's
/// signature.
#[derive(Clone, Copy, Default)]
pub struct DioxusRenderContext;
