use holon_api::block::Block;
use holon_api::decision_block;

use super::ShapeValidator;
use super::ShapeViolation;

/// The block home of a decision: the block adapter's read plus the core parse
/// must accept the subtree.
pub struct DecisionShape;

impl ShapeValidator for DecisionShape {
    fn tag(&self) -> &'static str {
        decision_block::DECISION_TAG
    }

    fn validate(&self, root: &Block, children: &[Block]) -> Result<(), ShapeViolation> {
        decision_block::parse(root, children)
            .map(drop)
            .map_err(|e| ShapeViolation {
                rule: e.code(),
                message: e.to_string(),
            })
    }

    fn created_by(&self) -> &'static str {
        "a decision is made in one step: create the question (task keyword `?`, tag \
         `decision`) and its `option` children in one `dense_patch` batch"
    }

    /// Titles are free text and a blank answer body is no rationale, so the
    /// adapter reads every text. The decision's own task keyword is not text:
    /// it is a property, written by its own op.
    fn judges_text(&self) -> bool {
        false
    }
}
