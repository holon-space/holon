//! The sidebar's disclosure affordance must be READABLE WITHOUT HOVER
//! (Martin 2026-07-30): you cannot see which sidebar items have children
//! without scrubbing the mouse across them, and on touch there is no hover at
//! all.
//!
//! The invariant is model-derived — for every rendered tree row, given only
//! `has_children` and `collapsed`:
//!
//! | model                      | disclosure element                        |
//! |----------------------------|-------------------------------------------|
//! | has_children && collapsed  | present, glyph `▶`, opaque, halo present  |
//! | has_children && expanded   | present, glyph `▼`, opaque, no halo       |
//! | leaf                       | absent (no chevron, no halo)              |
//!
//! Every row here carries `hovered = Some(false)` — exactly what
//! `wrap_tree_item` seeds for a row the pointer is nowhere near — and the test
//! never synthesizes a hover. So "opaque" is the load-bearing clause: it is the
//! assertion a hover-gated chevron cannot satisfy.
//!
//! Both sidebar shapes are covered: the bare tree, and the production
//! `view_mode_switcher`-wrapped drawer shape Martin's real vault renders
//! (a block with ≥2 render variants — see `left_sidebar_scroll.rs`).
//!
//! Run: `cargo test -p holon-gpui --test sidebar_disclosure_affordance`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::LayoutHint;
use holon_frontend::disclosure_halo_id_for;
use holon_frontend::expand_toggle_id_for;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveSlot;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::theme::ThemeRegistry;
use holon_frontend::theme::collapsed_halo_fill;
use holon_frontend::theme::collapsed_halo_glyph;
use holon_frontend::theme::contrast_ratio;
use holon_gpui::geometry::BoundsRegistry;
use support::BlockTreeRegistry;
use support::BlockTreeThunk;
use support::BoundsSnapshot;
use support::ReactiveFixtureView;
use support::render_fixture;

/// The two disclosure glyphs, as the spec states them (not imported from the
/// builder — the test is the specification of what must be painted).
const GLYPH_COLLAPSED: &str = "\u{25B6}";
const GLYPH_EXPANDED: &str = "\u{25BC}";

const SIDEBAR_BLOCK: &str = "block:default-left-sidebar";
const SIDEBAR_LOCAL: &str = "default-left-sidebar";
const SIDEBAR_W: f32 = 220.0;
const VIEWPORT_W: f32 = 700.0;
const VIEWPORT_H: f32 = 500.0;

/// A sidebar row's model state — the ONLY two facts the disclosure affordance
/// may depend on.
struct Row {
    id: &'static str,
    has_children: bool,
    collapsed: bool,
}

const ROWS: &[Row] = &[
    Row {
        id: "parent-collapsed",
        has_children: true,
        collapsed: true,
    },
    Row {
        id: "parent-expanded",
        has_children: true,
        collapsed: false,
    },
    Row {
        id: "leaf",
        has_children: false,
        collapsed: false,
    },
];

/// How a row carries its identity. Production rows carry NONE —
/// `wrap_tree_item` stamps only `depth` and `has_children`, and the row's id
/// lives on the content child's entity data. Blueprint/gallery rows stamp an
/// explicit `target_id`.
///
/// The disclosure observables must be registered for BOTH, and above all for
/// `Production`: a gate that only ever sees `ExplicitTargetId` rows would stay
/// green while the whole affordance is missing from the real sidebar
/// (dogfood F2, 2026-07-30 — 27 live rows, zero `target_id`s).
#[derive(Clone, Copy)]
enum IdShape {
    Production,
    ExplicitTargetId,
}

