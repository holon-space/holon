//! A settle waiter on `UiWatermark::changed` wakes to every change of what the
//! engine reports it: the emitted seq, the routed seq, the operations in
//! flight and the deliveries the subscriptions owe. Drawn writes on two keyed
//! places (each batch for one place is a skip for the other), writes held in
//! their claim, and a live eager re-query must leave the waiter's last wake
//! reading the engine as it is, once quiet.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::api::backend_engine::BackendEngine;
use holon::api::backend_engine::WatchViewKind;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::QueryEngine;
use holon_api::Value;
use holon_api::delivery_ledger::Owed;
use holon_api::wake_witness::WakeWitness;
use holon_frontend::render_epoch::EngineSeq;
use holon_frontend::render_epoch::UiWatermark;
use proptest::prelude::*;
use tokio_stream::StreamExt as _;

use crate::watch_context_membership_leak::block_engine;
use crate::watch_context_membership_leak::leaf_sql;

const PLACES: [&str; 2] = ["wake-a", "wake-b"];
/// A loud hang guard: a waiter that is owed a wake gets it within
/// milliseconds, so passing this means no wake is coming.
const QUIET_GUARD: Duration = Duration::from_secs(10);

/// An anti-join the IVM engine cannot maintain, so an eager re-query serves it
/// and routes through its own router.
const EAGER_SQL: &str = "SELECT b.id FROM block b WHERE NOT EXISTS \
    (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id AND bt.tag = 'human-only')";

#[derive(Clone, Debug)]
enum Step {
    /// A user writes the content of place `p`.
    Edit(usize),
    /// A user's write of place `p` parks inside its claim, unless one is
    /// parked already.
    Hold(usize),
    /// The parked write, if any, resumes.
    Release,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (0..PLACES.len()).prop_map(Step::Edit),
        (0..PLACES.len()).prop_map(Step::Hold),
        Just(Step::Release),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Reported {
    emitted: EngineSeq,
    routed: EngineSeq,
    in_flight: usize,
    owed: Vec<Owed>,
}

impl Reported {
    fn read(watermark: &UiWatermark) -> Self {
        Self {
            emitted: watermark
                .emitted()
                .expect("the engine reports its emitted seq"),
            routed: watermark
                .routed()
                .expect("the engine reports its routed seq"),
            in_flight: watermark.operations_in_flight(),
            owed: watermark.owed_deliveries(),
        }
    }
}

/// Waits on wakes until the waiter's last wake read the engine as it is now.
fn quiet(witness: &WakeWitness<Reported>, watermark: &UiWatermark, after: &str) {
    let guard = Instant::now() + QUIET_GUARD;
    loop {
        let wakes = witness.wakes();
        let now = Reported::read(watermark);
        let seen = witness.seen();
        if seen == now {
            return;
        }
        assert!(
            witness.wait_past(wakes, guard.saturating_duration_since(Instant::now())),
            "after {after}: the engine reports {now:?}, but the waiter's last wake read {seen:?} \
             and no change woke it since"
        );
    }
}

fn write(
    rt: &tokio::runtime::Runtime,
    engine: &Arc<BackendEngine>,
    place: usize,
    value: String,
) -> tokio::task::JoinHandle<()> {
    let engine = engine.clone();
    rt.spawn(async move {
        let params: holon_api::StorageEntity = HashMap::from([
            (
                "id".into(),
                Value::String(format!("block:{}", PLACES[place])),
            ),
            ("field".into(), Value::String("content".into())),
            ("value".into(), Value::String(value)),
        ]);
        engine
            .execute_operation(
                &EntityName::new("block"),
                "set_field",
                params,
                OpOrigin::User,
            )
            .await
            .expect("a drawn write lands");
    })
}

fn wakes_on_every_change(steps: &[Step]) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime");
    let engine = rt.block_on(block_engine()).expect("the block engine boots");
    let ctx = holon::testing::e2e_test_helpers::E2ETestContext::from_engine(engine.clone());
    rt.block_on(async {
        for place in PLACES {
            let params: holon_api::StorageEntity = HashMap::from([
                ("id".into(), Value::String(format!("block:{place}"))),
                ("content".into(), Value::String(place.into())),
                (
                    "parent_id".into(),
                    Value::String("sentinel:no_parent".into()),
                ),
            ]);
            ctx.execute_op("block", "create", params)
                .await
                .expect("a place is created");
        }
    });
    let mut streams = Vec::new();
    for place in PLACES {
        let key = format!("leaf:block:{place}");
        streams.push(
            rt.block_on(engine.query_and_watch_keyed(
                leaf_sql(),
                &key,
                &format!("block:{place}"),
                WatchViewKind::Leaf,
            ))
            .expect("a keyed watch opens"),
        );
    }
    streams.push(
        rt.block_on(engine.query_and_watch(EAGER_SQL.into(), HashMap::new(), None))
            .expect("the eager watch opens"),
    );
    let drains: Vec<_> = streams
        .into_iter()
        .map(|mut stream| rt.spawn(async move { while stream.next().await.is_some() {} }))
        .collect();

    let watermark = UiWatermark::new(Some(engine.clone() as Arc<dyn QueryEngine>));
    let witness = {
        let arm = watermark.clone();
        let read = watermark.clone();
        WakeWitness::new(
            move || Box::pin(arm.changed()),
            move || Reported::read(&read),
        )
    };
    quiet(&witness, &watermark, "the watches opened");

    let mut writes = Vec::new();
    let mut held = false;
    for (i, step) in steps.iter().enumerate() {
        match step {
            Step::Edit(p) => writes.push(write(&rt, &engine, *p, format!("edit {i}"))),
            Step::Hold(p) if !held => {
                engine.dispatch_hold().hold_next("block", "set_field");
                writes.push(write(&rt, &engine, *p, format!("held {i}")));
                held = true;
            }
            Step::Hold(_) => {}
            Step::Release if held => {
                rt.block_on(engine.dispatch_hold().release(1, QUIET_GUARD))
                    .expect("the held write parked");
                held = false;
            }
            Step::Release => {}
        }
        quiet(&witness, &watermark, &format!("step {i} {step:?}"));
    }
    if held {
        rt.block_on(engine.dispatch_hold().release(1, QUIET_GUARD))
            .expect("the held write parked");
    }
    for write in writes {
        rt.block_on(write).expect("a write task");
    }
    quiet(&witness, &watermark, "every write landed");
    drains.iter().for_each(|drain| drain.abort());
}

/// The found cases run first, then the drawn ones.
#[test]
fn every_engine_change_wakes_a_settle_waiter() {
    for steps in [
        vec![Step::Edit(0)],
        vec![Step::Hold(0), Step::Release],
        vec![Step::Hold(0), Step::Edit(0), Step::Edit(1), Step::Release],
        // The eager router and the demux race for each batch; many writes
        // draw the order where the eager router moves the routed seq last.
        (0..16).map(|i| Step::Edit(i % 2)).collect(),
    ] {
        wakes_on_every_change(&steps);
    }
    proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 8,
        failure_persistence: None,
        max_shrink_time: 60_000,
        ..ProptestConfig::default()
    })
    .run(&proptest::collection::vec(step(), 1..12), |steps| {
        wakes_on_every_change(&steps);
        Ok(())
    })
    .unwrap();
}
