//! `RefShapeEdits` — the outcome the oracle expects of each tagged-subtree
//! edit.

use holon_pbt_core::capabilities::ExpectedShapeOutcome;
use holon_pbt_core::capabilities::RefShapeEdits;

use crate::pbt::reference_state::ReferenceState;

impl RefShapeEdits for ReferenceState {
    fn expected_shape_outcomes(&self) -> Vec<ExpectedShapeOutcome> {
        self.shape.outcomes().to_vec()
    }
}
