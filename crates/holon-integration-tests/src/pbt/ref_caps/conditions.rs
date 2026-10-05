//! `RefConditions` — what the model expects the app to be disclosing.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::ExpectedDisclosure;
use holon_pbt_core::capabilities::RefConditions;

use crate::pbt::reference_state::ReferenceState;

impl RefConditions for ReferenceState {
    fn expected_conditions(&self) -> Vec<ExpectedDisclosure> {
        self.conditions
            .expected()
            .iter()
            .map(|c| ExpectedDisclosure {
                subject_name: c.subject_name.clone(),
                kind: c.kind,
                files: c.files.clone(),
                count: c.count,
            })
            .collect()
    }

    fn governed_condition_kinds(&self) -> BTreeSet<&'static str> {
        self.conditions.governed().clone()
    }
}

impl holon_pbt_core::capabilities::RefCopies for ReferenceState {
    fn model_copies(&self) -> Vec<holon_pbt_core::capabilities::ModelCopy> {
        ReferenceState::model_copies(self)
    }
}
