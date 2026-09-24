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
/// (fix the data → banner clears within one cycle). Latency findings are
/// *sticky* (the slow moment is gone by the time you look): they accumulate
/// until dismissed, capped per severity so a warning never evicts a violation.
pub struct OracleStatus {
    structural: RwLock<Vec<Finding>>,
    latency_violations: RwLock<Vec<Finding>>,
    latency_warnings: RwLock<Vec<Finding>>,
    generation_tx: tokio::sync::watch::Sender<u64>,
}

const LATENCY_CAP: usize = 5;

impl OracleStatus {
    fn new() -> Self {
        let (generation_tx, _) = tokio::sync::watch::channel(0);
        Self {
            structural: RwLock::new(Vec::new()),
            latency_violations: RwLock::new(Vec::new()),
            latency_warnings: RwLock::new(Vec::new()),
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
    /// [`LATENCY_CAP`] of its severity).
    pub fn push_latency(&self, finding: Finding) {
        {
            let mut guard = match finding.severity {
                Severity::Violation => self.latency_violations.write().unwrap(),
                Severity::Warning => self.latency_warnings.write().unwrap(),
            };
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
        self.latency_violations.write().unwrap().clear();
        self.latency_warnings.write().unwrap().clear();
        self.bump();
    }

    /// Snapshot every current finding, violations before warnings.
    pub fn snapshot(&self) -> Vec<Finding> {
        let mut all = self.structural.read().unwrap().clone();
        all.extend(self.latency_violations.read().unwrap().iter().cloned());
        all.extend(self.latency_warnings.read().unwrap().iter().cloned());
        all
    }

    /// Subscribe to change notifications (each change bumps a generation).
    pub fn watch(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latency(severity: Severity, message: &str) -> Finding {
        let make = match severity {
            Severity::Violation => Finding::violation,
            Severity::Warning => Finding::warning,
        };
        make("latency-slo", message.to_string(), SystemTime::now())
    }

    #[test]
    fn warnings_never_evict_or_bury_a_violation() {
        let status = OracleStatus::new();
        status.push_latency(latency(Severity::Violation, "service time"));
        for i in 0..2 * LATENCY_CAP {
            status.push_latency(latency(Severity::Warning, &format!("drain estimate {i}")));
        }

        let snapshot = status.snapshot();
        let messages: Vec<&str> = snapshot.iter().map(|f| f.message.as_str()).collect();
        assert_eq!(
            snapshot.first().map(|f| (f.severity, f.message.as_str())),
            Some((Severity::Violation, "service time")),
            "the violation must survive the warnings and lead the banner: {messages:?}"
        );
        assert_eq!(
            snapshot
                .iter()
                .filter(|f| f.severity == Severity::Warning)
                .count(),
            LATENCY_CAP,
            "{messages:?}"
        );
    }
}
