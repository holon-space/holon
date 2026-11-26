//! A waiter that samples a state each time its change future wakes it. A test
//! that changes the state and then finds the last sample stale has found a
//! change that woke no waiter.

use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::Weak;
use std::task::Context;
use std::task::Poll;
use std::task::Wake;
use std::task::Waker;
use std::time::Duration;

use futures::future::BoxFuture;

type Arm = Box<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;
type Sample<S> = Box<dyn Fn() -> S + Send + Sync>;

pub struct WakeWitness<S> {
    inner: Arc<Inner<S>>,
}

struct Inner<S> {
    arm: Arm,
    sample: Sample<S>,
    waiting: Mutex<Waiting<S>>,
    woke: Condvar,
}

struct Waiting<S> {
    change: BoxFuture<'static, ()>,
    seen: S,
    wakes: u64,
}

struct WitnessWaker<S>(Weak<Inner<S>>);

impl<S: Clone + Send + 'static> Wake for WitnessWaker<S> {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(inner) = self.0.upgrade() {
            inner.observe(1);
        }
    }
}

impl<S: Clone + Send + 'static> Inner<S> {
    /// Re-arms before it samples, so a change after the sample wakes again.
    /// The sample runs inside the waking call: whoever wakes must hold none of
    /// the locks `sample` takes.
    fn observe(self: &Arc<Self>, woke: u64) {
        let mut waiting = self.waiting.lock().unwrap();
        let waker = Waker::from(Arc::new(WitnessWaker(Arc::downgrade(self))));
        while let Poll::Ready(()) = waiting
            .change
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
        {
            waiting.change = (self.arm)();
        }
        waiting.seen = (self.sample)();
        waiting.wakes += woke;
        self.woke.notify_all();
    }
}

impl<S: Clone + Send + 'static> WakeWitness<S> {
    /// `arm` makes the future that resolves on the next change.
    pub fn new(
        arm: impl Fn() -> BoxFuture<'static, ()> + Send + Sync + 'static,
        sample: impl Fn() -> S + Send + Sync + 'static,
    ) -> Self {
        let change = arm();
        let seen = sample();
        let inner = Arc::new(Inner {
            arm: Box::new(arm),
            sample: Box::new(sample),
            waiting: Mutex::new(Waiting {
                change,
                seen,
                wakes: 0,
            }),
            woke: Condvar::new(),
        });
        inner.observe(0);
        Self { inner }
    }

    /// The state at the last wake.
    pub fn seen(&self) -> S {
        self.inner.waiting.lock().unwrap().seen.clone()
    }

    pub fn wakes(&self) -> u64 {
        self.inner.waiting.lock().unwrap().wakes
    }

    /// Blocks until a wake after the first `wakes`, or `guard` passes; true
    /// when it woke.
    pub fn wait_past(&self, wakes: u64, guard: Duration) -> bool {
        let waiting = self.inner.waiting.lock().unwrap();
        let (waiting, _) = self
            .inner
            .woke
            .wait_timeout_while(waiting, guard, |w| w.wakes <= wakes)
            .unwrap();
        waiting.wakes > wakes
    }
}
