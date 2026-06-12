//! Intent `ChangeSet` — the typed vocabulary the block-sync rework speaks.
//!
//! Today the sync paths emit *untyped* command-bus ops: a
//! `Vec<(String, StorageEntity)>` whose first element is one of `"create"`,
//! `"update"`, `"delete"` and whose second element is a flattened params map.
//! That tuple form is lossy as an *intent* — an `"update"` that moves a block
//! and rewrites its content is indistinguishable, at the type level, from one
//! that only retitles it.
//!
//! A [`ChangeSet`] is the same information re-expressed in the four intent
//! verbs the consolidator/sink architecture (Phase 3 base-diff, Phase 5 intent
//! log) needs from the start: [`ChangeOp::Create`], [`ChangeOp::SetField`],
//! [`ChangeOp::Relocate`], [`ChangeOp::Delete`]. Each carries [`Provenance`]
//! (the originating `command_id` and a base reference) so the inverse direction
//! can skip echoes and undo can correlate.
//!
//! # Phase 2 status: shadow only
//!
//! Nothing dispatches a `ChangeSet` yet. It is constructed *alongside* the live
//! op path via [`ChangeSet::from_ops`] and checked for **op-multiset
//! agreement** against the source ops (see [`ChangeSet::reencode_op_names`] and
//! the `agrees_with_ops` helper). This proves the intent vocabulary can
//! faithfully represent every op real traffic emits before any later phase
//! flips dispatch onto it.
//!
//! ## Equivalence relation (the shadow's "agreement")
//!
//! Two op streams agree iff their **decoded op-name multisets agree** under the
//! normalization map:
//!
//! | source op | decodes to | re-encodes to |
//! |-----------|------------|---------------|
//! | `create`  | one `Create` | `create` |
//! | `update`  | a `Relocate` iff `parent_id` or `sort_key` changed, plus one `SetField` per other changed field | `update` (iff it produced ≥1 op) |
//! | `delete`  | one `Delete` | `delete` |
//!
//! Agreement is on the **op-name multiset** (how many creates / updates /
//! deletes), not on raw hashes or serialized bytes — the two existing base
//! mechanisms hash differently, so byte-equality could never agree. An
//! `update` that decodes to zero typed ops (no representable field change) is a
//! **divergence**: the vocabulary failed to capture a real op.

use std::collections::BTreeMap;

use crate::entity::StorageEntity;
use crate::entity_uri::EntityUri;
use crate::Value;

/// Fields that an `update` op carries for bookkeeping, not as a representable
/// field mutation. They never decode to a [`ChangeOp::SetField`].
const UPDATE_BOOKKEEPING_FIELDS: &[&str] = &["id", "updated_at"];

/// Fields whose change is an ordering/parentage move, decoded as a
/// [`ChangeOp::Relocate`] rather than a [`ChangeOp::SetField`].
/// `after_block_id` joins preemptively: any future update carrying it
/// would otherwise leak as `SetField`.
const RELOCATE_FIELDS: &[&str] = &["parent_id", "sort_key", "after_block_id"];

/// Create params injected by the storage boundary, never domain content.
/// Scrubbed from [`ChangeOp::Create::fields`] because they are not
/// representable as [`ChangeOp::SetField`].
const CREATE_STORAGE_KEYS: &[&str] = &[
    "id",
    "parent_id",
    "created_at",
    "updated_at",
    "sort_key",
    "after_block_id",
];

/// Where a change originated, so the inverse sync direction can skip echoes and
/// undo can correlate. Both fields are optional in Phase 2 (the live op tuples
/// don't carry them yet); later phases populate them at construction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Provenance {
    /// The command that produced this change, if known. Phase 5 moves
    /// `command_id` from the `Event` onto the intent record; this is its home.
    pub command_id: Option<String>,
    /// An opaque reference to the base the change was computed against (a
    /// frontier encoding, a content hash). Phase 3 unifies the doubled base
    /// mechanisms onto one store; this records which base a `ChangeSet` diffed.
    pub base_ref: Option<String>,
}

