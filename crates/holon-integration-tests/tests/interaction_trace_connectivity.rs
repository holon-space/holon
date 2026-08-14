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
    "export",
    "commit_internal",
    "provider.orgmode.sync_changes",
];

/// Spans that root a trace of their own by construction: long-lived actors and
/// the parks they sit in, which serve no interaction in particular. Everything
/// else emitted during one interaction must be reachable from that
/// interaction's root.
const BACKGROUND_ROOTS: &[&str] = &["live_data.subscribe_actor", "live_data.stream_next"];

/// The coalesced org write-back pass. Both ends of the channel feeding it are
/// tasks spawned at container construction, so its only possible attribution is
/// the context the block feed carried on the message.
const ORG_WRITE_BACK_PASS: &str = "org.on_block_feed";

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
            // Wait for the PROPERTY, not for a proxy: the controller also runs
            // unattributed passes (boot seeding, the ingest poll), and stopping
            // at the first `org.on_block_feed` to appear routinely stopped
            // before the attributed one had run.
            let spans = collector.finished_spans();
            let attributed: Vec<String> = spans
                .iter()
                .filter(|s| s.name == "live_data.apply_batch")
                .filter(|s| s.parent_span_id != SpanId::INVALID)
                .filter(|s| {
                    s.attributes
                        .iter()
                        .any(|kv| kv.key.as_str() == "source" && kv.value.as_str() == "block")
                })
                .map(|s| trace_id(s))
                .collect();
            if !attributed.is_empty()
                && spans
                    .iter()
                    .any(|s| s.name == ORG_WRITE_BACK_PASS && attributed.contains(&trace_id(s)))
            {
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

        let orphans = unattributed(&spans, root);
        let mut roster: BTreeMap<&str, usize> = BTreeMap::new();
        for span in &orphans {
            *roster.entry(span.name.as_ref()).or_default() += 1;
        }
        println!(
            "UNATTRIBUTED {}/{} spans: {}",
            orphans.len(),
            spans.len(),
            roster
                .iter()
                .map(|(n, c)| format!("{c}x {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        // The seam is always the orphan SUBTREE's root: its descendants are
        // unbillable only because it is.
        for span in orphans.iter().filter(|s| {
            s.parent_span_id == SpanId::INVALID
                || !spans
                    .iter()
                    .any(|p| p.span_context.span_id() == s.parent_span_id)
        }) {
            println!(
                "UNATTRIBUTED ROOT {} (trace {}, links {})",
                span.name,
                trace_id(span),
                span.links.len()
            );
        }
        for span in &spans {
            println!(
                "SPAN {:<32} trace={} span={:016x} parent={:016x} links={}",
                span.name,
                trace_id(span),
                span.span_context.span_id(),
                span.parent_span_id,
                span.links.len()
            );
        }

        // (a) CONNECTIVITY. Every rung of the interaction's own dispatch path
        // descends from its root by parenthood. Scoped to named spans, so
        // concurrent background work in the same window cannot move it.
        let reachable = descendants_of(&spans, root);
        for required in REQUIRED_SPANS {
            assert!(
                in_trace.iter().any(|s| &s.name == required),
                "trace {interaction_trace} is BROKEN at `{required}`: it is absent from the \
                 interaction's trace. Spans in trace: {:?}",
                in_trace.iter().map(|s| &s.name).collect::<Vec<_>>()
            );
            for span in in_trace.iter().filter(|s| &s.name == required) {
                assert!(
                    reachable.contains(&span.span_context.span_id()),
                    "`{required}` carries the interaction's trace id but does not descend from \
                     `{ROOT_SPAN}` — its parent chain is broken, so the trace id is the only \
                     thing tying it to the interaction"
                );
            }
        }

        // (b) `local_with_current_span` is what stamps `_change_origin`, and
        // its `operation_id` becomes the parent of every mirror apply. It must
        // be the writing span's OTel id: a `tracing` registry Id parses as a
        // span id just as well and yields a parent that exists in no trace —
        // connected to look at, unfollowable in practice.
        let probe = tracing::info_span!("phantom_parent_probe");
        let stamped = probe
            .in_scope(|| {
                holon_api::ChangeOrigin::local_with_current_span().to_batch_trace_context()
            })
            .expect("an OTel layer is installed, so the probe span must yield a trace context");
        let probe_span_id = {
            use opentelemetry::trace::TraceContextExt;
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            probe.context().span().span_context().span_id()
        };
        assert_eq!(
            stamped.span_id,
            format!("{probe_span_id:016x}"),
            "`_change_origin` would carry {} as the parent of every mirror apply, but the \
             writing span's OTel id is {probe_span_id:016x}. Nothing downstream can resolve a \
             span id the exporter never emitted.",
            stamped.span_id
        );

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
        // No assertion that EVERY apply carries a parent: three of the four
        // mirror sources watch projections whose SELECT list omits
        // `_change_origin` (`focus_roots`, and the two live-entity reads), so
        // their batches structurally cannot be attributed — measured, task #27.
        // The `block` source can be, and is, asserted below.
        let block_applies: Vec<&&SpanData> = applies
            .iter()
            .filter(|a| {
                a.attributes
                    .iter()
                    .any(|kv| kv.key.as_str() == "source" && kv.value.as_str() == "block")
            })
            .collect();
        assert!(
            !block_applies.is_empty(),
            "no `live_data.apply_batch` for the `block` source: the one mirror whose \
             projection carries `_change_origin` never applied this write"
        );
        // Existence, not universality: `local_with_current_span` yields
        // `(None, None)` when no OTel context is in scope, so the boot ingest's
        // own rows carry an EMPTY origin and their batches are honestly
        // unattributable. They keep arriving during this window. The claim is
        // that the write this interaction issued is attributed.
        let parented_applies: Vec<&&&SpanData> = block_applies
            .iter()
            .filter(|a| a.parent_span_id != SpanId::INVALID)
            .collect();
        assert!(
            !parented_applies.is_empty(),
            "every `block` mirror apply is an unparented root: this mirror's rows DO carry \
             `_change_origin`, so a total absence of parents is a break in the stamp → CDC \
             → re-parent chain, not the boot-ingest case"
        );

        // (d) THE WRITE-BACK PASS IS BILLABLE. `org.on_block_feed` runs on the
        // file-sync controller task, spawned at container construction, and is
        // fed over a channel by a second such task — so before the provenance
        // the feed now carries, it could only be a child of the process boot.
        // It joins the trace of the pass that WROTE the row (the same trace the
        // mirror apply joins), not the interaction's: the Loro→SQL projection
        // between them is a long-lived pass, and that gap is this file's
        // standing caveat, not this assertion's business.
        let write_backs: Vec<&SpanData> = spans
            .iter()
            .filter(|s| s.name == ORG_WRITE_BACK_PASS)
            .collect();
        assert!(
            !write_backs.is_empty(),
            "no `{ORG_WRITE_BACK_PASS}` span: this draw never reached the feed-driven org \
             write-back, so the attribution below would be vacuous"
        );
        let apply_traces: Vec<String> = parented_applies.iter().map(|a| trace_id(a)).collect();
        // Existence, not universality: the controller also runs passes no
        // interaction asked for — boot seeding, the ingest poll — and those are
        // roots of their own, because the task they run on is spawned at
        // container construction and holds no span to inherit. The claim is
        // that the pass serving THIS write is billed to it.
        assert!(
            write_backs
                .iter()
                .any(|pass| apply_traces.contains(&trace_id(pass))),
            "no `{ORG_WRITE_BACK_PASS}` joined a trace the `block` mirror applied \
             ({apply_traces:?}): the passes sit in {:?}, so the feed's provenance never \
             reached the write-back and the org write this interaction caused is billable \
             to nobody",
            write_backs.iter().map(|p| trace_id(p)).collect::<Vec<_>>()
        );

        // Reported, not asserted. The window around one dispatch also catches
        // work no interaction owns — a second `backend.execute_operation` from
        // the ingest poll, mirror applies for background writes — so a bare
        // "no orphans" gate would fail on load, not on regression. Turning the
        // roster into a gate needs the origin plumbing that lets a
        // causally-downstream span PROVE which interaction it serves; until
        // then this number is the tracking metric for that work.
    });
}

/// One CDC batch carries whatever landed in the same commit window, so it
/// routinely serves several writers. Ruling D3.a: the apply parents to the
/// first and LINKS the rest — picking one and dropping the others is how the
/// largest redundancy class became unattributable in the first place.
#[test]
fn a_consolidated_batch_links_every_writer_past_the_first() {
    let collector = SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = Arc::new(builder.build().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        collector.reset();

        let ctx = |span: &str| holon_api::BatchTraceContext {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            span_id: span.to_string(),
            trace_flags: 0x01,
            trace_state: None,
        };
        let mut row: holon_api::StorageEntity = HashMap::new();
        row.insert("id".into(), Value::String("block:linked".into()));
        row.insert("content".into(), Value::String("consolidated".into()));

        let live: Arc<holon_api::live_data::LiveData<String>> = holon_api::live_data::LiveData::new(
            vec![],
            |r| Ok(r.get("id").unwrap().as_string().unwrap().to_string()),
            |r| Ok(r.get("content").unwrap().as_string().unwrap().to_string()),
        );
        live.subscribe(
            "linked_writers",
            tokio_stream::iter(vec![holon_api::BatchWithMetadata {
                inner: holon_api::Batch {
                    items: vec![holon_api::Change::Created {
                        data: row,
                        origin: holon_api::ChangeOrigin::Local {
                            operation_id: None,
                            trace_id: None,
                        },
                    }],
                },
                metadata: holon_api::BatchMetadata {
                    relation_name: "block".to_string(),
                    trace_context: Some(ctx("00f067aa0ba902b7")),
                    linked_contexts: vec![ctx("00f067aa0ba902b8"), ctx("00f067aa0ba902b9")],
                    sync_token: None,
                    seq: 1,
                },
            }]),
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        let apply = loop {
            if let Some(s) = collector
                .finished_spans()
                .into_iter()
                .find(|s| s.name == "live_data.apply_batch")
            {
                break s;
            }
            assert!(
                Instant::now() < deadline,
                "the mirror never applied the synthetic batch"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };

        let linked: Vec<String> = apply
            .links
            .iter()
            .map(|l| format!("{:016x}", l.span_context.span_id()))
            .collect();
        assert_eq!(
            linked,
            vec!["00f067aa0ba902b8", "00f067aa0ba902b9"],
            "a batch consolidating 3 writers must link the 2 past the parent, so each stays \
             attributable; got {linked:?}"
        );
    });
}

/// Span ids reachable from `root` by parenthood alone.
fn descendants_of(spans: &[SpanData], root: &SpanData) -> std::collections::HashSet<SpanId> {
    let mut children: HashMap<SpanId, Vec<SpanId>> = HashMap::new();
    for span in spans {
        children
            .entry(span.parent_span_id)
            .or_default()
            .push(span.span_context.span_id());
    }
    let mut seen: std::collections::HashSet<SpanId> =
        std::iter::once(root.span_context.span_id()).collect();
    let mut queue = vec![root.span_context.span_id()];
    while let Some(id) = queue.pop() {
        for next in children.get(&id).into_iter().flatten() {
            if seen.insert(*next) {
                queue.push(*next);
            }
        }
    }
    seen
}

/// Spans the interaction cannot be billed for: not reachable from its root by
/// parenthood, nor by the links a consolidated pass carries back to the
/// interactions it serves (ruling D3.a).
fn unattributed<'a>(spans: &'a [SpanData], root: &SpanData) -> Vec<&'a SpanData> {
    let mut edges: HashMap<SpanId, Vec<SpanId>> = HashMap::new();
    for span in spans {
        edges
            .entry(span.parent_span_id)
            .or_default()
            .push(span.span_context.span_id());
        // A link points from the consolidated pass BACK to an origin, so the
        // walk follows it in reverse to reach the pass from the interaction.
        for link in span.links.iter() {
            edges
                .entry(link.span_context.span_id())
                .or_default()
                .push(span.span_context.span_id());
        }
    }
    let mut seen: std::collections::HashSet<SpanId> =
        std::iter::once(root.span_context.span_id()).collect();
    let mut queue = vec![root.span_context.span_id()];
    while let Some(id) = queue.pop() {
        for next in edges.get(&id).into_iter().flatten() {
            if seen.insert(*next) {
                queue.push(*next);
            }
        }
    }
    spans
        .iter()
        .filter(|s| !seen.contains(&s.span_context.span_id()))
        .filter(|s| !BACKGROUND_ROOTS.contains(&s.name.as_ref()))
        .collect()
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
