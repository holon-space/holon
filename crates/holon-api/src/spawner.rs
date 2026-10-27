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
mod tests;