/// One typed intent. The block id is stored as the raw string the op carried
/// (schemed or bare) — normalization is the consolidator's job, not the
/// vocabulary's. `parent`/`parent_id` stay `String` in Phase 1 (they flip to
/// `EntityUri` with the live path in Phase 5); `after_sibling` is already
/// `EntityUri` so Phase 5 can dispatch directly.
#[derive(Clone, Debug, PartialEq)]
pub enum ChangeOp {
    /// Create a block under `parent_id`, positioned after `after_sibling`
    /// (a predecessor block id, not a sort_key). `fields` is every other
    /// create param (content, content_type, tags, properties, …).
    Create {
        id: String,
        parent_id: String,
        after_sibling: Option<EntityUri>,
        fields: BTreeMap<String, Value>,
    },
    /// Set a single non-structural field on an existing block.
    SetField {
        id: String,
        field: String,
        value: Value,
    },
    /// Move a block to `parent` (when the move changes its parent) positioned
    /// after `after_sibling`. `after_sibling` carries a predecessor block id
    /// (`Option<EntityUri>` — `None` = first child) in Phase 5. In the
    /// Phase 2 shadow decode it is populated from `after_block_id` params.
    Relocate {
        id: String,
        parent: Option<String>,
        after_sibling: Option<EntityUri>,
    },
    /// Delete a block.
    Delete { id: String },
}

impl ChangeOp {
    /// The source op-name this typed op re-encodes to (`create`/`update`/`delete`).
    /// `SetField` and `Relocate` both came from an `update`.
    pub fn source_op_name(&self) -> &'static str {
        match self {
            ChangeOp::Create { .. } => "create",
            ChangeOp::SetField { .. } | ChangeOp::Relocate { .. } => "update",
            ChangeOp::Delete { .. } => "delete",
        }
    }

    /// The block id this op targets.
    pub fn id(&self) -> &str {
        match self {
            ChangeOp::Create { id, .. }
            | ChangeOp::SetField { id, .. }
            | ChangeOp::Relocate { id, .. }
            | ChangeOp::Delete { id } => id,
        }
    }
}

/// A batch of typed intents with shared provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangeSet {
    pub ops: Vec<ChangeOp>,
    pub provenance: Provenance,
}

impl ChangeSet {
    /// Decode the live command-bus op tuples into the typed intent vocabulary.
    ///
    /// This is the Phase 2 shadow constructor: it does not change what the live
    /// path dispatches, it only re-expresses it. See the module docs for the
    /// normalization map.
    pub fn from_ops(ops: &[(String, StorageEntity)], provenance: Provenance) -> Self {
        let mut typed = Vec::new();
        for (op_name, params) in ops {
            match op_name.as_str() {
                "create" => typed.push(decode_create(params)),
                "update" => decode_update(params, &mut typed),
                "delete" => typed.push(ChangeOp::Delete { id: id_of(params) }),
                // Unknown op verbs are not part of the intent vocabulary. They
                // are reported as a divergence by `agrees_with_ops` (the
                // re-encoded multiset will be missing this verb) rather than
                // silently dropped.
                _ => {}
            }
        }
        Self {
            ops: typed,
            provenance,
        }
    }

    /// The op-name multiset this `ChangeSet` re-encodes to: each id that
    /// produced ≥1 `SetField`/`Relocate` re-encodes to a single `update`
    /// (matching how the source coalesces a block's field changes into one
    /// `update` op), every `Create`/`Delete` to itself.
    pub fn reencode_op_names(&self) -> BTreeMap<String, usize> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut update_ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for op in &self.ops {
            match op {
                ChangeOp::Create { .. } => *counts.entry("create".into()).or_default() += 1,
                ChangeOp::Delete { .. } => *counts.entry("delete".into()).or_default() += 1,
                ChangeOp::SetField { id, .. } | ChangeOp::Relocate { id, .. } => {
                    update_ids.insert(id.as_str());
                }
            }
        }
        if !update_ids.is_empty() {
            *counts.entry("update".into()).or_default() += update_ids.len();
        }
        counts
    }
}

