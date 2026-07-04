//! Reactive-rule (ADR 0024) compilation/runtime status — the action-side twin
//! of advice's [`holon_advice::AdviceRuleStatus`] (ADR 0022 "rule blocks render
//! their own status — fail loud, visibly").
//!
//! A rule block's parse/compile/exec outcome is carried here, keyed by the rule
//! (action) block's id, written by the
//! [`action_watcher`](crate::api::action_watcher) and read by the render path
//! so a broken — or deprecated — rule surfaces its error IN PLACE rather than
//! silently no-op'ing. In particular the retired `action` language records
//! [`RuleStatus::DeprecatedLanguage`] so it can never become silently inert
//! (ADR 0024 WP3 / A5).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

/// The compilation/runtime status of one reactive-rule block.
///
/// Only [`RuleStatus::Active`] renders normally; every other variant is a
/// fail-loud surface the rule card renders as its (red) error state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleStatus {
    /// The rule parsed, compiled, and its trigger watch is live. Nothing wrong.
    Active,
    /// The action DSL did not parse. The message is the typed parse error.
    ParseError(String),
    /// The trigger query did not compile to SQL. The message is the typed
    /// error.
    CompileError(String),
    /// The rule fired but its effect execution failed. The message is the
    /// error.
    ExecError(String),
    /// The rule fired but its effect was a benign no-op: the entity it would
    /// create ALREADY EXISTS under the deterministic id (an interim
    /// identity-collision refusal, plan §5). Distinct from [`ExecError`] so a
    /// periodic autonomous rule (the journal auto-create tick) does not
    /// error-storm — it settles on this status and logs ONCE. Disclosed
    /// degraded mode, not a red error state.
    Skipped(String),
    /// The rule uses the retired `action` language. It does NOT execute; it
    /// must be renamed to `holon_rule`. Surfaced loud, never silently inert
    /// (WP3 / A5).
    DeprecatedLanguage,
}

impl RuleStatus {
    /// True only for [`RuleStatus::Active`] — the normal render case.
    pub fn is_active(&self) -> bool {
        matches!(self, RuleStatus::Active)
    }
}

impl std::fmt::Display for RuleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleStatus::Active => write!(f, "active"),
            RuleStatus::ParseError(msg) => write!(f, "parse error: {msg}"),
            RuleStatus::CompileError(msg) => write!(f, "compile error: {msg}"),
            RuleStatus::ExecError(msg) => write!(f, "execution failed: {msg}"),
            RuleStatus::Skipped(msg) => write!(f, "skipped (already satisfied): {msg}"),
            RuleStatus::DeprecatedLanguage => {
                write!(
                    f,
                    "legacy 'action' language — rename source block to holon_rule"
                )
            }
        }
    }
}

/// Shared, cheaply-clonable handle to the rule-status map (block id → status).
///
/// The action watcher is the sole writer; render/MCP consumers are readers.
/// Reads clone the status out so no lock is held across the caller's `await`.
#[derive(Clone, Default)]
pub struct RuleStatusHandle(Arc<RwLock<HashMap<String, RuleStatus>>>);

impl RuleStatusHandle {
    /// A fresh, empty handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or overwrite) the status for a rule block id.
    pub fn set(&self, block_id: impl Into<String>, status: RuleStatus) {
        self.0
            .write()
            .expect("rule status map poisoned")
            .insert(block_id.into(), status);
    }

    /// Forget any status for a rule block id (e.g. the block was deleted).
    pub fn clear(&self, block_id: &str) {
        self.0
            .write()
            .expect("rule status map poisoned")
            .remove(block_id);
    }

    /// The current status for a block id, cloned out (no lock held on return).
    pub fn get(&self, block_id: &str) -> Option<RuleStatus> {
        self.0
            .read()
            .expect("rule status map poisoned")
            .get(block_id)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_language_is_not_active_and_reads_back() {
        let handle = RuleStatusHandle::new();
        handle.set("block:rule", RuleStatus::DeprecatedLanguage);
        let got = handle.get("block:rule").expect("status recorded");
        assert_eq!(got, RuleStatus::DeprecatedLanguage);
        assert!(!got.is_active());
        assert!(got.to_string().contains("holon_rule"));
    }

    #[test]
    fn active_reads_back_and_clear_forgets() {
        let handle = RuleStatusHandle::new();
        handle.set("block:rule", RuleStatus::Active);
        assert!(handle.get("block:rule").unwrap().is_active());
        handle.clear("block:rule");
        assert_eq!(handle.get("block:rule"), None);
    }
}
