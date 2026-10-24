//! Transition: the app dies with a write-back owed, then boots again.
//!
//! @pbt rung external
//!   the session dies while `doc`'s file is behind its store
//!   (`SutAppLifecycle::crash_restart`), and the next boot meets that file.
//! @pbt covers reboot-persistence — a crash must not let a stale file revert
//!   what the store already holds

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefApplyMutationMut;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReboot;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::types::RetitleWhileDown;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

const CRASH_RESTORE_AMBIGUOUS: &str = "crash-restore-ambiguous";

/// Crash-restart over a document whose write-back is owed: the store holds an
/// edit, `doc`'s file does not (a churning writer stalled the write), and the
/// process ends there. The churn dies with it, so the next boot meets a healthy
/// file system and ONE divergence — a file behind its store.
///
/// What a crash keeps is the oracle: the store keeps every committed edit (it
/// is a database, the write-back is not what commits it), the file keeps the
/// bytes last written to it, and the boot must reconcile the two WITHOUT
/// reverting the store — the file is older than the store, not newer. The model
/// therefore only drops in-memory state and ends the churn; every block and
/// every field stays as the store had it.
///
/// The one exception is a file that boots equal to its merge base while its
/// write-back was owed: the boot cannot tell a write that never landed from
/// one the crash undid. The store still wins, and the boot raises
/// `crash-restore-ambiguous` and keeps the file's bytes as a conflict copy.
/// A file the user edited while the app was down is out of that window, and
/// its edit lifts an earlier such condition.
///
/// Hand-authored only (`HOLON_PBT_WRITEBACK_FAULTS` gates the churn alphabet it
/// needs, and a reboot costs a whole boot).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "the app dies with the write-back of document {doc} owed and {retitle_while_down} saved \
     into its file while it is down, then boots again"
)]
pub struct CrashRestart {
    pub doc: EntityUri,
    /// What an external editor saves into `doc`'s file while the app is down —
    /// the user fixing up a file after the crash.
    #[serde(default)]
    pub retitle_while_down: Option<RetitleWhileDown>,
}

impl TransitionFactory<ReferenceState> for CrashRestart {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> holon_pbt_core::RequiredWiring {
        // Like `Reboot`: the boot re-opens `test.db`, which is the observation.
        holon_pbt_core::RequiredWiring::HasStorage(holon_pbt_core::StorageAdapter::Turso)
    }

    fn weighted_generator(
        _state: &ReferenceState,
    ) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl TransitionRef<ReferenceState> for CrashRestart {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!state.all_block_ids().is_empty(), Reason::BlockStateEmpty),
            check(
                state.files.churning_docs().any(|doc| *doc == self.doc),
                Reason::NoDocumentsAvailable,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let file = crate::pbt::copies_model::file_name_of(state, &self.doc);
        let boots_equal_to_base =
            self.retitle_while_down.is_none() && state.write_back_owed(&self.doc);
        let base_texts: Vec<String> = state.files.merge_base[&self.doc]
            .blocks
            .iter()
            .filter(|block| block.id != self.doc)
            .map(|block| block.content.clone())
            .collect();
        state.reboot_drops_in_memory_state();
        state.files.write_churn.remove(&self.doc);
        let copies = state
            .files
            .crash_restore_copies
            .entry(file.clone())
            .or_default();
        if boots_equal_to_base {
            copies.push(base_texts);
            state.conditions.raise(file, CRASH_RESTORE_AMBIGUOUS);
        } else if self.retitle_while_down.is_some() {
            state.conditions.clear(&file, CRASH_RESTORE_AMBIGUOUS);
        }
        if let Some(retitle) = &self.retitle_while_down {
            // The boot reads the file, so the retitle lands in the store like
            // any external edit.
            state.apply_content_mutation(
                &holon_pbt_core::types::Mutation::Update {
                    id: retitle.block.clone(),
                    fields: std::collections::HashMap::from([(
                        "content".to_string(),
                        holon_api::Value::String(retitle.text.clone()),
                    )]),
                },
                true,
            );
        }
    }
}

crate::cap_transition! {
    CrashRestart: SutAppLifecycle,
    |me, _state, sut| {
        // The composed harness intercepts this like `Reboot` — a reboot replaces
        // the whole cap map, which `apply_transition` cannot do.
        sut.crash_restart(&me.doc, me.retitle_while_down.as_ref()).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for CrashRestart {
    /// The boot sits outside the measurement window, as `Reboot` documents.
    fn expected_sql<R: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        state: &R,
    ) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4 + docs_tolerance(state),
        }
    }
}