/// The op-name multiset of the raw source ops, for comparison against
/// [`ChangeSet::reencode_op_names`].
pub fn source_op_names(ops: &[(String, StorageEntity)]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (op_name, _) in ops {
        *counts.entry(op_name.clone()).or_default() += 1;
    }
    counts
}

/// Whether the decoded `ChangeSet` agrees with the source ops under the
/// equivalence relation (op-name multiset equality after re-encode). Returns
/// `Ok(())` on agreement, or a human-readable divergence description.
pub fn agrees_with_ops(
    change_set: &ChangeSet,
    ops: &[(String, StorageEntity)],
) -> Result<(), String> {
    let source = source_op_names(ops);
    let reencoded = change_set.reencode_op_names();
    if source == reencoded {
        Ok(())
    } else {
        Err(format!(
            "op-multiset divergence: source={source:?} reencoded={reencoded:?}"
        ))
    }
}

fn id_of(params: &StorageEntity) -> String {
    params
        .get("id")
        .and_then(Value::as_string)
        .unwrap_or("<no-id>")
        .to_string()
}

fn decode_create(params: &StorageEntity) -> ChangeOp {
    let id = id_of(params);
    let parent_id = params
        .get("parent_id")
        .and_then(Value::as_string)
        .unwrap_or_default()
        .to_string();
    // Positional intent: a predecessor block id, never a sort_key.
    // Canonical key is holon::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM
    // = "after_block_id"; holon-api is a leaf crate so we inline the literal.
    let after_sibling = params
        .get("after_block_id")
        .and_then(Value::as_string)
        // ALLOW(entity_uri_from_raw): after_block_id arrives as a raw string in the StorageEntity operation-params map
        .map(|s| EntityUri::from_raw(s));
    // from_raw is infallible: bare strings → EntityUri::block(s),
    // schemed strings → parse; as_string already filtered non-String Values.
    let fields: BTreeMap<String, Value> = params
        .iter()
        .filter(|(k, _)| {
            let k: &str = k;
            !CREATE_STORAGE_KEYS.contains(&k)
        })
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    ChangeOp::Create {
        id,
        parent_id,
        after_sibling,
        fields,
    }
}

