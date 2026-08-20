//! Columnar table layout: a header row plus one row per data row, every column
//! left-aligned across the header and all rows.
//!
//! Registered as the renderer for the `table_columnar` LAYOUT, not as a widget
//! builder — the node is a collection, so it arrives through
//! `layout_renderer::lookup_renderer`. A bare `table` keeps the `table` layout
//! and the default `ReactiveShell` path, untouched.
//!
//! Alignment is not emergent. The builder ships one column-width vector in the
//! node props (`col{k}_width`), and this renderer applies the SAME vector to
//! the header and to every row. Fixed columns take their px; flex columns share
//! the remainder by weight. Because every row is a full-width flex row fed the
//! same vector, column `k` starts at the same x everywhere.
//!
//! Each header and cell is tracked under the contract id the windowed PBT reads
//! (`table-header-col-{k}`, `table-cell-col-{k}-{row}`). A cell hosts its
//! column's interpreted render-expr verbatim, so an interactive cell
//! (state_toggle, an ops list of op_buttons) keeps its wiring.

use holon_frontend::reactive_view_model::ReactiveViewModel;

use super::prelude::*;
use crate::geometry::TransparentTracker;

const COLUMN_GAP: f32 = 8.0;

enum ColWidth {
    Flex(f32),
    Fixed(f32),
}

fn parse_widths(node: &ReactiveViewModel) -> Vec<ColWidth> {
    let raw_count = node
        .prop_str("col_count")
        .unwrap_or_else(|| panic!("table node is missing a `col_count` prop"));
    let count: usize = raw_count
        .parse()
        .unwrap_or_else(|e| panic!("table `col_count` prop {raw_count:?} is not a number: {e}"));
    (0..count)
        .map(|k| {
            let raw = node
                .prop_str(&format!("col{k}_width"))
                .unwrap_or_else(|| panic!("table node is missing `col{k}_width`"));
            let (kind, n) = raw
                .split_once(':')
                .unwrap_or_else(|| panic!("malformed table width prop {raw:?}"));
            let n: f32 = n
                .parse()
                .unwrap_or_else(|_| panic!("non-numeric table width {raw:?}"));
            match kind {
                "flex" => ColWidth::Flex(n),
                "fixed" => ColWidth::Fixed(n),
                other => panic!("unknown table width kind {other:?}"),
            }
        })
        .collect()
}

fn header_label(node: &ReactiveViewModel, k: usize) -> String {
    node.prop_str(&format!("col{k}_header"))
        .unwrap_or_else(|| panic!("table node is missing `col{k}_header`"))
}

/// Apply a column's width to the flex item that carries the cell content.
fn sized(mut cell: Div, width: &ColWidth) -> Div {
    match width {
        ColWidth::Fixed(px_w) => cell.flex_shrink_0().w(px(*px_w)),
        // `Styled`'s flex helpers are weightless (grow is always 1), so the
        // per-column weight goes onto the style refinement directly.
        //
        // `min_size.width = 0` is what makes the column a pure function of its
        // weight: a flex item's automatic minimum is its MIN-CONTENT width, so
        // without this the widest cell in a column widens that column for its
        // row alone and the columns come out ragged.
        ColWidth::Flex(weight) => {
            let style = cell.style();
            style.flex_grow = Some(*weight);
            style.flex_shrink = Some(1.0);
            style.flex_basis = Some(px(0.0).into());
            style.min_size.width = Some(px(0.0).into());
            cell
        }
    }
}

fn row_container() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .gap(px(COLUMN_GAP))
}

/// The rows the table draws: the collection's current items.
fn rows(node: &ReactiveViewModel) -> Vec<std::sync::Arc<ReactiveViewModel>> {
    match node.collection {
        Some(ref view) => view.items.lock_ref().iter().cloned().collect(),
        None => node.children.to_vec(),
    }
}

/// The key a cell's contract id carries, joining a painted element to its row.
fn row_key(row: &ReactiveViewModel, index: usize) -> String {
    row.entity_id()
        .as_ref()
        .map(|u| u.to_string())
        .unwrap_or_else(|| format!("row{index}"))
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let widths = parse_widths(node);
    let fg = tc(ctx, |c| c.foreground);

    let mut header = row_container();
    for (k, width) in widths.iter().enumerate() {
        let label = header_label(node, k);
        let cell = sized(
            div()
                .child(label.clone())
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(fg),
            width,
        );
        header = header.child(
            TransparentTracker::new(
                format!("table-header-col-{k}"),
                "table_header",
                ctx.bounds_registry.clone(),
                cell.into_any_element(),
            )
            .with_displayed_text(label),
        );
    }

    let mut container = div().flex().flex_col().w_full().gap(px(4.0)).child(header);

    for (index, row) in rows(node).iter().enumerate() {
        let key = row_key(row, index);
        // The row template is synthesized from the SAME column vector these
        // widths come from, so a short row is an internal inconsistency, not a
        // shape the data can produce. Painting the missing cells blank would
        // hide it behind a table that merely looks under-filled.
        // `>=`, NOT `==`: advice weaving legitimately appends EXTRA children
        // past `col_count` (prelude.rs `weave_advice_into_item`); only a SHORT
        // row violates the invariant. Do not tighten this bound.
        assert!(
            row.children.len() >= widths.len(),
            "table row {key:?} has {} cells but the table has {} columns — the synthesized row \
             template and the column vector have diverged",
            row.children.len(),
            widths.len(),
        );
        let mut row_div = row_container();
        for (k, width) in widths.iter().enumerate() {
            let inner = super::render(&row.children[k], ctx);
            let cell = sized(div().child(inner), width);
            row_div = row_div.child(TransparentTracker::new(
                format!("table-cell-col-{k}-{key}"),
                "table_cell",
                ctx.bounds_registry.clone(),
                cell.into_any_element(),
            ));
        }
        container = container.child(row_div);
    }

    container.into_any_element()
}
