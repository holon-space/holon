//! Generated sequences of editor key events — keystrokes, Enter at any char
//! boundary and focus moves — on the production stack, in both editor
//! wirings: without the cell registry (GPUI: a keystroke is a `set_field`
//! dispatch) and with it (TUI: a keystroke is a CRDT cell edit). Every
//! interaction is one trace rooted at its key event or cell edit, with each
//! stage of its path a descendant in that trace, and it ends in exactly one
//! e2e close on a block mirror, as one superseded entry of a later close on
//! its block, or as one retire of a pass that changed nothing visible.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::editor_view_model::EditorKey;
use holon_frontend::editor_view_model::EditorViewModel;
use holon_frontend::editor_view_model::StructuralCaret;
use holon_frontend::editor_view_model::dispatch_structural_key;
use holon_frontend::editor_view_model::dispatch_text_change;
use holon_frontend::editor_view_model::structural_block_action;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_integration_tests::pbt::generators::CaretAt;
use holon_integration_tests::pbt::generators::EditorKeyStep;
use holon_integration_tests::pbt::generators::editor_key_steps;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;
use opentelemetry::trace::SpanId;
use opentelemetry_sdk::trace::SpanData;
use proptest::test_runner::Config as ProptestConfig;
use proptest::test_runner::TestRunner;

use super::interaction_trace_connectivity::descendants_of;
use super::key_event_trace::attr;
use super::key_event_trace::descends;
use super::key_event_trace::is_block_apply;
use super::key_event_trace::is_op;
use super::key_event_trace::trace_id;

const KEY: &str = "interaction.key";
/// The root of a split the TUI dispatches without a key span.
const DISPATCH: &str = "interaction.dispatch";
const CELL_EDIT: &str = "interaction.cell_edit";
const CELL_APPLY: &str = "cell.apply";
const PASS: &str = "loro.projection_pass";
const VISIBLE: &str = "interaction.visible";
const FOCUS: &str = "interaction.focus_new_block";
const RETIRED: &str = "interaction.retired";
const DEADLINE: Duration = Duration::from_secs(20);

/// Seed contents with multi-byte chars, so a char boundary is not a byte step.
const CONTENTS: [&str; 3] = ["typé here", "中 split 😀 me", "end ß"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wiring {
    /// No cell registry, as GPUI boots.
    SetField,
    /// The cell registry on, as the TUI boots.
    Cell,
}

/// One issued interaction, as its root span names it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Issued {
    root: Root,
    block: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Root {
    Text,
    CellEdit,
    Split,
}

impl Root {
    fn of(span: &SpanData) -> Option<Self> {
        if span.parent_span_id != SpanId::INVALID {
            return None;
        }
        match (span.name.as_ref(), attr(span, "key").as_deref()) {
            (CELL_EDIT, _) => Some(Self::CellEdit),
            (KEY, Some("text")) => Some(Self::Text),
            (KEY, Some("split_block")) => Some(Self::Split),
            (DISPATCH, _) if attr(span, "operation.name").as_deref() == Some("split_block") => {
                Some(Self::Split)
            }
            _ => None,
        }
    }
}

fn issued_of(span: &SpanData) -> Option<Issued> {
    Some(Issued {
        root: Root::of(span)?,
        block: attr(span, "block").expect("an interaction root names its block"),
    })
}

/// Each case seeds its own block ids: an interaction a failed case leaves
/// pending must not meet the next case's deliveries in the process-wide
/// correlator.
static CASE: AtomicUsize = AtomicUsize::new(0);

/// Roots delivered only under a pass that links them, and roots retired.
static LINKED: AtomicUsize = AtomicUsize::new(0);
static RETIRED_ROOTS: AtomicUsize = AtomicUsize::new(0);

fn run_case(wiring: Wiring, steps: Vec<EditorKeyStep>) {
    let collector = SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = Arc::new(builder.build().expect("tokio runtime"));
    let case = CASE.fetch_add(1, Ordering::Relaxed);
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        let ids: Vec<String> = (0..CONTENTS.len())
            .map(|i| format!("seq-trace-{wiring:?}-{case}-{i}").to_lowercase())
            .collect();
        let org: String = ids
            .iter()
            .zip(CONTENTS)
            .map(|(id, content)| format!("* {content}\n:PROPERTIES:\n:ID: {id}\n:END:\n"))
            .collect();
        env.write_org_file(&format!("seq-trace-{case}.org"), &org)
            .await
            .expect("seed the sequence document");
        if wiring == Wiring::Cell {
            env.enable_block_cell_registry();
        }
        env.start_app(true).await.expect("start_app");
        for id in &ids {
            env.wait_for_block(id, DEADLINE)
                .await
                .unwrap_or_else(|hung| panic!("{id} must ingest on boot: {hung}"));
        }
        let engine = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();
        let blocks: Vec<String> = ids.iter().map(|id| format!("block:{id}")).collect();
        collector.reset();
        let issued = drive(&env, &engine, wiring, &blocks, steps, collector).await;
        check(collector, wiring, &issued).await;
    });
}

