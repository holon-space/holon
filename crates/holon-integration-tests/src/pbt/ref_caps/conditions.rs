//! `RefConditions` — what the model expects the app to be disclosing.

use std::collections::BTreeSet;

use holon_api::EntityUri;
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
                message: c.message.clone(),
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

    fn write_held(&self) -> holon_pbt_core::capabilities::WriteHeld {
        let docs: BTreeSet<&EntityUri> = self.files.churning_docs().collect();
        holon_pbt_core::capabilities::WriteHeld {
            files: docs
                .iter()
                .map(|doc| crate::pbt::copies_model::file_name_of(self, doc))
                .collect(),
            blocks: self
                .domain
                .block_state
                .blocks
                .keys()
                .filter(|id| self.homed_in(id, &docs))
                .cloned()
                .collect(),
        }
    }
}

impl ReferenceState {
    /// Whether block `id` lives in the file of one of `docs`. A page has a
    /// file of its own.
    fn homed_in(&self, id: &EntityUri, docs: &BTreeSet<&EntityUri>) -> bool {
        let mut at = id;
        loop {
            if docs.contains(at) {
                return true;
            }
            match self.domain.block_state.blocks.get(at) {
                Some(block) if !block.is_page() => at = &block.parent_id,
                _ => return false,
            }
        }
    }
}
