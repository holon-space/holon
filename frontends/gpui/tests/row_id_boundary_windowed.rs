//! Contract: a query row's `id` column is a value the vault's own SQL chooses.
//! One that forms no URI must paint ONE error banner naming it — never unwind
//! inside the URI constructor, which kills the whole window rather than the
//! one row.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! row_id_boundary_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

mod support;

use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::interpret_pure;
use support::BoundsSnapshot;
use support::render_reactive_fixture_quiescent_sized;

/// A collection over one query row, painted through the real GPUI builders.
fn painted(cx: &mut TestAppContext, dsl: &str, id: &str) -> BoundsSnapshot {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(id.to_string()));
    row.insert("content".to_string(), Value::String("hello".to_string()));
    let expr = holon_api::render_dsl::parse_render_dsl(dsl).expect("dsl should parse");
    let vm = Arc::new(interpret_pure(
        &expr,
        &[Arc::new(row)],
        &StubBuilderServices::new(),
    ));

    let snap = render_reactive_fixture_quiescent_sized(cx, vm, size(px(900.0), px(600.0)));
    assert!(
        !snap.entries.is_empty(),
        "the window tracked NO elements at all — the fixture painted nothing, so the \
         assertion below would be vacuous"
    );
    snap
}

/// The messages the frame painted for error nodes, one per error row.
fn error_texts(snap: &BoundsSnapshot) -> Vec<String> {
    snap.of_type("error_message")
        .filter_map(|info| info.displayed_text.as_ref().map(|t| t.to_string()))
        .collect()
}

fn error_rows(snap: &BoundsSnapshot) -> usize {
    snap.of_type("error").count()
}

/// `badge` reaches the row id through `entity_id()`; the refusal replaces the
/// row before either leaf is built, so exactly one banner is painted.
#[gpui::test]
fn a_row_whose_id_forms_no_uri_paints_exactly_one_error_banner(cx: &mut TestAppContext) {
    let snap = painted(
        cx,
        "columns(#{item_template: row(#{children: [badge(col(\"content\")), \
         text(col(\"content\"))]})})",
        "my task",
    );
    let errors = error_texts(&snap);

    assert_eq!(
        error_rows(&snap),
        1,
        "one refused row paints one error row, not one per leaf"
    );
    assert_eq!(errors.len(), 1, "one message per error row: {errors:?}");
    assert!(errors[0].contains("my task"), "{errors:?}");
}

/// The control: the banner comes from the refusal, not from every collection.
#[gpui::test]
fn a_well_formed_row_paints_no_error_banner(cx: &mut TestAppContext) {
    let snap = painted(
        cx,
        "columns(#{item_template: badge(col(\"content\"))})",
        "block:ok",
    );

    assert_eq!(error_rows(&snap), 0, "dump:\n{}", snap.dump());
    assert_eq!(error_texts(&snap), Vec::<String>::new());
}

/// The board's STREAMING path binds the source row to the card's `data`
/// (`flat_driver::interpret_and_attach`) — nothing upstream of the GPUI
/// builder classifies it. A card whose row id forms no URI must become a
/// refusal banner in the lane, never a draggable card: every persistence leg
/// the board has (`set_field` on a lane change, `move_block` on a reorder) is
/// addressed by that id.
#[gpui::test]
fn a_board_card_whose_row_id_forms_no_uri_becomes_a_lane_refusal(cx: &mut TestAppContext) {
    let snap = painted_board(cx, "my task");

    let banners: Vec<String> = snap
        .of_type("error")
        .filter_map(|info| info.displayed_text.as_ref().map(|t| t.to_string()))
        .collect();
    assert_eq!(
        banners.len(),
        1,
        "the refused card paints one banner: {}",
        snap.dump()
    );
    assert!(
        banners[0].contains("my task"),
        "the banner must name the id it refused: {banners:?}"
    );
}

/// The control: a card whose row id names an entity still paints, with no
/// banner.
#[gpui::test]
fn a_board_card_with_a_schemed_row_id_paints_no_refusal(cx: &mut TestAppContext) {
    let snap = painted_board(cx, "block:ok");

    assert_eq!(error_rows(&snap), 0, "dump:\n{}", snap.dump());
    assert_eq!(error_texts(&snap), Vec::<String>::new());
}

/// A hand-built board → lane → card tree in the shape the streaming grouped
/// driver produces: the card carries its source row on `data` and NO `row_id`
/// prop (the prop is the static path's stamp).
fn painted_board(cx: &mut TestAppContext, id: &str) -> BoundsSnapshot {
    use holon_frontend::reactive_view_model::ReactiveViewModel;

    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(id.to_string()));
    row.insert("content".to_string(), Value::String("hello".to_string()));
    let row = Arc::new(row);

    let card = ReactiveViewModel::from_widget("card", Default::default())
        .with_entity(row)
        .with_children(vec![ReactiveViewModel::text("hello")]);
    let mut lane_props = std::collections::HashMap::new();
    lane_props.insert("title".to_string(), Value::String("Todo".to_string()));
    let lane = ReactiveViewModel::from_widget("board_lane", lane_props).with_children(vec![card]);
    let board =
        ReactiveViewModel::from_widget("board", Default::default()).with_children(vec![lane]);

    let snap =
        render_reactive_fixture_quiescent_sized(cx, Arc::new(board), size(px(900.0), px(600.0)));
    assert!(
        !snap.entries.is_empty(),
        "the window tracked NO elements at all — the fixture painted nothing"
    );
    snap
}

mod test_init;
