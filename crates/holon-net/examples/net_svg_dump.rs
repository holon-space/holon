//! Dump the derived net as a standalone SVG: `just net-svg-dump <out.svg>`.
//!
//! Without a net file it derives the one the production block catalog
//! compiles to — the same `derive_net` call `BackendEngine::derived_net()`
//! makes, with the engine's rule blocks left out because booting an engine
//! for a drawing is not worth it. Pass a `CompiledNet` JSON (second argument,
//! e.g. one an MCP session dumped from a live engine) to draw THAT net
//! instead, rules included.

use std::collections::BTreeSet;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .ok_or("usage: net_svg_dump <out.svg> [compiled-net.json]")?;

    let net = match args.next() {
        Some(path) => serde_json::from_str(&std::fs::read_to_string(&path)?)?,
        None => {
            let mut ops = holon_core::__operations_crud_operations::crud_operations(
                "block", "block", "block", "id",
            );
            ops.extend(holon_core::__operations_block_operations::block_operations(
                "block", "block", "block", "id",
            ));
            holon_net::derive_net(&ops, &[])?
        }
    };

    let view = holon_net::net_view_of(&net);
    let laid_out = holon_net_layout::layout(&view, &BTreeSet::new())?;
    let title = format!(
        "derived net — {} transitions, {} places, {} arcs",
        view.nodes
            .iter()
            .filter(|n| n.kind == holon_net_layout::NodeKind::Transition)
            .count(),
        view.nodes
            .iter()
            .filter(|n| n.kind == holon_net_layout::NodeKind::Place)
            .count(),
        laid_out.arcs.len(),
    );
    std::fs::write(&out, holon_net_layout::to_svg(&laid_out, &title))?;
    println!(
        "{title}\nwrote {out} ({}x{})",
        laid_out.width, laid_out.height
    );
    Ok(())
}
