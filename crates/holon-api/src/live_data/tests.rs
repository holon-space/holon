use std::task::Context;
use std::task::Poll;

use futures_signals::signal_map::MapDiff;
use futures_signals::signal_map::SignalMapExt;

use super::*;
use crate::Value;

fn make_row(id: &str, content: &str) -> StorageEntity {
    let mut row: StorageEntity = crate::StorageEntity::new();
    row.insert("id".into(), Value::String(id.to_string()));
    row.insert("content".into(), Value::String(content.to_string()));
    row
}

#[test]
fn test_live_data_initial_load() {
    let rows = vec![make_row("a", "hello"), make_row("b", "world")];
    let live: Arc<LiveData<String>> = LiveData::new(
        rows,
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
    );

    let items = live.read();
    assert_eq!(items.len(), 2);
    assert_eq!(**items.get("a").unwrap(), "hello");
    assert_eq!(**items.get("b").unwrap(), "world");
}

#[test]
fn test_apply_changes() {
    let live: Arc<LiveData<String>> = LiveData::new(
        vec![],
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
    );

    let created = Change::Created {
        data: make_row("c", "new"),
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    };
    live.apply_changes(vec![created]);

    let items = live.read();
    assert_eq!(**items.get("c").unwrap(), "new");
}

/// BugFunnel row 24 (apply-level): a single un-parseable CDC row must NOT
/// abort the batch — the good row in the same batch still lands, and the
/// row dropped is disclosed via an ERROR log. Before the fix
/// `apply_changes` `.expect()`-panicked on the bad row, which (on the
/// detached `subscribe` actor) silently killed the whole live feed.
#[test]
fn apply_changes_drops_unparseable_row_keeps_feed_alive() {
    let live: Arc<LiveData<String>> = LiveData::new(
        vec![],
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| {
            let content = row.get("content").unwrap().as_string().unwrap();
            if content == "POISON" {
                anyhow::bail!("poison row rejected by parse_fn");
            }
            Ok(content.to_string())
        },
    );

    let local = || crate::ChangeOrigin::Local {
        operation_id: None,
        trace_id: None,
    };
    live.apply_changes(vec![
        Change::Created {
            data: make_row("bad", "POISON"),
            origin: local(),
        },
        Change::Created {
            data: make_row("good", "fine"),
            origin: local(),
        },
    ]);

    let items = live.read();
    assert!(
        items.get("bad").is_none(),
        "the un-parseable row must be dropped, not inserted"
    );
    assert_eq!(
        **items.get("good").unwrap(),
        "fine",
        "the good row in the same batch must still land (feed stays alive)"
    );
}

/// A refused (un-parseable) update leaves the row's earlier version in the
/// mirror, so a profile edit refused at load keeps its older version
/// applied.
#[test]
fn an_unparseable_update_keeps_the_earlier_version() {
    let live: Arc<LiveData<String>> = LiveData::new(
        vec![make_row("k", "old")],
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| {
            let content = row.get("content").unwrap().as_string().unwrap();
            if content == "POISON" {
                anyhow::bail!("poison row rejected by parse_fn");
            }
            Ok(content.to_string())
        },
    );
    live.apply_changes(vec![Change::Updated {
        id: "k".to_string(),
        data: make_row("k", "POISON"),
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }]);
    assert_eq!(**live.read().get("k").unwrap(), "old");
}

