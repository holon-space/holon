//! Process-global oracle finding ledger.
//!
//! Global (not DI-scoped) because one of its producers is a
//! `tracing_subscriber` Layer — and the tracing subscriber is process-global.
//! Only the live runner / latency layer write here; the PBT harness uses the
//! pure [`crate::checks`] functions and never touches this.

use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::SystemTime;

/// How loud a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// An invariant or SLO the app broke. Logged at ERROR.
    Violation,
    /// A disclosure that judges nothing. Logged at WARN.
    Warning,
}

/// A single oracle finding, ready to render.
#[derive(Clone, Debug)]
pub struct Finding {
    /// Which oracle fired, e.g. `inv-no-orphan-blocks` or `latency-slo`.
    pub oracle: &'static str,
    pub message: String,
    pub at: SystemTime,
    pub severity: Severity,
}

impl Finding {
    pub fn violation(oracle: &'static str, message: String, at: SystemTime) -> Self {
        Self {
            oracle,
            message,
            at,
            severity: Severity::Violation,
        }
    }

    pub fn warning(oracle: &'static str, message: String, at: SystemTime) -> Self {
        Self {
            oracle,
            message,
            at,
            severity: Severity::Warning,
        }
    }
}

/// Current findings + a `watch` channel for UI bridges.
///
/// Structural violations are *live*: replaced wholesale by each runner cycle
/// (fix the data → banner clears within one cycle). Latency violations are
/// *sticky* (the slow moment is gone by the time you look): they accumulate
/// (capped) until dismissed.
pub struct OracleStatus {
    structural: RwLock<Vec<Finding>>,
    latency: RwLock<Vec<Finding>>,
    generation_tx: tokio::sync::watch::Sender<u64>,
}

const LATENCY_CAP: usize = 5;

impl OracleStatus {
    fn new() -> Self {
        let (generation_tx, _) = tokio::sync::watch::channel(0);
        Self {
            structural: RwLock::new(Vec::new()),
            latency: RwLock::new(Vec::new()),
            generation_tx,
        }
    }

    /// The process-global instance.
    pub fn global() -> &'static OracleStatus {
        static GLOBAL: OnceLock<OracleStatus> = OnceLock::new();
        GLOBAL.get_or_init(OracleStatus::new)
    }

    fn bump(&self) {
        self.generation_tx.send_modify(|g| *g += 1);
    }

    /// Replace the structural set with this cycle's findings.
    /// Notifies watchers only when the set actually changed shape.
    pub fn set_structural(&self, violations: Vec<Finding>) {
        let changed = {
            let mut guard = self.structural.write().unwrap();
            let changed = guard.len() != violations.len()
                || guard
                    .iter()
                    .zip(&violations)
                    .any(|(a, b)| a.message != b.message);
            *guard = violations;
            changed
        };
        if changed {
            self.bump();
        }
    }

    /// Append a sticky latency finding (capped at the most recent
    /// [`LATENCY_CAP`]).
    pub fn push_latency(&self, finding: Finding) {
        {
            let mut guard = self.latency.write().unwrap();
            guard.push(finding);
            let len = guard.len();
            if len > LATENCY_CAP {
                guard.drain(..len - LATENCY_CAP);
            }
        }
        self.bump();
    }

    /// Dismiss sticky latency findings (banner button).
    pub fn dismiss_latency(&self) {
        self.latency.write().unwrap().clear();
        self.bump();
    }

    /// Snapshot every current finding (structural first).
    pub fn snapshot(&self) -> Vec<Finding> {
        let mut all = self.structural.read().unwrap().clone();
        all.extend(self.latency.read().unwrap().iter().cloned());
        all
    }

    /// Subscribe to change notifications (each change bumps a generation).
    pub fn watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation_tx.subscribe()
    }
}
