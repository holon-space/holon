use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use super::*;

#[tokio::test]
async fn shutdown_stops_a_task_parked_on_its_cancellation() {
    let shutdown = SessionShutdown::new();
    let stopped = Arc::new(AtomicBool::new(false));

    let flag = stopped.clone();
    let cancelled = shutdown.cancelled();
    shutdown.spawn("parked-forever", async move {
        cancelled.await;
        flag.store(true, Ordering::SeqCst);
    });

    shutdown
        .shutdown(DEFAULT_SHUTDOWN_TIMEOUT)
        .await
        .expect("a task that observes cancellation must stop within the bound");
    assert!(stopped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_task_that_ignores_cancellation_is_reported_by_name() {
    let shutdown = SessionShutdown::new();
    shutdown.spawn("deaf-loop", async {
        std::future::pending::<()>().await;
    });

    let err = shutdown
        .shutdown(Duration::from_millis(50))
        .await
        .expect_err("a task that never observes cancellation must NOT be detached silently");
    assert_eq!(err.still_running, vec!["deaf-loop".to_string()]);
}

#[tokio::test]
async fn a_panicking_task_reports_its_panic_before_it_dies() {
    let shutdown = SessionShutdown::new();
    let (reported_tx, reported_rx) = tokio::sync::oneshot::channel::<String>();

    shutdown.spawn_disclosing_panic(
        "doomed",
        async { panic!("the task panicked mid-flight") },
        move |message| reported_tx.send(message.to_string()).unwrap(),
    );

    let reported = tokio::time::timeout(DEFAULT_SHUTDOWN_TIMEOUT, reported_rx)
        .await
        .expect("the panic report must arrive")
        .expect("the task died without calling its panic report");
    assert_eq!(reported, "the task panicked mid-flight");
}
