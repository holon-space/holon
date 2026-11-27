//! An editor key event without the cell registry (the GPUI wiring): the
//! handler opens the key-event span, and everything the key dispatches — the
//! operation, its SQL write, the CDC rows a block mirror applies and the e2e
//! close — descends from it by parenthood, across the tokio tasks between.
//!
//! Example-based: the property is the shape of one trace on the production
//! stack, which a generated input does not vary.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::Value;
use holon_frontend::editor_view_model::EditorKey;
use holon_frontend::editor_view_model::EditorViewModel;
use holon_frontend::editor_view_model::StructuralCaret;
use holon_frontend::editor_view_model::dispatch_structural_key;
use holon_frontend::editor_view_model::dispatch_text_change;
use holon_frontend::editor_view_model::structural_block_action;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;
use opentelemetry::trace::SpanId;
use opentelemetry_sdk::trace::SpanData;

use super::interaction_trace_connectivity::descendants_of;
use super::interaction_trace_connectivity::pick_target;
use super::interaction_trace_connectivity::reachable_with_links;

const KEY: &str = "interaction.key";
const DISPATCH: &str = "interaction.dispatch";
const VISIBLE: &str = "interaction.visible";
const FOCUS: &str = "interaction.focus_new_block";
const APPLY: &str = "live_data.apply_batch";

pub(super) fn attr(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.as_str().to_string())
}

pub(super) fn trace_id(span: &SpanData) -> String {
    format!("{:032x}", span.span_context.trace_id())
}

/// Boot without the cell registry, as GPUI does, and run `scenario`.
fn on_gpui_wiring<F, Fut>(scenario: F)
where
    F: FnOnce(Arc<ReactiveEngine>, String, &'static SpanCollector) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let collector = SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = Arc::new(builder.build().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.write_org_file(DOC_FILE, &doc_org())
            .await
            .expect("seed the key-trace document");
        env.start_app(true).await.expect("start_app");
        for (id, _) in BLOCKS {
            env.wait_for_block(id, DEADLINE)
                .await
                .unwrap_or_else(|hung| panic!("{id} must ingest on boot: {hung}"));
        }
        let reactive = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();
        let target = format!("block:{}", BLOCKS[0].0);
        collector.reset();
        scenario(reactive, target, collector).await;
    });
}

const DEADLINE: Duration = Duration::from_secs(20);

