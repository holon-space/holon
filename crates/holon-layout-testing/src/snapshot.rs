//! `BoundsSnapshot` — a flat list of element bounds captured after a render pass.

use holon_frontend::geometry::ElementInfo;

/// Result of rendering a fixture: flat list of `(element_id, info)` in the
/// order they were recorded during prepaint.
pub struct BoundsSnapshot {
    pub entries: Vec<(String, ElementInfo)>,
}

impl BoundsSnapshot {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries whose `widget_type` matches `name`.
    pub fn of_type<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a ElementInfo> + 'a {
        self.entries
            .iter()
            .filter(move |(_, info)| info.widget_type == name)
            .map(|(_, info)| info)
    }

    /// Pretty, one-widget-per-line dump for failure messages.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (id, info) in &self.entries {
            out.push_str(&format!(
                "  {id:24} @ ({:>6.1},{:>6.1}) {:>6.1}×{:<6.1}\n",
                info.x, info.y, info.width, info.height,
            ));
        }
        out
    }

    /// Deterministic, indented textual dump of the render tree for snapshot
    /// testing. Each line carries `{indent}{widget_type} {w}x{h}` where
    /// `w`/`h` are integer-rounded pixels. The tree is reconstructed from
    /// `parent_id` so snapshots reflect the tracked render hierarchy.
    pub fn structural_dump(&self) -> String {
        use std::collections::BTreeMap;

        let mut children: BTreeMap<Option<&str>, Vec<&(String, ElementInfo)>> = BTreeMap::new();
        for entry in &self.entries {
            children
                .entry(entry.1.parent_id.as_deref())
                .or_default()
                .push(entry);
        }

        let mut out = String::new();
        fn walk(
            parent_key: Option<&str>,
            depth: usize,
            children: &std::collections::BTreeMap<Option<&str>, Vec<&(String, ElementInfo)>>,
            out: &mut String,
        ) {
            if let Some(entries) = children.get(&parent_key) {
                for (id, info) in entries {
                    for _ in 0..depth {
                        out.push_str("  ");
                    }
                    out.push_str(&format!(
                        "{} {}x{}\n",
                        info.widget_type,
                        info.width.round() as i32,
                        info.height.round() as i32
                    ));
                    walk(Some(id.as_str()), depth + 1, children, out);
                }
            }
        }
        walk(None, 0, &children, &mut out);
        out
    }
}

impl BoundsSnapshot {
    pub fn to_svg(&self) -> String {
        let (max_w, max_h) = self
            .entries
            .iter()
            .fold((0.0f32, 0.0f32), |(w, h), (_, info)| {
                (w.max(info.x + info.width), h.max(info.y + info.height))
            });
        let vw = max_w.ceil() as i32 + 10;
        let vh = max_h.ceil() as i32 + 10;

        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {vw} {vh}" width="{vw}" height="{vh}" style="background:#1a1a1a">"#
        );
        svg.push('\n');

        let colors = [
            "#2a5a5a", "#5a2a2a", "#2a2a5a", "#5a5a2a", "#3a4a3a", "#4a3a4a",
        ];

        for (i, (id, info)) in self.entries.iter().enumerate() {
            let color = colors[i % colors.len()];
            let short_id = id.split("::").last().unwrap_or(id);
            let label = if short_id.len() > 20 {
                &short_id[..20]
            } else {
                short_id
            };
            svg.push_str(&format!(
                "  <rect x=\"{:.0}\" y=\"{:.0}\" width=\"{:.0}\" height=\"{:.0}\" \
                 fill=\"{color}\" stroke=\"#888\" stroke-width=\"0.5\" opacity=\"0.7\"/>\n",
                info.x, info.y, info.width, info.height
            ));
            if info.width > 20.0 && info.height > 8.0 {
                svg.push_str(&format!(
                    "  <text x=\"{:.0}\" y=\"{:.0}\" font-size=\"7\" fill=\"#ccc\" \
                     font-family=\"monospace\">{} {:.0}x{:.0}</text>\n",
                    info.x + 2.0,
                    info.y + 8.0,
                    label,
                    info.width,
                    info.height
                ));
            }
        }

        svg.push_str("</svg>\n");
        svg
    }
}

/// Widget types that count as "actually visible content". Excludes pure
/// structural wrappers (`col`, `row`, `card`, etc.) because those nest
/// the visible bits but aren't themselves visible as content.
pub const VISIBLE_LEAF_TYPES: &[&str] = &[
    "text",
    "badge",
    "icon",
    "checkbox",
    "editable_text",
    "spacer",
    "state_toggle",
    "source_block",
    "source_editor",
];

/// Axis-aligned bounding rectangle.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn of(info: &ElementInfo) -> Self {
        Self {
            x0: info.x,
            y0: info.y,
            x1: info.x + info.width,
            y1: info.y + info.height,
        }
    }

    /// Epsilon-tolerant "is `self` inside `parent`" check. Subpixel rounding
    /// in Taffy can put a child a hair outside its parent; allow up to 0.5px.
    pub fn inside(&self, parent: Rect) -> bool {
        const EPS: f32 = 0.5;
        self.x0 >= parent.x0 - EPS
            && self.y0 >= parent.y0 - EPS
            && self.x1 <= parent.x1 + EPS
            && self.y1 <= parent.y1 + EPS
    }

    /// True if the two rects have a non-trivial intersection (ignoring
    /// shared edges, which flex layouts produce intentionally).
    pub fn overlaps(&self, other: Rect) -> bool {
        const EPS: f32 = 0.5;
        self.x0 < other.x1 - EPS
            && other.x0 < self.x1 - EPS
            && self.y0 < other.y1 - EPS
            && other.y0 < self.y1 - EPS
    }
}
