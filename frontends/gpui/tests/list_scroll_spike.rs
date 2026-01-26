//! Spike A — can we drive `gpui::list` scrolling deterministically and have
//! the resulting bounds end up in our `BoundsRegistry` keyed by `entity_id`?
//!
//! The plan B refactor (devlog 2026-05-12-handoff-live-geometry-replaces-
//! region-prediction.md) wants the PBT to test sidebar/main-panel scrolling
//! as a real user verb: pick a target, scroll-into-view, then click. This
//! spike answers the load-bearing question before we touch the trait
//! surface: given a `gpui::list` with variable-height items, can we call
//! `ListState::scroll_to_reveal_item(N)` from outside and find the item's
//! `entity_id` in `BoundsRegistry` after the next paint? If yes, the rest
//! of the migration is mechanical.
//!
//! Run: `cargo test -p holon-gpui --test list_scroll_spike`

use gpui::Context;
use gpui::IntoElement;
use gpui::ListAlignment;
use gpui::ListState;
use gpui::Render;
use gpui::TestAppContext;
use gpui::Window;
use gpui::div;
use gpui::list;
use gpui::prelude::*;
use gpui::px;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::geometry::TransparentTracker;

const ITEM_COUNT: usize = 50;
/// Heights alternate so items are non-uniform — `list` not `uniform_list`.
const SHORT_H: f32 = 18.0;
const TALL_H: f32 = 36.0;
const LIST_OVERDRAW_PX: f32 = 64.0;

fn item_entity_id(ix: usize) -> String {
    format!("item:{ix}")
}

fn row_height(ix: usize) -> f32 {
    if ix.is_multiple_of(2) {
        SHORT_H
    } else {
        TALL_H
    }
}

/// Minimal fixture: a `gpui::list` with `ITEM_COUNT` variable-height items.
/// Each item is wrapped in `TransparentTracker::with_entity_id("item:N")` so
/// it registers in `BoundsRegistry` when prepainted. The fixture exposes its
/// `ListState` so the test can drive `scroll_to_reveal_item` from outside.
struct SpikeView {
    list_state: ListState,
    bounds_registry: BoundsRegistry,
}

impl SpikeView {
    fn new(bounds_registry: BoundsRegistry) -> Self {
        let list_state =
            ListState::new(ITEM_COUNT, ListAlignment::Top, px(LIST_OVERDRAW_PX)).measure_all();
        Self {
            list_state,
            bounds_registry,
        }
    }
}

impl Render for SpikeView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Mirror the real root view: rotate staged→committed at the start
        // of every render pass.
        self.bounds_registry.begin_pass();

        let registry = self.bounds_registry.clone();
        let list_element = list(self.list_state.clone(), move |ix, _, _| {
            let row = div().h(px(row_height(ix))).w_full();
            let el_id = format!("spike-row#{ix}");
            let tracker = TransparentTracker::new(
                el_id,
                "spike_row",
                registry.clone(),
                row.into_any_element(),
            )
            .with_entity_id(item_entity_id(ix));
            tracker.into_any_element()
        });

        div().id("spike-root").w_full().h_full().child(
            list_element
                .with_sizing_behavior(gpui::ListSizingBehavior::Auto)
                .w_full()
                .h_full(),
        )
    }
}

fn registry_has_entity(registry: &BoundsRegistry, entity_id: &str) -> bool {
    registry
        .all_elements()
        .iter()
        .any(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
}

#[gpui::test]
fn initial_paint_records_visible_items(cx: &mut TestAppContext) {
    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();
    let (_entity, vcx) = cx.add_window_view(move |_window, _cx| SpikeView::new(bounds_for_view));
    vcx.run_until_parked();

    // Promote whatever staged from the last paint into committed.
    bounds.flush();

    // First item must be in the committed set after the initial paint —
    // if not, the wrapper isn't reaching prepaint at all and everything
    // downstream is moot.
    assert!(
        registry_has_entity(&bounds, &item_entity_id(0)),
        "item:0 should be in BoundsRegistry after initial paint — TransparentTracker.prepaint \
         isn't recording. all_elements()={:?}",
        bounds
            .all_elements()
            .iter()
            .map(|(k, _)| k)
            .collect::<Vec<_>>()
    );
}

#[gpui::test]
fn scroll_to_reveal_item_makes_far_item_visible_in_bounds(cx: &mut TestAppContext) {
    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();
    let (entity, vcx) = cx.add_window_view(move |_window, _cx| SpikeView::new(bounds_for_view));
    vcx.run_until_parked();
    bounds.flush();

    let target = ITEM_COUNT - 5; // 45 — well beyond first viewport
    let initially_visible = registry_has_entity(&bounds, &item_entity_id(target));

    entity.update(&mut vcx.cx.clone(), |view, cx| {
        view.list_state.scroll_to_reveal_item(target);
        cx.notify();
    });
    vcx.run_until_parked();
    bounds.flush();

    let visible_after = registry_has_entity(&bounds, &item_entity_id(target));

    let all_ids: Vec<String> = bounds
        .all_elements()
        .iter()
        .filter_map(|(_, info)| info.entity_id.as_deref().map(str::to_string))
        .collect();

    assert!(
        visible_after,
        "after scroll_to_reveal_item({target}): expected {} in BoundsRegistry. \
         initially_visible={initially_visible}. entity_ids in registry after scroll: {all_ids:?}",
        item_entity_id(target)
    );
}

#[gpui::test]
fn bounds_for_item_returns_some_after_scroll(cx: &mut TestAppContext) {
    let bounds = BoundsRegistry::new();
    let bounds_for_view = bounds.clone();
    let (entity, vcx) = cx.add_window_view(move |_window, _cx| SpikeView::new(bounds_for_view));
    vcx.run_until_parked();

    let target = ITEM_COUNT - 5;

    // Grab a handle to the ListState before we scroll, so we can query
    // `bounds_for_item` post-paint without going through `entity.update`.
    let state_handle = entity.read_with(&vcx.cx, |view, _cx| view.list_state.clone());

    entity.update(&mut vcx.cx.clone(), |view, cx| {
        view.list_state.scroll_to_reveal_item(target);
        cx.notify();
    });
    vcx.run_until_parked();

    let item_bounds = state_handle.bounds_for_item(target);
    assert!(
        item_bounds.is_some(),
        "ListState::bounds_for_item({target}) is None — scroll_to_reveal_item didn't reach the \
         prepaint that records bounds. logical_scroll_top={:?}",
        state_handle.logical_scroll_top()
    );
}
