//! The layout contract: determinism, cycles, clusters, collapse, and a
//! golden geometry that reds when the pinned `dugong` moves.

use std::collections::BTreeSet;

use holon_net_layout::ArcKind;
use holon_net_layout::LayoutNodeKind;
use holon_net_layout::NetLayout;
use holon_net_layout::NetView;
use holon_net_layout::NodeId;
use holon_net_layout::NodeKind;
use holon_net_layout::SubnetId;
use holon_net_layout::SubnetView;
use holon_net_layout::layout;

/// Two chains `p -> t -> p' -> t'`, each chain in its own subnet, plus a
/// retry arc from the last transition back to the first place and one
/// cross-subnet arc. The shape the probe measured, small enough to golden.
fn chain_net() -> NetView {
    let mut view = NetView::default();
    for chain in 0..2 {
        for step in 0..2 {
            view.push_node(
                format!("rel{chain}.f{step}"),
                NodeKind::Place,
                format!("rel{chain}.f{step}"),
            );
            view.push_node(
                format!("op:rel{chain}.set{step}"),
                NodeKind::Transition,
                format!("rel{chain}.set{step}"),
            );
        }
        view.push_arc(
            format!("rel{chain}.f0"),
            format!("op:rel{chain}.set0"),
            ArcKind::Take,
        );
        view.push_arc(
            format!("op:rel{chain}.set0"),
            format!("rel{chain}.f1"),
            ArcKind::Produce,
        );
        view.push_arc(
            format!("rel{chain}.f1"),
            format!("op:rel{chain}.set1"),
            ArcKind::Read,
        );
        // The retry loop: back from the last transition to the first place.
        view.push_arc(
            format!("op:rel{chain}.set1"),
            format!("rel{chain}.f0"),
            ArcKind::Produce,
        );
        view.subnets.push(SubnetView {
            id: SubnetId::new(format!("rel{chain}")),
            label: format!("rel{chain}"),
            members: (0..2)
                .flat_map(|step| {
                    [
                        NodeId::new(format!("rel{chain}.f{step}")),
                        NodeId::new(format!("op:rel{chain}.set{step}")),
                    ]
                })
                .collect(),
        });
    }
    view.push_arc("rel0.f1", "op:rel1.set0", ArcKind::Read);
    view
}

fn reversed(view: &NetView) -> NetView {
    let mut shuffled = view.clone();
    shuffled.nodes.reverse();
    shuffled.arcs.reverse();
    shuffled.subnets.reverse();
    for subnet in &mut shuffled.subnets {
        subnet.members.reverse();
    }
    shuffled
}

fn json(layout: &NetLayout) -> String {
    serde_json::to_string_pretty(layout).expect("a layout serialises")
}

/// Same structure in, byte-equal geometry out — twice over, and again with
/// every input vector reversed. The reversal is the real teeth: it proves the
/// canonical ordering happens inside `layout`, not in the caller.
#[test]
fn equal_structure_lays_out_byte_equal() {
    let view = chain_net();
    let none = BTreeSet::new();
    let first = layout(&view, &none).expect("the chain net lays out");
    let second = layout(&view, &none).expect("the chain net lays out");
    assert_eq!(json(&first), json(&second), "two runs must agree");

    let shuffled = layout(&reversed(&view), &none).expect("the reversed net lays out");
    assert_eq!(
        json(&first),
        json(&shuffled),
        "insertion order must not reach the layout engine"
    );
    assert_eq!(
        first.nodes.len(),
        view.nodes.len(),
        "every node must be placed"
    );
    assert!(
        first.width > 0.0 && first.height > 0.0,
        "bounds must be set"
    );
}

/// A cyclic net lays out at all, and the arc that runs against the flow is
/// flagged so the renderer can curve it.
#[test]
fn a_retry_loop_is_laid_out_and_flagged_as_a_back_arc() {
    let view = chain_net();
    let laid_out = layout(&view, &BTreeSet::new()).expect("a cyclic net lays out");
    assert_eq!(
        laid_out.arcs.len(),
        view.arcs.len(),
        "every arc must be routed"
    );
    // WHICH arc of a cycle runs backwards is the greedy feedback-arc set's
    // choice, so the test pins the count per cycle, not the engine's pick:
    // each chain's retry loop costs exactly one back arc.
    let back: Vec<_> = laid_out
        .arcs
        .iter()
        .filter(|arc| arc.is_back_arc)
        .map(|arc| (arc.from.as_str(), arc.to.as_str()))
        .collect();
    assert_eq!(back.len(), 2, "one back arc per retry loop, got {back:?}");
    for chain in ["rel0", "rel1"] {
        assert_eq!(
            back.iter()
                .filter(|(from, to)| from.contains(chain) && to.contains(chain))
                .count(),
            1,
            "{chain}'s cycle must be broken by one of its own arcs, got {back:?}"
        );
    }
    for arc in &laid_out.arcs {
        assert!(
            arc.points.len() >= 2,
            "the arc {:?} -> {:?} has no polyline",
            arc.from,
            arc.to
        );
    }
}

