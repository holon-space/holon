//! One cell-editor keystroke through the production stack: its clock exists
//! when the commit wakes the projector, and it closes exactly once, under the
//! keystroke's own interaction span, through the pass that projected it.
//!
//! Example-based: one keystroke on one seeded block. The race it pins is an
//! ordering between two threads, which no generated input reaches; the
//! coalescing and claim rules have their PBT in `holon-api` latency_e2e.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use holon_api::Value;
use holon_frontend::editor_view_model::EditorViewModel;
use holon_frontend::reactive::BuilderServices;
use holon_integration_tests::test_environment::TestEnvironment;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;
use holon_loro::loro_document_store::DocScope;
use opentelemetry_sdk::trace::SpanData;

const INTERACTION: &str = "interaction.cell_edit";
const PASS: &str = "loro.projection_pass";
const VISIBLE: &str = "interaction.visible";

#[test]
fn a_keystroke_is_claimable_when_its_commit_wakes_the_projector_and_closes_once() {
    let collector = SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    let runtime = Arc::new(builder.build().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        // The cell leg is this test's subject (the TUI runs it; GPUI does not).
        env.enable_block_cell_registry();
        env.start_app(true).await.expect("start_app");
        let reactive = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();
        let target_id =
            super::interaction_trace_connectivity::pick_target(&env.session_arc()).await;
        let target = holon_api::EntityUri::parse(&target_id).expect("seeded block id is a URI");

        let cell = reactive
            .editable_text(&target, "content")
            .expect("a seeded content block has an editable text cell");
        let original = cell.current();
        let mut vm = EditorViewModel::new(
            Vec::new(),
            Vec::new(),
            HashMap::from([("id".to_string(), Value::String(target_id.clone()))]),
            "content".to_string(),
            original.clone(),
        );
        vm.attach_cell(cell);

        // The projector's wake is a `subscribe_root` callback on the committing
        // thread; this one runs at the same moment and asks what a pass
        // claiming right then would find.
        let store = env
            .injector()
            .expect("booted environment has an injector")
            .try_resolve::<holon_loro::LoroDocumentStore>()
            .expect("Loro is on, so a document store is wired");
        let keystroke_thread = std::thread::current().id();
        type Claimed = Vec<(holon_api::latency_e2e::CellOp, String)>;
        let at_wake: Arc<Mutex<Vec<Claimed>>> = Arc::default();
        // The probe runs under the keystroke's write guard, so no real pass can
        // read, and so claim, while the probe holds the clock.
        let mut docs = Vec::new();
        let mut subs = Vec::new();
        for scope in [DocScope::Global, DocScope::Layout] {
            let doc = store.get_doc(scope).await.unwrap().doc();
            subs.push(doc.subscribe_root(Arc::new({
                let at_wake = at_wake.clone();
                move |_| {
                    if std::thread::current().id() != keystroke_thread {
                        return;
                    }
                    let ops = Mutex::new(Vec::new());
                    let claim = holon_api::latency_e2e::projection_covers(|op| {
                        ops.lock().unwrap().push(op);
                        true
                    });
                    let names = claim
                        .spans()
                        .iter()
                        .map(|s| s.metadata().map_or("<disabled>", |m| m.name()).to_string());
                    let claimed = ops.into_inner().unwrap().into_iter().zip(names).collect();
                    at_wake.lock().unwrap().push(claimed);
                }
            })));
            docs.push(doc);
        }

        let before: Vec<loro::VersionVector> = docs.iter().map(|d| d.oplog_vv()).collect();
        collector.reset();
        vm.apply_local_edit(&format!("{original}k"))
            .expect("the keystroke applies");
        drop(vm.take_cell_edit_span());
        drop(vm);
        drop(subs);

        let at_wake = at_wake.lock().unwrap().clone();
        let [claimed] = at_wake.as_slice() else {
            panic!("one keystroke, one commit, one wake: {at_wake:?}");
        };
        let [(op, name)] = claimed.as_slice() else {
            panic!("the keystroke's commit must wake the projector with its one clock already pending: {claimed:?}");
        };
        assert_eq!(name, INTERACTION);
        // The announced op is the first op of the keystroke's commit.
        let at = docs
            .iter()
            .position(|d| Arc::as_ptr(d) as u64 == op.doc)
            .expect("the announced doc key names one of the two documents");
        let after = docs[at].oplog_vv();
        assert!(
            before[at].get(&op.peer).copied().unwrap_or(0) == op.counter
                && after.includes_id(loro::ID::new(op.peer, op.counter)),
            "the announced op {op:?} must be the commit's first op: before {:?}, after {after:?}, peer now {}",
            before[at],
            docs[at].peer_id(),
        );

        let deadline = Instant::now() + Duration::from_secs(20);
        let spans = loop {
            let spans = collector.finished_spans();
            if spans.iter().any(|s| s.name == INTERACTION) {
                break spans;
            }
            assert!(
                Instant::now() < deadline,
                "the keystroke's interaction span never closed: no e2e close. Spans: {:?}",
                spans.iter().map(|s| &s.name).collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        let named = |name: &'static str| spans.iter().filter(move |s| s.name == name);
        let interaction: Vec<&SpanData> = named(INTERACTION).collect();
        assert_eq!(interaction.len(), 1, "one keystroke, one interaction span");
        let root = interaction[0].span_context.span_id();
        let child_of_root = |s: &&SpanData| s.parent_span_id == root;
        assert_eq!(
            named(VISIBLE).filter(child_of_root).count(),
            1,
            "the keystroke must close exactly once, under its own interaction"
        );
        assert!(
            named(PASS).any(|s| child_of_root(&s)),
            "the pass that projected the keystroke must be a child of its interaction"
        );
    });
}
