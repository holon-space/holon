//! Regression (#39): `text(col("session_count"))` renders "" for an INTEGER
//! column.
//!
//! The build-time snapshot coerces numerics correctly
//! (`ResolvedArgs::get_positional_string`), so MCP / PBT / headless snapshots
//! show the digits. The live frontends spawn a CDC subscription that
//! *re-derives* `content` from the row on every write — and that derivation
//! used `Value::as_string()`, which is `None` for every non-String variant.
//! The subscription fires immediately with the current row, so the digits were
//! overwritten with "" before the first frame.

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;

fn col(name: &str) -> RenderExpr {
    RenderExpr::ColumnRef {
        name: name.to_string(),
    }
}

fn text_of(expr: RenderExpr) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "text".to_string(),
        args: vec![Arg {
            name: None,
            value: expr,
        }],
    }
}

fn row(pairs: &[(&str, Value)]) -> Arc<DataRow> {
    let mut m: DataRow = HashMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    Arc::new(m)
}

/// Interpret through the prod shadow-builder path with a live CDC row handle,
/// then let the spawned subscription run before reading the props.
async fn rendered_content(column: &str, r: Arc<DataRow>) -> ReactiveViewModel {
    let services = StubBuilderServices::new();
    let cell = Mutable::new(r).read_only();
    let ctx = RenderContext::default().with_row_mutable(cell.clone());
    let vm = services.interpret(&text_of(col(column)), &ctx);
    // The derivation runs on a spawned task; give it a chance to land.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    vm
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn integer_column_renders_its_digits() {
    let vm = rendered_content(
        "session_count",
        row(&[("session_count", Value::Integer(42))]),
    )
    .await;
    assert_eq!(
        vm.prop_str("content").as_deref(),
        Some("42"),
        "an INTEGER column must render its digits, not blank"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn float_and_bool_columns_render_deliberately() {
    let f = rendered_content("ratio", row(&[("ratio", Value::Float(1.5))])).await;
    assert_eq!(f.prop_str("content").as_deref(), Some("1.5"));

    let b = rendered_content("done", row(&[("done", Value::Boolean(true))])).await;
    assert_eq!(b.prop_str("content").as_deref(), Some("true"));

    let d = rendered_content(
        "last_activity",
        row(&[(
            "last_activity",
            Value::DateTime("2026-07-30T10:00:00Z".to_string()),
        )]),
    )
    .await;
    assert_eq!(
        d.prop_str("content").as_deref(),
        Some("2026-07-30T10:00:00Z")
    );
}

/// SQL NULL is genuinely empty — that is the ONE correct empty rendering.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn null_column_renders_empty() {
    let vm = rendered_content("session_count", row(&[("session_count", Value::Null)])).await;
    assert_eq!(vm.prop_str("content").as_deref(), Some(""));
}

/// A row that does not carry the column at all (pre-first-batch empty row)
/// must NOT blank the build-time snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absent_column_keeps_the_build_time_snapshot() {
    let services = StubBuilderServices::new();
    let full = row(&[("session_count", Value::Integer(7))]);
    let cell = Mutable::new(full);
    let ctx = RenderContext::default().with_row_mutable(cell.read_only());
    let vm = services.interpret(&text_of(col("session_count")), &ctx);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(vm.prop_str("content").as_deref(), Some("7"));

    // A subsequent CDC write that drops the column must not wipe the digits.
    cell.set(row(&[("other", Value::Integer(1))]));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(
        vm.prop_str("content").as_deref(),
        Some("7"),
        "a row without the bound column must leave content untouched, not blank it"
    );
}
