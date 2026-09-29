//! Test-only hold hook at the top of
//! [`DispatchingOperationEngine`](crate::api::operation_engine::DispatchingOperationEngine)'s
//! `execute_operation`: a test parks the next run of a named operation until it
//! releases it, or makes that run fail, so an interleave that is otherwise a
//! scheduler race is forced deterministically.
//!
//! A second hook delays admission itself, so a dispatch that admits on a
//! spawned task instead of at its call is reordered on purpose.
//!
//! A third parks a judged write after its shape judgement, holding every claim
//! the judgement took, and a fourth refuses a judged write's shape claim as if
//! it closed a wait cycle.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use holon_api::EntityName;
use tokio::sync::oneshot;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    Park,
    Fail,
    ParkJudged,
    RefuseNest,
}

#[derive(Debug)]
struct Rule {
    entity: String,
    op: String,
    effect: Effect,
}

#[derive(Default)]
struct HoldState {
    rules: Vec<Rule>,
    parked: Vec<(String, oneshot::Sender<()>)>,
    failed: Vec<String>,
    admission_delays: Vec<(String, VecDeque<Duration>)>,
}

pub struct DispatchHold {
    state: Mutex<HoldState>,
    parked_count: watch::Sender<usize>,
    delayed_admissions: AtomicUsize,
}

impl Default for DispatchHold {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            parked_count: watch::Sender::new(0),
            delayed_admissions: AtomicUsize::new(0),
        }
    }
}

impl DispatchHold {
    /// Park the next run of `entity.op` at the hook until [`Self::release`].
    pub fn hold_next(&self, entity: &str, op: &str) {
        self.add_rule(entity, op, Effect::Park);
    }

    /// Make the next run of `entity.op` fail at the hook without reaching a
    /// provider.
    pub fn fail_next(&self, entity: &str, op: &str) {
        self.add_rule(entity, op, Effect::Fail);
    }

    /// Park the next judged run of `entity.op` after its shape judgement, until
    /// [`Self::release`].
    pub fn hold_next_judged(&self, entity: &str, op: &str) {
        self.add_rule(entity, op, Effect::ParkJudged);
    }

    /// Refuse the shape claim of each of the next `times` judged runs of
    /// `entity.op` that nest one, as a wait cycle would.
    pub fn refuse_next_nests(&self, entity: &str, op: &str, times: usize) {
        for _ in 0..times {
            self.add_rule(entity, op, Effect::RefuseNest);
        }
    }

    /// Runs of `entity.op` parked at the hook right now.
    pub fn parked_runs(&self, entity: &str, op: &str) -> usize {
        let name = format!("{entity}.{op}");
        let state = self.state.lock().unwrap();
        state.parked.iter().filter(|(n, _)| *n == name).count()
    }

    /// Runs of `entity.op` the hook failed since the last call.
    pub fn take_failed(&self, entity: &str, op: &str) -> usize {
        let name = format!("{entity}.{op}");
        let mut state = self.state.lock().unwrap();
        let before = state.failed.len();
        state.failed.retain(|n| *n != name);
        before - state.failed.len()
    }

    /// Runs parked at the hook right now, across every `entity.op`.
    pub fn parked(&self) -> usize {
        *self.parked_count.borrow()
    }

    /// The `entity.op` of every run parked at the hook right now.
    pub fn parked_names(&self) -> Vec<String> {
        let state = self.state.lock().unwrap();
        state.parked.iter().map(|(n, _)| n.clone()).collect()
    }

    /// Block each of the next `delays.len()` admissions of `entity.op` for its
    /// delay, in the order they reach admission. Admission at the call blocks
    /// the caller, so the order holds; admission on spawned tasks blocks
    /// separate workers, so descending delays reverse it.
    pub fn delay_next_admissions(&self, entity: &str, op: &str, delays: Vec<Duration>) {
        assert!(
            !delays.is_empty(),
            "no admission delay given for {entity}.{op}"
        );
        self.state
            .lock()
            .unwrap()
            .admission_delays
            .push((format!("{entity}.{op}"), delays.into()));
    }

    /// Admissions sleeping on their delay right now.
    pub fn delayed_admissions(&self) -> usize {
        self.delayed_admissions.load(Ordering::SeqCst)
    }

    fn add_rule(&self, entity: &str, op: &str, effect: Effect) {
        self.state.lock().unwrap().rules.push(Rule {
            entity: entity.to_string(),
            op: op.to_string(),
            effect,
        });
    }

