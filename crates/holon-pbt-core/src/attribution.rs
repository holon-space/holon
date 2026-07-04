//! Pipeline attribution: where an invariant sits in the single-edit data flow,
//! and where its wiring lives.
//!
//! Every [`CapInvariant`](crate::composition::CapInvariant) carries an
//! [`Attribution`] because its constructors take one — an unattributed
//! invariant is not constructible, so the first-divergent verdict never has to
//! guess a layer or disclose an unmapped id.
//!
//! Attribution belongs to the WIRING, not the body: one comparator fans out to
//! facets at different layers (the `blocks-match-ref` observable spans `/loro`
//! = store, `/matview` = projection, `/org` = org round-trip).

/// The single-edit data-flow pipeline. Declared bottom→top: `derive(Ord)` makes
/// `StoreCrdt < Projection < … < OrgRoundTrip`, so `min` over the failing
/// layers is the first-divergent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// Block store, tree structure, Loro/CRDT consolidator (Model.md layers
    /// 1–2).
    StoreCrdt,
    /// Turso base tables + materialized views + SQL projection (Model.md layer
    /// 3).
    Projection,
    /// Reactive pipeline → ViewModel tree, focus, watches, value-fns, editor
    /// mirror (Model.md layers 4–5, headless).
    ViewModel,
    /// Windowed paint: bounds registry, wheel/scroll, displayed widget text,
    /// draggable handles (Model.md layer 5, windowed).
    Render,
    /// Org-file writeback replica: render fixed point, per-page files, page
    /// headings (Model.md layer 1, the org replica).
    OrgRoundTrip,
}

/// Every pipeline layer, bottom→top. Used to enumerate the layers BELOW a
/// failing one so each gets an explicit verified/unverified disposition.
pub const ALL_LAYERS: &[Layer] = &[
    Layer::StoreCrdt,
    Layer::Projection,
    Layer::ViewModel,
    Layer::Render,
    Layer::OrgRoundTrip,
];

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::StoreCrdt => "store/CRDT",
            Layer::Projection => "matview/SQL",
            Layer::ViewModel => "viewmodel",
            Layer::Render => "render",
            Layer::OrgRoundTrip => "org round-trip",
        }
    }
}

/// An invariant's position in the pipeline. [`Position::CrossCutting`] is a
/// declared decision — a health/budget guard with no single layer — never a
/// default that silence produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Layer(Layer),
    CrossCutting,
}

/// An invariant's pipeline position plus the source to open when it is the
/// verdict. Both halves travel together: a position without a wiring pointer,
/// or the reverse, is not constructible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attribution {
    position: Position,
    wiring: &'static str,
}

impl Attribution {
    /// Attribute to a pipeline layer. Pass `file!()` as `wiring`: the
    /// constructor cannot capture it, so it expands at the wiring site.
    pub const fn at(layer: Layer, wiring: &'static str) -> Self {
        Self {
            position: Position::Layer(layer),
            wiring,
        }
    }

    /// A health/budget guard with no single pipeline position, reported apart
    /// from the layer ordering instead of forced into it.
    pub const fn cross_cutting(wiring: &'static str) -> Self {
        Self {
            position: Position::CrossCutting,
            wiring,
        }
    }

    /// `None` for a cross-cutting guard — it has no place in the ordering.
    pub fn layer(&self) -> Option<Layer> {
        match self.position {
            Position::Layer(l) => Some(l),
            Position::CrossCutting => None,
        }
    }

    pub fn wiring(&self) -> &'static str {
        self.wiring
    }
}