/// A production-shaped tree row: `wrap_tree_item`'s props (`depth`,
/// `has_children`), its per-instance `expanded` cell, and its `hovered` cell
/// seeded to NOT-hovered. Under [`IdShape::Production`] the row's identity is
/// reachable ONLY through the content child's entity data, exactly as the live
/// org tree builds it — schemed (`block:…`), which is also what makes the
/// registry key's scheme-normalisation load-bearing.
fn tree_row(row: &Row, shape: IdShape) -> ReactiveViewModel {
    let mut props = HashMap::new();
    props.insert("depth".to_string(), Value::Float(0.0));
    props.insert("has_children".to_string(), Value::Boolean(row.has_children));
    if let IdShape::ExplicitTargetId = shape {
        props.insert("target_id".to_string(), Value::String(row.id.to_string()));
    }

    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(format!("block:{}", row.id)));
    data.insert("content".to_string(), Value::String(row.id.to_string()));
    let mut content = ReactiveViewModel::text(row.id);
    content.data = Mutable::new(Arc::new(data)).read_only();

    let mut vm = ReactiveViewModel::from_widget("tree_item", props).with_children(vec![content]);
    vm.expanded = Some(Mutable::new(!row.collapsed));
    vm.hovered = Some(Mutable::new(false));
    vm
}

fn info<'a>(
    snap: &'a BoundsSnapshot,
    el_id: &str,
) -> Option<&'a holon_frontend::geometry::ElementInfo> {
    snap.entries
        .iter()
        .find(|(id, _)| id == el_id)
        .map(|(_, i)| i)
}

/// THE invariant: every row's disclosure affordance is exactly what its model
/// state prescribes, with no hover anywhere in the picture.
fn assert_disclosure_matches_model(snap: &BoundsSnapshot, shape: &str) {
    for row in ROWS {
        let chevron = info(snap, &expand_toggle_id_for(row.id));
        let halo = info(snap, &disclosure_halo_id_for(row.id));

        if !row.has_children {
            assert!(
                chevron.is_none(),
                "[{shape}] leaf row `{}` must draw NO disclosure chevron, got {chevron:?}",
                row.id
            );
            assert!(
                halo.is_none(),
                "[{shape}] leaf row `{}` must draw no collapsed halo, got {halo:?}",
                row.id
            );
            continue;
        }

        let chevron = chevron.unwrap_or_else(|| {
            panic!(
                "[{shape}] parent row `{}` must draw a disclosure chevron; none registered:\n{}",
                row.id,
                snap.dump()
            )
        });

        assert!(
            chevron.has_visible_area(),
            "[{shape}] parent row `{}` chevron has no on-screen extent: {chevron:?}",
            row.id
        );

        // The persistent-affordance clause. The row is NOT hovered, so a
        // hover-revealed chevron paints at alpha 0 — present in layout,
        // invisible to Martin. A parent must be identifiable at a glance.
        assert_eq!(
            chevron.opacity,
            Some(1.0),
            "[{shape}] parent row `{}` chevron must be fully opaque WITHOUT hover — the \
             disclosure affordance is persistent, not hover-revealed",
            row.id
        );

        let expected_glyph = if row.collapsed {
            GLYPH_COLLAPSED
        } else {
            GLYPH_EXPANDED
        };
        assert_eq!(
            chevron.displayed_text.as_deref(),
            Some(expected_glyph),
            "[{shape}] row `{}` (collapsed={}) must point {}",
            row.id,
            row.collapsed,
            if row.collapsed { "right" } else { "down" }
        );

        // The collapsed halo: hidden content is scannable at a glance.
        assert_eq!(
            halo.is_some(),
            row.collapsed,
            "[{shape}] row `{}` (collapsed={}) halo presence wrong (got {})",
            row.id,
            row.collapsed,
            halo.is_some()
        );
        if let Some(halo) = halo {
            assert!(
                halo.has_visible_area(),
                "[{shape}] collapsed halo on `{}` has no on-screen extent: {halo:?}",
                row.id
            );
        }
    }
}

fn rows_column(shape: IdShape) -> Arc<ReactiveViewModel> {
    let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
    col.children = ROWS.iter().map(|r| Arc::new(tree_row(r, shape))).collect();
    Arc::new(col)
}

