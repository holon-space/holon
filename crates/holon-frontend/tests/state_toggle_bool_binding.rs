//! `state_toggle(#{binding: "bool"})` over a SQLite INTEGER column.
//!
//! SQLite has no boolean type: `integration_state.enabled INTEGER` comes back
//! as `Value::Integer(0|1)` (`crates/holon-turso/src/turso.rs:1062`), and
//! `Value::as_bool` matches `Value::Boolean` alone. A toggle that read the
//! column with `as_bool` would see `None` for every row and paint every switch
//! off — the same shape as #39, where `as_string` on an INTEGER column rendered
//! "" and the CDC subscription overwrote the build-time snapshot with it.
//!
//! So both legs are asserted here: the build-time read AND the re-derivation
//! the live subscription performs.
//!
//! @pbt kind harness
//! @pbt covers state-toggle-bool-binding — a bool-bound toggle reads an INTEGER
//! column as a typed bool, at build time and after a CDC re-derive, and refuses
//! a value that is neither
//! @pbt slips-if-removed every integration switch paints off regardless of the
//! stored decision

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;

fn row(pairs: &[(&str, Value)]) -> Arc<DataRow> {
    let mut m: DataRow = HashMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    Arc::new(m)
}

/// Parsed from real DSL source rather than hand-built, so the test exercises
/// the same arg shape a layout author produces.
fn bool_toggle_expr() -> RenderExpr {
    holon_api::render_dsl::parse_render_dsl(
        r#"state_toggle(#{field: "enabled", binding: "bool", appearance: "switch"})"#,
    )
    .expect("the bool-bound toggle source must parse")
}

/// Interpret through the prod shadow-builder path with a live CDC row handle.
fn build(r: Arc<DataRow>) -> (ReactiveViewModel, Mutable<Arc<DataRow>>) {
    let services = StubBuilderServices::new();
    let cell = Mutable::new(r);
    let ctx = RenderContext::default().with_row_mutable(cell.clone().read_only());
    let vm = services.interpret(&bool_toggle_expr(), &ctx);
    (vm, cell)
}

fn current(vm: &ReactiveViewModel) -> Option<Value> {
    vm.props.lock_ref().get("current").cloned()
}

#[test]
fn a_bool_bound_toggle_reads_an_integer_column() {
    let (vm, _cell) = build(row(&[("enabled", Value::Integer(1))]));
    assert_eq!(
        current(&vm),
        Some(Value::Boolean(true)),
        "a bool-bound toggle must read INTEGER 1 as a typed `true`; `as_bool` on the raw column \
         yields None and would paint the switch off"
    );
}

#[test]
fn zero_reads_as_false_rather_than_as_absent() {
    let (vm, _cell) = build(row(&[("enabled", Value::Integer(0))]));
    assert_eq!(
        current(&vm),
        Some(Value::Boolean(false)),
        "INTEGER 0 is a stored `false`, not a missing value"
    );
}

#[tokio::test]
async fn the_cdc_rederivation_keeps_the_bool() {
    let (vm, cell) = build(row(&[("enabled", Value::Integer(0))]));
    cell.set(row(&[("enabled", Value::Integer(1))]));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    assert_eq!(
        current(&vm),
        Some(Value::Boolean(true)),
        "the live subscription must re-derive through the same typed read as the build-time \
         snapshot"
    );
}

#[test]
#[should_panic(expected = "String(\"yes\")")]
fn a_value_that_is_neither_integer_nor_bool_is_refused() {
    let _ = build(row(&[("enabled", Value::String("yes".to_string()))]));
}

#[test]
#[should_panic(expected = "Integer(7)")]
fn an_integer_outside_zero_and_one_is_refused() {
    let _ = build(row(&[("enabled", Value::Integer(7))]));
}
