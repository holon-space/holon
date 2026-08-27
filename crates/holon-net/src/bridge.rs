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
use thiserror::Error;

/// A failure of the dispatch↔net vocabulary bridge: a dispatch-side name that
/// the net's grammar cannot encode.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NetError {
    #[error(
        "entity name {entity:?} contains the `.` that separates entity from op in a transition \
         key, so the pair it names has no unambiguous key"
    )]
    DottedEntityName { entity: String },
}

/// An entity name that a transition key can encode: dotless, because `.` is
/// the key's own entity/op separator.
///
/// Parsed once, at the boundary where a dispatch-side name enters the net
/// (`holon_core::classify_for_net` refuses the same shape earlier, at the
/// catalog). Holding the parsed form is what lets [`TransitionSource::key`]
/// and every analysis over a compiled net stay infallible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NetEntity(String);

impl NetEntity {
    pub fn parse(entity: &str) -> Result<Self, NetError> {
        if entity.contains('.') {
            return Err(NetError::DottedEntityName {
                entity: entity.to_string(),
            });
        }
        Ok(NetEntity(entity.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NetEntity {
    type Error = NetError;

    fn try_from(entity: String) -> Result<Self, NetError> {
        NetEntity::parse(&entity)
    }
}

impl From<NetEntity> for String {
    fn from(entity: NetEntity) -> String {
        entity.0
    }
}

impl fmt::Display for NetEntity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

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
    /// The key for a `(entity, op)` pair given as raw dispatch-side strings —
    /// the one place the entity's grammar is checked.
    pub fn operation(entity: &str, op: &str) -> Result<Self, NetError> {
        Ok(TransitionKey::for_entity(&NetEntity::parse(entity)?, op))
    }

    /// The key for an entity already parsed. Infallible by construction.
    pub fn for_entity(entity: &NetEntity, op: &str) -> Self {
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
    Operation { entity: NetEntity, op: String },
    /// A `holon_rule` block, fired by its watcher. `active` is the WATCHER's
    /// own verdict ([`crate::RuleAcceptance`]), never re-derived from the
    /// rule's shape. A parked or unparseable rule is still modelled, because
    /// declared automation that does not run is still declared.
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
            TransitionSource::Operation { entity, op } => TransitionKey::for_entity(entity, op),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The grammar's last-line guard, independent of any upstream filter: a
    /// dotted entity name has no unambiguous key, so minting one fails here.
    /// Whatever the catalog boundary decides to exclude, nothing that reaches
    /// this point can smuggle a `.` past it — and it refuses as a typed error
    /// the caller can report, never a panic through a library boundary.
    #[test]
    fn a_dotted_entity_name_cannot_be_lowered_to_a_key() {
        let err = TransitionKey::operation("orgmode.sync", "sync")
            .expect_err("a dotted entity name has no key");
        assert_eq!(
            err,
            NetError::DottedEntityName {
                entity: "orgmode.sync".to_string()
            }
        );
        let message = err.to_string();
        assert!(
            message.contains("orgmode.sync") && message.contains("separates entity from op"),
            "the error must name the offender and why it has no key: {message}"
        );
    }

    /// The parsed form is what makes a compiled net's keys infallible, so it
    /// must not be reconstructible from a dotted name through serde either.
    #[test]
    fn a_dotted_entity_name_cannot_be_deserialized() {
        let err = serde_json::from_str::<NetEntity>(r#""orgmode.sync""#)
            .expect_err("serde parses through NetEntity::parse");
        assert!(
            err.to_string().contains("orgmode.sync"),
            "the serde error must name the offender: {err}"
        );
    }

    #[test]
    fn a_dotless_entity_name_keys_and_round_trips() {
        let entity = NetEntity::parse("block").expect("dotless");
        assert_eq!(
            TransitionKey::for_entity(&entity, "set_field").as_str(),
            "op:block.set_field"
        );
        assert_eq!(
            serde_json::from_str::<NetEntity>(r#""block""#).expect("dotless"),
            entity
        );
    }
}
