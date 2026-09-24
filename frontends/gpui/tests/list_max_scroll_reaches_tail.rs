//! A `ReactiveShell` list must scroll far enough to show its LAST row after
//! its rows change under it.
//!
//! `ReactiveShell` measures every row only on the first layout. Rows that
//! arrive later (a live query's `Replace`, a `Push`) stay unmeasured until
//! they are painted, and upstream `gpui::list` counts an unmeasured row as
//! 0px tall: the scroll range then ends long before the last row. The
//! holon-space zed fork estimates unmeasured heights from the measured ones;
//! these rungs pin that behaviour so a fork rebase cannot drop it silently.
//!
//! ONE wheel fling to the end, as a user does it: repeated wheels would each
//! measure another `LIST_OVERDRAW_PX` of rows and reach the tail without the
//! estimate.
//!
//! Run: cargo nextest run -p holon-gpui --test list_max_scroll_reaches_tail

mod support;

use std::sync::Arc;

use gpui::TestAppContext;
use gpui::point;
use gpui::px;
use gpui::size;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::ReactiveFixtureView;
use support::simulate_wheel_at;

const VIEWPORT_W: f32 = 600.0;
const VIEWPORT_H: f32 = 400.0;
const ROWS: usize = 200;

fn text_rows(prefix: &str, count: usize) -> Vec<Arc<ReactiveViewModel>> {
    (0..count)
        .map(|i| Arc::new(ReactiveViewModel::text(format!("{prefix} {i}"))))
        .collect()
}

enum LateRows {
    ReplaceAll,
    PushAfterFirstPaint,
}

fn max_scroll_shows_the_last_row(cx: &mut TestAppContext, late: LateRows) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let initial = 5;
    let view = Arc::new(ReactiveView::new_static_with_layout(
        (0..initial)
            .map(|i| ReactiveViewModel::text(format!("first {i}")))
            .collect(),
        CollectionVariant::list(0.0),
    ));
    let root = Arc::new(ReactiveViewModel {
        collection: Some(view.clone()),
        ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
    });
    let (entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ReactiveFixtureView::new(root, size(px(VIEWPORT_W), px(VIEWPORT_H)))
    });
    vcx.run_until_parked();

    match late {
        LateRows::ReplaceAll => view
            .items
            .lock_mut()
            .replace_cloned(text_rows("second", ROWS)),
        LateRows::PushAfterFirstPaint => {
            let mut items = view.items.lock_mut();
            for row in text_rows("pushed", ROWS - initial) {
                items.push_cloned(row);
            }
        }
    }
    vcx.run_until_parked();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&view))
        .expect("ReactiveShell entity missing — render pipeline bypassed the shell");
    let list_state = shell.read_with(vcx, |s, _| s.list_state_handle());
    assert_eq!(
        list_state.item_count(),
        ROWS,
        "precondition: the late rows reached the list"
    );

    let wheel_pos = point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0));
    simulate_wheel_at(vcx, wheel_pos, px(-100_000.0));
    vcx.run_until_parked();

    let last = ROWS - 1;
    let viewport = list_state.viewport_bounds();
    let last_row = list_state.bounds_for_item(last).unwrap_or_else(|| {
        panic!(
            "after scrolling to the maximum the last row ({last}) was not laid out; scroll top \
             is {:?} of {ROWS} rows, viewport {viewport:?}. The scroll range ends before the \
             tail: unmeasured rows count as 0px (holon-space zed fork `list: estimate unmeasured \
             item heights` missing?)",
            list_state.logical_scroll_top()
        )
    });
    assert!(
        last_row.bottom() <= viewport.bottom() + px(1.0),
        "the last row must be fully inside the viewport at maximum scroll: row {last_row:?}, \
         viewport {viewport:?}"
    );
}

#[gpui::test]
fn max_scroll_shows_the_last_row_after_the_rows_are_replaced(cx: &mut TestAppContext) {
    max_scroll_shows_the_last_row(cx, LateRows::ReplaceAll);
}

#[gpui::test]
fn max_scroll_shows_the_last_row_after_rows_are_pushed(cx: &mut TestAppContext) {
    max_scroll_shows_the_last_row(cx, LateRows::PushAfterFirstPaint);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
