use std::sync::Arc;

use super::*;

#[tokio::test]
async fn tokio_spawner_runs_the_future_on_its_handle() {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawner: Arc<dyn Spawner> = Arc::new(TokioSpawner::new(tokio::runtime::Handle::current()));

    spawner.spawn(Box::pin(async move {
        let _ = tx.send(());
    }));

    assert!(
        rx.await.is_ok(),
        "spawned future never ran: the handle the spawner holds is not driving it"
    );
}
