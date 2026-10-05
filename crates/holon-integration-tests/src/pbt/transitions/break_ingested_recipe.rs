//! Transition: an external editor breaks a recipe that had ingested fine.
//!
//! @pbt rung external
//!   overwrites the seeded read-only `.cook` file in the watched in-memory
//!   vault with bytes the cooklang adapter refuses, and lets the production
//!   FileSyncController — watcher and poll backstop — re-read it.
//! @pbt covers ingest-refusals-group-by-format — a file that ingested and is
//!   then refused joins its format's group like a file that never ingested
//!   (D71.b)
//!
//! The refusal puts nothing into the store: the blocks of the last good ingest
//! stay. The only model effect is the disclosure.

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

use super::save_refused_recipe::REFUSED_RECIPE_COOK;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external editor breaks the ingested recipe {file}")]
pub struct BreakIngestedRecipe {
    pub file: String,
}

/// The seeded recipe's file, while it still ingests.
fn ingested_recipe(state: &ReferenceState) -> Option<String> {
    state
        .read_only_recipe_file()
        .filter(|file| !state.refused_recipes.contains(*file))
        .map(str::to_string)
}

impl TransitionFactory<ReferenceState> for BreakIngestedRecipe {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let file = ingested_recipe(state);
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(file.is_some(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let file = file.expect("checked above");
            (2, Just(BreakIngestedRecipe { file }).boxed())
        })
    }
}

impl TransitionRef<ReferenceState> for BreakIngestedRecipe {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                ingested_recipe(state).as_deref() == Some(self.file.as_str()),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.refused_recipes.insert(self.file.clone());
        state.disclose_refused_recipes();
    }
}

crate::cap_transition! {
    BreakIngestedRecipe: SutSeamMutate,
    where R: [ RefLifecycle + RefReadOnlyHomes ],
    |me, _state, sut| {
        sut.save_vault_file(&me.file, REFUSED_RECIPE_COOK).await;
    }
    sql_budget: |_me, _state| {
        // The adapter refuses before any write; the ingest reads the file's
        // record and nothing else.
        ExpectedSql {
            reads: 4,
            writes: 0,
            ddl: 0,
            tolerance: 32,
        }
    }
}
