//! Layered layout of a net's structure, over `dugong` (a port of dagre).
//!
//! Nothing of `dugong` appears in this module's public API: the crate pins an
//! exact `dugong` version and owns [`NetLayout`], so an upstream rename is a
//! compile error here rather than a break in every renderer.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use dugong::EdgeLabel;
use dugong::GraphLabel;
use dugong::NodeLabel;
use dugong::RankDir;
use serde::Deserialize;
use serde::Serialize;

use crate::net_view::ArcKind;
use crate::net_view::NetView;
use crate::net_view::NodeId;
use crate::net_view::NodeKind;
use crate::net_view::SubnetId;

/// Fixed node sizes, in layout units. Fixed is the point: a token decoration
/// draws inside or beside a node and never resizes it, so a marking change
/// cannot move anything.
const PLACE_SIZE: (f64, f64) = (32.0, 32.0);
/// Left-to-right flow, so a transition is a vertical bar.
const TRANSITION_SIZE: (f64, f64) = (12.0, 40.0);
const COLLAPSED_SUBNET_SIZE: (f64, f64) = (56.0, 40.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutNodeKind {
    Place,
    Transition,
    /// A subnet the caller asked to collapse: one node standing for all its
    /// members, with every border-crossing arc rewired onto it.
    CollapsedSubnet,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayoutPoint {
    pub x: f32,
    pub y: f32,
}

/// A laid-out node. `x`/`y` are the node's CENTRE, as dagre reports it; a
/// renderer that wants a top-left corner subtracts half the size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutNode {
    pub id: NodeId,
    pub kind: LayoutNodeKind,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The bounding box of an EXPANDED subnet. A collapsed one is a
/// [`LayoutNode`] instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutCluster {
    pub id: SubnetId,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutArc {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: ArcKind,
    /// How many input arcs this one polyline stands for. Above 1 only after a
    /// collapse merged several border-crossing arcs; the renderer shows it as
    /// a badge.
    pub multiplicity: u32,
    /// The arc runs against the flow direction — a retry loop or a bounded
    /// cycle. Derived from the laid-out coordinates, so it is true of what is
    /// drawn rather than of an intermediate ranking step.
    pub is_back_arc: bool,
    pub points: Vec<LayoutPoint>,
}

/// Geometry, not graph semantics: both frontends only draw this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetLayout {
    pub width: f32,
    pub height: f32,
    /// Sorted by `(kind, id)`.
    pub nodes: Vec<LayoutNode>,
    /// Sorted by `id`.
    pub clusters: Vec<LayoutCluster>,
    /// Sorted by `(from, to, kind)`.
    pub arcs: Vec<LayoutArc>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("the net view declares the node {0:?} twice")]
    DuplicateNode(NodeId),
    #[error("the net view declares the subnet {0:?} twice")]
    DuplicateSubnet(SubnetId),
    #[error("the arc {from:?} -> {to:?} ({kind:?}) names the unknown node {unknown:?}")]
    UnknownArcEndpoint {
        from: NodeId,
        to: NodeId,
        kind: ArcKind,
        unknown: NodeId,
    },
    #[error("the subnet {subnet:?} names the unknown member {member:?}")]
    UnknownSubnetMember { subnet: SubnetId, member: NodeId },
    #[error("the node {node:?} belongs to both subnet {first:?} and subnet {second:?}")]
    NodeInTwoSubnets {
        node: NodeId,
        first: SubnetId,
        second: SubnetId,
    },
    #[error("the subnet {0:?} has no members")]
    EmptySubnet(SubnetId),
    #[error("the collapse set names the unknown subnet {0:?}")]
    UnknownCollapsedSubnet(SubnetId),
    #[error("the subnet {0:?} has the same id as a node")]
    SubnetIdClash(SubnetId),
    #[error("dugong refused the graph: {0}")]
    Engine(String),
    #[error("dugong left the node {0:?} without coordinates")]
    MissingGeometry(NodeId),
}

/// Lay out `view`, drawing every subnet in `collapsed` as one node.
///
/// Deterministic: everything read from `view` is sorted first, so insertion
/// order cannot reach the engine.
pub fn layout(view: &NetView, collapsed: &BTreeSet<SubnetId>) -> Result<NetLayout, LayoutError> {
    let plan = Plan::build(view, collapsed)?;
    let mut graph = plan.to_graph();
    dugong::layout(&mut graph).map_err(|e| LayoutError::Engine(format!("{e:?}")))?;
    plan.read_back(&graph)
}

/// One laid-out node before the engine runs: a place, a transition, or a
/// collapsed subnet, with the size its kind fixes.
struct PlannedNode {
    kind: LayoutNodeKind,
    label: String,
    /// The expanded subnet this node is drawn inside, if any.
    cluster: Option<SubnetId>,
}

