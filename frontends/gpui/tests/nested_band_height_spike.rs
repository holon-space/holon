//! Bug #69, fixture tier: the height a virtualized outline RESERVES for a row
//! that contains a nested content-sized band must be the height that band
//! PAINTS.
//!
//! The windowed rungs in `gpui_window_slice.rs`
//! (`band_rows_do_not_overlap_the_following_sibling_row`,
//! `page_with_a_nested_band_scrolls_to_its_last_row`) judge the same contract
//! over a real vault and a real query. This file judges it over a fixture, so
//! the mechanism can be localized in seconds instead of ~90s per boot: same
//! element shapes, no backend.
//!
//! Shape under test — the production nesting from Martin's ClaudeCode page:
//!
//! ```text
//!   columns( column( collection[            ← main outline, VIRTUALIZED (gpui::list)
//!       text "pre-0", text "pre-1",
//!       column( collection[ 18 × text ] ),  ← the nested BAND: under a `Nested`
//!                                             placement this renders EAGERLY at
//!                                             content height
//!                                             (`column::eager_collection_div`)
//!       text "sib",                         ← must start BELOW the band
//!       text "post-0" …
//!   ] ) )
//! ```
//!
//! Run: `cargo test -p holon-gpui --test nested_band_height_spike`

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

const VIEWPORT_W: f32 = 600.0;
const VIEWPORT_H: f32 = 600.0;
/// Rows the nested band holds. Several times taller than a single outline row,
/// so a reserved-vs-painted mismatch is unmissable.
const BAND_ROWS: usize = 18;
/// Plain rows after the band's sibling. Far more than fit in the viewport, so
/// the outline demonstrably VIRTUALIZES — without that this file is not a
/// control for the virtualized production outline at all.
const POST_ROWS: usize = 200;
/// Ceiling on rows built per frame that proves virtualization is in play.
const VIRTUALIZATION_CEILING: usize = 100;
/// Adjacent rows may share an edge; deeper than this is real overlap.
const OVERLAP_EPSILON_PX: f32 = 1.0;

/// A `text` row whose production render path records bounds keyed by `data.id`.
fn text_row(id: &str) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".into(), Value::String(id.to_string()));
    data.insert("content".into(), Value::String(id.to_string()));

    let mut props = HashMap::new();
    props.insert("content".into(), Value::String(id.to_string()));
    props.insert("field".into(), Value::String("content".into()));

    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

/// One outline row that OWNS a nested collection — the fixture stand-in for a
/// query block's `live_block`. A `column` wrapping a collection child is the
/// shape `builders::render` routes to `eager_collection_div` under a `Nested`
/// placement, i.e. exactly the band under test. The band's `ReactiveView` is
/// returned too, so a test can GROW it after the first layout the way a query's
/// rows actually arrive.
fn band_row(rows: usize) -> (ReactiveViewModel, Arc<ReactiveView>) {
    let items: Vec<ReactiveViewModel> = (0..rows)
        .map(|i| text_row(&format!("band-row-{i:02}")))
        .collect();
    let inner = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let collection_child = Arc::new(ReactiveViewModel {
        collection: Some(inner.clone()),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    });

    let mut column = ReactiveViewModel::from_widget("column", HashMap::new());
    column.children = vec![collection_child];
    (column, inner)
}

/// `columns( column( <outline collection> ) )` — the production main-panel
/// shape (same as `main_outline_virtualized_pbt::main_panel_columns_root`).
fn page_root(band_rows: usize) -> (Arc<ReactiveViewModel>, Arc<ReactiveView>) {
    let mut items = vec![text_row("pre-0"), text_row("pre-1")];
    let (band, band_view) = band_row(band_rows);
    items.push(band);
    items.push(text_row("sib"));
    items.extend((0..POST_ROWS).map(|i| text_row(&format!("post-{i:02}"))));

    let outline = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let collection_child = Arc::new(ReactiveViewModel {
        collection: Some(outline),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    });

    let mut main_column = ReactiveViewModel::from_widget("column", HashMap::new());
    main_column.children = vec![collection_child];

    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![main_column],
        CollectionVariant::columns(4.0),
    ));
    (
        Arc::new(ReactiveViewModel {
            collection: Some(columns_view),
            ..ReactiveViewModel::from_widget("columns", HashMap::new())
        }),
        band_view,
    )
}

/// `(y, height)` of the painted element carrying `entity_id`, if it has real
/// area on screen.
fn row_box(bounds: &BoundsRegistry, entity_id: &str) -> Option<(f32, f32)> {
    bounds
        .all_elements()
        .iter()
        .filter(|(_, i)| i.entity_id.as_deref() == Some(entity_id))
        .filter(|(_, i)| i.width > 1.0 && i.height > 0.0)
        .map(|(_, i)| (i.y, i.height))
        .next()
}

