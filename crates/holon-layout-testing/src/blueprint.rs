//! `Shape`, `Blueprint`, and `BlockHandle` — the building blocks for
//! generating proptest `Scenario` values.

use std::fmt;
use std::sync::Arc;

use holon_frontend::reactive_view_model::ReactiveViewModel;

use crate::registry::BlockTreeThunk;

// ── Shape ─────────────────────────────────────────────────────────────────

/// A reusable thunk that materialises a fresh `ReactiveViewModel` on demand.
///
/// Wrapping a thunk (rather than a `ReactiveViewModel` directly) sidesteps
/// the fact that `ReactiveViewModel` isn't `Clone`, and lets strategies
/// compose cheaply by cloning `Arc<dyn Fn>`.
#[derive(Clone)]
pub struct Shape(pub Arc<dyn Fn() -> ReactiveViewModel + Send + Sync>);

impl Shape {
    pub fn materialize(&self) -> ReactiveViewModel {
        (self.0)()
    }
}

impl fmt::Debug for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\n{}", self.materialize().snapshot().pretty_print(0))
    }
}

// ── BlockHandle ───────────────────────────────────────────────────────────

/// One mode-switchable block discovered while building a blueprint.
#[derive(Clone)]
pub struct BlockHandle {
    /// The block id as passed to `EntityUri::from_raw(...).to_string()`.
    pub block_id: String,
    /// Mode names in display order.
    pub mode_names: Vec<String>,
    /// Thunks that materialise each mode's inner `ReactiveViewModel`.
    pub mode_thunks: Vec<BlockTreeThunk>,
    /// True when this live_block is inside a drawer. VMS buttons inside
    /// closed drawers aren't rendered, so SwitchViewMode actions targeting
    /// these handles must be excluded from the action sequence.
    pub in_drawer: bool,
    /// Initial mode index (default 0). Deferred live_blocks start at a
    /// placeholder mode and switch to the real content via `DeliverBlockContent`.
    pub initial_mode: usize,
}

impl fmt::Debug for BlockHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockHandle")
            .field("block_id", &self.block_id)
            .field("mode_names", &self.mode_names)
            .finish()
    }
}

// ── DrawerHandle ──────────────────────────────────────────────────────────

/// One toggleable drawer discovered while building a blueprint. Drawers
/// default to open; scenarios may emit `ToggleDrawer { block_id }` actions
/// targeting any of these ids.
#[derive(Clone, Debug)]
pub struct DrawerHandle {
    pub block_id: String,
}

// ── Blueprint ─────────────────────────────────────────────────────────────

/// A proptest-generated widget tree plus the handles it contains.
#[derive(Clone)]
pub struct Blueprint {
    pub shape: Shape,
    pub handles: Vec<BlockHandle>,
    pub drawers: Vec<DrawerHandle>,
}

impl fmt::Debug for Blueprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Blueprint {{")?;
        writeln!(f, "  shape: {:?}", self.shape)?;
        writeln!(f, "  handles: {} switchable block(s)", self.handles.len())?;
        for h in &self.handles {
            writeln!(f, "    {} (modes: {})", h.block_id, h.mode_names.join(", "))?;
        }
        writeln!(f, "  drawers: {} toggleable drawer(s)", self.drawers.len())?;
        for d in &self.drawers {
            writeln!(f, "    {}", d.block_id)?;
        }
        write!(f, "}}")
    }
}

impl Blueprint {
    pub fn leaf(shape: Shape) -> Self {
        Blueprint {
            shape,
            handles: vec![],
            drawers: vec![],
        }
    }

    /// Build a new blueprint by transforming this one's shape while
    /// carrying its handles and drawers forward. Used for single-child wrappers.
    pub fn map_shape(self, f: impl FnOnce(Shape) -> Shape) -> Self {
        Blueprint {
            shape: f(self.shape),
            handles: self.handles,
            drawers: self.drawers,
        }
    }

    /// Compose a parent blueprint from child blueprints, merging every
    /// child's handles and drawers into the result.
    pub fn with_children(
        children: Vec<Blueprint>,
        combine: impl FnOnce(Vec<Shape>) -> Shape,
    ) -> Self {
        let handles: Vec<BlockHandle> = children
            .iter()
            .flat_map(|c| c.handles.iter().cloned())
            .collect();
        let drawers: Vec<DrawerHandle> = children
            .iter()
            .flat_map(|c| c.drawers.iter().cloned())
            .collect();
        let shapes: Vec<Shape> = children.into_iter().map(|c| c.shape).collect();
        Blueprint {
            shape: combine(shapes),
            handles,
            drawers,
        }
    }
}
