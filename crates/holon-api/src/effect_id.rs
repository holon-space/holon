// # Deterministic effect IDs (ADR 0024 P4)
//
// A rule-fired `block.create` must mint the *same* block id on every replica
// that fires the same rule for the same key. Then the CRDT tree merge collapses
// the concurrent creates into one node — at-most-once-per-key becomes a naming
// discipline, not an execution-semantics problem.
//
// The id is a name-based UUIDv5 of `(rule-id, firing-key, output-slot)` under a
// fixed Holon namespace. It is deterministic (same inputs → same UUID) and
// distinct across rules, keys, and output slots.

use uuid::Uuid;

use crate::entity::StorageEntity;
use crate::entity_uri::EntityUri;
use crate::Value;

/// Fixed, checked-in namespace for all Holon rule-effect UUIDs. Changing this
/// re-homes every deterministic id, so it is a hard-coded constant, never
/// derived at runtime.
pub const HOLON_RULE_NAMESPACE: Uuid = Uuid::from_u128(0x9f8b7c6d_5e4f_4a3b_8c2d_1e0f9a8b7c6d);

/// The identity of the rule whose firing produced an effect — the discovery
/// `action_id`, parsed once at the discovery boundary and threaded through the
/// watcher. Never a bare `String` at a call site.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RuleId(String);

impl RuleId {
    pub fn new(action_id: impl Into<String>) -> Self {
        Self(action_id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The key that identifies *which* firing produced an effect: a canonical
/// serialization of the produced trigger row (sorted `key=value` pairs, typed
/// value rendering). Convergent across replicas because projection is total and
/// each replica derives the same row (ADR 0024 A4).
///
/// Phase-1 stopgap (plan Q3): the whole row is the key. Phase 2 replaces this
/// with the explicit `emit` key / interpolated builtins; the newtype boundary
/// keeps that swap local.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FiringKey(String);

impl FiringKey {
    pub fn from_row(row: &StorageEntity) -> Self {
        // Internal columns (`_rowid`, watermarks) are excluded: they are not part
        // of the semantic binding, and CDC delivers `_rowid` with a different
        // Value type on the Created vs Updated path (Integer vs String), which
        // would mint path-dependent ids and break cross-replica convergence.
        let mut entries: Vec<String> = row
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| format!("{k}={}", canonical_value(v)))
            .collect();
        entries.sort();
        Self(entries.join("\n"))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which output of a firing this id names. A single rule firing may emit more
/// than one effect; each distinct output gets a distinct slot so the two ids do
/// not collide. Phase-1 rules emit exactly one create per firing, so they use
/// [`OutputSlot::first`], but the id signature carries the slot from day one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OutputSlot(u32);

impl OutputSlot {
    pub fn first() -> Self {
        Self(0)
    }
    pub fn nth(index: u32) -> Self {
        Self(index)
    }
}

/// Mint the deterministic block id for one emitted effect of a rule firing.
pub fn deterministic_block_id(rule: &RuleId, key: &FiringKey, slot: &OutputSlot) -> EntityUri {
    // Unit separator between components keeps them unambiguous even if a rule id
    // or firing key happens to contain the delimiter characters used inside a
    // component (`=`, `\n`).
    let name = format!("{}\x1f{}\x1f{}", rule.0, key.0, slot.0);
    let uuid = Uuid::new_v5(&HOLON_RULE_NAMESPACE, name.as_bytes());
    EntityUri::block(&uuid.to_string())
}

/// Typed, deterministic rendering of a value for the firing key. The type tag
/// prevents `Integer(1)` and `String("1")` from producing the same key; nested
/// objects are rendered with sorted keys so map iteration order never leaks in.
fn canonical_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("s:{s}"),
        Value::Integer(i) => format!("i:{i}"),
        Value::Float(f) => format!("f:{f}"),
        Value::Boolean(b) => format!("b:{b}"),
        Value::DateTime(s) => format!("dt:{s}"),
        Value::Json(s) => format!("j:{s}"),
        Value::Null => "null".to_string(),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonical_value).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            let mut kv: Vec<(&String, &Value)> = map.iter().collect();
            kv.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = kv
                .iter()
                .map(|(k, val)| format!("{k}={}", canonical_value(val)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn row(pairs: &[(&str, Value)]) -> StorageEntity {
        pairs
            .iter()
            .map(|(k, v)| (Arc::from(*k), v.clone()))
            .collect()
    }

    #[test]
    fn id_is_stable_across_calls() {
        let rule = RuleId::new("journals::action::0");
        let key = FiringKey::from_row(&row(&[("name", Value::String("2026-07-10".into()))]));
        let slot = OutputSlot::first();
        let a = deterministic_block_id(&rule, &key, &slot);
        let b = deterministic_block_id(&rule, &key, &slot);
        assert_eq!(a.as_str(), b.as_str());
        assert!(a.as_str().starts_with("block:"));
    }

    #[test]
    fn id_distinct_across_rule_key_slot() {
        let rule_a = RuleId::new("rule-a");
        let rule_b = RuleId::new("rule-b");
        let key_1 = FiringKey::from_row(&row(&[("name", Value::String("2026-07-10".into()))]));
        let key_2 = FiringKey::from_row(&row(&[("name", Value::String("2026-07-11".into()))]));
        let slot_0 = OutputSlot::first();
        let slot_1 = OutputSlot::nth(1);

        let base = deterministic_block_id(&rule_a, &key_1, &slot_0);
        // different rule
        assert_ne!(
            base.as_str(),
            deterministic_block_id(&rule_b, &key_1, &slot_0).as_str()
        );
        // different key
        assert_ne!(
            base.as_str(),
            deterministic_block_id(&rule_a, &key_2, &slot_0).as_str()
        );
        // two slots of one firing → two distinct ids
        assert_ne!(
            base.as_str(),
            deterministic_block_id(&rule_a, &key_1, &slot_1).as_str()
        );
    }

    #[test]
    fn firing_key_is_order_independent_and_type_tagged() {
        let r1 = row(&[
            ("name", Value::String("d".into())),
            ("parent_id", Value::String("block:journals".into())),
        ]);
        let r2 = row(&[
            ("parent_id", Value::String("block:journals".into())),
            ("name", Value::String("d".into())),
        ]);
        assert_eq!(
            FiringKey::from_row(&r1).as_str(),
            FiringKey::from_row(&r2).as_str()
        );
        // Integer 1 and String "1" must not collide.
        let as_int = FiringKey::from_row(&row(&[("v", Value::Integer(1))]));
        let as_str = FiringKey::from_row(&row(&[("v", Value::String("1".into()))]));
        assert_ne!(as_int.as_str(), as_str.as_str());
    }

    #[test]
    fn firing_key_excludes_internal_columns() {
        // CDC delivers _rowid as Integer on the Created path but String on the
        // Updated path; the key must be identical either way (and with no
        // _rowid at all), or the same day's journal gets path-dependent ids.
        let semantic = row(&[("name", Value::String("2026-07-10".into()))]);
        let created_path = row(&[
            ("name", Value::String("2026-07-10".into())),
            ("_rowid", Value::Integer(1)),
        ]);
        let updated_path = row(&[
            ("name", Value::String("2026-07-10".into())),
            ("_rowid", Value::String("1".into())),
        ]);
        assert_eq!(
            FiringKey::from_row(&semantic).as_str(),
            FiringKey::from_row(&created_path).as_str()
        );
        assert_eq!(
            FiringKey::from_row(&created_path).as_str(),
            FiringKey::from_row(&updated_path).as_str()
        );
    }
}
