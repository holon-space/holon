//! Advice-rule compilation/runtime status (ADR 0022 "rule blocks render their
//! own status — fail loud, visibly").
//!
//! A rule block's synthesis outcome is carried here, keyed by the rule block's
//! id, written by the engine's advice reconciler and read by the UI watchers so
//! a broken rule renders its error IN PLACE rather than silently no-op'ing.
//! Synchronous parse errors and asynchronous DDL failures alike surface through
//! this one map.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

/// The compilation/runtime status of one advice-rule block.
///
/// Only [`AdviceRuleStatus::Active`] renders normally; every other variant
/// replaces the block's render with its message (the fail-loud-visibly
/// surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdviceRuleStatus {
    /// Rule parsed and its matview reconciled (or was torn down because the
    /// rule is inactive) — nothing is wrong. Renders normally (no banner in
    /// v1).
    // TODO(ADR 0022 sub-decision 2): an over-cap rule (matched anchors > cap) still
    // renders `Active` today; add a disclosed "truncated at N anchors" banner when
    // read-time truncation lands.
    Active { slug: String },
    /// The rule YAML did not parse. The message is the typed parse error.
    ParseError(String),
    /// The rule parsed but its matview synthesis / reconcile failed (an
    /// asynchronous DDL failure at reconcile time — never a silent no-op,
    /// ADR 0022).
    DdlError(String),
    /// A second rule block claimed a slug already owned by a live different
    /// block. First-owner-wins: the newcomer is refused (no reconcile), and
    /// this status names the winning owner block.
    SlugCollision { slug: String, owner: String },
}

impl AdviceRuleStatus {
    /// True only for [`AdviceRuleStatus::Active`] — the normal render case.
    pub fn is_active(&self) -> bool {
        matches!(self, AdviceRuleStatus::Active { .. })
    }
}

impl std::fmt::Display for AdviceRuleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdviceRuleStatus::Active { slug } => write!(f, "active ({slug})"),
            AdviceRuleStatus::ParseError(msg) => write!(f, "parse error: {msg}"),
            AdviceRuleStatus::DdlError(msg) => write!(f, "synthesis failed: {msg}"),
            AdviceRuleStatus::SlugCollision { slug, owner } => {
                write!(f, "slug `{slug}` already owned by block {owner}")
            }
        }
    }
}

/// Shared, cheaply-clonable handle to the advice-rule status map (block id →
/// status).
///
/// The engine's reconciler task is the sole writer; the UI watchers are
/// readers. Reads clone the status out so no lock is held across the caller's
/// `await`.
#[derive(Clone, Default)]
pub struct AdviceRuleStatusHandle(Arc<RwLock<HashMap<String, AdviceRuleStatus>>>);

impl AdviceRuleStatusHandle {
    /// A fresh, empty handle (the no-advice / no-Turso default).
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or overwrite) the status for a rule block id.
    pub fn set(&self, block_id: impl Into<String>, status: AdviceRuleStatus) {
        self.0
            .write()
            .expect("advice status map poisoned")
            .insert(block_id.into(), status);
    }

    /// Forget any status for a rule block id (e.g. the block was deleted).
    pub fn clear(&self, block_id: &str) {
        self.0
            .write()
            .expect("advice status map poisoned")
            .remove(block_id);
    }

    /// The current status for a block id, cloned out (no lock held on return).
    pub fn get(&self, block_id: &str) -> Option<AdviceRuleStatus> {
        self.0
            .read()
            .expect("advice status map poisoned")
            .get(block_id)
            .cloned()
    }
}
