use std::task::Poll;

use super::*;

#[tokio::test]
async fn a_panicking_stream_reaches_the_give_up_seam() {
    let shutdown = SessionShutdown::new();
    let (gave_up_tx, gave_up_rx) = std::sync::mpsc::channel::<String>();
    let gave_up_tx = std::sync::Mutex::new(gave_up_tx);

    let mut consumer = spawn_supervised::<(), _, _, _>(
        "panicking-feed",
        || {
            futures::stream::poll_fn(|_| -> Poll<Option<anyhow::Result<()>>> {
                panic!("the feed panicked while polled")
            })
        },
        move |component, _, err| {
            gave_up_tx
                .lock()
                .unwrap()
                .send(format!("{component}: {err:#}"))
                .unwrap();
        },
        &shutdown,
    );

    assert_eq!(consumer.recv().await, Some(Supervised::Reset));
    assert_eq!(
        consumer.recv().await,
        None,
        "the supervisor must be gone after its stream panicked"
    );
    let disclosed = gave_up_rx.try_recv();
    assert!(
        disclosed
            .as_ref()
            .is_ok_and(|m| m.contains("the feed panicked while polled")),
        "the supervisor died by panic without calling the give-up seam: {disclosed:?}"
    );
}
