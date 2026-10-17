//! A tokio shell over the sans-IO [`holon_net::exec::Executor`]: a
//! monotonic clock, the effect loop, manual fires from other tasks and
//! waiting for an occurrence to settle.
//!
//! The executor sits behind a lock that is never held across an effect, so a
//! fire or a marking read from another task waits only for one step.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;
use std::time::Instant;

use holon_net::exec::Accepted;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::Executor;
use holon_net::exec::FireError;
use holon_net::exec::LoadError;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Settled;
use holon_net::exec::UnknownOccurrence;

#[derive(Clone)]
pub struct Shell {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell").finish_non_exhaustive()
    }
}

struct Inner {
    exec: Mutex<Executor>,
    /// The effects a `write`, `fire` or `complete` quiesced into, oldest
    /// first: the next [`Shell::step`] hands them to the owner.
    pending: Mutex<Vec<EffectRequest>>,
    epoch: Instant,
    /// Woken by a manual fire: its effect runs on the next [`Shell::drive`].
    fired: tokio::sync::Notify,
    /// Bumped whenever an occurrence may have settled.
    progress: tokio::sync::watch::Sender<u64>,
}

impl Shell {
    pub fn new(net: NetDef) -> Result<Self, LoadError> {
        Ok(Self {
            inner: Arc::new(Inner {
                exec: Mutex::new(Executor::new(net)?),
                pending: Mutex::new(Vec::new()),
                epoch: Instant::now(),
                fired: tokio::sync::Notify::new(),
                progress: tokio::sync::watch::Sender::new(0),
            }),
        })
    }

    fn now(&self) -> Duration {
        self.inner.epoch.elapsed()
    }

    fn exec(&self) -> MutexGuard<'_, Executor> {
        self.inner
            .exec
            .lock()
            .expect("the net executor lock is poisoned")
    }

    fn queue(&self, requests: Vec<EffectRequest>) {
        self.inner
            .pending
            .lock()
            .expect("the net pending-effect lock is poisoned")
            .extend(requests);
    }

    /// Reads the executor: the marking, marks, occurrences.
    pub fn read<R>(&self, f: impl FnOnce(&Executor) -> R) -> R {
        f(&self.exec())
    }

    /// An outside commit. The auto transitions it enables run at once; their
    /// effects wait for the next [`Shell::step`] / [`Shell::drive`].
    ///
    /// A net that does not quiesce after a write is a net-definition bug.
    pub fn write(&self, place: &str, subject: &str, content: Content) {
        let now = self.now();
        let quiesced = self
            .exec()
            .write(place, subject, content, now)
            .unwrap_or_else(|e| panic!("the net has no quiescence after a write to {place}: {e}"));
        self.queue(quiesced.requests);
    }

    /// A manual fire, answered at bind. The owner's next [`Shell::drive`]
    /// runs its effect; [`Shell::fired`] wakes the owner for it.
    pub fn fire(
        &self,
        transition: &str,
        subject: &str,
        occurrence: OccurrenceId,
    ) -> Result<Accepted, FireError> {
        let now = self.now();
        let accepted = self
            .exec()
            .fire(transition, subject, occurrence, Content::new(), now)?;
        self.queue(accepted.requests);
        self.inner.progress.send_modify(|n| *n += 1);
        self.inner.fired.notify_one();
        Ok(accepted.value)
    }

    /// Completes when a manual fire left an effect for [`Shell::drive`].
    pub async fn fired(&self) {
        self.inner.fired.notified().await;
    }

    /// One step of the net: the effects it asks for. Each is handed back
    /// with [`Shell::complete`]; an empty step means the net is quiescent.
    /// [`Shell::drive`] is the loop for an owner whose effects are `Send` in
    /// every lifetime.
    pub fn step(&self) -> Result<Vec<EffectRequest>, ExecError> {
        let now = self.now();
        let new = self.exec().step(now)?;
        let mut requests = std::mem::take(
            &mut *self
                .inner
                .pending
                .lock()
                .expect("the net pending-effect lock is poisoned"),
        );
        requests.extend(new);
        if requests.is_empty() {
            self.inner.progress.send_modify(|n| *n += 1);
        }
        Ok(requests)
    }

    /// Hands the result of a [`Shell::step`] effect back.
    pub fn complete(&self, request: &EffectRequest, result: EffectResult) -> Result<(), ExecError> {
        let now = self.now();
        let quiesced = self.exec().complete(request.attempt, result, now)?;
        self.queue(quiesced.requests);
        self.inner.progress.send_modify(|n| *n += 1);
        Ok(())
    }

    /// Steps the net to quiescence, running each effect with `run` and
    /// handing its result back, until no step returns an effect.
    pub async fn drive(
        &self,
        mut run: impl AsyncFnMut(&EffectRequest) -> EffectResult,
    ) -> Result<(), ExecError> {
        loop {
            let requests = self.step()?;
            if requests.is_empty() {
                return Ok(());
            }
            for request in requests {
                let result = run(&request).await;
                self.complete(&request, result)?;
            }
        }
    }

    /// The settled outcome of `occurrence`, once it settles.
    pub async fn settled(&self, occurrence: &OccurrenceId) -> Result<Settled, UnknownOccurrence> {
        let mut progress = self.inner.progress.subscribe();
        loop {
            if let Some(settled) = self.exec().settled(occurrence)? {
                return Ok(settled);
            }
            progress
                .changed()
                .await
                .expect("the shell owns the progress sender");
        }
    }
}

#[cfg(test)]
mod tests {
    use holon_net::exec::Body;
    use holon_net::exec::Class;
    use holon_net::exec::Output;
    use holon_net::exec::Reader;
    use holon_net::exec::Receiver;
    use holon_net::exec::Subject;
    use holon_net::exec::TransitionDef;
    use holon_net::exec::Trigger;

    use super::*;

    fn bind(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
        r.present("In", s).then(Content::new)
    }

    fn net() -> NetDef {
        NetDef::default()
            .place("In", Class::Process)
            .register("Out", Class::Process)
            .transition(
                TransitionDef::new(
                    "Go",
                    Trigger::Manual,
                    Arc::new(bind),
                    Body::Effectful(Receiver::None),
                )
                .consumes("In")
                .fact("Out"),
            )
    }

    #[tokio::test]
    async fn a_manual_fire_settles_after_the_owner_drives_its_effect() {
        let shell = Shell::new(net()).unwrap();
        shell.write("In", "a", Content::new());
        let accepted = shell.fire("Go", "a", OccurrenceId::caller("op-1")).unwrap();
        let waiter = {
            let shell = shell.clone();
            let occ = accepted.0.clone();
            tokio::spawn(async move { shell.settled(&occ).await })
        };
        shell.fired().await;
        let mut ran = Vec::new();
        shell
            .drive(async |r: &EffectRequest| {
                ran.push(r.subject.to_string());
                EffectResult::Done(vec![Output::new("Out", &r.subject, Content::new())])
            })
            .await
            .unwrap();
        assert_eq!(ran, vec!["a".to_string()]);
        assert_eq!(waiter.await.unwrap(), Ok(Settled::Done));
        assert!(shell.read(|e| e.tokens().token("Out", "a").is_some()));
    }
}
