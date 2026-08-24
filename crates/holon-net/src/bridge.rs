//! The dispatch↔net vocabulary bridge (ADR 0032 §8). Dispatch keeps its
//! `Operation*` names; this module is the one place the two vocabularies
//! meet:
//!
//! | Dispatch layer | Net layer |
//! |---|---|
//! | `OperationDescriptor` | [`crate::NetTransition`] |
//! | operation instance (entity + params) | binding / firing request |
//! | successful `execute_operation` | occurrence |
//! | `GuardWorld` check | enabledness |

use serde::Deserialize;
use serde::Serialize;

/// What produced a transition — the literal half of the §8 bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransitionSource {
    /// An [`holon_api::OperationDescriptor`], fired as a dispatched
    /// operation.
    Operation { entity: String, op: String },
    /// A parsed `holon_rule` block, fired by its watcher. `active` states
    /// whether the watcher runs it: only clock-subject rules fire today,
    /// block-subject rules are parked by `holon_rule_watcher` — the net
    /// still models them, because a parked rule is declared automation.
    Rule {
        block_id: String,
        name: String,
        active: bool,
    },
}

impl TransitionSource {
    /// A stable human-readable label for reports.
    pub fn label(&self) -> String {
        match self {
            TransitionSource::Operation { entity, op } => format!("{entity}.{op}"),
            TransitionSource::Rule { name, .. } => format!("rule:{name}"),
        }
    }
}
