//! Trace connectivity across the two hops that each detach onto a task of their
//! own, and would otherwise each start a fresh trace.
//!
//! WRITE path — one interaction, one trace. `dispatch_intent*` hands the
//! operation to a spawned task, so without an explicit `.instrument()` every
//! hop below it becomes a disconnected root. Asserted via [`REQUIRED_SPANS`]:
//! dispatch, both operation rungs, the Turso write, and the org write-back all
//! share the root's trace id.
//!
//! READ-BACK — the mirror apply joins the trace of the pass that wrote the row.
//! `SqlOperationProvider`'s batch writer stamps `_change_origin` from the
//! writing span, Turso's CDC reassembles it into `BatchMetadata.trace_context`,
//! and `LiveData::subscribe` re-parents `live_data.apply_batch` to it. Asserted
//! by requiring that span to carry a parent — it runs on an actor that outlives
//! every interaction, so a parent can only have come across the CDC hop.
//!
//! The two are NOT one trace, and the assertions deliberately do not claim they
//! are: the Loro→SQL projection runs on its own long-lived pass, so the writing
//! span is the projection's, not the interaction's. One pass may consolidate
//! ops from several interactions, so joining them needs a ruling on what "the"
//! originating trace of a batch is, then context on the projection queue entry.
//!
//! Run: `cargo nextest run -p holon-integration-tests --features pbt \
//!       --test interaction_trace_connectivity`

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;
use opentelemetry::trace::SpanId;
use opentelemetry_sdk::trace::SpanData;

/// The span that opens an interaction. Every other span in the trace must
/// descend from it.
const ROOT_SPAN: &str = "interaction.dispatch";

/// Spans that must share the root's trace id: both operation rungs, the Turso
/// write the projection issues, and the org write-back that consumes the
/// change.
const REQUIRED_SPANS: &[&str] = &[
    "dispatcher.execute_operation",
    "backend.execute_operation",
    "execute",
    "provider.orgmode.sync_changes",
];

fn trace_id(span: &SpanData) -> String {
    format!("{:032x}", span.span_context.trace_id())
}

/// Pick a stable, editable seeded block: a `block:` content block with
/// non-empty content that is not a structural `::src::` / `::render::` node.
async fn pick_target(session: &holon_frontend::FrontendSession) -> String {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    snap.iter_blocks()
        .find(|b| {
            b.id.as_str().starts_with("block:")
                && !b.content.trim().is_empty()
                && !b.content.contains("::src::")
                && !b.content.contains("::render::")
        })
        .map(|b| b.id.as_str().to_string())
        .expect("seeded vault must contain an editable content block")
}

#[test]
fn one_interaction_produces_one_connected_trace() {
    // Installs the OTel tracer provider and the global subscriber. Must run
    // before the SUT boots or the interaction's spans go uncollected.
    let collector = SpanCollector::global();
    // Binds the runtime's workers to this scope, so the CDC emission on the
    // Turso worker lands in the window this thread reads.
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = Arc::new(builder.build().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");

        let session = env.session_arc();
        let reactive = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();

        let target_id = pick_target(&session).await;

        // Drop every span the boot emitted; what remains is the interaction's.
        collector.reset();

        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(target_id.clone()));
        params.insert("field".to_string(), Value::String("content".to_string()));
        params.insert(
            "value".to_string(),
            Value::String("traced-by-connectivity-proof".to_string()),
        );
        params.insert(
            "write_seq".to_string(),
            Value::Integer(holon_api::write_seq::next().get()),
        );

        holon_frontend::reactive::BuilderServices::dispatch_intent_sync(
            reactive.as_ref(),
            holon_frontend::operations::OperationIntent::new(
                holon_api::EntityName::new("block"),
                "set_field".to_string(),
                params,
            ),
        )
        .await
        .expect("set_field dispatch");

        // The write-back trails the op return.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let spans = collector.finished_spans();
            if spans.iter().any(|s| s.name == "live_data.apply_batch") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let spans = collector.finished_spans();
        let root = spans
            .iter()
            .find(|s| s.name == ROOT_SPAN)
            .unwrap_or_else(|| {
                panic!(
                    "no `{ROOT_SPAN}` span: the dispatch seam is not instrumented. Collected: {:?}",
                    spans.iter().map(|s| &s.name).collect::<Vec<_>>()
                )
            });
        let interaction_trace = trace_id(root);

        let mut by_trace: BTreeMap<String, Vec<&SpanData>> = BTreeMap::new();
        for span in &spans {
            by_trace.entry(trace_id(span)).or_default().push(span);
        }
        let in_trace = &by_trace[&interaction_trace];

        println!("{}", render_report(&interaction_trace, &by_trace));

        for required in REQUIRED_SPANS {
            assert!(
                in_trace.iter().any(|s| &s.name == required),
                "trace {interaction_trace} is BROKEN at `{required}`: it is absent from the \
                 interaction's trace. Spans in trace: {:?}",
                in_trace.iter().map(|s| &s.name).collect::<Vec<_>>()
            );
        }

        // The mirror apply runs on the LiveData actor, off the writing task. It
        // can only carry a parent if the write's trace context survived the
        // `_change_origin` round trip through Turso's CDC.
        let applies: Vec<&SpanData> = spans
            .iter()
            .filter(|s| s.name == "live_data.apply_batch")
            .collect();
        assert!(
            !applies.is_empty(),
            "no `live_data.apply_batch` span: the mirror never applied the write"
        );
        for a in &applies {
            let attrs: Vec<String> = a
                .attributes
                .iter()
                .map(|kv| format!("{}={}", kv.key, kv.value.as_str()))
                .collect();
            println!(
                "APPLY parented={} attrs=[{}]",
                a.parent_span_id != SpanId::INVALID,
                attrs.join(", ")
            );
        }
        assert!(
            applies.iter().all(|s| s.parent_span_id != SpanId::INVALID),
            "a `live_data.apply_batch` is a trace ROOT: no trace context survived the CDC \
             hop, so `_change_origin` carried no `ChangeOrigin` the reader could reassemble. \
             {}/{} applies unparented",
            applies
                .iter()
                .filter(|s| s.parent_span_id == SpanId::INVALID)
                .count(),
            applies.len()
        );
    });
}

fn render_report(interaction_trace: &str, by_trace: &BTreeMap<String, Vec<&SpanData>>) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    writeln!(out, "interaction trace id: {interaction_trace}").unwrap();
    for (tid, spans) in by_trace {
        let marker = if tid == interaction_trace {
            " <== INTERACTION"
        } else {
            ""
        };
        writeln!(out, "\ntrace {tid} ({} spans){marker}", spans.len()).unwrap();
        let mut names: BTreeMap<&str, usize> = BTreeMap::new();
        for span in spans {
            *names.entry(span.name.as_ref()).or_default() += 1;
        }
        for (name, count) in names {
            writeln!(out, "  {count:4}x {name}").unwrap();
        }
    }
    out
}
