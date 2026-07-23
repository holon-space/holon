//! Windowed smoke — the SEEDED main panel renders the accordion split.
//!
//! Unlike `accordion_bounded_pbt.rs` (which hand-builds the
//! `columns(column(list, divider, accordion(text rows)))` shape as a VM tree),
//! this test drives the REAL render expression authored in
//! `assets/default/index.org` for `block:default-main-panel`
//! (`column(collection_view(), divider(), accordion(#{... max_height_fraction:
//! 0.33 ...}, live_query(#{... backlinks ...})))`). It parses that exact string
//! with the production `parse_render_dsl`, so a regression that breaks the
//! seeded expression — a bad edit to `index.org`, a parser change, or a rewire
//! of the accordion split — fails HERE.
//!
//! Two seed-time / backend-bound leaves have no backend in this fast-UI layer,
//! so they get faithful stand-ins that preserve the geometry under test:
//!   - `collection_view()` — a seed marker that production substitutes with the
//!     block's profile-derived (content-height) outline collection. We swap in
//!     a static content-height `list` of data-bound rows at the VM level (the
//!     same recipe `accordion_bounded_pbt` uses for its outline).
//!   - `live_query(#{... backlinks ...})` — fed by `TestServices`' canned
//!     `watch_query_live` (see `support/mod.rs`).
//!
//! Everything else — the `column` / `divider` / `accordion` structure, the
//! `max_height_fraction: 0.33` cap, the pinned-footer split — is the untouched
//! seeded expression. This test proves the seed EXPRESSION composes the split:
//! it parses, the flow-panel split fires, the accordion is tagged and pinned as
//! a bottom footer bounded by `max_h`, the seed's `live_query` is wired INSIDE
//! that footer, and the outline (collection_view stand-in) renders above it.
//!
//! FINDING + FIX — the `live_query` gpui builder wraps its `ReactiveShell` in a
//! forced `height: relative(1.0)` (greedy) style (live_query.rs:72). A
//! percentage height needs a DEFINITE parent; the accordion body is
//! `flex_1 min_h_0` (needed for the internal scroll), which in a
//! shrink-to-content (`max_h`) region resolves to 0 — so the shell and every
//! backlink row would collapse to 0 (header-only region). FIX: `render_bounded`
//! now gives the region a DEFINITE height (`h(relative(f))`, fixed AT the cap)
//! when it holds a greedy slot child, so the shell fills the cap — capped +
//! scrollable + VISIBLE (the plan's accepted "no shrink-to-content for
//! live_query" R4 tradeoff). Text/collection children keep `max_h` and shrink.
//! This is a LAYOUT fix (sound by the Inc 0 spike's proven `relative`-against-
//! definite-parent principle). This fast-UI harness still cannot exercise ROW
//! VISIBILITY (the stub `ReactiveShell` needs the real reactive engine + a
//! driven runtime), so cap saturation / backlink rows are NOT asserted here;
//! that is confirmed by dogfood (Inc 6) or a real-backend windowed test
//! (boot + `seed_default_layout` + render `block:default-main-panel`). What IS
//! asserted below: the seed parses, the split fires, the accordion is tagged +
//! bounded, and the `live_query` is wired inside the footer.
//!
//! Run: `cargo test -p holon-gpui --test seeded_accordion_panel_smoke`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::render_dsl::parse_render_dsl;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// The bundled seed, at compile time — the SAME asset `holon_app::seed` embeds
/// via `include_str!`. Guarantees this test tracks the real seeded expression.
const SEED_ORG: &str = include_str!("../../../assets/default/index.org");

/// Seed value (mirrors `index.org`). Asserted present in the extracted string
/// so a change to the seed's cap fraction can't silently pass this smoke.
const FRACTION: f32 = 0.33;
const WINDOW_W: f32 = 1000.0;
const WINDOW_H: f32 = 900.0;
/// Rows in the injected outline collection — several viewports tall.
const OUTLINE_N: usize = 40;
/// Cap slack (px): the tagged accordion region resolves against `relative(f)`,
/// which rounds; matches `accordion_bounded_pbt`'s `EPS`.
const EPS: f32 = 2.0;

/// Pull the `default-main-panel::render::0` SRC block body out of the seed org.
fn extract_main_panel_render() -> String {
    let start = "#+BEGIN_SRC render :id default-main-panel::render::0";
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

/// Replace the `collection_view()` seed marker with an empty `list()` sentinel
/// (later populated at the VM level). Mirrors the recursive shape of
/// `block_domain::substitute_collection_view`.
fn substitute_collection_view(expr: RenderExpr) -> RenderExpr {
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            if name == "collection_view" {
                return RenderExpr::FunctionCall {
                    name: "list".to_string(),
                    args: vec![],
                };
            }
            RenderExpr::FunctionCall {
                name,
                args: args
                    .into_iter()
                    .map(|a| Arg {
                        name: a.name,
                        value: substitute_collection_view(a.value),
                    })
                    .collect(),
            }
        }
        RenderExpr::BinaryOp { op, left, right } => RenderExpr::BinaryOp {
            op,
            left: Box::new(substitute_collection_view(*left)),
            right: Box::new(substitute_collection_view(*right)),
        },
        RenderExpr::Array { items } => RenderExpr::Array {
            items: items.into_iter().map(substitute_collection_view).collect(),
        },
        RenderExpr::Object { fields } => RenderExpr::Object {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, substitute_collection_view(v)))
                .collect(),
        },
        other => other,
    }
}

