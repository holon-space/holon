//! A standalone SVG dump of a finished layout.
//!
//! The debugging output that needs no frontend: pipe a net through
//! `just net-svg-dump` and open the file. It draws structure only — a
//! [`NetLayout`] holds no marking, so no token value can leak into a file an
//! agent or a bug report carries around.

use std::fmt::Write as _;

use crate::NetLayout;
use crate::layout::LayoutNodeKind;
use crate::net_view::ArcKind;

/// A self-contained SVG document. `title` names the net in `<title>` and in
/// the caption above the drawing.
pub fn to_svg(layout: &NetLayout, title: &str) -> String {
    let caption = 24.0;
    let mut svg = String::new();
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" font-family="ui-sans-serif, system-ui, sans-serif">
<title>{title}</title>
<defs>
<marker id="arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,0 L8,4 L0,8 z" fill="#475569"/></marker>
<marker id="inhibit" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><circle cx="4" cy="4" r="3" fill="none" stroke="#475569" stroke-width="1"/></marker>
</defs>
<rect width="100%" height="100%" fill="#f8fafc"/>
<text x="8" y="16" font-size="12" fill="#0f172a">{title}</text>
<g transform="translate(0,{caption})">
"##,
        w = layout.width.max(1.0),
        h = layout.height + caption,
        title = escape(title),
    );

    for cluster in &layout.clusters {
        let (x, y) = top_left(cluster.x, cluster.y, cluster.width, cluster.height);
        let _ = write!(
            svg,
            r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="6" fill="#e2e8f0" fill-opacity="0.5" stroke="#94a3b8" stroke-dasharray="4 3"/>
<text x="{tx}" y="{ty}" font-size="10" fill="#64748b">{label}</text>
"##,
            w = cluster.width,
            h = cluster.height,
            tx = x + 4.0,
            ty = y - 2.0,
            label = escape(&cluster.label),
        );
    }

    for arc in &layout.arcs {
        let points: Vec<String> = arc
            .points
            .iter()
            .map(|p| format!("{},{}", p.x, p.y))
            .collect();
        let (dash, head) = match arc.kind {
            // A read arc tests without consuming, so it carries no head.
            ArcKind::Read => (r##" stroke-dasharray="3 3""##, ""),
            ArcKind::Inhibit => (
                r##" stroke-dasharray="3 3""##,
                r##" marker-end="url(#inhibit)""##,
            ),
            _ => ("", r##" marker-end="url(#arrow)""##),
        };
        let stroke = if arc.is_back_arc {
            "#b45309"
        } else {
            "#475569"
        };
        let _ = writeln!(
            svg,
            r##"<polyline points="{points}" fill="none" stroke="{stroke}" stroke-width="1.2"{dash}{head}/>"##,
            points = points.join(" "),
        );
        if arc.multiplicity > 1 {
            let mid = arc.points[arc.points.len() / 2];
            let _ = write!(
                svg,
                r##"<text x="{x}" y="{y}" font-size="9" fill="#b45309">x{n}</text>
            "##,
                x = mid.x + 3.0,
                y = mid.y - 3.0,
                n = arc.multiplicity,
            );
        }
    }

    for node in &layout.nodes {
        let (x, y) = top_left(node.x, node.y, node.width, node.height);
        match node.kind {
            LayoutNodeKind::Place => {
                let _ = write!(
                    svg,
                    r##"<circle cx="{cx}" cy="{cy}" r="{r}" fill="#ffffff" stroke="#334155" stroke-width="1.4"><title>{label}</title></circle>
<text x="{cx}" y="{ty}" font-size="9" text-anchor="middle" fill="#334155">{short}</text>
"##,
                    cx = node.x,
                    cy = node.y,
                    r = node.width.min(node.height) / 2.0,
                    ty = y + node.height + 10.0,
                    label = escape(&node.label),
                    short = escape(&truncate(&node.label)),
                );
            }
            LayoutNodeKind::Transition => {
                let _ = write!(
                    svg,
                    r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#334155"><title>{label}</title></rect>
<text x="{cx}" y="{ty}" font-size="9" text-anchor="middle" fill="#334155">{short}</text>
"##,
                    w = node.width,
                    h = node.height,
                    cx = node.x,
                    ty = y + node.height + 10.0,
                    label = escape(&node.label),
                    short = escape(&truncate(&node.label)),
                );
            }
            LayoutNodeKind::CollapsedSubnet => {
                // The WoPeD/YAWL "subprocess" convention: a double border.
                let _ = write!(
                    svg,
                    r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="3" fill="#ffffff" stroke="#334155" stroke-width="1.4"><title>{label}</title></rect>
<rect x="{ix}" y="{iy}" width="{iw}" height="{ih}" rx="2" fill="none" stroke="#334155" stroke-width="1"/>
<text x="{cx}" y="{ty}" font-size="9" text-anchor="middle" fill="#334155">{short}</text>
"##,
                    w = node.width,
                    h = node.height,
                    ix = x + 3.0,
                    iy = y + 3.0,
                    iw = node.width - 6.0,
                    ih = node.height - 6.0,
                    cx = node.x,
                    ty = y + node.height + 10.0,
                    label = escape(&node.label),
                    short = escape(&truncate(&node.label)),
                );
            }
        }
    }

    svg.push_str("</g>\n</svg>\n");
    svg
}

fn top_left(x: f32, y: f32, width: f32, height: f32) -> (f32, f32) {
    (x - width / 2.0, y - height / 2.0)
}

/// Node sizes are fixed, so a long label is cut here and kept in full in the
/// `<title>` the viewer sees on hover.
fn truncate(label: &str) -> String {
    const MAX: usize = 18;
    if label.chars().count() <= MAX {
        return label.to_string();
    }
    let kept: String = label.chars().take(MAX - 1).collect();
    format!("{kept}…")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
