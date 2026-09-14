//! `RefConditions` — what the model expects the app to be disclosing.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::RefConditions;

use crate::pbt::reference_state::ReferenceState;

impl RefConditions for ReferenceState {
    fn expected_conditions(&self) -> Vec<(String, &'static str)> {
        self.conditions
            .expected()
            .iter()
            .map(|c| (c.subject_name.clone(), c.kind))
            .collect()
    }

    fn governed_condition_kinds(&self) -> BTreeSet<&'static str> {
        self.conditions.governed().clone()
    }
}