/// The production sidebar shape: `view_mode_switcher(column(<tree rows>,
/// divider()))` behind a shrink `drawer` + `live_block`, exactly as
/// `left_sidebar_scroll.rs` mounts it.
fn register_sidebar(registry: &BlockTreeRegistry, shape: IdShape) {
    let thunk: BlockTreeThunk = Arc::new(move || {
        let items: Vec<ReactiveViewModel> = ROWS.iter().map(|r| tree_row(r, shape)).collect();
        let view = Arc::new(ReactiveView::new_static_with_layout(
            items,
            CollectionVariant::list(0.0),
        ));
        let collection = ReactiveViewModel {
            collection: Some(view),
            ..ReactiveViewModel::from_widget("list", HashMap::new())
        };
        let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
        col.children = vec![
            Arc::new(collection),
            Arc::new(ReactiveViewModel::from_widget("divider", HashMap::new())),
        ];

        let mut props = HashMap::new();
        props.insert(
            "entity_uri".to_string(),
            Value::String(SIDEBAR_BLOCK.to_string()),
        );
        props.insert(
            "modes".to_string(),
            Value::String(
                "[{\"name\":\"tree\",\"icon\":\"list\"},{\"name\":\"table\",\"icon\":\"table\"}]"
                    .to_string(),
            ),
        );
        props.insert("active_mode".to_string(), Value::String("tree".to_string()));
        ReactiveViewModel {
            slot: Some(ReactiveSlot::new(col)),
            ..ReactiveViewModel::from_widget("view_mode_switcher", props)
        }
    });
    registry.register(SIDEBAR_BLOCK, vec![("default".to_string(), thunk)], 0);
}

fn sidebar_root() -> Arc<ReactiveViewModel> {
    let mut drawer_props = HashMap::new();
    drawer_props.insert("mode".to_string(), Value::String("shrink".to_string()));
    drawer_props.insert(
        "block_id".to_string(),
        Value::String(SIDEBAR_BLOCK.to_string()),
    );
    drawer_props.insert("width".to_string(), Value::Float(SIDEBAR_W as f64));
    let drawer = ReactiveViewModel {
        children: vec![Arc::new(ReactiveViewModel::live_block(EntityUri::block(
            SIDEBAR_LOCAL,
        )))],
        layout_hint: LayoutHint::Fixed { px: SIDEBAR_W },
        ..ReactiveViewModel::from_widget("drawer", drawer_props)
    };

    let mut main = ReactiveViewModel::from_widget("column", HashMap::new());
    main.children = vec![Arc::new(ReactiveViewModel::text("main panel"))];

    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![drawer, main],
        CollectionVariant::columns(4.0),
    ));
    Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