fn decode_update(params: &StorageEntity, out: &mut Vec<ChangeOp>) {
    let id = id_of(params);

    let parent_changed = params.contains_key("parent_id");
    // ALLOW(fallback): until the Phase 5 flip, structural moves emit `sort_key`
    // but not `after_block_id`; accepting both keys keeps Relocate firing for
    // indent/outdent/move.
    let order_changed = params.contains_key("after_block_id") || params.contains_key("sort_key");
    if parent_changed || order_changed {
        out.push(ChangeOp::Relocate {
            id: id.clone(),
            parent: params
                .get("parent_id")
                .and_then(Value::as_string)
                .map(str::to_string),
            after_sibling: params
                .get("after_block_id")
                .and_then(Value::as_string)
                // ALLOW(entity_uri_from_raw): after_block_id arrives as a raw string in the StorageEntity operation-params map
                .map(|s| EntityUri::from_raw(s)),
        });
    }

    for (field, value) in params {
        if UPDATE_BOOKKEEPING_FIELDS.contains(&field.as_ref())
            || RELOCATE_FIELDS.contains(&field.as_ref())
        {
            continue;
        }
        out.push(ChangeOp::SetField {
            id: id.clone(),
            field: field.to_string(),
            value: value.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    fn ops(items: Vec<(&str, Vec<(&str, Value)>)>) -> Vec<(String, StorageEntity)> {
        items
            .into_iter()
            .map(|(name, kvs)| {
                let map: StorageEntity = kvs.into_iter().map(|(k, v)| (k.into(), v)).collect();
                (name.to_string(), map)
            })
            .collect()
    }

    #[test]
    fn create_decodes_to_one_create_and_agrees() {
        let ops = ops(vec![(
            "create",
            vec![
                ("id", s("block:a")),
                ("parent_id", s("block:root")),
                ("content", s("hello")),
                ("after_block_id", s("block:predecessor")),
            ],
        )]);
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        assert_eq!(cs.ops.len(), 1);
        match &cs.ops[0] {
            ChangeOp::Create {
                id,
                parent_id,
                after_sibling,
                fields,
            } => {
                assert_eq!(id, "block:a");
                assert_eq!(parent_id, "block:root");
                assert_eq!(
                    after_sibling.as_ref().map(|u| u.to_string()),
                    Some("block:predecessor".into())
                );
                assert!(fields.contains_key("content"));
                assert!(!fields.contains_key("id"));
                assert!(!fields.contains_key("parent_id"));
                assert!(!fields.contains_key("sort_key"));
                assert!(!fields.contains_key("after_block_id"));
            }
            other => panic!("expected Create, got {other:?}"),
        }
        assert!(agrees_with_ops(&cs, &ops).is_ok());
    }

    #[test]
    fn update_with_content_only_decodes_to_setfield() {
        let ops = ops(vec![(
            "update",
            vec![
                ("id", s("block:a")),
                ("updated_at", Value::Integer(123)),
                ("content", s("changed")),
            ],
        )]);
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        assert_eq!(cs.ops.len(), 1);
        assert!(matches!(&cs.ops[0], ChangeOp::SetField { field, .. } if field == "content"));
        assert!(agrees_with_ops(&cs, &ops).is_ok());
    }

    #[test]
    fn update_with_parent_and_content_decodes_to_relocate_plus_setfield_one_update() {
        let ops = ops(vec![(
            "update",
            vec![
                ("id", s("block:a")),
                ("updated_at", Value::Integer(1)),
                ("parent_id", s("block:newparent")),
                ("sort_key", s("A1")),
                ("content", s("x")),
            ],
        )]);
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        // One Relocate + one SetField (content); sort_key/parent_id fold into Relocate.
        assert_eq!(cs.ops.len(), 2);
        assert!(cs
            .ops
            .iter()
            .any(|o| matches!(o, ChangeOp::Relocate { .. })));
        assert!(cs
            .ops
            .iter()
            .any(|o| matches!(o, ChangeOp::SetField { field, .. } if field == "content")));
        // Re-encodes to exactly one `update` for block:a.
        assert_eq!(*cs.reencode_op_names().get("update").unwrap(), 1);
        assert!(agrees_with_ops(&cs, &ops).is_ok());
    }

    #[test]
    fn update_with_only_bookkeeping_diverges() {
        // An `update` carrying only id + updated_at represents no field change —
        // the vocabulary produces zero ops, which the relation flags as a
        // divergence (it should never happen; the diff guard suppresses no-ops).
        let ops = ops(vec![(
            "update",
            vec![("id", s("block:a")), ("updated_at", Value::Integer(1))],
        )]);
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        assert!(cs.ops.is_empty());
        assert!(agrees_with_ops(&cs, &ops).is_err());
    }

    #[test]
    fn delete_decodes_and_agrees() {
        let ops = ops(vec![("delete", vec![("id", s("block:a"))])]);
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        assert_eq!(
            cs.ops,
            vec![ChangeOp::Delete {
                id: "block:a".into()
            }]
        );
        assert!(agrees_with_ops(&cs, &ops).is_ok());
    }

    #[test]
    fn mixed_batch_agrees() {
        let ops = ops(vec![
            (
                "create",
                vec![("id", s("block:a")), ("parent_id", s("block:root"))],
            ),
            ("update", vec![("id", s("block:b")), ("content", s("y"))]),
            ("delete", vec![("id", s("block:c"))]),
        ]);
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        assert!(agrees_with_ops(&cs, &ops).is_ok());
        let names = cs.reencode_op_names();
        assert_eq!(*names.get("create").unwrap(), 1);
        assert_eq!(*names.get("update").unwrap(), 1);
        assert_eq!(*names.get("delete").unwrap(), 1);
    }
}
