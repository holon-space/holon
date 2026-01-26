//! Scroll layout + interaction tests.
//!
//! Two categories, all routed through production code paths:
//!
//! 1. **`uniform_list` structural + interaction tests** — draw a
//!    `ScrollableListView` wrapped in the production
//!    `scrollable_list_wrapper` chain, then either assert on
//!    `UniformListScrollHandle::last_item_size` (structural) or dispatch
//!    a real wheel event through `add_window_view` + `simulate_wheel_at`
//!    (interaction). Catches regressions in the outer wrapper chain.
//!
//! 2. **Real-reactive tests** — render an actual `ReactiveViewModel::
//!    Reactive { view }` through `builders::render` via
//!    `ReactiveFixtureView` + `TestServices`, then drive the inner
//!    `ReactiveShell`'s `ListState` either with a programmatic
//!    `scroll_by` ("viewport invariants" section) or a wheel event
//!    through the full hit-testing pipeline.
//!
//! The canonical fix reference is `reactive_shell.rs`: the April 2026
//! `ListSizingBehavior::Infer` → `Auto` + `h_full` change. Any change
//! that puts a scrollable list back into a flex-grow sizing mode will
//! fail the viewport-invariant tests before reaching a user.

mod support;

use gpui::{point, prelude::*, px, size, AvailableSpace, TestAppContext, UniformListScrollHandle};

use holon_gpui::geometry::BoundsRegistry;
use support::{simulate_wheel_at, LayoutMode, ReactiveFixtureView, ScrollableListView};

const VIEWPORT_W: f32 = 600.0;
const VIEWPORT_H: f32 = 400.0;

/// Draw a `ScrollableListView` via the one-shot `cx.draw` path at an
/// explicit pixel size. Returns the observable
/// `UniformListScrollHandle`. Use for **structural** assertions only —
/// no hitboxes survive this, so wheel events won't hit-test.
fn draw_scroll_fixture(
    cx: &mut TestAppContext,
    item_count: usize,
    mode: LayoutMode,
) -> UniformListScrollHandle {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let scroll_handle = UniformListScrollHandle::new();
    let returned = scroll_handle.clone();
    let handle_for_view = scroll_handle;

    let vcx = cx.add_empty_window();
    vcx.draw(
        point(px(0.0), px(0.0)),
        size(
            AvailableSpace::Definite(px(VIEWPORT_W)),
            AvailableSpace::Definite(px(VIEWPORT_H)),
        ),
        move |_window, cx| {
            cx.new(|_cx| {
                ScrollableListView::with_handle(
                    item_count,
                    mode,
                    BoundsRegistry::new(),
                    handle_for_view,
                )
            })
            .into_any_element()
        },
    );

    returned
}

// ── Fast structural tests ─────────────────────────────────────────────

/// Production chain + many items → `contents.height > item.height`.
/// Viewport was measured as the list's resolved height via the wrapper
/// chain; content is 200 × 24 = 4800px.
#[gpui::test]
fn production_chain_overflows_viewport(cx: &mut TestAppContext) {
    let handle = draw_scroll_fixture(cx, 200, LayoutMode::Production);

    let sizes = handle
        .0
        .borrow()
        .last_item_size
        .expect("uniform_list never measured — last_item_size is None. Did prepaint run?");

    assert!(
        sizes.contents.height > sizes.item.height,
        "production chain should produce contents ({:?}) > viewport ({:?}), \
         got contents={:?} viewport={:?}. Cascade regression class.",
        sizes.contents.height,
        sizes.item.height,
        sizes.contents,
        sizes.item,
    );
    assert!(
        sizes.item.height < px(1000.0),
        "viewport height should be ~{VIEWPORT_H}px but was {:?}. \
         Content height leaked into the viewport — cascade regression.",
        sizes.item.height
    );
}

