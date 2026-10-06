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

    /// A stalled write-back is disclosed while its churn lasts and must be
    /// gone once no file churns: the model judges the kind only then, and
    /// expects none of it.
    fn governed_condition_kinds(&self) -> BTreeSet<&'static str> {
        let mut governed = self.conditions.governed().clone();
        if self.files.write_churn_ran && !self.files.write_churn_armed() {
            governed.insert(holon_api::ConditionKind::WRITEBACK_DEGRADED);
        }
        governed
    }
}

impl holon_pbt_core::capabilities::RefCopies for ReferenceState {
    fn model_copies(&self) -> Vec<holon_pbt_core::capabilities::ModelCopy> {
        ReferenceState::model_copies(self)
    }

    fn write_churn_armed(&self) -> bool {
        self.files.write_churn_armed()
    }
}