/// BugFunnel row 24 (actor-level): the `subscribe` background actor must
/// survive a poison row and keep draining the stream. Before the fix the
/// `apply_changes` panic unwound the actor task (swallowed by tokio), the
/// CDC stream was never drained again, and every downstream mirror froze
/// silently — "feed DIES silently mid-session, app looks healthy".
#[tokio::test]
async fn subscribe_actor_survives_unparseable_row() {
    let live: Arc<LiveData<String>> = LiveData::new(
        vec![],
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| {
            let content = row.get("content").unwrap().as_string().unwrap();
            if content == "POISON" {
                anyhow::bail!("poison row rejected by parse_fn");
            }
            Ok(content.to_string())
        },
    );

    let local = || crate::ChangeOrigin::Local {
        operation_id: None,
        trace_id: None,
    };
    let batch = |seq: u64, change: Change<StorageEntity>| crate::streaming::WithMetadata {
        inner: crate::streaming::Batch {
            items: vec![change],
        },
        metadata: crate::streaming::BatchMetadata {
            seq,
            ..Default::default()
        },
    };

    // Poison batch FIRST (would have killed the pre-fix actor), then a good
    // batch that must still be delivered.
    let stream = tokio_stream::iter(vec![
        batch(
            1,
            Change::Created {
                data: make_row("bad", "POISON"),
                origin: local(),
            },
        ),
        batch(
            2,
            Change::Created {
                data: make_row("good", "fine"),
                origin: local(),
            },
        ),
    ]);

    live.subscribe("test_feed", stream);

    let landed = live
        .wait_until(
            |m| m.contains_key("good"),
            std::time::Duration::from_secs(5),
        )
        .await;
    assert!(
        landed,
        "the actor must survive the poison row and deliver the subsequent good row"
    );
    assert!(
        live.read().get("bad").is_none(),
        "the poison row must have been dropped"
    );
}

fn current_span_name() -> Option<&'static str> {
    tracing::Span::current().metadata().map(|m| m.name())
}

fn string_live_data() -> Arc<LiveData<String>> {
    LiveData::new(
        vec![],
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
    )
}

/// A span a task keeps entered while it is parked stays current on the
/// worker thread. Once the task resumes on another worker, that thread's
/// span stack holds the id after the span closes, and tracing-subscriber's
/// registry panics ("tried to clone a span ... that already closed") in
/// whichever task next opens a span there.
#[tokio::test]
async fn a_parked_subscribe_actor_leaves_no_span_current_on_its_thread() {
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::registry());
    let live = string_live_data();

    live.subscribe(
        "test_feed",
        tokio_stream::pending::<crate::streaming::BatchWithMetadata<Change<StorageEntity>>>(),
    );
    tokio::task::yield_now().await;

    assert_eq!(
        current_span_name(),
        None,
        "the actor is parked on its stream, yet its span is current for the next task \
             on this thread"
    );
}

#[tokio::test]
async fn a_parked_quiescence_wait_leaves_no_span_current_on_its_thread() {
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::registry());
    let live = string_live_data();

    let waiter = tokio::spawn({
        let live = Arc::clone(&live);
        async move {
            live.wait_for_quiescent(
                std::time::Duration::from_secs(60),
                std::time::Duration::from_secs(60),
            )
            .await
        }
    });
    tokio::task::yield_now().await;

    let leaked = current_span_name();
    waiter.abort();
    assert_eq!(
        leaked, None,
        "the quiescence wait is parked, yet its span is current for the next task on \
             this thread"
    );
}

/// Holds the closing of `live_data.wait_for_quiescent` open until the test
/// has opened a span on the thread the wait first ran on: the registry
/// frees a closed span only after every layer's `on_close` returns.
struct PauseOnQuiescenceClose {
    closing: std::sync::mpsc::SyncSender<()>,
    resume: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl<S> tracing_subscriber::Layer<S> for PauseOnQuiescenceClose
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
        let name = ctx
            .span(&id)
            .expect("closing span is still registered")
            .name();
        if name == "live_data.wait_for_quiescent" {
            self.closing.send(()).unwrap();
            self.resume.lock().unwrap().recv().unwrap();
        }
    }
}

