//! Imports shared by every builder.
//!
//! Mirrors `frontends/gpui/src/render/builders/prelude.rs` — each widget
//! file imports `use super::prelude::*;` for a uniform shape across the
//! builders directory.

pub use dioxus::prelude::*;
pub use holon_frontend::view_model::{LazyChildren, ViewModel};

pub use super::{DioxusRenderContext, RenderNode};
