use std::future::Future;
use std::pin::Pin;

/// A detached future: spawned work the caller cannot join or cancel.
pub type DetachedFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// Background-task authority. Passed as a typed value so a test can hold
/// spawned work in a queue and release it on a schedule it chooses, instead of
/// every spawn reaching the ambient runtime.
pub trait Spawner: Send + Sync {
    fn spawn(&self, future: DetachedFuture);
}

/// Spawns onto a tokio runtime.
#[derive(Debug, Clone)]
pub struct TokioSpawner(tokio::runtime::Handle);

impl TokioSpawner {
    pub fn new(handle: tokio::runtime::Handle) -> Self {
        Self(handle)
    }
}

impl Spawner for TokioSpawner {
    fn spawn(&self, future: DetachedFuture) {
        self.0.spawn(future);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn tokio_spawner_runs_the_future_on_its_handle() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let spawner: Arc<dyn Spawner> =
            Arc::new(TokioSpawner::new(tokio::runtime::Handle::current()));

        spawner.spawn(Box::pin(async move {
            let _ = tx.send(());
        }));

        assert!(
            rx.await.is_ok(),
            "spawned future never ran: the handle the spawner holds is not driving it"
        );
    }
}
