//! Test-only hold hook at the top of
//! [`DispatchingOperationEngine`](crate::api::operation_engine::DispatchingOperationEngine)'s
//! `execute_operation`: a test parks the next run of a named operation until it
//! releases it, or makes that run fail, so an interleave that is otherwise a
//! scheduler race is forced deterministically.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use holon_api::EntityName;
use tokio::sync::oneshot;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    Park,
    Fail,
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
}

pub struct DispatchHold {
    state: Mutex<HoldState>,
    parked_count: watch::Sender<usize>,
}

impl Default for DispatchHold {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            parked_count: watch::Sender::new(0),
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

    /// Runs of `entity.op` that reached the hook and did not land: parked
    /// now, or failed.
    pub fn withheld(&self, entity: &str, op: &str) -> usize {
        let name = format!("{entity}.{op}");
        let state = self.state.lock().unwrap();
        let parked = state.parked.iter().filter(|(n, _)| *n == name).count();
        let failed = state.failed.iter().filter(|n| **n == name).count();
        parked + failed
    }

    /// Runs parked at the hook right now, across every `entity.op`.
    pub fn parked(&self) -> usize {
        *self.parked_count.borrow()
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
        state.rules.retain(|r| r.effect == Effect::Fail);
        for (_, resume) in state.parked.drain(..) {
            // A parked run whose task was dropped has nothing left to resume.
            let _ = resume.send(());
        }
        self.parked_count.send_replace(0);
        Ok(())
    }

    /// The hook itself: a no-op unless a rule names this run.
    pub(crate) async fn checkpoint(&self, entity: &EntityName, op: &str) -> Result<()> {
        let resumed = {
            let mut state = self.state.lock().unwrap();
            let Some(at) = state
                .rules
                .iter()
                .position(|r| r.entity == entity.as_str() && r.op == op)
            else {
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
}
