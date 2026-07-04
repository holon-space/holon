//! Transition: rename an org FILE on disk (a user-side `mv A.org B.org`).
//!
//! @pbt rung external
//!   moves the org file within the watched org_root (`#+ID:` carried
//!   unchanged); the production `FileSyncController` watcher re-ingests the
//!   moved file, whose `#+ID:` resolves to the SAME existing document.
//! @pbt covers doc-file-rename — renaming a document's file must retitle its
//! page to the new file stem (the file-move spec production violates)
//!
//! ## Why this transition exists (D2 — CLOSED by the atomic Rename port)
//!
//! `docs/…`: a page's title FOLLOWS its file name. When a user renames
//! `A.org` → `B.org`, the page titled "A" must become "B". The `FileChange`
//! port now carries an atomic `Rename { from }` kind; the in-memory fs emits it
//! and `FileSyncController::on_file_renamed` re-homes the doc WITHOUT a
//! delete-then-create window and retitles the doc-root to the new file stem.
//! Both the reference (`RefDocumentsMut::rename_document`) and the SUT
//! (`SutAppLifecycle::rename_document` → `FileSystem::rename`) now share that
//! atomic path, so they CONVERGE on the retitle. `#99`'s `RenamePage`
//! deliberately EXCLUDED document-root pages ("file-move semantics the
//! reference does not model"); this transition models exactly that case and now
//! passes green (see the `doc-file-rename-title-followed` keystone case).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefDocumentsMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::CACHE_EVENT_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::READS_PER_WATCH;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::cdc_tolerance;

/// Move the org file `old_file_name` → `new_file_name` on disk.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RenameDocument {
    pub old_file_name: String,
    pub new_file_name: String,
}

/// Fixed pool of rename targets. A tiny, fixed set keeps replay deterministic
/// and shrinking meaningful — the property does not care WHAT the new name is,
/// only that the page title must track it.
const RENAME_TARGET_POOL: [&str; 4] = ["moved_a.org", "moved_b.org", "moved_c.org", "moved_d.org"];

/// Whether `name` matches the synthetic `doc_<n>.org` pattern `CreateDocument`
/// generates. Generation is DELIBERATELY narrowed to those (as `DeleteDocument`
/// is) so the keystone never renames a reserved/seed file (`index.org`,
/// `Journals.org`, structural/companion docs). The deterministic hand-authored
/// witness uses a `#+ID`-bearing `WriteOrgFile` doc, which `preconditions`
/// admits by name alone.
fn is_synthetic_doc_name(name: &str) -> bool {
    name.strip_prefix("doc_")
        .and_then(|rest| rest.strip_suffix(".org"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn renameable_doc_names<R: RefDocuments>(state: &R) -> Vec<String> {
    let mut names: Vec<String> = state
        .document_names()
        .into_iter()
        .filter(|name| is_synthetic_doc_name(name))
        .collect();
    names.sort();
    names
}

fn free_targets<R: RefDocuments>(state: &R) -> Vec<String> {
    RENAME_TARGET_POOL
        .iter()
        .map(|t| (*t).to_string())
        .filter(|t| !state.has_document(t))
        .collect()
}

impl<R: RefLifecycle + RefDocumentsMut> TransitionFactory<R> for RenameDocument {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = renameable_doc_names(state);
        let targets = free_targets(state);
        // OFF by default: kept env-gated so the RANDOM composed alphabet is
        // unchanged (the atomic-Rename fix landed via the DETERMINISTIC
        // `doc-file-rename-title-followed` keystone case, which is gated by
        // `preconditions` alone, not this generator). The removal-cascade
        // over-delete and the atomic-Rename-kind gaps are now CLOSED
        // (`on_file_renamed` re-homes + retitles with no delete window). Opt the
        // random rung in with `HOLON_PBT_DOC_RENAME=1`.
        let enabled = std::env::var("HOLON_PBT_DOC_RENAME").is_ok();
        let checks: Vec<Validated<(), Reason>> = vec![
            check(enabled, Reason::PreconditionFailed),
            check(state.app_started(), Reason::AppNotStarted),
            check(!candidates.is_empty(), Reason::NoDocumentsAvailable),
            check(!targets.is_empty(), Reason::PreconditionFailed),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| {
                let strat = (
                    proptest::sample::select(candidates),
                    proptest::sample::select(targets),
                )
                    .prop_map(|(old_file_name, new_file_name)| RenameDocument {
                        old_file_name,
                        new_file_name,
                    })
                    .boxed();
                (2, strat)
            })
    }
}

impl<R: RefLifecycle + RefDocumentsMut> TransitionRef<R> for RenameDocument {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            // The doc must still exist (shrinking may have dropped its create).
            check(
                state.has_document(&self.old_file_name),
                Reason::NoDocumentsAvailable,
            ),
            // The target name must be free, and a rename must move.
            check(
                !state.has_document(&self.new_file_name),
                Reason::PreconditionFailed,
            ),
            check(
                self.old_file_name != self.new_file_name,
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.rename_document(&self.old_file_name, &self.new_file_name);
    }
}

crate::cap_transition! {
    RenameDocument: SutAppLifecycle,
    where R: [ RefLifecycle + RefDocumentsMut ],
    |me, _state, sut| {
        sut.rename_document(&me.old_file_name, &me.new_file_name).await;
    }
    sql_budget: |_me, state| {
        // A rename is a create-at-new + remove-at-old pair of watcher ingests,
        // so bound it like DeleteDocument (whose cascade cost dominates).
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 4 + watches * READS_PER_WATCH,
            writes: 4,
            ddl: 0,
            tolerance: cdc_tolerance(blocks + 5, docs + 1) + watches * 4 + blocks * 6,
        }
    }
}