/// Few items → content fits viewport → scroll unnecessary.
#[gpui::test]
fn production_chain_with_few_items_fits_viewport(cx: &mut TestAppContext) {
    let handle = draw_scroll_fixture(cx, 3, LayoutMode::Production);

    let sizes = handle
        .0
        .borrow()
        .last_item_size
        .expect("list should have measured");

    assert!(
        sizes.contents.height <= sizes.item.height,
        "3 items × 24px should fit a {VIEWPORT_H}px viewport but got contents={:?} viewport={:?}",
        sizes.contents,
        sizes.item,
    );
}

// ── Interaction tests: real wheel-event pipeline ─────────────────────

/// Wheel event in production chain → `base_handle.offset().y` decreases.
///
/// Uses `add_window_view` (persistent) rather than `cx.draw` (one-shot).
/// `cx.draw` drops its element after the closure returns and clears the
/// arena, so hitboxes die before any event reaches them.
#[gpui::test]
fn wheel_event_scrolls_production_list(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let scroll_handle = UniformListScrollHandle::new();
    let handle_for_view = scroll_handle.clone();
    let (_entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ScrollableListView::with_handle(
            200,
            LayoutMode::Production,
            BoundsRegistry::new(),
            handle_for_view,
        )
    });
    vcx.run_until_parked();

    let base = scroll_handle.0.borrow().base_handle.clone();
    let list_bounds = base.bounds();
    let max_offset = base.max_offset();
    let before = base.offset();

    assert_eq!(
        before.y,
        px(0.0),
        "initial scroll offset should be 0, got {before:?}"
    );
    assert!(
        list_bounds.size.height > px(0.0),
        "list hasn't painted — list_bounds={list_bounds:?}"
    );
    assert!(
        max_offset.y > px(0.0),
        "list has no scrollable distance — max_offset={max_offset:?}. \
         Content may have leaked into viewport (cascade regression)."
    );

    // Dispatch wheel at the list's centre — guaranteed hit-testable.
    simulate_wheel_at(vcx, list_bounds.center(), px(-240.0));

    let after = base.offset();
    assert!(
        after.y < before.y,
        "scroll wheel should decrease offset.y (content moves up) but before={before:?} after={after:?}. \
         list_bounds={list_bounds:?}, max_offset={max_offset:?}. \
         Regression class: hit-testing broke, `overflow_hidden` missing, or scroll hitbox not registered."
    );
}

/// Negative control: **few items** → `max_offset = 0`, wheel events
/// must NOT move offset. Proves the positive test isn't vacuous.
#[gpui::test]
fn wheel_event_does_not_scroll_when_content_fits(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let scroll_handle = UniformListScrollHandle::new();
    let handle_for_view = scroll_handle.clone();
    let (_entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ScrollableListView::with_handle(
            3, // content fits easily
            LayoutMode::Production,
            BoundsRegistry::new(),
            handle_for_view,
        )
    });
    vcx.run_until_parked();

    let base = scroll_handle.0.borrow().base_handle.clone();
    let list_bounds = base.bounds();
    let before = base.offset();

    simulate_wheel_at(vcx, list_bounds.center(), px(-240.0));

    let after = base.offset();
    assert_eq!(
        before, after,
        "with only 3 items (no scroll needed), wheel should leave offset unchanged but before={before:?} after={after:?}. \
         Negative control broken: the positive test may be vacuous."
    );
}

// ── Real-reactive scroll tests (Option B) ────────────────────────────
//
// These tests route a real `ReactiveViewModel::Reactive { view }` tree
// through `render::builders::render` via `ReactiveFixtureView` +
// `TestServices`, exercising the same
// `get_or_create_reactive_shell` → `ReactiveShell` → `gpui::list` stack
// production uses. No hand-reconstructed layout chains, so divergence
// between fixture and production is impossible.

use std::sync::Arc;

use holon_api::EntityUri;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::{CollectionVariant, ReactiveSlot, ReactiveViewModel};

/// Build a `ReactiveView` with `count` text items under the given layout.
fn build_text_list(count: usize, layout: CollectionVariant) -> Arc<ReactiveView> {
    let items: Vec<ReactiveViewModel> = (0..count)
        .map(|i| ReactiveViewModel::text(format!("item {i}")))
        .collect();
    Arc::new(ReactiveView::new_static_with_layout(items, layout))
}

