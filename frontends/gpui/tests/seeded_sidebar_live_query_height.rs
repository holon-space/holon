//! Windowed regression — the SEEDED left sidebar's `live_query` (Integrations)
//! paints NON-ZERO height.
//!
//! Drives the REAL render expression authored in `assets/default/index.org` for
//! `block:default-left-sidebar` (`column(tree(...), divider(), row(...
//! "Integrations" ...), live_query(#{sql: "... FROM integration_state
//! ..."}))`), parsed by the production DSL parser, composed the way production
//! composes it (registered block tree + `live_block` inside `columns`, so the
//! shell is `ShellPlacement::Panel`).
//!
//! BUG (BugFunnel 2026-08-02): `live_query`'s GPUI builder forces
//! `height: relative(1.0)` whenever `placement == Panel`. A percentage height
//! needs a DEFINITE parent; the sidebar's `column` is content-sized
//! (`div().flex().flex_col()`, no height), so the shell resolved to 0 px and
//! the whole Integrations section vanished — header visible, rows gone — even
//! with a real `integration_state` row present. Every content-height container
//! now routes a `live_query` child through `render_content_height`
//! (`column::push_content_child`).
//!
//! The `integration_state` rows come from `TestServices`' canned
//! `watch_query_live` (`support/mod.rs`), which recognises the seeded SQL and
//! yields data-bound `text` rows keyed `integration-{ix}` — so this asserts
//! RENDERED ROWS, not just the region box, which is exactly the assertion the
//! BugFunnel COVERAGE row says nothing in the suite made.
//!
//! Run: `cargo test -p holon-gpui --test seeded_sidebar_live_query_height`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::render_dsl::parse_render_dsl;
use holon_frontend::RenderContext;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// The bundled seed, at compile time — the SAME asset `holon_app::seed` embeds.
const SEED_ORG: &str = include_str!("../../../assets/default/index.org");

const WINDOW_W: f32 = 1000.0;
const WINDOW_H: f32 = 900.0;

/// Pull the `left_sidebar::render::0` SRC block body out of the seed org.
fn extract_sidebar_render() -> String {
    let start = "#+BEGIN_SRC render :id left_sidebar::render::0";
    let mut body = Vec::new();
    let mut in_block = false;
    for line in SEED_ORG.lines() {
        if in_block {
            if line.trim_start().starts_with("#+END_SRC") {
                break;
            }
            body.push(line);
        } else if line.contains(start) {
            in_block = true;
        }
    }
    assert!(
        !body.is_empty(),
        "the seed must contain a `{start}` render block — did the id change?"
    );
    body.join("\n")
}

fn rows_with_entity_prefix<'a>(
    snap: &'a support::BoundsSnapshot,
    prefix: &str,
) -> Vec<&'a ElementInfo> {
    snap.of_type("text")
        .filter(|i| {
            i.entity_id
                .as_deref()
                .is_some_and(|e| e.starts_with(prefix))
        })
        .collect()
}

#[gpui::test]
fn seeded_sidebar_live_query_paints_nonzero_height(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    // 1. The REAL seeded expression, parsed by the production DSL parser.
    let src = extract_sidebar_render();
    assert!(
        src.contains("integration_state"),
        "the seeded left sidebar must still carry the `integration_state` live_query \
         (the Integrations section under test); got:\n{src}"
    );
    let column_expr = parse_render_dsl(&src).expect("seeded left-sidebar render must parse");

    // 2. Interpret through a QUIESCENT `TestServices` threaded as the builder
    //    services, so the seeded `live_query` reaches the canned `watch_query_live`
    //    instead of a real engine.
    let registry = Arc::new(support::BlockTreeRegistry::new());
    let services = support::TestServices::with_registry_quiescent(registry.clone());
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = services;
    let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
    let ctx = RenderContext::default();
    let column_vm = interp.interpret(&column_expr, &ctx, &*services);
    assert!(
        column_vm
            .children
            .iter()
            .any(|c| c.widget_name().as_deref() == Some("live_query")),
        "the seeded sidebar column must hold the `live_query` as a DIRECT child \
         (bare-column placement is the shape under test)"
    );

    // 3. PRODUCTION-FAITHFUL composition: register the column as
    //    `block:default-left-sidebar` and wrap it in a `live_block` inside
    //    `columns`, so it renders through the REAL per-block `ReactiveShell` at
    //    `ShellPlacement::Panel` — the placement that triggers the greedy height.
    let column_slot = std::sync::Mutex::new(Some(column_vm));
    let thunk: support::BlockTreeThunk = Arc::new(move || {
        column_slot
            .lock()
            .unwrap()
            .take()
            .expect("watch_live called more than once for the seeded left sidebar")
    });
    registry.register(
        "block:default-left-sidebar",
        vec![("default".to_string(), thunk)],
        0,
    );
    let live_block =
        ReactiveViewModel::live_block(holon_api::EntityUri::block("default-left-sidebar"));
    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![live_block],
        CollectionVariant::columns(4.0),
    ));
    let root = Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    });

    let snap = support::render_reactive_fixture_quiescent_sized_with_services(
        cx,
        root,
        size(px(WINDOW_W), px(WINDOW_H)),
        services,
    );

    // The `live_query` region itself must occupy pixels.
    let lqs: Vec<&ElementInfo> = snap.of_type("live_query").collect();
    assert!(
        !lqs.is_empty(),
        "the seeded sidebar must render a tagged `live_query` node.\n{}",
        snap.dump()
    );
    let tallest = lqs
        .iter()
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .unwrap();
    assert!(
        tallest.height > 0.0,
        "the sidebar's Integrations `live_query` collapsed to {} px — a greedy \
         `height: relative(1.0)` resolved against the content-sized sidebar `column`.\n{}",
        tallest.height,
        snap.dump()
    );

    // ... and its ROWS must be laid out with real height (the assertion the
    // whole widget exists for: a builder that resolves rows and paints them at
    // zero height must not pass).
    // `support::TestServices` ids the canned integration rows `integration-<n>`
    // (its `watch_query_live` picks the prefix off the query's table name).
    let rows = rows_with_entity_prefix(&snap, "integration-");
    assert!(
        rows.len() >= 3,
        "the Integrations section must render its `integration_state` rows; found {} \
         (canned watcher yields {}).\n{}",
        rows.len(),
        support::CANNED_LIVE_QUERY_ROWS,
        snap.dump()
    );
    // Rows starting below the panel bottom are clipped by the sidebar's own
    // scroll viewport and legitimately measure 0 — the collapse under test is
    // rows that start ON SCREEN and still have no height.
    let onscreen: Vec<&&ElementInfo> = rows.iter().filter(|r| r.y < WINDOW_H).collect();
    assert!(
        onscreen.iter().all(|r| r.height > 0.0),
        "every on-screen `integration_state` row must have nonzero height; zero-height rows: {:?}\n{}",
        onscreen
            .iter()
            .filter(|r| r.height <= 0.0)
            .map(|r| r.entity_id.clone())
            .collect::<Vec<_>>(),
        snap.dump()
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
