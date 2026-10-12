//! The layout input: a net's STRUCTURE and nothing else.
//!
//! No marking, no token, no count reaches this module. That is the whole
//! stability argument of the viewer: a token cannot move a node, because the
//! function that places nodes cannot see tokens.

use serde::Deserialize;
use serde::Serialize;

/// A place or a transition, identified by a string the producer owns
/// (`"block.task_state"`, `"op:block.set_field"`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(String);

impl NodeId {
    pub fn new(id: impl Into<String>) -> NodeId {
        NodeId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A group of nodes drawn as one box: a subnet, or — for the production
/// derived net, which has no subnets yet — one relation's places.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubnetId(String);

impl SubnetId {
    pub fn new(id: impl Into<String>) -> SubnetId {
        SubnetId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The two node sorts of a bipartite net. A collapsed subnet is NOT one of
/// them: it exists only in a [`crate::NetLayout`], so a caller cannot hand in
/// a node that pretends to be a collapsed group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Place,
    Transition,
}

/// What an arc does to the tokens in its place. The renderer draws the head:
/// an arrow for `Take`/`Produce`/`Relocate`, no head for `Read`, a circle for
/// `Inhibit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcKind {
    Take,
    Read,
    Inhibit,
    Produce,
    Relocate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetNode {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Display only. Node sizes are fixed, so a long label is truncated by
    /// the renderer and shown in full on hover.
    pub label: String,
}

/// A directed arc. `from`/`to` carry the drawing direction, so a `Take` arc
/// runs place → transition and a `Produce` arc transition → place.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NetViewArc {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: ArcKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetView {
    pub id: SubnetId,
    pub label: String,
    pub members: Vec<NodeId>,
}

/// One net's structure, in any order: [`crate::layout`] sorts everything it
/// reads, so two `NetView`s that differ only in insertion order lay out
/// byte-identically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetView {
    pub nodes: Vec<NetNode>,
    pub arcs: Vec<NetViewArc>,
    pub subnets: Vec<SubnetView>,
}

impl NetView {
    pub fn push_node(&mut self, id: impl Into<String>, kind: NodeKind, label: impl Into<String>) {
        self.nodes.push(NetNode {
            id: NodeId::new(id),
            kind,
            label: label.into(),
        });
    }

    pub fn push_arc(&mut self, from: impl Into<String>, to: impl Into<String>, kind: ArcKind) {
        self.arcs.push(NetViewArc {
            from: NodeId::new(from),
            to: NodeId::new(to),
            kind,
        });
    }
}