/// Wrap a `ReactiveView` into a collection-backed `ReactiveViewModel` node
/// that `builders::render` dispatches through `get_or_create_reactive_shell`.
fn reactive_root(view: Arc<ReactiveView>) -> Arc<ReactiveViewModel> {
    Arc::new(ReactiveViewModel {
        collection: Some(view),
        ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
    })
}

/// **Bug reproducer — root list, no columns wrapper**. A `ReactiveShell`
/// hosting a 200-item list. Programmatic `list_state.scroll_by(240)`
/// successfully advances `logical_scroll_top` (so the list_state is real
/// and items are measured), but a `ScrollWheelEvent` with the same delta
/// **snaps the offset back to `{0, 0}`** instead of advancing.
///
/// This is a **production-observed bug** — not a fixture artifact. Running
/// the real app shows the same snap-to-top behaviour under the reactive
/// shell. The earlier session's hypothesis (wheel routing broken only
/// *inside* `columns::render`) was too narrow: the bug reproduces at the
/// root level with no columns wrapper at all.
///
/// Currently red; flips to green when the fix lands. Do not disable —
/// this test is the canonical automated repro for the bug, routing
/// through the real `builders::render` pipeline via `ReactiveFixtureView`
/// + `TestServices` (no hand-rolled scaffolding).
#[gpui::test]
fn production_chain_wheel_event_scrolls_real_reactive(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let view = build_text_list(200, CollectionVariant::list(0.0));
    let root = reactive_root(view.clone());

    let (entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ReactiveFixtureView::new(root, size(px(VIEWPORT_W), px(VIEWPORT_H)))
    });
    vcx.run_until_parked();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&view))
        .expect(
            "ReactiveShell entity was not created by render pipeline — \
             get_or_create_reactive_shell key mismatch or cache didn't persist",
        );
    let list_state = shell.read_with(vcx, |s, _| s.list_state_handle());
    let before = list_state.logical_scroll_top();
    assert_eq!(
        before.item_ix, 0,
        "list should start at the top, got {before:?}"
    );

    let wheel_pos = point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0));
    simulate_wheel_at(vcx, wheel_pos, px(-240.0));

    let after = list_state.logical_scroll_top();
    assert!(
        after.item_ix > before.item_ix || after.offset_in_item > before.offset_in_item,
        "wheel event did not advance logical_scroll_top past {before:?} — got {after:?}. \
         The `ReactiveShell` + `gpui::list` scroll path is broken: wheel events \
         don't move the viewport. Historically this manifested as snap-to-top; \
         the `Infer`→`Auto` sizing fix cleared that, so check `reactive_shell.rs` \
         for any regressions."
    );
}