impl PlannedNode {
    fn size(&self) -> (f64, f64) {
        match self.kind {
            LayoutNodeKind::Place => PLACE_SIZE,
            LayoutNodeKind::Transition => TRANSITION_SIZE,
            LayoutNodeKind::CollapsedSubnet => COLLAPSED_SUBNET_SIZE,
        }
    }
}

/// The validated, collapsed and canonically ordered graph to lay out. Built
/// from `BTreeMap`s, so the engine sees one order for one structure however
/// the caller spelled it.
struct Plan {
    nodes: BTreeMap<NodeId, PlannedNode>,
    /// `(from, to, kind) -> how many input arcs merged into it`.
    arcs: BTreeMap<(NodeId, NodeId, ArcKind), u32>,
    clusters: BTreeMap<SubnetId, String>,
}

impl Plan {
    fn build(view: &NetView, collapsed: &BTreeSet<SubnetId>) -> Result<Plan, LayoutError> {
        let mut kinds: BTreeMap<NodeId, (NodeKind, String)> = BTreeMap::new();
        for node in &view.nodes {
            if kinds
                .insert(node.id.clone(), (node.kind, node.label.clone()))
                .is_some()
            {
                return Err(LayoutError::DuplicateNode(node.id.clone()));
            }
        }

        // Which subnet owns which member, and which subnet is drawn as one
        // node. A member of a collapsed subnet is replaced by that subnet.
        let mut owner: BTreeMap<NodeId, SubnetId> = BTreeMap::new();
        let mut subnet_labels: BTreeMap<SubnetId, String> = BTreeMap::new();
        for subnet in &view.subnets {
            if subnet_labels
                .insert(subnet.id.clone(), subnet.label.clone())
                .is_some()
            {
                return Err(LayoutError::DuplicateSubnet(subnet.id.clone()));
            }
            if subnet.members.is_empty() {
                return Err(LayoutError::EmptySubnet(subnet.id.clone()));
            }
            // Collapsed, the id would mint a node that already exists;
            // expanded, cluster and node fuse into one engine node.
            if kinds.contains_key(&NodeId::new(subnet.id.as_str())) {
                return Err(LayoutError::SubnetIdClash(subnet.id.clone()));
            }
            for member in &subnet.members {
                if !kinds.contains_key(member) {
                    return Err(LayoutError::UnknownSubnetMember {
                        subnet: subnet.id.clone(),
                        member: member.clone(),
                    });
                }
                if let Some(first) = owner.insert(member.clone(), subnet.id.clone()) {
                    return Err(LayoutError::NodeInTwoSubnets {
                        node: member.clone(),
                        first,
                        second: subnet.id.clone(),
                    });
                }
            }
        }
        for subnet in collapsed {
            if !subnet_labels.contains_key(subnet) {
                return Err(LayoutError::UnknownCollapsedSubnet(subnet.clone()));
            }
        }

        let stand_in = |id: &NodeId| -> NodeId {
            match owner.get(id) {
                Some(subnet) if collapsed.contains(subnet) => NodeId::new(subnet.as_str()),
                _ => id.clone(),
            }
        };

        let mut nodes: BTreeMap<NodeId, PlannedNode> = BTreeMap::new();
        for (id, (kind, label)) in &kinds {
            let Some(subnet) = owner.get(id) else {
                nodes.insert(
                    id.clone(),
                    PlannedNode {
                        kind: layout_kind(*kind),
                        label: label.clone(),
                        cluster: None,
                    },
                );
                continue;
            };
            if collapsed.contains(subnet) {
                continue;
            }
            nodes.insert(
                id.clone(),
                PlannedNode {
                    kind: layout_kind(*kind),
                    label: label.clone(),
                    cluster: Some(subnet.clone()),
                },
            );
        }
        for subnet in collapsed {
            nodes.insert(
                NodeId::new(subnet.as_str()),
                PlannedNode {
                    kind: LayoutNodeKind::CollapsedSubnet,
                    label: subnet_labels[subnet].clone(),
                    cluster: None,
                },
            );
        }

        let mut arcs: BTreeMap<(NodeId, NodeId, ArcKind), u32> = BTreeMap::new();
        for arc in &view.arcs {
            for end in [&arc.from, &arc.to] {
                if !kinds.contains_key(end) {
                    return Err(LayoutError::UnknownArcEndpoint {
                        from: arc.from.clone(),
                        to: arc.to.clone(),
                        kind: arc.kind,
                        unknown: end.clone(),
                    });
                }
            }
            let (from, to) = (stand_in(&arc.from), stand_in(&arc.to));
            // An arc wholly inside a collapsed subnet has nothing left to
            // connect; the badge on the border arcs is what survives.
            if from == to {
                continue;
            }
            *arcs.entry((from, to, arc.kind)).or_insert(0) += 1;
        }

        let clusters = subnet_labels
            .into_iter()
            .filter(|(id, _)| !collapsed.contains(id))
            .collect();
        Ok(Plan {
            nodes,
            arcs,
            clusters,
        })
    }

