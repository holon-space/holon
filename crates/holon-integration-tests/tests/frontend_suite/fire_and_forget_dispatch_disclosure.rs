//! **Every fire-and-forget dispatch is visible while in flight and loud when
//! it fails.**
//!
//! The keystone drains the dispatch journal before it reads the store, so an
//! un-journaled spawn races the oracle, and a chain failure that skips the
//! op-failure surface reads as success to the user.
//!
//! @pbt kind harness
//! @pbt covers dispatch-journal — rule spawns and intent chains

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityName;
use holon_api::Value;
use holon_frontend::dispatch_journal::DispatchOutcome;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::reactive::dispatch_intent_chain;
use holon_integration_tests::TestEnvironment;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    )
}

async fn running_engine(
    rt: Arc<tokio::runtime::Runtime>,
) -> (TestEnvironment, Arc<ReactiveEngine>) {
    let env = TestEnvironment::new_running(rt)
        .await
        .expect("start a running Turso environment");
    let engine = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    (env, engine)
}

#[test]
fn a_dangling_link_follow_is_journaled_before_it_runs() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let (_env, engine) = running_engine(rt.clone()).await;
        let journal = engine.ui_state().dispatch_journal();
        let mark = journal.mark();

        let services: &dyn BuilderServices = engine.as_ref();
        services.follow_dangling_link("Journal Probe Page".to_string(), "main".to_string());

        let recorded = journal
            .since(mark)
            .expect("mark is inside the retained window");
        assert!(
            recorded
                .iter()
                .any(|d| d.entity_name == "block" && d.op_name == "create_page_from_link"),
            "follow_dangling_link returned with nothing in the dispatch journal: a drain that \
             reads the journal now sees no work in flight while the create runs. Journal: \
             {recorded:?}"
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let entry = journal
                .since(mark)
                .unwrap()
                .into_iter()
                .find(|d| d.op_name == "create_page_from_link")
                .unwrap();
            match entry.outcome {
                DispatchOutcome::Succeeded => break,
                DispatchOutcome::Failed(e) => panic!("create_page_from_link failed: {e}"),
                DispatchOutcome::Pending => {
                    assert!(
                        Instant::now() < deadline,
                        "create_page_from_link never settled"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    });
}

#[test]
fn a_failed_chain_intent_reaches_the_op_failure_surface() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let (_env, engine) = running_engine(rt.clone()).await;
        let messages: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink_messages = messages.clone();
        engine
            .ui_state()
            .set_op_failure_sink(Arc::new(move |m| sink_messages.lock().unwrap().push(m)));
        let errors_before = engine.session().error_tracker().errors();
        let journal = engine.ui_state().dispatch_journal();

        let services: Arc<dyn BuilderServices> = engine.clone();
        dispatch_intent_chain(
            &services,
            vec![OperationIntent::new(
                EntityName::new("block"),
                "join_block".to_string(),
                [(
                    "id".to_string(),
                    Value::String("block:no-such-block-for-join".to_string()),
                )]
                .into_iter()
                .collect(),
            )],
        );

        let deadline = Instant::now() + Duration::from_secs(10);
        while journal.open_chains() > 0 {
            assert!(Instant::now() < deadline, "the chain never finished");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(
            messages.lock().unwrap().len(),
            1,
            "the op-failure sink did not receive the failed chain intent"
        );
        assert_eq!(
            engine.session().error_tracker().errors(),
            errors_before + 1,
            "the error tracker did not count the failed chain intent"
        );
    });
}