/// Assert the band's painted rows and the row after them do not share vertical
/// space. `phase` names the moment for the failure message.
fn assert_band_and_sibling_disjoint(bounds: &BoundsRegistry, expected_rows: usize, phase: &str) {
    let painted: Vec<(String, f32, f32)> = (0..expected_rows)
        .filter_map(|i| {
            let id = format!("band-row-{i:02}");
            row_box(bounds, &id).map(|(y, h)| (id, y, h))
        })
        .collect();
    assert_eq!(
        painted.len(),
        expected_rows,
        "precondition ({phase}): the nested band must paint all {expected_rows} of its rows, \
         painted {} — that is the #60 defect (a `Nested` shell claiming a parent height), not the \
         reserved-height defect this test exists for",
        painted.len(),
    );

    let band_bottom = painted
        .iter()
        .map(|(_, y, h)| y + h)
        .fold(f32::MIN, f32::max);
    let (sib_top, sib_height) = row_box(bounds, "sib").unwrap_or_else(|| {
        panic!("precondition ({phase}): the row after the band must be painted")
    });

    let post_built = bounds
        .all_elements()
        .iter()
        .filter(|(_, i)| {
            i.entity_id
                .as_deref()
                .is_some_and(|id| id.starts_with("post-"))
        })
        .count();
    eprintln!(
        "[band-spike] {phase}: band rows painted={} bottom={band_bottom:.1} sib_top={sib_top:.1} \
         sib_height={sib_height:.1} post_rows_built={post_built}/{POST_ROWS}",
        painted.len(),
    );

    assert!(
        post_built < VIRTUALIZATION_CEILING,
        "precondition ({phase}): this file only controls for the production outline if that \
         outline is VIRTUALIZED, but {post_built} of {POST_ROWS} filler rows were built in one \
         frame (ceiling {VIRTUALIZATION_CEILING}) — the fixture fell onto the eager path and \
         judges a different code path than the windowed rungs do"
    );

    let overlap = band_bottom - sib_top;
    assert!(
        overlap <= OVERLAP_EPSILON_PX,
        "({phase}) the outline placed the row after the nested band {overlap:.1}px INSIDE it: the \
         band's last painted row ends at y={band_bottom:.1}, `sib` starts at y={sib_top:.1} (band \
         rows: {:?}). The virtualized outline reserved the height the band's row MEASURED at, not \
         the height it PAINTED.",
        painted
            .iter()
            .map(|(id, y, h)| format!("{id}@{y:.1}+{h:.1}"))
            .collect::<Vec<_>>(),
    );
}

/// Control: with the band's rows present from the FIRST layout, the outline
/// reserves the right height. Establishes that the layout shape itself is
/// sound, so a failure of the growth case below is about invalidation, not
/// about the band's box model.
#[gpui::test]
fn outline_reserves_the_height_a_born_full_band_paints(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let bounds = BoundsRegistry::new();
    let (root, _band) = page_root(BAND_ROWS);
    let (_entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| {
            ReactiveFixtureView::with_bounds(root, size(px(VIEWPORT_W), px(VIEWPORT_H)), bounds)
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    assert_band_and_sibling_disjoint(&bounds, BAND_ROWS, "born full");
}

/// Bug #69. A band that GROWS after the outline has already measured its row —
/// which is what every real query block does, since its rows arrive
/// asynchronously — must still get the height it paints.
///
/// The virtualized outline caches a measured height per row (gpui `ListState`
/// keeps `ListItem::Measured` until something invalidates it). Growing a nested
/// band changes only the band's own element tree; nothing tells the enclosing
/// list that the row it already measured is now taller. The list keeps placing
/// the next sibling at the STALE offset, so the sibling draws on top of the
/// band's lower rows, and the list's scroll extent
/// (`items.summary().height`) stays short by the same difference.
#[gpui::test]
fn outline_reserves_the_height_a_grown_band_paints(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    // Start with a band the size a query block has before its rows land.
    const INITIAL_ROWS: usize = 1;

    let bounds = BoundsRegistry::new();
    let (root, band) = page_root(INITIAL_ROWS);
    let (entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| {
            ReactiveFixtureView::with_bounds(root, size(px(VIEWPORT_W), px(VIEWPORT_H)), bounds)
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    assert_band_and_sibling_disjoint(&bounds, INITIAL_ROWS, "before growth");

    // The query's rows arrive: the band grows to its full size.
    {
        let mut items = band.items.lock_mut();
        for i in INITIAL_ROWS..BAND_ROWS {
            items.push_cloned(Arc::new(text_row(&format!("band-row-{i:02}"))));
        }
    }
    entity.update(&mut vcx.clone(), |_, cx| cx.notify());
    vcx.run_until_parked();
    bounds.flush();

    assert_band_and_sibling_disjoint(&bounds, BAND_ROWS, "after growth");
}
