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
    /// Whether the draw's boot ingests read-only formats at all. Outlives the
    /// documents: deleting the last one leaves the adapter ingesting.
    seeded: bool,
    attempts: usize,
}

impl ReadOnlyRefState {
    /// The read-only document's page: refused, but not one of the `homes`
    /// whose ingested content the invariant pins.
    pub fn seed_document(&mut self, page: EntityUri, file: &str) {
        self.files.insert(page.clone(), file.to_string());
        self.documents.insert(page);
        self.seeded = true;
    }

    pub fn seeded(&self) -> bool {
        self.seeded
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

    /// The vault file `file` was deleted: its document and steps leave the
    /// store, so nothing is refused against it any more. Returns the retired
    /// ids — the document page and its homes.
    pub fn retire_file(&mut self, file: &str) -> BTreeSet<EntityUri> {
        let retired: BTreeSet<EntityUri> = self
            .files
            .iter()
            .filter(|(_, home)| *home == file)
            .map(|(id, _)| id.clone())
            .collect();
        assert!(
            !retired.is_empty(),
            "retire_file: no read-only block is homed in {file}"
        );
        self.files.retain(|_, home| home != file);
        self.homes.retain(|id| !retired.contains(id));
        self.documents.retain(|id| !retired.contains(id));
        retired
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
