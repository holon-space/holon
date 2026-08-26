//! The rule watcher's published acceptance verdicts (ADR 0032 §2, ruling
//! D30.a's F2 half) — the rule half of the derived net's source union.
//!
//! [`holon_rule_watcher`](crate::api::holon_rule_watcher) is the only authority
//! on whether a `holon_rule` block fires. Before this registry the net
//! re-derived that answer from the rule's guard subject, which already differed
//! from the watcher (it ignored the paired-rule skip). Publishing the verdict
//! deletes the mirror: `derive_net` reads what the watcher decided.
//!
//! The watcher writes here from its discovery loop, so every write takes and
//! releases the lock inside a single statement and never holds it across an
//! `await` — the loop's matview subscription must not wait on a reader.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::RwLock;

use holon_net::RuleAcceptance;
use holon_net::RuleSource;

/// Shared, cheaply-clonable handle to the accepted-rule map (block id →
/// verdict).
///
/// The `holon_rule` watcher is the sole writer; the net derivation is the
/// reader. Reads clone the sources out so no lock is held on return.
#[derive(Clone, Default)]
pub struct AcceptedRuleHandle(Arc<RwLock<BTreeMap<String, RuleAcceptance>>>);

impl AcceptedRuleHandle {
    /// A fresh, empty handle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or overwrite) the verdict for a rule block id.
    pub fn set(&self, block_id: impl Into<String>, acceptance: RuleAcceptance) {
        self.0
            .write()
            .expect("accepted-rule map poisoned")
            .insert(block_id.into(), acceptance);
    }

    /// Forget any verdict for a rule block id — the block was deleted, so it
    /// is no longer declared automation.
    pub fn clear(&self, block_id: &str) {
        self.0
            .write()
            .expect("accepted-rule map poisoned")
            .remove(block_id);
    }

    /// Every verdict, as the sources `derive_net` consumes.
    pub fn sources(&self) -> Vec<RuleSource> {
        self.0
            .read()
            .expect("accepted-rule map poisoned")
            .iter()
            .map(|(block_id, acceptance)| RuleSource {
                block_id: block_id.clone(),
                acceptance: acceptance.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_published_as_a_source_and_a_delete_forgets_it() {
        let handle = AcceptedRuleHandle::new();
        handle.set(
            "block:rule-broken",
            RuleAcceptance::Opaque {
                reason: "parse failed".to_string(),
            },
        );
        let sources = handle.sources();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].block_id, "block:rule-broken");
        assert!(
            !sources[0].acceptance.is_running(),
            "a refused rule must not read as running",
        );

        handle.clear("block:rule-broken");
        assert!(handle.sources().is_empty());
    }
}