/// **Bug reproducer — list wrapped in a Columns layout**. Same symptom
/// as the root-list reproducer above — wheel events snap scroll back to
/// zero instead of advancing. Kept as a separate test so we can verify
/// whether the fix is general (both turn green) or only addresses the
/// columns wrapper (only this one turns green, root reproducer stays red —
/// which would indicate a second, unrelated bug).
///
/// The earlier session's hypothesis — `columns::render`'s non-drawer panel
/// wrapper is missing `min_h_0` / `overflow_hidden` — remains *a*
/// possible contributor, but it cannot be the whole story because the
/// root-list case doesn't go through `columns::render` at all.
#[gpui::test]
fn columns_child_wheel_event_scrolls_real_reactive(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    // Simplest columns repro: a single list wrapped in a Columns layout.
    // With the `columns.rs` fix, the non-drawer branch now gives each
    // panel a `flex_1 relative → absolute size_full` wrapper so
    // descendant scrollable lists get a definite viewport.
    let inner_list = build_text_list(200, CollectionVariant::list(0.0));
    let main_list = ReactiveViewModel {
        collection: Some(inner_list.clone()),
        ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
    };
    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![main_list],
        CollectionVariant::columns(4.0),
    ));
    let root = reactive_root(columns_view);

    let (entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ReactiveFixtureView::new(root, size(px(VIEWPORT_W), px(VIEWPORT_H)))
    });
    vcx.run_until_parked();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&inner_list))
        .expect(
            "inner ReactiveShell entity was not created — columns::render may \
             have taken a branch that bypasses get_or_create_reactive_shell",
        );
    let list_state = shell.read_with(vcx, |s, _| s.list_state_handle());
    let before = list_state.logical_scroll_top();

    let wheel_pos = point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0));
    simulate_wheel_at(vcx, wheel_pos, px(-240.0));

    let after = list_state.logical_scroll_top();
    assert!(
        after.item_ix > before.item_ix || after.offset_in_item > before.offset_in_item,
        "columns-wrapped wheel event did not advance logical_scroll_top \
         (before={before:?}, after={after:?}). The list panel is nested \
         inside `columns::render`'s `flex_1 relative → absolute size_full \
         flex_col` wrapper. If this is the only broken scroll scenario, \
         the fix is most likely in how that wrapper propagates definite \
         height through to the `scrollable_list_wrapper` + `ReactiveShell` \
         chain."
    );
}

// ── Viewport invariants ──────────────────────────────────────────────
//
// Categorical guard for the `ListSizingBehavior::Infer` trap. The bug it
// catches isn't visible at the type level: a descendant `gpui::list` in a
// flex ancestor chain with ambient `items_stretch` can silently take its
// content height as the viewport, collapsing `scroll_max` to zero. Wheel
// events aren't needed — `list_state.scroll_by()` only advances when the
// list has a definite, smaller-than-content viewport, so programmatic
// scroll is a sufficient black-box proof.
//
// Items are ~30px tall and the viewport is 400px (~13 visible rows).
// `scroll_by(240px)` at the top should land around item_ix 8. We accept
// `1..=20` — loose enough to be font/platform tolerant, tight enough to
// distinguish real scroll (small advance) from the "unmeasured items →
// seek to end" degenerate where `item_ix` jumps to ~200.

/// Root-list viewport: no columns wrapper. Programmatic `scroll_by`
/// must move `logical_scroll_top` into the mid-range of the list.
#[gpui::test]
fn root_list_viewport_is_scrollable(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let view = build_text_list(200, CollectionVariant::list(0.0));
    let root = reactive_root(view.clone());

    let (entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ReactiveFixtureView::new(root, size(px(VIEWPORT_W), px(VIEWPORT_H)))
    });
    vcx.run_until_parked();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&view))
        .expect("ReactiveShell entity missing — render pipeline bypassed the shell");
    let list_state = shell.read_with(vcx, |s, _| s.list_state_handle());
    assert_eq!(
        list_state.logical_scroll_top().item_ix,
        0,
        "list should start at the top"
    );

    list_state.scroll_by(px(240.0));
    vcx.run_until_parked();

    let after = list_state.logical_scroll_top();
    assert!(
        (1..=20).contains(&after.item_ix),
        "scroll_by(240) should land in the mid-range (1..=20) but got {after:?}. \
         item_ix 0 → viewport collapsed to content height (Infer sizing trap, \
         `scroll_max = 0`). item_ix ~200 → items were never measured so \
         `cursor.seek_forward(Height)` walked off the end. Check that \
         `ReactiveShell` uses `ListSizingBehavior::Auto` + a definite-height \
         ancestor chain (`h_full` / `size_full`, not `flex_grow`)."
    );
}

