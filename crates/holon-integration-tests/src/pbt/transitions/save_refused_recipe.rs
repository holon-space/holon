//! Transition: an external editor saves a recipe the cooklang adapter refuses.
//!
//! @pbt rung external
//!   writes a broken `.cook` file into the watched in-memory vault and lets the
//!   production FileSyncController ingest it.
//! @pbt covers ingest-refusals-group-by-format — every refused file of one
//!   format is ONE `vault-ingest-failed` condition that counts them and names
//!   the first few (D71.b)
//!
//! A refused file puts nothing into the store, so the only model effect is the
//! disclosure: [`ReferenceState::disclose_refused_recipes`].

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

/// The vault file names a refused recipe is saved under. More than
/// `IngestRefusals::EXAMPLES`, so a run can push the count past the files the
/// condition names.
pub const REFUSED_RECIPE_FILES: [&str; 5] = [
    "refused-recipe-0.cook",
    "refused-recipe-1.cook",
    "refused-recipe-2.cook",
    "refused-recipe-3.cook",
    "refused-recipe-4.cook",
];

/// A recipe the cooklang adapter refuses: the unclosed brace drops the
/// quantity, and the plugin refuses that at the boundary.
pub const REFUSED_RECIPE_COOK: &str = "Mix the @flour{200%g into the bowl.\n";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external editor saves the broken recipe {file}")]
pub struct SaveRefusedRecipe {
    pub file: String,
}

/// The cooklang adapter ingests only on a draw whose boot reads the vault's
/// files — the same draw that seeds the read-only recipe.
fn cooklang_ingests(state: &ReferenceState) -> bool {
    state.read_only.seeded()
}

impl TransitionFactory<ReferenceState> for SaveRefusedRecipe {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(cooklang_ingests(state), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| {
            let strat = proptest::sample::select(REFUSED_RECIPE_FILES.to_vec())
                .prop_map(|file| SaveRefusedRecipe {
                    file: file.to_string(),
                })
                .boxed();
            (4, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for SaveRefusedRecipe {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(cooklang_ingests(state), Reason::PreconditionFailed),
            check(
                REFUSED_RECIPE_FILES.contains(&self.file.as_str()),
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
    SaveRefusedRecipe: SutSeamMutate,
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
