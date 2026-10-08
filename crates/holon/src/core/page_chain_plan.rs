//! The typed plan the `page_chain_plan` read op hands the engine-level
//! `create_page_from_link` compound: the page a wiki-link target names, and
//! the pages that must be created to reach it.

use holon_api::Value;

use crate::core::block_to_page_plan::PlanSegment;
use crate::core::block_to_page_plan::get_str;

#[derive(Debug, Clone, PartialEq)]
pub struct PageChainPlan {
    /// The page the target names once `missing` exists.
    pub leaf_id: String,
    /// Pages to create, root→leaf; empty when the whole chain already exists.
    pub missing: Vec<PlanSegment>,
}

impl PageChainPlan {
    pub fn to_value(&self) -> Value {
        let mut obj = std::collections::HashMap::new();
        obj.insert("leaf_id".to_string(), Value::String(self.leaf_id.clone()));
        obj.insert(
            "missing".to_string(),
            PlanSegment::list_to_value(&self.missing),
        );
        Value::Object(obj)
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let obj = match value {
            Value::Object(o) => o,
            other => return Err(format!("PageChainPlan: expected Object, got {other:?}")),
        };
        Ok(Self {
            leaf_id: get_str(obj, "leaf_id").map_err(|e| format!("PageChainPlan: {e}"))?,
            missing: PlanSegment::list_from_value(obj.get("missing"))
                .map_err(|e| format!("PageChainPlan: 'missing': {e}"))?,
        })
    }
}