/// Reproducer for the "blank main panel" bug: the real root layout puts
/// each panel (main + sidebars) inside a `view_mode_switcher`, which
/// wraps its slot content in `div().size_full().flex_1().flex_col()` with
/// a tiny `switcher_bar` + the real content as stacked flex children —
/// but nothing gives the content child a definite height. When the inner
/// `ReactiveShell` uses `h_full()` to claim its list viewport (required
/// by the `Auto` sizing fix), `100%` resolves against an undefined
/// cross-axis and collapses to zero. Everything downstream disappears.
///
/// This test wraps a real 200-item reactive list in a ViewModeSwitcher
/// and asserts the inner list is scrollable. Red before the fix.
#[gpui::test]
fn view_mode_switcher_child_viewport_is_scrollable(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let inner_list = build_text_list(200, CollectionVariant::list(0.0));
    let slot_vm = ReactiveViewModel {
        collection: Some(inner_list.clone()),
        ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
    };
    let mut vms_props = std::collections::HashMap::new();
    vms_props.insert(
        "entity_uri".to_string(),
        holon_api::Value::String("block:test-main-panel".to_string()),
    );
    vms_props.insert(
        "modes".to_string(),
        holon_api::Value::String(r#"[{"name":"list","icon":"list"}]"#.to_string()),
    );
    let switcher = Arc::new(ReactiveViewModel {
        slot: Some(ReactiveSlot::new(slot_vm)),
        render_ctx: Some(Default::default()),
        ..ReactiveViewModel::from_widget("view_mode_switcher", vms_props)
    });

    let (entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ReactiveFixtureView::new(switcher, size(px(VIEWPORT_W), px(VIEWPORT_H)))
    });
    vcx.run_until_parked();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&inner_list))
        .expect(
            "ReactiveShell for the inner list was not created under the \
             view_mode_switcher — get_or_create_reactive_shell key mismatch \
             or the switcher's slot rendering skipped the shell",
        );
    let list_state = shell.read_with(vcx, |s, _| s.list_state_handle());

    list_state.scroll_by(px(240.0));
    vcx.run_until_parked();

    let after = list_state.logical_scroll_top();
    assert!(
        (1..=20).contains(&after.item_ix),
        "view_mode_switcher slot collapsed the inner list to zero height — \
         `scroll_by(240)` should have landed in 1..=20 but got {after:?}. \
         The switcher wraps its slot in `div().size_full().flex_1().flex_col()` \
         with a small switcher bar + the slot content stacked as flex children; \
         nothing gives the content child a definite height, so the \
         ReactiveShell's `h_full()` collapses. Fix in \
         `builders/view_mode_switcher.rs`: wrap `child_el` in a \
         `flex_1 min_h_0` div so the list inherits a definite viewport."
    );
}

/// Same invariant, wrapped in a Columns layout. Catches regressions in
/// `columns::render`'s panel wrapper — if it stops handing each child a
/// definite viewport (`flex_1 relative → absolute size_full`), the inner
/// list's `scroll_max` collapses the same way.
#[gpui::test]
fn columns_child_viewport_is_scrollable(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let inner_list = build_text_list(200, CollectionVariant::list(0.0));
    let main_list = ReactiveViewModel {
        collection: Some(inner_list.clone()),
        ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
    };
    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![main_list],
        CollectionVariant::columns(4.0),
    ));
    let root = reactive_root(columns_view);

    let (entity, vcx) = cx.add_window_view(move |_window, _cx| {
        ReactiveFixtureView::new(root, size(px(VIEWPORT_W), px(VIEWPORT_H)))
    });
    vcx.run_until_parked();

    let shell = entity
        .read_with(vcx, |fv, _| fv.reactive_shell(&inner_list))
        .expect("inner ReactiveShell entity missing — columns::render bypassed the shell");
    let list_state = shell.read_with(vcx, |s, _| s.list_state_handle());
    assert_eq!(list_state.logical_scroll_top().item_ix, 0);

    list_state.scroll_by(px(240.0));
    vcx.run_until_parked();

    let after = list_state.logical_scroll_top();
    assert!(
        (1..=20).contains(&after.item_ix),
        "columns-wrapped `scroll_by(240)` should land in 1..=20 but got {after:?}. \
         If item_ix is 0, `columns::render` is handing the inner panel a \
         non-definite height (regression in the `flex_1 relative → absolute \
         size_full` wrapper chain). See `builders/columns.rs`."
    );
}