    /// Wait until exactly `expect_parked` runs are parked, then resume them all
    /// and drop every hold no run has matched. Waiting is what keeps a parked
    /// run that was spawned (not awaited) by the caller deterministic.
    pub async fn release(&self, expect_parked: usize, within: Duration) -> Result<()> {
        let mut parked = self.parked_count.subscribe();
        // The `watch::Ref` that `wait_for` yields holds the channel's read lock,
        // so it must be dropped before `send_replace` below takes the write lock.
        let reached = matches!(
            tokio::time::timeout(within, parked.wait_for(|n| *n >= expect_parked)).await,
            Ok(Ok(_))
        );
        let mut state = self.state.lock().unwrap();
        let names: Vec<&str> = state.parked.iter().map(|(n, _)| n.as_str()).collect();
        anyhow::ensure!(
            reached && state.parked.len() == expect_parked,
            "dispatch hold: expected {expect_parked} parked run(s) within {within:?}, found {} \
             ({names:?}); unmatched holds: {:?}",
            state.parked.len(),
            state.rules
        );
        anyhow::ensure!(
            state.admission_delays.is_empty(),
            "dispatch hold: admission delays never reached: {:?}",
            state.admission_delays
        );
        state
            .rules
            .retain(|r| matches!(r.effect, Effect::Fail | Effect::RefuseNest));
        for (_, resume) in state.parked.drain(..) {
            // A parked run whose task was dropped has nothing left to resume.
            let _ = resume.send(());
        }
        self.parked_count.send_replace(0);
        Ok(())
    }

    /// The hook itself: a no-op unless a rule names this run.
    pub(crate) async fn checkpoint(&self, entity: &EntityName, op: &str) -> Result<()> {
        self.park_on(entity, op, &[Effect::Park, Effect::Fail])
            .await
    }

    /// The judged-run hook: a no-op unless a judged-run rule names this run.
    pub(crate) async fn judged_checkpoint(&self, entity: &EntityName, op: &str) -> Result<()> {
        self.park_on(entity, op, &[Effect::ParkJudged]).await
    }

    /// Whether a rule refuses this judged run's shape claim; consumes it.
    pub(crate) fn refuses_nest(&self, entity: &EntityName, op: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let Some(at) = state.rules.iter().position(|r| {
            r.effect == Effect::RefuseNest && r.entity == entity.as_str() && r.op == op
        }) else {
            return false;
        };
        state.rules.remove(at);
        tracing::info!("dispatch hold refuses the shape claim of {entity}.{op}");
        true
    }

    async fn park_on(&self, entity: &EntityName, op: &str, effects: &[Effect]) -> Result<()> {
        let resumed = {
            let mut state = self.state.lock().unwrap();
            let Some(at) = state.rules.iter().position(|r| {
                effects.contains(&r.effect) && r.entity == entity.as_str() && r.op == op
            }) else {
                return Ok(());
            };
            let rule = state.rules.remove(at);
            tracing::info!(effect = ?rule.effect, "dispatch hold engaged on {entity}.{op}");
            if rule.effect == Effect::Fail {
                state.failed.push(format!("{entity}.{op}"));
                anyhow::bail!("injected dispatch failure (dispatch hold) for {entity}.{op}");
            }
            let (resume, resumed) = oneshot::channel();
            state.parked.push((format!("{entity}.{op}"), resume));
            self.parked_count.send_replace(state.parked.len());
            resumed
        };
        resumed.await.map_err(|_| {
            anyhow::anyhow!("dispatch hold dropped while {entity}.{op} was parked, never released")
        })
    }

    /// The admission hook: a no-op unless a delay names this admission.
    pub(crate) fn admission_checkpoint(&self, entity: &EntityName, op: &str) {
        let name = format!("{entity}.{op}");
        let delay = {
            let mut state = self.state.lock().unwrap();
            let Some(at) = state.admission_delays.iter().position(|(n, _)| *n == name) else {
                return;
            };
            let delay = state.admission_delays[at]
                .1
                .pop_front()
                .expect("an admission delay rule is removed when it runs empty");
            if state.admission_delays[at].1.is_empty() {
                state.admission_delays.remove(at);
            }
            delay
        };
        tracing::info!("admission of {name} delayed by {delay:?}");
        self.delayed_admissions.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(delay);
        self.delayed_admissions.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_failed_run_is_reported_once() {
        let hold = DispatchHold::default();
        hold.fail_next("block", "create");
        let entity = EntityName::new("block");
        assert!(hold.checkpoint(&entity, "create").await.is_err());
        assert_eq!(hold.take_failed("block", "create"), 1);
        assert_eq!(hold.take_failed("block", "create"), 0);
    }
}
