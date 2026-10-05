//! Reference-model fragment for blocks homed in a `WriteTier::ReadOnly` format.
//!
//! The oracle does not model the read-only document's CONTENT: its blocks are
//! seed-classified (booted but not in the working tree), so every block
//! invariant already ignores them and the file — not the oracle — is the
//! authority on what they say. What the oracle must know is which ids exist, so
//! `AttemptReadOnlyEdit` can aim at one, which file each belongs to, and that a
//! write against them changes nothing.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_api::entity_uri::EntityUri;

/// Which blocks are homed in a read-only format, and how many writes the run
/// aimed at them. Empty on an org-only draw, which is every draw whose fixture
/// seeds no second format.
#[derive(Debug, Clone, Default)]
pub struct ReadOnlyRefState {
    homes: BTreeSet<EntityUri>,
    /// Every block the write-tier authority refuses — the document page and its
    /// steps — with the vault file name the refusal is disclosed against.
    files: BTreeMap<EntityUri, String>,
    /// The read-only documents' pages.
    documents: BTreeSet<EntityUri>,
    attempts: usize,
}

impl ReadOnlyRefState {
    /// The read-only document's page: refused, but not one of the `homes`
    /// whose ingested content the invariant pins.
    pub fn seed_document(&mut self, page: EntityUri, file: &str) {
        self.files.insert(page.clone(), file.to_string());
        self.documents.insert(page);
    }

    pub fn documents(&self) -> &BTreeSet<EntityUri> {
        &self.documents
    }

    pub fn seed_home(&mut self, id: EntityUri, file: &str) {
        self.files.insert(id.clone(), file.to_string());
        self.homes.insert(id);
    }

    /// The vault file `from` was renamed to `to`: its blocks keep their ids and
    /// are now refused against the new file.
    pub fn rename_file(&mut self, from: &str, to: &str) {
        for file in self.files.values_mut().filter(|file| *file == from) {
            *file = to.to_string();
        }
    }

    pub fn homes(&self) -> &BTreeSet<EntityUri> {
        &self.homes
    }

    /// The file whose read-only tier refuses a write naming `id`, if any.
    pub fn refusing_file(&self, id: &EntityUri) -> Option<&str> {
        self.files.get(id).map(String::as_str)
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