fn render_sidebar(cx: &mut TestAppContext, shape: IdShape) -> BoundsSnapshot {
    cx.update(|cx| gpui_component::init(cx));
    let registry = Arc::new(BlockTreeRegistry::new());
    register_sidebar(&registry, shape);
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
        support::TestServices::with_registry_quiescent(registry);
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        let services = services.clone();
        move |_, _| {
            ReactiveFixtureView::with_services_and_bounds(
                sidebar_root(),
                services,
                size(px(VIEWPORT_W), px(VIEWPORT_H)),
                bounds,
            )
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    BoundsSnapshot {
        entries: bounds.all_elements(),
    }
}

#[gpui::test]
fn disclosure_affordance_matches_model_in_production_row_shape(cx: &mut TestAppContext) {
    let snap = render_fixture(cx, rows_column(IdShape::Production));
    assert_disclosure_matches_model(&snap, "plain tree, production ids");
}

/// Blueprint / design-gallery rows stamp an explicit `target_id`. Distinct from
/// the case above: it guards the id-precedence rule (explicit wins over the
/// content child's entity id), which the layout PBT's `ToggleCollapse` needs.
#[gpui::test]
fn disclosure_affordance_matches_model_with_explicit_target_id(cx: &mut TestAppContext) {
    let snap = render_fixture(cx, rows_column(IdShape::ExplicitTargetId));
    assert_disclosure_matches_model(&snap, "plain tree, explicit target_id");
}

/// Martin's live shape: the sidebar block has ≥2 render variants, so its tree
/// root is a `view_mode_switcher` inside a shrink drawer — production rows.
#[gpui::test]
fn disclosure_affordance_matches_model_in_view_mode_switcher_sidebar(cx: &mut TestAppContext) {
    let snap = render_sidebar(cx, IdShape::Production);
    assert_disclosure_matches_model(&snap, "view_mode_switcher sidebar");
}

/// The registry key is scheme-normalised: a row whose entity id is schemed
/// (`block:parent-collapsed`, as the live org tree carries it) registers under
/// the SAME key a driver builds from the bare id. Without this the observables
/// exist but nothing can find them — `GpuiUserDriver::set_block_expanded`
/// strips the scheme before looking the chevron up.
#[gpui::test]
fn disclosure_registers_under_the_scheme_normalised_key(cx: &mut TestAppContext) {
    let snap = render_fixture(cx, rows_column(IdShape::Production));
    assert!(
        info(&snap, "expand_toggle::parent-collapsed").is_some(),
        "chevron must register under the bare-id key a driver looks up:\n{}",
        snap.dump()
    );
    assert!(
        info(&snap, "disclosure_halo::parent-collapsed").is_some(),
        "halo must register under the bare-id key:\n{}",
        snap.dump()
    );
}

/// The collapsed halo has to be SEEN to mean anything. Dogfood F1 (2026-07-30)
/// measured it at 1.05:1 against the sidebar in both holon themes — present in
/// the render tree, invisible on screen, which is exactly the failure mode a
/// geometry-only assertion cannot catch.
///
/// The floor for a non-text state indicator is 3:1. Asserted over EVERY builtin
/// theme and computed from the theme constants themselves, so swapping the
/// token can never silently drop below it again. The GPUI builder reaches these
/// same colours as `theme.muted_foreground` / `theme.background`, which
/// `apply_holon_theme` (frontends/gpui/src/lib.rs) maps from `text_secondary` /
/// `background`.
#[test]
fn collapsed_halo_clears_the_contrast_floor_in_every_theme() {
    const FLOOR: f32 = 3.0;
    let registry = ThemeRegistry::load(None);
    let mut checked = 0;

    for (name, _) in registry.available() {
        let c = &registry
            .get(name)
            .unwrap_or_else(|| panic!("theme {name} listed but not loadable"))
            .colors;

        let vs_surface = contrast_ratio(collapsed_halo_fill(c), c.sidebar_background);
        assert!(
            vs_surface >= FLOOR,
            "theme {name}: collapsed halo is {vs_surface:.2}:1 against the sidebar, below the \
             {FLOOR}:1 floor for a non-text state indicator — it will not be seen"
        );

        let glyph_vs_halo = contrast_ratio(collapsed_halo_glyph(c), collapsed_halo_fill(c));
        assert!(
            glyph_vs_halo >= FLOOR,
            "theme {name}: the chevron is {glyph_vs_halo:.2}:1 against its own halo, below the \
             {FLOOR}:1 floor — the glyph disappears into the circle"
        );
        checked += 1;
    }

    assert!(
        checked >= 2,
        "expected the builtin themes to load, got {checked}"
    );
}

/// The scan-weight relationship dogfood F1 also flagged: a leaf's bullet used
/// to read HEAVIER than a parent's chevron, inverting the hierarchy. Parents
/// carry full disclosure weight; leaf bullets are demoted below it.
#[test]
fn parents_outweigh_leaves_in_the_scan() {
    assert!(
        holon_gpui::render::builders::LEAF_BULLET_WEIGHT
            < holon_gpui::render::builders::DISCLOSURE_WEIGHT,
        "a leaf's bullet must not out-ink a parent's disclosure affordance"
    );
}

/// The halo is PAINT, never layout: a collapsed parent's chevron and its row
/// content sit exactly where the expanded parent's do. Without this, toggling a
/// row would shift the whole sidebar sideways.
#[gpui::test]
fn collapsed_halo_does_not_shift_layout(cx: &mut TestAppContext) {
    fn geom(snap: &BoundsSnapshot, id: &str) -> (f32, f32, f32) {
        let i = info(snap, &expand_toggle_id_for(id))
            .unwrap_or_else(|| panic!("no chevron for {id}:\n{}", snap.dump()));
        (i.x, i.width, i.height)
    }

    let snap = render_fixture(cx, rows_column(IdShape::Production));
    assert_eq!(
        geom(&snap, "parent-collapsed"),
        geom(&snap, "parent-expanded"),
        "collapsed halo must not change the chevron's box — it is background paint, not layout"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