/// Data-bound `text` row (see `builders/text.rs`): `data.id` + `field ==
/// "content"` makes its bounds appear keyed by `entity_id` (`outline-{ix}`).
fn outline_row(ix: usize) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(format!("outline-{ix}")));
    data.insert(
        "content".to_string(),
        Value::String(format!("outline {ix}")),
    );
    let mut props = HashMap::new();
    props.insert(
        "content".to_string(),
        Value::String(format!("outline {ix}")),
    );
    props.insert("field".to_string(), Value::String("content".to_string()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

/// A static, content-height outline collection — the faithful stand-in for the
/// seed's profile-derived `collection_view()`.
fn outline_collection() -> ReactiveViewModel {
    let items: Vec<ReactiveViewModel> = (0..OUTLINE_N).map(outline_row).collect();
    let view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    ReactiveViewModel {
        collection: Some(view),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    }
}

fn rows_with_entity_prefix<'a>(
    snap: &'a support::BoundsSnapshot,
    prefix: &str,
) -> Vec<&'a ElementInfo> {
    snap.of_type("text")
        .filter(|i| {
            i.height > 0.0
                && i.entity_id
                    .as_deref()
                    .is_some_and(|e| e.starts_with(prefix))
        })
        .collect()
}

#[gpui::test]
fn seeded_main_panel_renders_capped_accordion_split(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    // 1. The REAL seeded expression, parsed by the production DSL parser.
    let src = extract_main_panel_render();
    assert!(
        src.contains("max_height_fraction: 0.33"),
        "seed cap fraction drifted from this test's FRACTION={FRACTION}; got:\n{src}"
    );
    let column_expr = substitute_collection_view(
        parse_render_dsl(&src).expect("seeded main-panel render must parse"),
    );

    // 2. Interpret through a QUIESCENT `TestServices` threaded as the builder
    //    services (so `live_query`'s `watch_query` returns Ok and `text(col)`
    //    subscriptions queue on the never-driven current-thread runtime).
    let services =
        support::TestServices::with_registry_quiescent(Arc::new(support::BlockTreeRegistry::new()));
    let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
    let ctx = RenderContext::default();
    let mut column_vm = interp.interpret(&column_expr, &ctx, &*services);

    // 3. Swap the `list()` sentinel for a populated content-height outline.
    let sentinel = column_vm
        .children
        .iter()
        .position(|c| c.widget_name().as_deref() == Some("list"))
        .expect("substituted `collection_view` -> `list` sentinel must be a direct column child");
    column_vm.children[sentinel] = Arc::new(outline_collection());

    // 4. Wrap in `columns(...)` — the panel-wrap that triggers `columns::render` ->
    //    `render_accordion_split` (the split fires only under a columns panel,
    //    exactly as the seeded main panel is hosted).
    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![column_vm],
        CollectionVariant::columns(4.0),
    ));
    let root = Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    });

    // 5. Render at a definite window (the panel height the cap resolves against).
    let snap = support::render_reactive_fixture_quiescent_sized(
        cx,
        root,
        size(px(WINDOW_W), px(WINDOW_H)),
    );

    let cap = FRACTION * WINDOW_H;
    let accs: Vec<&ElementInfo> = snap.of_type("accordion").collect();
    assert!(
        !accs.is_empty(),
        "the seeded main panel must render a tagged `accordion` footer region — \
         the split failed to fire for the real seed expression.\n{}",
        snap.dump()
    );
    let acc = accs
        .iter()
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .unwrap();

    // Bounded: the accordion region never EXCEEDS its `max_h(relative(0.33))`
    // cap. (Cap SATURATION — the R4 "greedy live_query fills to the cap" claim —
    // is NOT asserted here: the fast-UI stub cannot render the live_query
    // `ReactiveShell`'s inner rows, which need the real reactive engine, so the
    // region reports header height only. See this file's FINDING note and the
    // real-backend windowed follow-up.)
    assert!(
        acc.height <= cap + EPS,
        "accordion region height {} EXCEEDS cap {} (0.33 x {WINDOW_H}) — the bounded footer \
         failed to clamp its content.\n{}",
        acc.height,
        cap,
        snap.dump()
    );
    // Present, not degenerate: at least the header renders.
    assert!(
        acc.height > 0.0,
        "accordion footer region must have nonzero height.\n{}",
        snap.dump()
    );

    // The split wired the seed's `live_query` INSIDE the accordion footer (not
    // as a sibling / main-region child): a `live_query` node is present at/below
    // the accordion region top.
    let live_queries: Vec<&ElementInfo> = snap.of_type("live_query").collect();
    assert!(
        !live_queries.is_empty(),
        "the seeded accordion must wrap a `live_query` node (the backlinks source).\n{}",
        snap.dump()
    );
    assert!(
        live_queries.iter().any(|lq| lq.y >= acc.y - EPS),
        "the backlinks `live_query` must render inside the pinned accordion footer \
         (y >= {}), not the scrollable main region.",
        acc.y
    );

    // Pinned FOOTER placement: the accordion sits in the lower panel, below the
    // outline main region (R8 pinned-footer split — not stacked at the top).
    assert!(
        acc.y > WINDOW_H * 0.5,
        "accordion footer should be pinned in the lower panel (y {} > {}), proving the \
         flow-panel split placed it as a footer under the outline.\n{}",
        acc.y,
        WINDOW_H * 0.5,
        snap.dump()
    );

    // The outline (collection_view stand-in) is present and ABOVE the footer —
    // the scrollable main region, not clipped away by the split.
    let outline = rows_with_entity_prefix(&snap, "outline-");
    assert!(
        outline.len() >= 3,
        "the outline region above the accordion must render rows (collection_view stand-in).\n{}",
        snap.dump()
    );
    assert!(
        outline.iter().any(|r| r.y < acc.y),
        "at least some outline rows must sit ABOVE the pinned accordion footer (main region)."
    );
}
