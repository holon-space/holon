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

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// A transition's identity, stable across recompiles and total and injective
/// over both source families: an operation is keyed by `(entity, op)`, a rule
/// by the block that hosts it. A rule's `name` is display, never identity —
/// two blocks may declare the same name.
///
/// The rendered form (`op:block.set_field`, `rule:block:rule-daily-journal`)
/// is the join key MCP payloads and divergence ledgers carry, so it is a wire
/// contract: pinned by `tests/transition_key.rs`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransitionKey(String);

impl TransitionKey {
    pub fn operation(entity: &str, op: &str) -> Self {
        assert!(
            !entity.contains('.'),
            "entity name {entity:?} contains the `.` that separates it from the op in a \
             transition key, which would make the key ambiguous"
        );
        TransitionKey(format!("op:{entity}.{op}"))
    }

    pub fn rule(block_id: &str) -> Self {
        TransitionKey(format!("rule:{block_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransitionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

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
    /// This source's transition identity.
    pub fn key(&self) -> TransitionKey {
        match self {
            TransitionSource::Operation { entity, op } => TransitionKey::operation(entity, op),
            TransitionSource::Rule { block_id, .. } => TransitionKey::rule(block_id),
        }
    }

    /// A stable human-readable label for reports.
    pub fn label(&self) -> String {
        match self {
            TransitionSource::Operation { entity, op } => format!("{entity}.{op}"),
            TransitionSource::Rule { name, .. } => format!("rule:{name}"),
        }
    }
}
