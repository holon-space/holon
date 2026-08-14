//! Regression (#45): `await_ready` returned after the Structure event alone.
//!
//! `describe_ui` awaits readiness before snapshotting. Structure only carries
//! the render expression; the first Data batch lands ~200µs later. A cold probe
//! therefore snapshotted a data-driven list with zero rows and reported it as
//! genuinely empty — a phantom that fabricated two false mechanisms in one week
//! of debugging.
//!
//! The data barrier must be conditional: a render expression that reads no
//! column has no data stream, and blocking it would turn every static block
//! into a five-second timeout.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_api::streaming::Batch;
use holon_api::streaming::BatchMetadata;
use holon_api::streaming::Change;
use holon_api::streaming::ChangeOrigin;
use holon_api::streaming::UiEvent;
use holon_api::streaming::WithMetadata;
use holon_frontend::reactive::ReactiveRenderedRows;

/// `list(collection: ..., row: text(col("name")))` — reads a column, so it is
/// data driven.
fn data_driven_expr() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "list".to_string(),
        args: vec![Arg {
            name: Some("row".to_string()),
            value: RenderExpr::FunctionCall {
                name: "text".to_string(),
                args: vec![Arg {
                    name: None,
                    value: RenderExpr::ColumnRef {
                        name: "name".to_string(),
                    },
                }],
            },
        }],
    }
}

/// A static label — no column anywhere, so no data stream to wait for.
fn static_expr() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "text".to_string(),
        args: vec![Arg {
            name: None,
            value: RenderExpr::Literal {
                value: Value::String("Journals".to_string()),
            },
        }],
    }
}

fn structure(expr: RenderExpr, generation: u64) -> UiEvent {
    UiEvent::Structure {
        render_expr: expr,
        candidates: Vec::new(),
        generation,
    }
}

fn data_batch(generation: u64) -> UiEvent {
    let row: HashMap<String, Value> = HashMap::from([
        ("id".to_string(), Value::String("block:r1".to_string())),
        ("name".to_string(), Value::String("row one".to_string())),
    ]);
    UiEvent::Data {
        batch: WithMetadata {
            inner: Batch {
                items: vec![Change::Created {
                    data: row,
                    origin: ChangeOrigin::Local {
                        operation_id: None,
                        trace_id: None,
                    },
                }],
            },
            metadata: BatchMetadata {
                relation_name: "rows".into(),
                trace_context: None,
                linked_contexts: Vec::new(),
                sync_token: None,
                seq: 0,
            },
        },
        generation,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readiness_waits_for_the_first_data_batch_of_a_data_driven_block() {
    let rows = Arc::new(ReactiveRenderedRows::new());
    rows.apply_event(structure(data_driven_expr(), 1));

    // Structure alone must NOT satisfy readiness — this is the phantom.
    let too_early = tokio::time::timeout(
        std::time::Duration::from_millis(150),
        rows.wait_until_ready(),
    )
    .await;
    assert!(
        too_early.is_err(),
        "readiness returned on Structure alone — a cold probe would snapshot an empty list and \
         report it as genuinely empty"
    );

    let ready = tokio::spawn({
        let rows = Arc::clone(&rows);
        async move { rows.wait_until_ready().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    rows.apply_event(data_batch(1));

    tokio::time::timeout(std::time::Duration::from_secs(2), ready)
        .await
        .expect("readiness must resolve once the first Data batch lands")
        .expect("readiness task panicked");
    assert_eq!(rows.snapshot().1.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readiness_returns_immediately_once_data_already_landed() {
    let rows = ReactiveRenderedRows::new();
    rows.apply_event(structure(data_driven_expr(), 1));
    rows.apply_event(data_batch(1));

    tokio::time::timeout(
        std::time::Duration::from_millis(150),
        rows.wait_until_ready(),
    )
    .await
    .expect("data already landed — readiness must not block");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_block_with_no_query_does_not_wait_for_data() {
    let rows = ReactiveRenderedRows::new();
    rows.apply_event(structure(static_expr(), 1));

    tokio::time::timeout(
        std::time::Duration::from_millis(150),
        rows.wait_until_ready(),
    )
    .await
    .expect("a render expression reading no column has no data stream to wait for");
}
