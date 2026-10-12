//! @c4 component
//! @c4 layer Core
//! Pattern: Adapter
//!
//! Petri-net viewer geometry: the layered layout of a net's structure, the
//! serde contract a net executor publishes to a viewer, and a standalone SVG
//! dump.
//!
//! Three rules hold the crate together:
//! - The layout input is STRUCTURE only ([`NetView`]). A marking cannot move a
//!   node, because [`layout`] cannot see one.
//! - Frontends get geometry, not graph semantics ([`NetLayout`]): both the GPUI
//!   and the Dioxus-Web renderer only draw.
//! - Nothing of `dugong`, the pinned dagre port underneath, is public here.
//!
//! wasm-clean: `serde` and `thiserror` beside `dugong`, nothing else, so the
//! browser frontend links the same layout code as the desktop one. Pinned by
//! `just check-net-layout-wasm`.

pub mod contract;
mod layout;
pub mod net_view;
mod svg;

pub use contract::MarkingSnapshot;
pub use contract::NetStructure;
pub use contract::StepDelta;
pub use contract::WhyNot;
pub use layout::LayoutArc;
pub use layout::LayoutCluster;
pub use layout::LayoutError;
pub use layout::LayoutNode;
pub use layout::LayoutNodeKind;
pub use layout::LayoutPoint;
pub use layout::NetLayout;
pub use layout::layout;
pub use net_view::ArcKind;
pub use net_view::NetNode;
pub use net_view::NetView;
pub use net_view::NetViewArc;
pub use net_view::NodeId;
pub use net_view::NodeKind;
pub use net_view::SubnetId;
pub use net_view::SubnetView;
pub use svg::to_svg;
