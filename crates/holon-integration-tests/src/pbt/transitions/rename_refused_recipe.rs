//! Transition: the user renames a recipe the cooklang adapter refuses.
//!
//! @pbt rung external
//!   moves the `.cook` file in the watched in-memory vault in one atomic
//!   rename and lets the production FileSyncController ingest the move.
//! @pbt covers ingest-refusals-group-by-format — a renamed refused file leaves
//!   its format's group under the old path and stays in it under the new one,
//!   whether or not it has a document (D71.b)
//!
//! A file saved broken has no document; the seeded recipe broken by
//! `BreakIngestedRecipe` has one, which keeps its id and is retitled to the new
//! file stem.

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

/// The names a refused recipe is renamed to. None of them is a name
/// `SaveRefusedRecipe` writes, so a rename never lands on a file another
/// transition owns.
pub const MOVED_RECIPE_FILES: [&str; 3] = [
    "moved-recipe-0.cook",
    "moved-recipe-1.cook",
    "moved-recipe-2.cook",
];

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I rename the broken recipe {from} to {to}")]
pub struct RenameRefusedRecipe {
    pub from: String,
    pub to: String,
}

/// The rename targets no file occupies.
fn free_targets(state: &ReferenceState) -> Vec<String> {
    MOVED_RECIPE_FILES
        .iter()
        .filter(|name| {
            !state.refused_recipes.contains(**name) && state.read_only_recipe_file() != Some(**name)
        })
        .map(|name| name.to_string())
        .collect()
}

impl TransitionFactory<ReferenceState> for RenameRefusedRecipe {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let refused: Vec<String> = state.refused_recipes.iter().cloned().collect();
        let targets = free_targets(state);
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!refused.is_empty(), Reason::PreconditionFailed),
            check(!targets.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let strat = (
                proptest::sample::select(refused),
                proptest::sample::select(targets),
            )
                .prop_map(|(from, to)| RenameRefusedRecipe { from, to })
                .boxed();
            (3, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for RenameRefusedRecipe {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.refused_recipes.contains(&self.from),
                Reason::PreconditionFailed,
            ),
            check(
                free_targets(state).contains(&self.to),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if state.read_only_recipe_file() == Some(self.from.as_str()) {
            // File-move spec: the page title follows the file name, refused
            // bytes or not.
            let page = crate::pbt::composed::wide_e2e::read_only_recipe_page();
            let stem = std::path::Path::new(&self.to)
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("a rename target has a stem")
                .to_string();
            state.read_only.rename_file(&self.from, &self.to);
            state
                .domain
                .block_state
                .blocks
                .get_mut(&page)
                .expect("the seeded recipe's page is modeled")
                .content = stem;
            state.recanon_and_rebuild();
        }
        state.refused_recipes.remove(&self.from);
        state.refused_recipes.insert(self.to.clone());
        state.disclose_refused_recipes();
    }
}

crate::cap_transition! {
    RenameRefusedRecipe: SutSeamMutate,
    where R: [ RefLifecycle + RefReadOnlyHomes ],
    |me, _state, sut| {
        sut.rename_vault_file(&me.from, &me.to).await;
    }
    sql_budget: |_me, _state| {
        // The refused destination writes nothing; a recipe with a document
        // re-homes it and retitles its page.
        ExpectedSql {
            reads: 8,
            writes: 4,
            ddl: 0,
            tolerance: 48,
        }
    }
}
