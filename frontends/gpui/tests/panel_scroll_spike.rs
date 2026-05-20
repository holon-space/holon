//! Spike D — production-path cache lookup → ReactiveShell → ListState →
//! scroll → BoundsRegistry has remote item keyed by entity_id.
//!
//! Spike A (`list_scroll_spike.rs`) proved that `ListState::scroll_to_reveal_item`
//! drives the test scheduler past a paint that records bounds keyed by
//! `entity_id` in `BoundsRegistry`, using a synthetic fixture that wraps each
//! row in `TransparentTracker::with_entity_id` directly.
//!
//! Spike D is the load-bearing question for the trait-redesign plan:
//! when the list is rendered by the *production* path
//! (`ReactiveShell::new_for_collection` cached under
//! `CacheKey::ReactiveShell(view.stable_cache_key())`, items dispatched
//! through `builders::render`), can a test driver look up the panel's
//! `ListState` from outside the render closure, drive `scroll_to_reveal_item`,
//! and observe the post-scroll items in `BoundsRegistry` keyed by their
//! `entity_id`?
//!
//! Run: `cargo test -p holon-gpui --test panel_scroll_spike`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{px, size, Pixels, Size, TestAppContext};

use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::{CollectionVariant, ReactiveViewModel};
use holon_gpui::geometry::BoundsRegistry;

use support::ReactiveFixtureView;

const ITEM_COUNT: usize = 50;
const TARGET_IX: usize = 45;
const VIEWPORT_W: f32 = 400.0;
const VIEWPORT_H: f32 = 300.0;

fn item_id(ix: usize) -> String {
    format!("test-item-{ix}")
}

/// Build a `text` ReactiveViewModel whose render path (`builders/text.rs:68`)
/// records bounds keyed by `row_id` IFF `data.id` is set AND `props.field`
/// is `"content"`. This is the smallest production-faithful construction
/// that makes `entity_id`s appear in `BoundsRegistry`.
fn text_item(ix: usize) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".into(), Value::String(item_id(ix)));
    data.insert("content".into(), Value::String(format!("row {ix}")));

    let mut props = HashMap::new();
    props.insert("content".into(), Value::String(format!("row {ix}")));
    props.insert("field".into(), Value::String("content".into()));

    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

fn reactive_root(view: Arc<ReactiveView>) -> Arc<ReactiveViewModel> {
    Arc::new(ReactiveViewModel {
        collection: Some(view),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    })
}

fn registry_has_entity(registry: &BoundsRegistry, entity_id: &str) -> bool {
    registry
        .all_elements()
        .iter()
        .any(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
}

fn viewport() -> Size<Pixels> {
    size(px(VIEWPORT_W), px(VIEWPORT_H))
}

#[gpui::test]
fn cache_lookup_yields_real_reactive_shell(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(text_item).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let root = reactive_root(view.clone());

    let bounds = BoundsRegistry::new();
    let (entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, viewport(), bounds)
    });
    vcx.run_until_parked();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&view))
        .expect(
            "ReactiveShell entity not in cache — production render path didn't \
             reach get_or_create_reactive_shell for our collection",
        );
    let item_count = shell.read_with(vcx, |s, _| s.list_state_handle().item_count());
    assert_eq!(
        item_count, ITEM_COUNT,
        "shell's ListState item count should match the view, got {item_count}"
    );
}

#[gpui::test]
fn initial_paint_records_first_item_entity_id(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(text_item).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let root = reactive_root(view.clone());

    let bounds = BoundsRegistry::new();
    let (_entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, viewport(), bounds)
    });
    vcx.run_until_parked();
    bounds.flush();

    assert!(
        registry_has_entity(&bounds, &item_id(0)),
        "item 0 missing from BoundsRegistry after initial paint — \
         entity_id wiring in production text builder is broken. \
         entity_ids in registry: {:?}",
        bounds
            .all_elements()
            .iter()
            .filter_map(|(_, info)| info.entity_id.as_deref().map(str::to_string))
            .collect::<Vec<_>>()
    );
}