/// Run `steps` and return what they issued. Keystrokes do not wait for each
/// other. An Enter moves the focus where production does, to the block its
/// focus stage names, once that stage ran; its own clock may still be
/// pending. A block an Enter split is waited for before its next event.
async fn drive(
    env: &TestEnvironment,
    engine: &Arc<ReactiveEngine>,
    wiring: Wiring,
    blocks: &[String],
    steps: Vec<EditorKeyStep>,
    collector: &SpanCollector,
) -> Vec<Issued> {
    let services: Arc<dyn BuilderServices> = engine.clone();
    let mut editors: HashMap<String, EditorViewModel> = HashMap::new();
    let mut splitting: HashSet<String> = HashSet::new();
    let mut issued = Vec::new();
    let mut focus = blocks[0].clone();
    for step in steps {
        if let EditorKeyStep::Focus { block } = &step {
            focus = blocks[block % blocks.len()].clone();
            continue;
        }
        if splitting.remove(&focus) {
            settled(collector, &issued).await;
            editors.remove(&focus);
        }
        if !editors.contains_key(&focus) {
            let vm = open_editor(env, engine, wiring, &focus).await;
            editors.insert(focus.clone(), vm);
        }
        let vm = editors.get_mut(&focus).expect("opened above");
        match step {
            EditorKeyStep::Type { at, ch } => {
                let mut text = vm.buffer().to_string();
                text.insert(at.byte_in(&text), ch);
                match wiring {
                    Wiring::SetField => {
                        dispatch_text_change(vm, engine.as_ref(), &text)
                            .expect("the keystroke applies")
                            .expect("a changed text dispatches a write");
                        issued.push(Issued {
                            root: Root::Text,
                            block: focus.clone(),
                        });
                    }
                    Wiring::Cell => {
                        vm.apply_local_edit(&text).expect("the keystroke applies");
                        drop(vm.take_cell_edit_span());
                        issued.push(Issued {
                            root: Root::CellEdit,
                            block: focus.clone(),
                        });
                    }
                }
            }
            EditorKeyStep::Enter { at } => {
                let text = vm.buffer().to_string();
                let caret = StructuralCaret::on_plain_text(at.byte_in(&text));
                let split = structural_block_action(EditorKey::Enter, &focus, caret)
                    .expect("Enter is a split");
                let vm = Mutex::new(editors.remove(&focus).expect("opened above"));
                splitting.insert(focus.clone());
                match wiring {
                    Wiring::SetField => {
                        let splits = issued.iter().filter(|i| i.root == Root::Split).count() + 1;
                        dispatch_structural_key(&vm, &services, &text, split);
                        issued.push(Issued {
                            root: Root::Split,
                            block: focus.clone(),
                        });
                        focus = focus_after_split(collector, &services, splits).await;
                    }
                    Wiring::Cell => {
                        engine
                            .dispatch_intent_sync(split)
                            .await
                            .expect("the split applies");
                        issued.push(Issued {
                            root: Root::Split,
                            block: focus.clone(),
                        });
                    }
                }
            }
            EditorKeyStep::Focus { .. } => unreachable!("handled above"),
        }
    }
    settled(collector, &issued).await;
    issued
}

