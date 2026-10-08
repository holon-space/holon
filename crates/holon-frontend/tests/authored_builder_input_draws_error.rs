//! A render expression is authored data. A builder that rejects its arguments
//! draws an `error` node naming the builder, the argument and the problem; a
//! panic there aborts the app (`panic = "abort"`).

use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;

fn render_json_over(dsl: &str, row: DataRow) -> serde_json::Value {
    let expr = holon_api::render_dsl::parse_render_dsl(dsl)
        .unwrap_or_else(|e| panic!("{dsl} does not parse: {e:#}"));
    let services = StubBuilderServices::new();
    let ctx = RenderContext::default().with_row(Arc::new(row));
    let vm = services.interpret(&expr, &ctx).snapshot();
    serde_json::to_value(&vm).expect("ViewModel serializes")
}

fn error_message(json: &serde_json::Value) -> Option<&str> {
    if json.get("widget").and_then(|w| w.as_str()) != Some("error") {
        return None;
    }
    json.get("message").and_then(|m| m.as_str())
}

/// `dsl` over `row` draws an error node whose message contains every needle.
fn assert_error_names(dsl: &str, row: DataRow, needles: &[&str]) {
    let json = render_json_over(dsl, row);
    let message = error_message(&json)
        .unwrap_or_else(|| panic!("`{dsl}` must draw an error node, got: {json}"));
    for needle in needles {
        assert!(
            message.contains(needle),
            "`{dsl}`: the error must name {needle:?}; message: {message:?}"
        );
    }
}

fn no_row() -> DataRow {
    DataRow::new()
}

#[test]
fn op_button_without_a_name_or_target_draws_an_error() {
    assert_error_names("op_button()", no_row(), &["op_button", "op_name"]);
}

#[test]
fn table_columns_not_a_list() {
    assert_error_names(
        r#"table(#{columns: "a"})"#,
        no_row(),
        &["table", "columns", "list"],
    );
}

#[test]
fn table_columns_empty() {
    assert_error_names(
        "table(#{columns: []})",
        no_row(),
        &["table", "columns", "at least one"],
    );
}

#[test]
fn table_column_not_a_map() {
    assert_error_names(
        r#"table(#{columns: ["a"]})"#,
        no_row(),
        &["table", "column", "map"],
    );
}

#[test]
fn table_column_without_header() {
    assert_error_names(
        r#"table(#{columns: [#{cell: "x"}]})"#,
        no_row(),
        &["table", "header", "missing"],
    );
}

#[test]
fn table_column_header_not_a_string() {
    assert_error_names(
        r#"table(#{columns: [#{header: 3, cell: "x"}]})"#,
        no_row(),
        &["table", "header", "string"],
    );
}

#[test]
fn table_column_without_cell() {
    assert_error_names(
        r#"table(#{columns: [#{header: "A"}]})"#,
        no_row(),
        &["table", "`A`", "cell"],
    );
}

#[test]
fn table_column_fixed_width_without_px() {
    assert_error_names(
        r#"table(#{columns: [#{header: "A", cell: "x", width: fixed("wide")}]})"#,
        no_row(),
        &["table", "width", "fixed(px)", "numeric"],
    );
}

#[test]
fn table_column_unknown_width_function() {
    assert_error_names(
        r#"table(#{columns: [#{header: "A", cell: "x", width: auto()}]})"#,
        no_row(),
        &["table", "width", "auto", "unsupported"],
    );
}

#[test]
fn table_column_width_not_a_function() {
    assert_error_names(
        r#"table(#{columns: [#{header: "A", cell: "x", width: 3}]})"#,
        no_row(),
        &["table", "width", "flex(w) or fixed(px)"],
    );
}

#[test]
fn table_min_width_not_a_number() {
    assert_error_names(
        r#"table(#{columns: [#{header: "A", cell: "x"}], min_width: "wide"})"#,
        no_row(),
        &["table", "min_width", "number"],
    );
}

#[test]
fn table_min_width_not_positive() {
    assert_error_names(
        r#"table(#{columns: [#{header: "A", cell: "x"}], min_width: 0})"#,
        no_row(),
        &["table", "min_width", "positive"],
    );
}

#[test]
fn columns_without_children_or_template() {
    assert_error_names("columns()", no_row(), &["columns", "item_template"]);
}

#[test]
fn state_toggle_unknown_appearance() {
    assert_error_names(
        r#"state_toggle(#{appearance: "round"})"#,
        no_row(),
        &["state_toggle", "appearance", "round"],
    );
}

#[test]
fn state_toggle_unknown_binding() {
    assert_error_names(
        r#"state_toggle(#{binding: "maybe"})"#,
        no_row(),
        &["state_toggle", "binding", "maybe"],
    );
}

#[test]
fn state_toggle_bool_binding_over_a_non_bool_value() {
    let row = DataRow::from([("picked".to_string(), Value::String("yes".to_string()))]);
    assert_error_names(
        r#"state_toggle(col("picked"), #{binding: "bool"})"#,
        row,
        &["state_toggle", "picked", "bool"],
    );
}

/// A row that turns non-bool AFTER the toggle was built flips the live node to
/// an error, and a valid value brings the toggle back.
#[test]
fn state_toggle_bool_binding_live_row_turns_non_bool() {
    let services = StubBuilderServices::new();
    let expr = holon_api::render_dsl::parse_render_dsl(
        r#"state_toggle(col("picked"), #{binding: "bool"})"#,
    )
    .expect("parses");
    let row = |v: Value| Arc::new(DataRow::from([("picked".to_string(), v)]));
    let cell = Mutable::new(row(Value::Boolean(true)));
    let ctx = RenderContext::default().with_row_mutable(cell.read_only());
    let vm = services.interpret(&expr, &ctx);
    let widget = |vm: &holon_frontend::ReactiveViewModel| {
        serde_json::to_value(vm.snapshot()).expect("serializes")
    };
    let settle = |pred: &dyn Fn(&serde_json::Value) -> bool| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let json = widget(&vm);
            if pred(&json) {
                return json;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "live state_toggle never settled; last snapshot: {json}"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    };
    let is_toggle =
        |j: &serde_json::Value| j.get("widget").and_then(|w| w.as_str()) == Some("state_toggle");

    settle(&is_toggle);
    cell.set(row(Value::String("yes".to_string())));
    let json = settle(&|j| error_message(j).is_some());
    let message = error_message(&json).expect("error node");
    assert!(
        message.contains("state_toggle") && message.contains("picked"),
        "the live error must name the builder and the field: {message:?}"
    );
    cell.set(row(Value::Integer(0)));
    let json = settle(&is_toggle);
    assert_eq!(
        json.get("current").and_then(|c| c.as_str()),
        Some("false"),
        "a valid value restores the toggle: {json}"
    );
}

#[test]
fn text_ellipsis_names_no_end() {
    assert_error_names(
        r#"text("x", #{ellipsis: "middle"})"#,
        no_row(),
        &["text", "ellipsis", "middle", "\"start\"", "\"end\""],
    );
}

#[test]
fn view_mode_switcher_without_entity_uri() {
    assert_error_names(
        r#"view_mode_switcher(#{modes: "[\"tree\"]"})"#,
        no_row(),
        &["view_mode_switcher", "entity_uri"],
    );
}
