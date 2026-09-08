//! Reference-model fragment for blocks homed in a `WriteTier::ReadOnly` format.
//!
//! The oracle does not model the read-only document's CONTENT: its blocks are
//! seed-classified (booted but not in the working tree), so every block
//! invariant already ignores them and the file — not the oracle — is the
//! authority on what they say. What the oracle must know is which ids exist, so
//! `AttemptReadOnlyEdit` can aim at one, and that a write against them changes
//! nothing.

use std::collections::BTreeSet;

use holon_api::entity_uri::EntityUri;

/// Which blocks are homed in a read-only format, and how many writes the run
/// aimed at them. Empty on an org-only draw, which is every draw whose fixture
/// seeds no second format.
#[derive(Debug, Clone, Default)]
pub struct ReadOnlyRefState {
    homes: BTreeSet<EntityUri>,
    attempts: usize,
}

impl ReadOnlyRefState {
    pub fn seed_home(&mut self, id: EntityUri) {
        self.homes.insert(id);
    }

    pub fn homes(&self) -> &BTreeSet<EntityUri> {
        &self.homes
    }

    /// Record one attempted write. The block's content is deliberately NOT
    /// touched: a refused write is the modeled outcome.
    pub fn record_attempt(&mut self) {
        self.attempts += 1;
    }

    pub fn attempts(&self) -> usize {
        self.attempts
    }
}