/// The focused block once the focus stage of the `splits`-th Enter ran.
async fn focus_after_split(
    collector: &SpanCollector,
    services: &Arc<dyn BuilderServices>,
    splits: usize,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    while collector
        .finished_spans()
        .iter()
        .filter(|s| s.name == FOCUS)
        .count()
        < splits
    {
        assert!(
            Instant::now() < deadline,
            "Enter #{splits} never moved the focus"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    services
        .focused_block()
        .expect("an Enter leaves a block focused")
        .to_string()
}

/// An editor on `block` holding its current text, wired as `wiring` boots,
/// once the block is visible to the editor (a split's new row appears after
/// the focus moved to it).
async fn open_editor(
    env: &TestEnvironment,
    engine: &ReactiveEngine,
    wiring: Wiring,
    block: &str,
) -> EditorViewModel {
    let editor = |text: String| {
        EditorViewModel::new(
            Vec::new(),
            Vec::new(),
            HashMap::from([("id".to_string(), Value::String(block.to_string()))]),
            "content".to_string(),
            text,
        )
    };
    let uri = EntityUri::parse(block).expect("a block id is a URI");
    let deadline = Instant::now() + DEADLINE;
    loop {
        let snap = env
            .session_arc()
            .block_query()
            .snapshot()
            .await
            .expect("block snapshot");
        let stored = snap
            .iter_blocks()
            .find(|b| b.id.as_str() == block)
            .map(|b| b.content.to_string());
        match (wiring, stored) {
            (Wiring::SetField, Some(content)) => return editor(content),
            (Wiring::Cell, Some(_)) => {
                let cell = engine
                    .editable_text(&uri, "content")
                    .expect("a stored content block has an editable text cell");
                let mut vm = editor(cell.current());
                vm.attach_cell(cell);
                return vm;
            }
            (_, None) => {}
        }
        assert!(
            Instant::now() < deadline,
            "{block} never became visible to an editor"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The finished interaction roots, once every issued one has finished. A root
/// finishes when its clock is gone: closed, superseded or failed.
async fn settled(collector: &SpanCollector, issued: &[Issued]) -> Vec<SpanData> {
    let mut want: HashMap<&Issued, usize> = HashMap::new();
    for i in issued {
        *want.entry(i).or_default() += 1;
    }
    let deadline = Instant::now() + DEADLINE;
    loop {
        let spans = collector.finished_spans();
        let mut have: HashMap<Issued, usize> = HashMap::new();
        for root in spans.iter().filter_map(issued_of) {
            *have.entry(root).or_default() += 1;
        }
        if want.iter().all(|(i, n)| have.get(*i) == Some(n)) {
            return spans;
        }
        assert!(
            Instant::now() < deadline,
            "issued interactions {want:?} never all finished; finished roots {have:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn check(collector: &SpanCollector, wiring: Wiring, issued: &[Issued]) {
    let spans = settled(collector, issued).await;
    let roots: Vec<&SpanData> = spans.iter().filter(|s| issued_of(s).is_some()).collect();
    assert_eq!(
        roots.len(),
        issued.len(),
        "one root per interaction, no stray root: issued {issued:?}"
    );
    let mut traces = HashSet::new();
    for root in &roots {
        assert!(
            traces.insert(trace_id(root)),
            "two interactions share trace {}",
            trace_id(root)
        );
    }

    let mut unclosed: HashMap<String, usize> = HashMap::new();
    for root in &roots {
        let Issued { root: kind, block } = issued_of(root).expect("filtered above");
        let reachable = descendants_of(&spans, root);
        let closes: Vec<&SpanData> = spans
            .iter()
            .filter(|s| s.name == VISIBLE && reachable.contains(&s.span_context.span_id()))
            .collect();
        let retires: Vec<&SpanData> = spans
            .iter()
            .filter(|s| s.name == RETIRED && reachable.contains(&s.span_context.span_id()))
            .collect();
        assert!(
            closes.len() + retires.len() <= 1,
            "{kind:?} on {block} closed {} and retired {} times",
            closes.len(),
            retires.len()
        );
        for retire in &retires {
            assert_eq!(attr(retire, "block").as_deref(), Some(block.as_str()));
            assert!(
                matches!(
                    attr(retire, "reason").as_deref(),
                    Some("no_visible_change" | "unwritten")
                ),
                "{kind:?} on {block} retired for {:?}",
                attr(retire, "reason")
            );
        }
        for close in &closes {
            assert_eq!(
                trace_id(close),
                trace_id(root),
                "a close stays in its interaction's trace"
            );
            assert_eq!(
                attr(close, "source").as_deref(),
                Some("block"),
                "{kind:?} on {block} closed on a delivery that is not a block mirror's"
            );
            assert_eq!(attr(close, "block").as_deref(), Some(block.as_str()));
        }
        let label = format!("{kind:?} on {block} ({wiring:?})");
        match kind {
            Root::Text => {
                descends(collector, root, &label, is_op("set_field")).await;
                descends(collector, root, &label, |s| s.name == "commit_internal").await;
            }
            Root::CellEdit => {
                descends(collector, root, &label, |s| s.name == CELL_APPLY).await;
            }
            Root::Split => {
                descends(collector, root, &label, is_op("split_block")).await;
                descends(collector, root, &label, |s| s.name == FOCUS).await;
            }
        }
        let only_linked = if !retires.is_empty() {
            RETIRED_ROOTS.fetch_add(1, Ordering::Relaxed);
            false
        } else if closes.is_empty() {
            *unclosed.entry(block).or_default() += 1;
            false
        } else if kind == Root::CellEdit {
            delivered(collector, root, &label, |s| s.name == PASS).await
        } else {
            delivered(collector, root, &label, is_block_apply).await
        };
        if only_linked {
            LINKED.fetch_add(1, Ordering::Relaxed);
        }
    }

    let mut superseded: HashMap<String, usize> = HashMap::new();
    for close in spans.iter().filter(|s| s.name == VISIBLE) {
        let n: usize = attr(close, "superseded")
            .expect("a close counts what it superseded")
            .parse()
            .expect("a count");
        if n > 0 {
            *superseded
                .entry(attr(close, "block").expect("a close names its block"))
                .or_default() += n;
        }
    }
    assert_eq!(
        unclosed, superseded,
        "every interaction without its own close must be one superseded entry of a close on its block"
    );
}

/// A span matching `what` in `root`'s trace, or under a projection pass that
/// links `root`'s trace: a pass that reads several commits is the child of
/// the newest one and links the rest. True when only a linking pass has one.
async fn delivered(
    collector: &SpanCollector,
    root: &SpanData,
    label: &str,
    what: impl Fn(&SpanData) -> bool,
) -> bool {
    let trace = root.span_context.trace_id();
    let deadline = Instant::now() + DEADLINE;
    loop {
        let spans = collector.finished_spans();
        let own = descendants_of(&spans, root);
        let linked: HashSet<SpanId> = spans
            .iter()
            .filter(|s| {
                s.name == PASS && s.links.iter().any(|l| l.span_context.trace_id() == trace)
            })
            .flat_map(|pass| descendants_of(&spans, pass))
            .collect();
        let matching: Vec<SpanId> = spans
            .iter()
            .filter(|s| what(s))
            .map(|s| s.span_context.span_id())
            .collect();
        let in_own = spans.iter().any(|s| {
            what(s) && own.contains(&s.span_context.span_id()) && s.span_context.trace_id() == trace
        });
        if in_own {
            return false;
        }
        if matching.iter().any(|id| linked.contains(id)) {
            return true;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: no matching span descends from the root or from a pass that links trace {trace}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Cases every run starts with: Enter at either end, a keystroke burst on
/// one block, a keystroke on another block while an Enter is in flight, a
/// keystroke into the row an Enter focused, Enter right after a burst on its
/// block, a keystroke that adds only edge whitespace at either end, a
/// keystroke on another block right after an Enter, and Enter right after an
/// edge-whitespace keystroke.
fn fixed_cases() -> Vec<Vec<EditorKeyStep>> {
    use EditorKeyStep::*;
    vec![
        vec![Enter { at: CaretAt::Start }],
        vec![Enter { at: CaretAt::End }],
        vec![
            Type {
                at: CaretAt::End,
                ch: 'a',
            },
            Type {
                at: CaretAt::End,
                ch: '€',
            },
            Type {
                at: CaretAt::Start,
                ch: '😀',
            },
        ],
        vec![
            Enter { at: CaretAt::Start },
            Focus { block: 1 },
            Type {
                at: CaretAt::End,
                ch: 'x',
            },
        ],
        vec![
            Enter { at: CaretAt::End },
            Type {
                at: CaretAt::Start,
                ch: 'y',
            },
        ],
        vec![
            Type {
                at: CaretAt::End,
                ch: 'a',
            },
            Type {
                at: CaretAt::End,
                ch: 'b',
            },
            Type {
                at: CaretAt::End,
                ch: 'c',
            },
            Enter { at: CaretAt::End },
        ],
        vec![
            Focus { block: 5 },
            Type {
                at: CaretAt::End,
                ch: ' ',
            },
        ],
        vec![
            Type {
                at: CaretAt::Start,
                ch: ' ',
            },
            Type {
                at: CaretAt::Start,
                ch: ' ',
            },
        ],
        vec![
            Enter { at: CaretAt::End },
            Focus { block: 1 },
            Type {
                at: CaretAt::End,
                ch: 'x',
            },
        ],
        vec![
            Type {
                at: CaretAt::End,
                ch: 'é',
            },
            Type {
                at: CaretAt::End,
                ch: 'i',
            },
            Type {
                at: CaretAt::End,
                ch: ' ',
            },
            Enter { at: CaretAt::End },
            Type {
                at: CaretAt::End,
                ch: 'h',
            },
            Type {
                at: CaretAt::Start,
                ch: '€',
            },
            Focus { block: 0 },
            Type {
                at: CaretAt::End,
                ch: '🎉',
            },
        ],
    ]
}

const GENERATED_CASES: u32 = 12;

fn property(wiring: Wiring) {
    let fixed = fixed_cases();
    for steps in fixed.clone() {
        run_case(wiring, steps);
    }
    let mut runner = TestRunner::new(ProptestConfig {
        cases: GENERATED_CASES,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    runner
        .run(&editor_key_steps(10), |steps| {
            run_case(wiring, steps);
            Ok(())
        })
        .unwrap_or_else(|failure| panic!("{wiring:?}: {failure}"));
    println!(
        "{wiring:?}: {} fixed and {GENERATED_CASES} generated cases passed; {} roots retired, {} \
         delivered only under a linking pass",
        fixed.len(),
        RETIRED_ROOTS.load(Ordering::Relaxed),
        LINKED.load(Ordering::Relaxed)
    );
}

#[test]
fn every_key_event_without_the_cell_registry_is_one_trace_that_closes_once() {
    property(Wiring::SetField);
}

#[test]
fn every_cell_keystroke_and_enter_is_one_trace_that_closes_once() {
    property(Wiring::Cell);
}