/// The production shape of the leak: the wait parks on one worker thread,
/// resumes and finishes on another, and a task on the first thread opens a
/// span while the finished wait's span closes.
#[test]
fn a_quiescence_wait_that_migrates_threads_does_not_panic_the_next_span_on_the_old_thread() {
    use tracing_subscriber::layer::SubscriberExt;

    let (closing_tx, closing_rx) = std::sync::mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
    let dispatch =
        tracing::Dispatch::new(tracing_subscriber::registry().with(PauseOnQuiescenceClose {
            closing: closing_tx,
            resume: std::sync::Mutex::new(resume_rx),
        }));
    let _subscriber = tracing::dispatcher::set_default(&dispatch);
    let rt = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap(),
    );
    let live = string_live_data();

    let mut wait = Box::pin(async move {
        live.wait_for_quiescent(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(60),
        )
        .await
    });
    {
        let _rt = rt.enter();
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(wait.as_mut(), &mut cx).is_pending());
    }

    let finisher = std::thread::spawn({
        let rt = Arc::clone(&rt);
        let dispatch = dispatch.clone();
        move || tracing::dispatcher::with_default(&dispatch, || rt.block_on(wait))
    });
    closing_rx.recv().unwrap();
    let next_span = std::panic::catch_unwind(|| {
        drop(tracing::info_span!("next_task_on_the_old_thread"));
    });
    resume_tx.send(()).unwrap();
    finisher.join().unwrap();

    if let Err(payload) = next_span {
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_else(|| format!("{payload:?}"));
        panic!("opening a span on the thread the wait left panicked: {message}");
    }
}

fn deleted(id: &str) -> Change<StorageEntity> {
    Change::Deleted {
        id: id.to_string(),
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }
}

fn string_mirror(rows: Vec<StorageEntity>) -> Arc<LiveData<String>> {
    LiveData::new(
        rows,
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
    )
}

#[test]
fn the_delete_hook_has_run_by_the_time_the_removal_is_observable() {
    use std::sync::atomic::AtomicBool;
    for _ in 0..50 {
        let live = string_mirror(vec![make_row("x", "data")]);
        let hook_ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&hook_ran);
        live.on_delete(move |key| {
            assert_eq!(key, "x");
            flag.store(true, Ordering::SeqCst);
        });
        let observer = {
            let (live, hook_ran) = (Arc::clone(&live), Arc::clone(&hook_ran));
            std::thread::spawn(move || {
                while live.read().contains_key("x") {
                    std::hint::spin_loop();
                }
                hook_ran.load(Ordering::SeqCst)
            })
        };
        live.apply_changes(vec![deleted("x")]);
        assert!(
            observer.join().expect("observer thread"),
            "the removal was observable before the delete hook ran"
        );
    }
}

