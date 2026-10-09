//! Transition: an external editor deletes a recipe the cooklang adapter
//! refused.
//!
//! @pbt rung external
//!   removes the `.cook` file from the watched in-memory vault and lets the
//!   production FileSyncController ingest the deletion.
//! @pbt covers ingest-refusals-group-by-format — a file that is gone leaves
//!   its format's group; the last one out clears the condition (D71.b)

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefReadOnlyHomes;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external editor deletes the broken recipe {file}")]
pub struct DeleteRefusedRecipe {
    pub file: String,
}

impl TransitionFactory<ReferenceState> for DeleteRefusedRecipe {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let refused: Vec<String> = state.refused_recipes.iter().cloned().collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!refused.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = proptest::sample::select(refused)
                .prop_map(|file| DeleteRefusedRecipe { file })
                .boxed();
            (4, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for DeleteRefusedRecipe {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.refused_recipes.contains(&self.file),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if state.read_only_recipe_file() == Some(self.file.as_str()) {
            state.retire_read_only_file(&self.file);
        }
        state.refused_recipes.remove(&self.file);
        state.disclose_refused_recipes();
    }
}

crate::cap_transition! {
    DeleteRefusedRecipe: SutSeamMutate,
    where R: [ RefLifecycle + RefReadOnlyHomes ],
    |me, _state, sut| {
        sut.delete_vault_file(&me.file).await;
    }
    sql_budget: |_me, state| {
        // A broken recipe that once ingested takes its page and steps out; a
        // file that never ingested owns no rows. With Loro in the write path
        // the deletion only reads. Without it, `org.on_file_deleted`
        // cascades through `DELETE FROM block_raw` and friends (34 reads, 9 writes).
        let (reads, writes) = if state.content_writes_reach_sql() {
            (34, 9)
        } else {
            (15, 0)
        };
        ExpectedSql {
            reads,
            writes,
            ddl: 0,
            tolerance: 6,
        }
    }
}
