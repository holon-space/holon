//! The derived net as viewer input: [`CompiledNet`] → `NetView`.
//!
//! The one lowering from Holon's net vocabulary to the viewer's geometry
//! crate, so a renderer never reads a `NetTransition` and the two cannot
//! drift. Structure only: a `NetView` has no marking by construction, which
//! is what keeps a token from moving a node.

use holon_net_layout::ArcKind;
use holon_net_layout::NetView;
use holon_net_layout::NodeId;
use holon_net_layout::NodeKind;
use holon_net_layout::SubnetView;

use crate::Analyzability;
use crate::CompiledNet;
use crate::Flow;
use crate::net::NetArc;

/// Lower the compiled net for the viewer.
///
/// Production has no subnets yet (ADR 0032 §2 derives one flat net), so the
/// cluster a renderer draws is the RELATION: every place of `block` falls
/// into one box. That is the grouping a reader of the real derived net wants,
/// and it needs no declaration the compiler does not already have.
pub fn net_view_of(net: &CompiledNet) -> NetView {
    let mut view = NetView::default();
    let mut places: Vec<String> = Vec::new();

    for transition in &net.transitions {
        let key = transition.key();
        // An operation that declares neither half compiles to a transition
        // with no arcs at all. It draws as a bar standing beside the net, and
        // saying so in the label is what keeps that from reading as a lost
        // arc (ADR 0032 §4: "cannot say", never "touches nothing").
        let label = match transition.analyzability {
            Analyzability::Analyzable => transition.source.label(),
            Analyzability::Unanalyzable { .. } => {
                format!("{} (unanalyzable)", transition.source.label())
            }
        };
        view.push_node(key.as_str(), NodeKind::Transition, label);
        for arc in transition.arcs() {
            let place = format!("{}.{}", arc.place.relation.as_str(), arc.place.field);
            if !places.contains(&place) {
                places.push(place.clone());
                view.push_node(&place, NodeKind::Place, &place);
            }
            let kind = arc_kind(arc);
            match kind {
                // Drawn transition → place: what a firing writes.
                ArcKind::Produce => view.push_arc(key.as_str(), &place, kind),
                // Drawn place → transition: what a firing tests or takes.
                _ => view.push_arc(&place, key.as_str(), kind),
            }
        }
    }

    // `layout` sorts its input, so the arcs' duplicates across a
    // transition's DNF modes collapse there; the subnets must still be
    // unique, hence the grouping by relation name here.
    let mut relations: Vec<&str> = places
        .iter()
        .map(|place| place.split_once('.').expect("a place is relation.field").0)
        .collect();
    relations.sort_unstable();
    relations.dedup();
    for relation in relations {
        view.subnets.push(SubnetView {
            id: holon_net_layout::SubnetId::new(relation),
            label: relation.to_string(),
            members: places
                .iter()
                .filter(|place| place.starts_with(&format!("{relation}.")))
                .map(NodeId::new)
                .collect(),
        });
    }
    view
}

fn arc_kind(arc: &NetArc) -> ArcKind {
    match arc.flow {
        Flow::Read => ArcKind::Read,
        Flow::Consume => ArcKind::Take,
        Flow::Produce => ArcKind::Produce,
        Flow::Relocate => ArcKind::Relocate,
    }
}