/// The finished spans once one span satisfies `what`.
async fn until(
    collector: &SpanCollector,
    label: &str,
    what: impl Fn(&SpanData) -> bool,
) -> Vec<SpanData> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let spans = collector.finished_spans();
        if spans.iter().any(&what) {
            return spans;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: no such span. Spans: {:?}",
            spans
                .iter()
                .map(|s| (s.name.to_string(), attr(s, "action")))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The one key-event root of `key` on `block`, once it has finished.
async fn key_root(collector: &SpanCollector, key: &str, block: &str) -> SpanData {
    let is_root = |s: &SpanData| {
        s.name == KEY
            && attr(s, "key").as_deref() == Some(key)
            && attr(s, "block").as_deref() == Some(block)
    };
    let spans = until(collector, &format!("`{key}` key event on {block}"), is_root).await;
    let roots: Vec<&SpanData> = spans.iter().filter(|s| is_root(s)).collect();
    let [root] = roots.as_slice() else {
        panic!(
            "one `{key}` key event on {block} must open exactly one `{KEY}` span, got {}",
            roots.len()
        );
    };
    assert_eq!(
        root.parent_span_id,
        SpanId::INVALID,
        "the key event is a root"
    );
    (*root).clone()
}

/// The names up `span`'s finished parent chain, ending in `?` when a parent
/// has not finished (or is not collected).
fn ancestors(spans: &[SpanData], span: &SpanData) -> Vec<String> {
    let mut chain = vec![span.name.to_string()];
    let mut parent = span.parent_span_id;
    while parent != SpanId::INVALID {
        match spans.iter().find(|p| p.span_context.span_id() == parent) {
            Some(p) => {
                chain.push(p.name.to_string());
                parent = p.parent_span_id;
            }
            None => {
                chain.push(format!("?{parent:016x}"));
                break;
            }
        }
    }
    chain
}

/// Wait until a span matching `what` descends from `root` by parenthood and
/// carries its trace id. Waits because a stage's span finishes after the
/// close it encloses (the mirror's apply span ends after its e2e close).
pub(super) async fn descends(
    collector: &SpanCollector,
    root: &SpanData,
    label: &str,
    what: impl Fn(&SpanData) -> bool,
) {
    reaches(collector, root, label, what, false).await
}

/// As [`descends`], but a consolidated pass parented to a newer interaction
/// also counts when it links back to `root` (ruling D3.a); it then carries the
/// newer interaction's trace id.
async fn served(
    collector: &SpanCollector,
    root: &SpanData,
    label: &str,
    what: impl Fn(&SpanData) -> bool,
) {
    reaches(collector, root, label, what, true).await
}

async fn reaches(
    collector: &SpanCollector,
    root: &SpanData,
    label: &str,
    what: impl Fn(&SpanData) -> bool,
    by_links: bool,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let spans = collector.finished_spans();
        let reachable = if by_links {
            reachable_with_links(&spans, root)
        } else {
            descendants_of(&spans, root)
        };
        let found: Vec<&SpanData> = spans.iter().filter(|s| what(s)).collect();
        if found.iter().any(|s| {
            reachable.contains(&s.span_context.span_id())
                && (by_links || trace_id(s) == trace_id(root))
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: none of the {} matching spans is reached from the key event {} (trace {}, \
             links {by_links}); their ancestors: {:?}",
            found.len(),
            root.name,
            trace_id(root),
            found
                .iter()
                .map(|s| ancestors(&spans, s))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub(super) fn is_op(op: &'static str) -> impl Fn(&SpanData) -> bool {
    move |s| s.name == DISPATCH && attr(s, "operation.name").as_deref() == Some(op)
}

/// The e2e close of `action`, fired by a block mirror's delivery, never by the
/// editor's own echo.
fn is_visible(action: &'static str) -> impl Fn(&SpanData) -> bool {
    move |s| {
        s.name == VISIBLE
            && attr(s, "action").as_deref() == Some(action)
            && attr(s, "source").as_deref() == Some("block")
    }
}

pub(super) fn is_block_apply(s: &SpanData) -> bool {
    s.name == APPLY && attr(s, "source").as_deref() == Some("block")
}

fn type_text(engine: &ReactiveEngine, vm: &mut EditorViewModel, text: &str) {
    dispatch_text_change(vm, engine, text)
        .expect("the keystroke applies")
        .expect("a changed text dispatches a write");
}

fn editor(block: &str) -> EditorViewModel {
    EditorViewModel::new(
        Vec::new(),
        Vec::new(),
        HashMap::from([("id".to_string(), Value::String(block.to_string()))]),
        "content".to_string(),
        String::new(),
    )
}

#[test]
fn a_keystroke_without_the_cell_registry_is_one_trace_from_the_key_event() {
    on_gpui_wiring(|engine, block, collector| async move {
        let mut vm = editor(&block);
        type_text(&engine, &mut vm, "typed by the key-event trace");

        let root = key_root(collector, "text", &block).await;
        descends(collector, &root, "set_field dispatch", is_op("set_field")).await;
        descends(collector, &root, "SQL write", |s| {
            s.name == "commit_internal"
        })
        .await;
        descends(
            collector,
            &root,
            "CDC rows in a block mirror",
            is_block_apply,
        )
        .await;
        descends(collector, &root, "e2e close", is_visible("set_field")).await;
    });
}

/// Where Enter splits the `i`-th of the split blocks: the start, a char
/// boundary in the middle, the end of its `content`. A fixed list, not a
/// generator: the three differ in which rows the split changes (the source
/// row stays as it is at either end), and each costs a split of its own block.
fn split_position(i: usize, content: &str) -> usize {
    match i {
        0 => 0,
        1 => content
            .char_indices()
            .map(|(at, _)| at)
            .find(|&at| at >= content.len() / 2)
            .expect("a block of three chars or more has a middle"),
        2 => content.len(),
        _ => unreachable!("three split positions"),
    }
}

#[test]
fn enter_anywhere_in_a_block_is_one_trace_up_to_the_new_row_and_a_later_keystroke_stays_apart() {
    on_gpui_wiring(|engine, _, collector| async move {
        let blocks = editable_blocks();
        let [(typed, _), splits @ ..] = blocks.as_slice() else {
            unreachable!("editable_blocks returns four blocks");
        };
        let services: Arc<dyn BuilderServices> = engine.clone();
        for (i, (block, content)) in splits.iter().enumerate() {
            let at = split_position(i, content);
            let split = structural_block_action(
                EditorKey::Enter,
                block,
                StructuralCaret::on_plain_text(at),
            )
            .expect("Enter is a split");
            dispatch_structural_key(&std::sync::Mutex::new(editor(block)), &services, "", split);
            if at == 0 {
                // Typed into another block while the split is in flight: on the
                // same block the newer clock supersedes the split's.
                let mut vm = editor(typed);
                type_text(&engine, &mut vm, "typed while Enter is in flight");
            }
            let label = format!("split of {block} at {at} of {}", content.len());
            let closed = split_closed(block);
            until(collector, &label, &closed).await;
            let enter = key_root(collector, "split_block", block).await;
            descends(collector, &enter, &label, is_op("split_block")).await;
            if at == 0 {
                // One pass may carry the split and the in-flight keystroke.
                served(collector, &enter, &label, is_block_apply).await;
            } else {
                descends(collector, &enter, &label, is_block_apply).await;
            }
            descends(collector, &enter, &label, &closed).await;
            descends(collector, &enter, &label, |s| s.name == FOCUS).await;
        }

        let spans = until(collector, "set_field close", is_visible("set_field")).await;
        let text = key_root(collector, "text", typed).await;
        for (block, _) in splits {
            let enter = key_root(collector, "split_block", block).await;
            assert_ne!(
                trace_id(&text),
                trace_id(&enter),
                "two key events, two traces"
            );
            let closes = spans.iter().filter(|s| split_closed(block)(s)).count();
            assert_eq!(closes, 1, "the split of {block} closes exactly once");
        }
        descends(
            collector,
            &text,
            "the later keystroke's set_field",
            is_op("set_field"),
        )
        .await;
    });
}

fn split_closed(block: &str) -> impl Fn(&SpanData) -> bool + '_ {
    move |s| is_visible("split_block")(s) && attr(s, "block").as_deref() == Some(block)
}

const DOC_FILE: &str = "key-event-trace.org";

/// The seeded blocks, as (bare id, content): the first takes keystrokes, the
/// other three are split at the start, the middle and the end.
const BLOCKS: [(&str, &str); 4] = [
    ("key-trace-typed", "typed into here"),
    ("key-trace-split-start", "split at the start"),
    ("key-trace-split-middle", "split in the middle"),
    ("key-trace-split-end", "split at the end"),
];

fn doc_org() -> String {
    BLOCKS
        .iter()
        .map(|(id, content)| format!("* {content}\n:PROPERTIES:\n:ID: {id}\n:END:\n"))
        .collect()
}

/// [`BLOCKS`] as (block uri, content).
fn editable_blocks() -> Vec<(String, String)> {
    BLOCKS
        .iter()
        .map(|(id, content)| (format!("block:{id}"), content.to_string()))
        .collect()
}

/// Type `texts` into `block` key by key and return its stored content once it
/// holds the last one.
async fn typed_content(env: &TestEnvironment, block: &str, texts: &[&str]) -> String {
    let engine = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let mut vm = editor(block);
    for text in texts {
        type_text(&engine, &mut vm, text);
    }
    let last = texts.last().expect("at least one keystroke");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snap = env
            .session_arc()
            .block_query()
            .snapshot()
            .await
            .expect("block snapshot");
        let content = snap
            .iter_blocks()
            .find(|b| b.id.as_str() == block)
            .map(|b| b.content.to_string())
            .expect("the typed block exists");
        if content == *last {
            return content;
        }
        assert!(
            Instant::now() < deadline,
            "the typed block holds {content:?}, never the last keystroke's {last:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn boot_and_type(runtime: Arc<tokio::runtime::Runtime>, texts: &'static [&'static str]) -> String {
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        let block = pick_target(&env.session_arc()).await;
        typed_content(&env, &block, texts).await
    })
}

/// Measurement only: the same keystrokes reach the same stored text with no
/// subscriber at all and under the collecting one. Example-based: two boots
/// per sequence, and the spans do not depend on the typed text.
#[test]
fn the_same_keystrokes_give_the_same_text_with_and_without_a_tracing_subscriber() {
    const TEXTS: &[&str] = &["k", "ke", "key", "keys typed", "keys typed twice"];
    let runtime = || {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();
        builder
    };

    assert!(
        !tracing::dispatcher::has_been_set(),
        "the untraced run must run before any subscriber is installed"
    );
    let untraced = boot_and_type(Arc::new(runtime().build().expect("tokio runtime")), TEXTS);

    SpanCollector::global();
    let mut traced_runtime = runtime();
    attach_scope_to_runtime(&mut traced_runtime, begin_test_scope());
    let traced = boot_and_type(
        Arc::new(traced_runtime.build().expect("tokio runtime")),
        TEXTS,
    );

    assert_eq!(untraced, traced);
}
