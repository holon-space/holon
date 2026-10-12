//! The SVG dump is the evidence artefact of the viewer's first step, so it
//! must draw every node and must never carry a value.

use std::collections::BTreeSet;

use holon_net_layout::ArcKind;
use holon_net_layout::NetView;
use holon_net_layout::NodeKind;
use holon_net_layout::layout;
use holon_net_layout::to_svg;

#[test]
fn every_node_and_arc_reaches_the_svg() {
    let mut view = NetView::default();
    view.push_node("block.task_state", NodeKind::Place, "block.task_state");
    view.push_node("op:block.set_field", NodeKind::Transition, "set_field");
    view.push_arc("block.task_state", "op:block.set_field", ArcKind::Take);
    view.push_arc("op:block.set_field", "block.task_state", ArcKind::Produce);

    let laid_out = layout(&view, &BTreeSet::new()).expect("the net lays out");
    let svg = to_svg(&laid_out, "block ops");

    assert!(svg.starts_with("<svg "), "a standalone document");
    assert!(svg.ends_with("</svg>\n"));
    // `</circle>` and `</rect>`: only a NODE carries a `<title>` child, so
    // the closing tag counts nodes and never the `<defs>` arrow markers.
    assert_eq!(svg.matches("</circle>").count(), 1, "one place");
    assert_eq!(svg.matches("</rect>").count(), 1, "one transition");
    assert_eq!(svg.matches("<polyline ").count(), 2, "both arcs");
    assert!(svg.contains("block.task_state"), "labels are drawn");
}

/// A label that could break the document is escaped, not emitted raw.
#[test]
fn a_label_with_markup_is_escaped() {
    let mut view = NetView::default();
    view.push_node("p", NodeKind::Place, "<script>&\"");
    let laid_out = layout(&view, &BTreeSet::new()).expect("the net lays out");
    let svg = to_svg(&laid_out, "<title>");
    assert!(!svg.contains("<script>"), "{svg}");
    assert!(svg.contains("&lt;script&gt;&amp;&quot;"), "{svg}");
}
