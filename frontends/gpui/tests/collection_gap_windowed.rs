//! A collection's parsed `gap` is the spacing GPUI draws between its items, on
//! both render paths: the virtualized `gpui::list` a panel-placed collection
//! takes and the eager content-height div a nested one takes.
//!
//! Run: `cargo test -p holon-gpui --test collection_gap_windowed`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use support::ReactiveFixtureView;

const ITEM_COUNT: usize = 4;
/// Distinct from every layout's default gap and from the virtualized path's
/// old 2px floor.
const GAP: f32 = 11.0;

fn text_item(ix: usize) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".into(), Value::String(format!("block:gap-item-{ix}")));
    data.insert("content".into(), Value::String(format!("row {ix}")));
    let mut props = HashMap::new();
    props.insert("content".into(), Value::String(format!("row {ix}")));
    props.insert("field".into(), Value::String("content".into()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

/// A `table` collection of [`ITEM_COUNT`] text rows at `GAP`.
fn table() -> ReactiveViewModel {
    let items = (0..ITEM_COUNT).map(text_item).collect();
    let layout = CollectionVariant::from_name("table", GAP).expect("`table` is registered");
    ReactiveViewModel {
        collection: Some(Arc::new(ReactiveView::new_static_with_layout(
            items, layout,
        ))),
        ..ReactiveViewModel::from_widget("table", HashMap::new())
    }
}

/// `table` as the one row of a panel-placed `list`: a row of a virtualized list
/// is laid out at content height, so the table renders eagerly.
fn nested_in_a_list_row(table: ReactiveViewModel) -> ReactiveViewModel {
    ReactiveViewModel {
        collection: Some(Arc::new(ReactiveView::new_static_with_layout(
            vec![table],
            CollectionVariant::list(0.0),
        ))),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    }
}

/// The vertical space between each consecutive pair of rows.
fn row_spacings(root: ReactiveViewModel, cx: &mut TestAppContext) -> Vec<f32> {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let root = Arc::new(root);
    let (_entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, size(px(600.0), px(600.0)), bounds)
    });
    vcx.run_until_parked();
    bounds.flush();
    let mut rows: Vec<(usize, f32, f32)> = bounds
        .all_elements()
        .iter()
        .filter_map(|(_, info)| {
            let ix = info.entity_id.as_deref()?.strip_prefix("block:gap-item-")?;
            Some((ix.parse().expect("row index"), info.y, info.height))
        })
        .collect();
    rows.sort_by_key(|(ix, _, _)| *ix);
    rows.dedup_by_key(|(ix, _, _)| *ix);
    assert_eq!(rows.len(), ITEM_COUNT, "every row is laid out: {rows:?}");
    rows.windows(2)
        .map(|pair| pair[1].1 - (pair[0].1 + pair[0].2))
        .collect()
}

fn assert_gap(spacings: &[f32], path: &str) {
    for spacing in spacings {
        assert!(
            (spacing - GAP).abs() < 0.5,
            "{path}: a table at gap {GAP} draws its rows {spacings:?} apart"
        );
    }
}

#[gpui::test]
fn a_panel_placed_table_draws_its_gap(cx: &mut TestAppContext) {
    let spacings = row_spacings(table(), cx);
    assert_gap(&spacings, "virtualized panel path");
}

#[gpui::test]
fn a_nested_table_draws_its_gap(cx: &mut TestAppContext) {
    let spacings = row_spacings(nested_in_a_list_row(table()), cx);
    assert_gap(&spacings, "eager nested path");
}
