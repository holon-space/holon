//! The real derived net must survive the lowering and lay out: the viewer's
//! first input is production's own net, not a hand-written sample.

use std::collections::BTreeSet;

use holon_net_layout::NodeKind;

fn block_net() -> holon_net::CompiledNet {
    let mut ops =
        holon_core::__operations_crud_operations::crud_operations("block", "block", "block", "id");
    ops.extend(holon_core::__operations_block_operations::block_operations(
        "block", "block", "block", "id",
    ));
    holon_net::derive_net(&ops, &[]).expect("the block catalog compiles")
}

#[test]
fn the_derived_block_net_lowers_and_lays_out() {
    let net = block_net();
    let view = holon_net::net_view_of(&net);

    let transitions: Vec<_> = view
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Transition)
        .collect();
    assert_eq!(
        transitions.len(),
        net.transitions.len(),
        "every transition is lowered"
    );

    // An arc-less transition is legal — an operation that declares neither
    // half compiles to one — but ONLY then, and the drawing must say so:
    // anything else is a lowering that dropped arcs.
    for transition in &transitions {
        let drawn = view
            .arcs
            .iter()
            .any(|arc| arc.from == transition.id || arc.to == transition.id);
        let unanalyzable = transition.label.ends_with("(unanalyzable)");
        assert_eq!(
            drawn,
            !unanalyzable,
            "{} has arcs={drawn} but is labelled {:?}",
            transition.id.as_str(),
            transition.label
        );
    }

    let laid_out =
        holon_net_layout::layout(&view, &BTreeSet::new()).expect("the production net has a layout");
    assert_eq!(laid_out.nodes.len(), view.nodes.len());
    assert_eq!(
        laid_out.clusters.len(),
        view.subnets.len(),
        "each relation gets a box"
    );
}
