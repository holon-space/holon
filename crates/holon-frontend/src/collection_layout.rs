//! Collection layout registry.
//!
//! Replaces the closed `CollectionVariant` enum with a name-keyed registry.
//! Each layout declares its `LayoutShape`, a default gap, and any small
//! pieces of metadata the streaming runtime / view-mode switcher need.
//!
//! ## How layouts dispatch
//!
//! Three independent seams pick up a layout, all keyed on the **render-DSL
//! function name**:
//!
//! 1. **Shadow widget builder** — registered in
//!    `shadow_builders/mod.rs::build_shadow_interpreter` as a normal widget.
//!    Produces a `ReactiveViewModel`.
//! 2. **Streaming runtime** — `reactive_view.rs` reads the layout's
//!    `LayoutShape` via this registry and picks the right driver
//!    (`tree_driver` for `Hierarchical`, `flat_driver` for `Flat`).
//! 3. **Platform renderer** — registered in each frontend's `builders` module
//!    as a render fn keyed by name (e.g. `frontends/gpui/src/render/builders/board.rs`).
//!
//! The registry here is the **second** of those three seams. The other two
//! were already plug-in via name-based dispatch.
//!
//! ## Adding a new layout
//!
//! 1. Register a `LayoutSpec` here (or from a frontend at startup).
//! 2. Register a shadow builder for the same name.
//! 3. Register a platform render fn for the same name.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// Logical shape of a collection layout — the only thing the streaming
/// runtime actually branches on.
///
/// New shapes can be added without breaking existing callers as long as
/// the streaming runtime gains a corresponding driver. Most user-defined
/// layouts (board, calendar, gallery-grid, …) fit the `Flat` shape; the
/// shadow builder handles any per-row grouping/partitioning before the
/// runtime sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutShape {
    /// Children are a flat list of siblings (list, table, columns, board).
    /// Per-row grouping (e.g. board lanes) is the shadow builder's job —
    /// it produces a flat list of pre-grouped lane sub-trees and the
    /// runtime treats them as siblings.
    Flat,
    /// Children form a hierarchy via `parent_id` (tree, outline). The
    /// runtime walks descendants and emits depth-tagged items.
    Hierarchical,
}

/// Static descriptor for a registered layout.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSpec {
    pub name: String,
    pub shape: LayoutShape,
    /// Default gap (px) when the render expression doesn't specify one.
    /// Layouts that don't care about gap (tree, table, …) report 0.
    pub default_gap: f32,
}

impl LayoutSpec {
    pub fn flat(name: impl Into<String>, default_gap: f32) -> Self {
        Self {
            name: name.into(),
            shape: LayoutShape::Flat,
            default_gap,
        }
    }

    pub fn hierarchical(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            shape: LayoutShape::Hierarchical,
            default_gap: 0.0,
        }
    }
}

/// Registry of layout specs keyed by render-DSL function name.
pub struct LayoutRegistry {
    by_name: HashMap<String, LayoutSpec>,
}

impl LayoutRegistry {
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }

    pub fn register(&mut self, spec: LayoutSpec) {
        self.by_name.insert(spec.name.clone(), spec);
    }

    pub fn lookup(&self, name: &str) -> Option<&LayoutSpec> {
        self.by_name.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(|s| s.as_str())
    }
}

impl Default for LayoutRegistry {
    fn default() -> Self {
        Self::new()
    }
}

static REGISTRY: OnceLock<RwLock<LayoutRegistry>> = OnceLock::new();

fn registry() -> &'static RwLock<LayoutRegistry> {
    REGISTRY.get_or_init(|| {
        let mut r = LayoutRegistry::new();
        register_builtins(&mut r);
        RwLock::new(r)
    })
}

/// Register the layouts shipped with `holon-frontend`. Called automatically
/// the first time the registry is touched.
fn register_builtins(r: &mut LayoutRegistry) {
    r.register(LayoutSpec::flat("list", 4.0));
    r.register(LayoutSpec::flat("table", 0.0));
    r.register(LayoutSpec::flat("columns", 16.0));
    r.register(LayoutSpec::flat("board", 0.0));
    r.register(LayoutSpec::hierarchical("tree"));
    r.register(LayoutSpec::hierarchical("outline"));
}

/// Register a layout from outside `holon-frontend` (e.g. a frontend-side
/// layout that doesn't ship with the core). Idempotent: re-registering an
/// existing name overwrites the spec.
pub fn register_layout(spec: LayoutSpec) {
    registry().write().unwrap().register(spec);
}

/// Look up a layout by name. `None` means "not a collection layout"
/// (caller should treat the widget as a plain leaf / non-layout node).
pub fn lookup_layout(name: &str) -> Option<LayoutSpec> {
    registry().read().unwrap().lookup(name).cloned()
}

/// True iff `name` is registered as a collection layout. Convenience
/// wrapper around `lookup_layout(name).is_some()`.
pub fn is_layout(name: &str) -> bool {
    registry().read().unwrap().lookup(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_register() {
        for name in ["list", "table", "columns", "board", "tree", "outline"] {
            assert!(
                lookup_layout(name).is_some(),
                "builtin layout {name} should be registered",
            );
        }
    }

    #[test]
    fn tree_and_outline_are_hierarchical() {
        assert_eq!(
            lookup_layout("tree").unwrap().shape,
            LayoutShape::Hierarchical
        );
        assert_eq!(
            lookup_layout("outline").unwrap().shape,
            LayoutShape::Hierarchical
        );
    }

    #[test]
    fn board_is_flat() {
        assert_eq!(lookup_layout("board").unwrap().shape, LayoutShape::Flat);
    }

    #[test]
    fn external_registration_works() {
        register_layout(LayoutSpec::flat("calendar_month", 0.0));
        let spec = lookup_layout("calendar_month").expect("registered");
        assert_eq!(spec.shape, LayoutShape::Flat);
    }
}