#[test]
fn the_delete_hook_runs_for_a_key_the_mirror_never_held() {
    let live = string_mirror(vec![]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    live.on_delete(move |key| sink.lock().unwrap().push(key.to_string()));
    live.apply_changes(vec![deleted("refused")]);
    assert_eq!(*seen.lock().unwrap(), ["refused"]);
}

#[tokio::test]
#[should_panic(expected = "already subscribed")]
async fn a_delete_hook_cannot_be_installed_after_subscribe() {
    let live = string_mirror(vec![]);
    live.subscribe(
        "test_feed",
        tokio_stream::iter(Vec::<
            crate::streaming::BatchWithMetadata<Change<StorageEntity>>,
        >::new()),
    );
    live.on_delete(|_| {});
}

#[tokio::test]
async fn a_panicking_delete_hook_leaves_the_subscribe_actor_alive() {
    let live = string_mirror(vec![make_row("x", "data")]);
    live.on_delete(|_| panic!("hook failure"));
    let batch = |seq: u64, change: Change<StorageEntity>| crate::streaming::WithMetadata {
        inner: crate::streaming::Batch {
            items: vec![change],
        },
        metadata: crate::streaming::BatchMetadata {
            seq,
            ..Default::default()
        },
    };
    live.subscribe(
        "test_feed",
        tokio_stream::iter(vec![
            batch(1, deleted("x")),
            batch(
                2,
                Change::Created {
                    data: make_row("after", "fine"),
                    origin: crate::ChangeOrigin::Local {
                        operation_id: None,
                        trace_id: None,
                    },
                },
            ),
        ]),
    );
    let landed = live
        .wait_until(
            |m| m.contains_key("after") && !m.contains_key("x"),
            std::time::Duration::from_secs(5),
        )
        .await;
    assert!(
        landed,
        "the delete must apply and the actor must survive the hook's panic"
    );
}

#[test]
fn test_apply_delete() {
    let live: Arc<LiveData<String>> = LiveData::new(
        vec![make_row("x", "data")],
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
    );

    assert_eq!(live.read().len(), 1);

    let deleted = Change::Deleted {
        id: "x".to_string(),
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    };
    live.apply_changes(vec![deleted]);

    assert_eq!(live.read().len(), 0);
}

/// Reproduces the focus_roots stale-Vec bug: matview rows with no `id`
/// column are deleted via `Change::Deleted { id: rowid, .. }`, but the
/// LiveData stores items keyed by id_fn output (e.g. composite
/// "region|root_id"). Without rowid → user_key tracking, the delete
/// silently misses and the stale entry lingers.
#[test]
fn test_apply_delete_composite_key_via_rowid() {
    // id_fn: composite key from (region, root_id) — mirrors `live_focus_roots`.
    let live: Arc<LiveData<String>> = LiveData::new(
        vec![],
        |row| {
            let region = row.get("region").unwrap().as_string().unwrap();
            let root_id = row.get("root_id").unwrap().as_string().unwrap();
            Ok(format!("{region}\u{1F}{root_id}"))
        },
        |row| Ok(row.get("root_id").unwrap().as_string().unwrap().to_string()),
    );

    // Simulate a CDC Created event the matview emits for a new focus_roots
    // row. process_cdc_event injects `_rowid` into the data — this is what
    // our fix uses to remember the rowid → user-key mapping.
    let mut row: StorageEntity = crate::StorageEntity::new();
    row.insert("region".into(), Value::String("right_sidebar".to_string()));
    row.insert("root_id".into(), Value::String("block:abc".to_string()));
    row.insert("_rowid".into(), Value::String("42".to_string()));

    live.apply_changes(vec![Change::Created {
        data: row,
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }]);

    assert_eq!(live.read().len(), 1, "Created should have inserted 1 row");

    // CDC Delete for the same matview row. `process_cdc_event` falls back
    // to `change.id.to_string()` (rowid) because data has no `id` column.
    live.apply_changes(vec![Change::Deleted {
        id: "42".to_string(),
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }]);

    assert_eq!(
        live.read().len(),
        0,
        "rowid-keyed delete must remove the composite-key entry"
    );
}

#[test]
fn test_signal_map_notifies() {
    let live: Arc<LiveData<String>> = LiveData::new(
        vec![make_row("pre", "seeded")],
        |row| Ok(row.get("id").unwrap().as_string().unwrap().to_string()),
        |row| Ok(row.get("content").unwrap().as_string().unwrap().to_string()),
    );

    let mut signal = live.signal_map();
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    // First poll: initial MapDiff::Replace with pre-seeded entries
    match signal.poll_map_change_unpin(&mut cx) {
        Poll::Ready(Some(MapDiff::Replace { entries })) => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].0, "pre");
            assert_eq!(*entries[0].1, "seeded");
        }
        other => panic!("Expected Ready(Some(Replace)), got {:?}", other),
    }

    // Apply a CDC change
    live.apply_changes(vec![Change::Created {
        data: make_row("a", "new-item"),
        origin: crate::ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        },
    }]);

    // Second poll: MapDiff::Insert for the new entry
    match signal.poll_map_change_unpin(&mut cx) {
        Poll::Ready(Some(MapDiff::Insert { key, value })) => {
            assert_eq!(key, "a");
            assert_eq!(*value, "new-item");
        }
        other => panic!("Expected Ready(Some(Insert)), got {:?}", other),
    }
}