#[gpui::test]
fn production_shell_scroll_to_reveal_item_brings_far_row_into_bounds(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(text_item).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let root = reactive_root(view.clone());

    let bounds = BoundsRegistry::new();
    let (entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, viewport(), bounds)
    });
    vcx.run_until_parked();
    bounds.flush();

    let target = item_id(TARGET_IX);
    let visible_before = registry_has_entity(&bounds, &target);

    // Production-facing cache lookup: how the future GPUI UserDriver impl
    // for `scroll_to_entity` would reach a panel's ListState.
    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&view))
        .expect("ReactiveShell entity not in cache");
    let state = shell.read_with(vcx, |s, _| s.list_state_handle());

    state.scroll_to_reveal_item(TARGET_IX);

    // Trigger a re-render through the production update pathway.
    shell.update(&mut vcx.cx.clone(), |_, cx| cx.notify());
    vcx.run_until_parked();
    bounds.flush();

    let visible_after = registry_has_entity(&bounds, &target);
    let bounds_for_target = state.bounds_for_item(TARGET_IX);

    let registry_ids: Vec<String> = bounds
        .all_elements()
        .iter()
        .filter_map(|(_, info)| info.entity_id.as_deref().map(str::to_string))
        .collect();

    assert!(
        visible_after,
        "post-scroll: {target} missing from BoundsRegistry. \
         visible_before={visible_before}, bounds_for_item({TARGET_IX})={bounds_for_target:?}. \
         entity_ids present: {registry_ids:?}"
    );
}

// ─── Phase-2 main-panel virtualization regression tests ──────────────

const BIG_ITEM_COUNT: usize = 200;

/// The memory property virtualization buys: a ~200-row collection paints
/// only ~viewport-many rows into `BoundsRegistry`, not all N. This is the
/// path the main panel now takes (its block-mode shell falls through to
/// `builders::render`'s collection arm instead of the removed eager
/// every-row-every-frame branch).
#[gpui::test]
fn virtualized_list_bounds_stay_viewport_sized(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let items: Vec<ReactiveViewModel> = (0..BIG_ITEM_COUNT).map(text_item).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let root = reactive_root(view.clone());

    let bounds = BoundsRegistry::new();
    let (_entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, viewport(), bounds)
    });
    vcx.run_until_parked();
    bounds.flush();

    let tracked_rows: Vec<String> = bounds
        .all_elements()
        .iter()
        .filter_map(|(_, info)| info.entity_id.as_deref().map(str::to_string))
        .filter(|id| id.starts_with("test-item-"))
        .collect();

    assert!(
        registry_has_entity(&bounds, &item_id(0)),
        "first row must be painted (registry rows: {tracked_rows:?})"
    );
    assert!(
        !registry_has_entity(&bounds, &item_id(BIG_ITEM_COUNT - 1)),
        "last of {BIG_ITEM_COUNT} rows has bounds without scrolling — the \
         collection is being rendered eagerly, virtualization regressed"
    );
    assert!(
        tracked_rows.len() < BIG_ITEM_COUNT / 2,
        "{} of {BIG_ITEM_COUNT} rows have bounds — far more than a viewport's \
         worth; virtualization regressed",
        tracked_rows.len()
    );
}

/// `scroll_to_reveal_raw_index` (raw `children_snapshot()` position mapped
/// through `visible_indices`) reveals a far row — the API
/// `scroll_entity_into_view` uses now that the main panel virtualizes.
#[gpui::test]
fn visible_index_scroll_reveals_far_row(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(text_item).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let root = reactive_root(view.clone());

    let bounds = BoundsRegistry::new();
    let (entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| ReactiveFixtureView::with_bounds(root, viewport(), bounds)
    });
    vcx.run_until_parked();
    bounds.flush();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&view))
        .expect("ReactiveShell entity not in cache");
    // ALLOW(entity_uri_from_raw): test fixture row id (schemeless, matches text_item's data.id)
    let target_uri = holon_api::EntityUri::from_raw(&item_id(TARGET_IX));
    let ix = shell
        .read_with(vcx, |s, _| s.visible_index_of(&target_uri))
        .unwrap_or_else(|| {
            panic!("visible_index_of({target_uri}) returned None in an all-visible list")
        });
    assert_eq!(
        ix, TARGET_IX,
        "all-visible list: visible index should equal raw index"
    );
    shell.read_with(vcx, |s, _| s.list_state_handle().scroll_to_reveal_item(ix));

    shell.update(&mut vcx.cx.clone(), |_, cx| cx.notify());
    vcx.run_until_parked();
    bounds.flush();

    assert!(
        registry_has_entity(&bounds, &item_id(TARGET_IX)),
        "post-scroll: {} missing from BoundsRegistry",
        item_id(TARGET_IX)
    );
}