    fn to_graph(&self) -> dugong::graphlib::Graph<NodeLabel, EdgeLabel, GraphLabel> {
        let mut graph = dugong::graphlib::Graph::new(dugong::graphlib::GraphOptions {
            multigraph: true,
            compound: true,
            directed: true,
        });
        graph.set_graph(GraphLabel {
            rankdir: RankDir::LR,
            nodesep: 24.0,
            ranksep: 48.0,
            edgesep: 12.0,
            marginx: 16.0,
            marginy: 16.0,
            ..Default::default()
        });
        graph.set_default_edge_label(EdgeLabel::default);

        for (id, label) in &self.clusters {
            let _ = label;
            graph.set_node(id.as_str(), NodeLabel::default());
        }
        for (id, node) in &self.nodes {
            let (width, height) = node.size();
            graph.set_node(
                id.as_str(),
                NodeLabel {
                    width,
                    height,
                    ..Default::default()
                },
            );
            if let Some(cluster) = &node.cluster {
                graph.set_parent(id.as_str(), cluster.as_str());
            }
        }
        for (from, to, kind) in self.arcs.keys() {
            graph.set_edge_named(
                from.as_str(),
                to.as_str(),
                Some(edge_name(*kind)),
                Some(EdgeLabel {
                    // dugong's `EdgeLabel::default` leaves `weight` at 0,
                    // where dagre's default is 1. A zero-weight graph ranks
                    // every node into one layer.
                    weight: 1.0,
                    ..Default::default()
                }),
            );
        }
        graph
    }

    fn read_back(
        &self,
        graph: &dugong::graphlib::Graph<NodeLabel, EdgeLabel, GraphLabel>,
    ) -> Result<NetLayout, LayoutError> {
        let geometry = |id: &str| -> Result<(f32, f32, f32, f32), LayoutError> {
            let node = graph
                .node(id)
                .ok_or_else(|| LayoutError::MissingGeometry(NodeId::new(id)))?;
            let (Some(x), Some(y)) = (node.x, node.y) else {
                return Err(LayoutError::MissingGeometry(NodeId::new(id)));
            };
            Ok((x as f32, y as f32, node.width as f32, node.height as f32))
        };

        let mut nodes = Vec::with_capacity(self.nodes.len());
        for (id, planned) in &self.nodes {
            let (x, y, width, height) = geometry(id.as_str())?;
            nodes.push(LayoutNode {
                id: id.clone(),
                kind: planned.kind,
                label: planned.label.clone(),
                x,
                y,
                width,
                height,
            });
        }
        nodes.sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));

        let mut clusters = Vec::with_capacity(self.clusters.len());
        for (id, label) in &self.clusters {
            let (x, y, width, height) = geometry(id.as_str())?;
            clusters.push(LayoutCluster {
                id: id.clone(),
                label: label.clone(),
                x,
                y,
                width,
                height,
            });
        }

        let centre: BTreeMap<&NodeId, f32> = nodes.iter().map(|n| (&n.id, n.x)).collect();
        let mut arcs = Vec::with_capacity(self.arcs.len());
        for ((from, to, kind), multiplicity) in &self.arcs {
            let edge = graph
                .edge(from.as_str(), to.as_str(), Some(edge_name(*kind)))
                .ok_or_else(|| LayoutError::MissingGeometry(from.clone()))?;
            arcs.push(LayoutArc {
                from: from.clone(),
                to: to.clone(),
                kind: *kind,
                multiplicity: *multiplicity,
                // Left-to-right flow, so "backwards" is literally "its head
                // sits left of its tail".
                is_back_arc: centre[to] < centre[from],
                points: edge
                    .points
                    .iter()
                    .map(|p| LayoutPoint {
                        x: p.x as f32,
                        y: p.y as f32,
                    })
                    .collect(),
            });
        }

        Ok(NetLayout {
            width: graph.graph().width as f32,
            height: graph.graph().height as f32,
            nodes,
            clusters,
            arcs,
        })
    }
}

fn layout_kind(kind: NodeKind) -> LayoutNodeKind {
    match kind {
        NodeKind::Place => LayoutNodeKind::Place,
        NodeKind::Transition => LayoutNodeKind::Transition,
    }
}

/// Two nodes may be joined by arcs of several kinds at once (a transition
/// that reads and writes the same place), so the kind is the multigraph edge
/// name.
fn edge_name(kind: ArcKind) -> &'static str {
    match kind {
        ArcKind::Take => "take",
        ArcKind::Read => "read",
        ArcKind::Inhibit => "inhibit",
        ArcKind::Produce => "produce",
        ArcKind::Relocate => "relocate",
    }
}