/// An expanded subnet becomes a cluster box that encloses its members.
#[test]
fn an_expanded_subnet_gets_a_box_around_its_members() {
    let view = chain_net();
    let laid_out = layout(&view, &BTreeSet::new()).expect("the chain net lays out");
    assert_eq!(laid_out.clusters.len(), 2, "both subnets get a box");
    for cluster in &laid_out.clusters {
        let members: Vec<_> = laid_out
            .nodes
            .iter()
            .filter(|node| node.id.as_str().contains(cluster.id.as_str()))
            .collect();
        assert_eq!(members.len(), 4, "{} has four members", cluster.id.as_str());
        for member in members {
            assert!(
                member.x - member.width / 2.0 >= cluster.x - cluster.width / 2.0
                    && member.x + member.width / 2.0 <= cluster.x + cluster.width / 2.0
                    && member.y - member.height / 2.0 >= cluster.y - cluster.height / 2.0
                    && member.y + member.height / 2.0 <= cluster.y + cluster.height / 2.0,
                "{} lies outside its cluster {}",
                member.id.as_str(),
                cluster.id.as_str()
            );
        }
    }
}

/// Collapsing a subnet replaces its members by one node and rewires every
/// border-crossing arc onto it; the arcs inside the subnet vanish.
#[test]
fn collapsing_a_subnet_rewires_its_border_arcs() {
    let view = chain_net();
    let collapsed = BTreeSet::from([SubnetId::new("rel0")]);
    let laid_out = layout(&view, &collapsed).expect("the collapsed net lays out");

    assert!(
        laid_out.clusters.iter().all(|c| c.id.as_str() != "rel0"),
        "a collapsed subnet is a node, not a box"
    );
    let node = laid_out
        .nodes
        .iter()
        .find(|n| n.id.as_str() == "rel0")
        .expect("the collapsed subnet is one node");
    assert_eq!(node.kind, LayoutNodeKind::CollapsedSubnet);
    assert!(
        laid_out
            .nodes
            .iter()
            .all(|n| n.id.as_str() == "rel0" || !n.id.as_str().contains("rel0")),
        "no member of a collapsed subnet survives"
    );
    let crossing: Vec<_> = laid_out
        .arcs
        .iter()
        .filter(|arc| arc.from.as_str() == "rel0" || arc.to.as_str() == "rel0")
        .collect();
    assert_eq!(
        crossing.len(),
        1,
        "only the cross-subnet arc crosses rel0's border"
    );
    assert_eq!(crossing[0].to.as_str(), "op:rel1.set0");
}

/// A structure error is an error, never a quietly smaller layout.
#[test]
fn an_arc_to_an_unknown_node_is_refused() {
    let mut view = chain_net();
    view.push_arc("rel0.f0", "op:nowhere.set", ArcKind::Take);
    let error = layout(&view, &BTreeSet::new()).expect_err("a dangling arc has no layout");
    let message = error.to_string();
    assert!(
        message.contains("op:nowhere.set"),
        "the error must name the offender: {message}"
    );
}

/// A collapse request for a subnet that does not exist is a caller bug, and
/// is reported as one rather than silently ignored.
#[test]
fn collapsing_an_unknown_subnet_is_refused() {
    let view = chain_net();
    let error = layout(&view, &BTreeSet::from([SubnetId::new("nosuch")]))
        .expect_err("an unknown subnet cannot be collapsed");
    assert!(error.to_string().contains("nosuch"), "{error}");
}

/// The tripwire for the pinned `dugong`: its pipeline API is explicitly not a
/// supported surface, so a geometry change on upgrade must red here and not
/// in a frontend.
///
/// Update with `UPDATE_GOLDEN=1 cargo test -p holon-net-layout --test layout`
/// after READING the diff.
#[test]
fn the_chain_net_geometry_is_golden() {
    let laid_out = layout(&chain_net(), &BTreeSet::new()).expect("the chain net lays out");
    let golden = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/chain_net.json");
    let actual = json(&laid_out);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(golden, format!("{actual}\n")).expect("the golden file is writable");
        return;
    }
    let expected = std::fs::read_to_string(golden).expect("the golden file exists");
    assert_eq!(
        actual.trim(),
        expected.trim(),
        "the layout moved — read the diff, then UPDATE_GOLDEN=1 if the move is wanted"
    );
}
