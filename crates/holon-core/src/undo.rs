//! Undo/Redo substrate: C-shaped, serializable, word-boundary-grouped history.
//!
//! An [`UndoEntry`] is a self-describing, serializable record of one reversible
//! step: the forward `ops`, their `inverse_ops`, the [`OpOrigin`] provenance, a
//! `group_id`, and two [`Precondition`] fingerprints (the state each direction
//! expects to find). Undo executes `inverse_ops`; redo re-executes `ops`; no
//! inverse is ever recomputed at replay time — it is stored.
//!
//! Grouping is **word-boundary** and content-based (ADR-ruled by Martin, NOT
//! focus-edge, NOT time-window): consecutive single-alphanumeric-character
//! typing edits on the SAME (entity, field) coalesce into the open group; a
//! non-alphanumeric character is included and CLOSES the group;
//! single-character deletions coalesce into their own runs; any structural op
//! is its own entry and closes the open group. Deterministic — no clocks.

use async_trait::async_trait;
use holon_api::OpOrigin;
use holon_api::Operation;
use holon_api::Value;
use serde::Deserialize;
use serde::Serialize;

use crate::traits::DeltaFingerprint;
use crate::traits::FieldDelta;

/// Read the current value of a projected (entity, field) so a stored
/// [`Precondition`] can be verified against live state at replay time.
/// Implemented in the `holon` crate over the replica's projection table.
#[async_trait]
pub trait UndoStateReader: Send + Sync {
    async fn field_value(&self, entity_id: &str, field: &str) -> anyhow::Result<Option<Value>>;
}

/// Persist the undo/redo history per replica DB so it survives a restart.
/// A single serialized snapshot of the whole [`UndoStack`] is the compacted
/// equivalent of a per-entry log; correctness (re-verified at replay via the
/// staleness policy) is what makes persistence safe.
#[async_trait]
pub trait UndoStore: Send + Sync {
    /// The last saved snapshot JSON, or `None` on a fresh replica.
    async fn load(&self) -> anyhow::Result<Option<String>>;
    /// Persist the current snapshot JSON with a monotonic sequence number.
    async fn save(&self, state_json: &str, seq: i64) -> anyhow::Result<()>;
}

/// Verify a [`Precondition`] against live state via `reader`. Returns
/// `Ok(None)` when the state still matches (safe to replay), or `Ok(Some(msg))`
/// naming the first divergent field (stale — the replay must be abandoned).
pub async fn verify_precondition(
    reader: &dyn UndoStateReader,
    precondition: &Precondition,
) -> anyhow::Result<Option<String>> {
    for fp in &precondition.fields {
        let current = reader.field_value(&fp.entity_id, &fp.field).await?;
        let matches = match &current {
            Some(v) => v == &fp.expected,
            None => matches!(fp.expected, Value::Null),
        };
        if !matches {
            return Ok(Some(format!(
                "state changed under undo: {}.{} expected {:?} but found {:?}",
                fp.entity_id, fp.field, fp.expected, current
            )));
        }
    }
    Ok(None)
}

/// The expected current value of one (entity, field) — one component of a
/// [`Precondition`] fingerprint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldFingerprint {
    pub entity_id: String,
    pub field: String,
    pub expected: Value,
}

/// A fingerprint of the state an inverse (or forward) replay was computed
/// against. Cheapest reliable field available at the dispatch layer: the exact
/// (entity, field) → value the step wrote, taken from the operation's
/// [`FieldDelta`]s. Staleness = current value diverges from the fingerprint.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Precondition {
    pub fields: Vec<FieldFingerprint>,
}

impl Precondition {
    /// Fingerprint of the *post-forward* state (what the ops wrote): checked
    /// before an undo replays the inverse.
    pub fn forward(changes: &[FieldDelta]) -> Self {
        Self {
            fields: changes
                .iter()
                .filter(|d| d.fingerprint == DeltaFingerprint::Readable)
                .map(|d| FieldFingerprint {
                    entity_id: d.entity_id.clone(),
                    field: d.field.clone(),
                    expected: d.new_value.clone(),
                })
                .collect(),
        }
    }

    /// Fingerprint of the *post-inverse* state (what an undo restores): checked
    /// before a redo replays the forward ops.
    pub fn inverse(changes: &[FieldDelta]) -> Self {
        Self {
            fields: changes
                .iter()
                .filter(|d| d.fingerprint == DeltaFingerprint::Readable)
                .map(|d| FieldFingerprint {
                    entity_id: d.entity_id.clone(),
                    field: d.field.clone(),
                    expected: d.old_value.clone(),
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// A single reversible history step. Serializable so it survives a restart and
/// is re-verified against live state (the same staleness policy) at replay.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UndoEntry {
    /// Forward operations (Vec so a future compound split/join is one entry;
    /// a single-op edit is a Vec-of-1). Redo re-executes these in order.
    pub ops: Vec<Operation>,
    /// Inverse operations. Undo executes these in order. Frozen at group open
    /// ("first pre-state wins") so one undo restores the pre-group state.
    pub inverse_ops: Vec<Operation>,
    /// Who caused this step. Only [`OpOrigin::User`] entries are ever stored.
    pub origin: OpOrigin,
    /// Coalescing group identity (stable across a coalesced run).
    pub group_id: u64,
    /// Post-forward fingerprint; advances as the group coalesces so external
    /// mutations (not our own in-group typing) are what trips staleness.
    pub precondition: Precondition,
    /// Post-inverse fingerprint; frozen at group open (redo staleness target).
    pub redo_precondition: Precondition,
}

impl UndoEntry {
    /// Human-readable label for the undo direction (UI).
    pub fn undo_display_name(&self) -> &str {
        self.inverse_ops
            .first()
            .map(|o| o.display_name.as_str())
            .unwrap_or("")
    }

    /// Human-readable label for the redo direction (UI).
    pub fn redo_display_name(&self) -> &str {
        self.ops
            .first()
            .map(|o| o.display_name.as_str())
            .unwrap_or("")
    }

    /// If this entry is a single-character text edit on one (entity, field),
    /// return `(entity_id, field, old_text, new_text)`. Only such entries
    /// participate in word-boundary coalescing.
    fn coalescible_edit(&self) -> Option<(String, String, String, String)> {
        if self.ops.len() != 1 || self.inverse_ops.len() != 1 {
            return None;
        }
        let fwd = &self.ops[0];
        let inv = &self.inverse_ops[0];
        if fwd.op_name != "set_field" {
            return None;
        }
        let entity_id = fwd.params.get("id").and_then(Value::as_string_owned)?;
        let field = fwd.params.get("field").and_then(Value::as_string_owned)?;
        let new_text = fwd.params.get("value").and_then(Value::as_string_owned)?;
        let old_text = inv.params.get("value").and_then(Value::as_string_owned)?;
        Some((entity_id, field, old_text, new_text))
    }
}

/// The mode of an open coalescing group.
#[derive(Clone, Copy, PartialEq, Debug)]
enum GroupMode {
    Typing,
    Deleting,
}

/// The single alphanumeric/other classification of an `old -> new` edit.
#[derive(Clone, Copy, PartialEq, Debug)]
enum EditDelta {
    /// Exactly one alphanumeric char inserted.
    InsertAlnum,
    /// Exactly one non-alphanumeric char inserted (closes the group).
    InsertOther,
    /// Exactly one char deleted.
    DeleteOne,
    /// Anything else (multi-char, paste, replacement): not coalescible.
    Other,
}

/// Classify the character delta between two strings.
fn classify_delta(old: &str, new: &str) -> EditDelta {
    let o: Vec<char> = old.chars().collect();
    let n: Vec<char> = new.chars().collect();
    let prefix = o.iter().zip(n.iter()).take_while(|(a, b)| a == b).count();
    if n.len() == o.len() + 1 {
        // one char inserted: common prefix + common suffix must cover `old`
        let suffix = o[prefix..]
            .iter()
            .rev()
            .zip(n[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if prefix + suffix == o.len() {
            let inserted = n[prefix];
            return if inserted.is_alphanumeric() {
                EditDelta::InsertAlnum
            } else {
                EditDelta::InsertOther
            };
        }
    } else if o.len() == n.len() + 1 {
        let suffix = n[prefix..]
            .iter()
            .rev()
            .zip(o[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if prefix + suffix == n.len() {
            return EditDelta::DeleteOne;
        }
    }
    EditDelta::Other
}

/// Grouping state for the top-of-stack open group.
#[derive(Clone, Debug)]
struct OpenGroup {
    key: (String, String),
    mode: GroupMode,
}

/// Undo/redo history stack of C-shaped [`UndoEntry`] records.
#[derive(Debug, Serialize, Deserialize)]
pub struct UndoStack {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    max_size: usize,
    next_group_id: u64,
    /// Transient grouping cursor; never serialized — a restart closes the open
    /// group so the first post-restart edit starts a fresh group.
    #[serde(skip)]
    open: Option<OpenGroup>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::with_max_size(100)
    }

    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            max_size,
            next_group_id: 0,
            open: None,
        }
    }

    /// Push a freshly-executed User entry, applying word-boundary grouping.
    ///
    /// Coalescing extends the open entry's `ops` and advances its forward
    /// precondition, keeping the ORIGINAL `inverse_ops` and `redo_precondition`
    /// so one undo restores the pre-group state. Any new push clears redo.
    pub fn push(&mut self, mut entry: UndoEntry) {
        self.redo.clear();

        if let Some((entity_id, field, old_text, new_text)) = entry.coalescible_edit() {
            let key = (entity_id, field);
            let delta = classify_delta(&old_text, &new_text);
            if let Some(open) = &self.open {
                if open.key == key {
                    match (open.mode, delta) {
                        (GroupMode::Typing, EditDelta::InsertAlnum) => {
                            return self.coalesce(entry, false);
                        }
                        (GroupMode::Typing, EditDelta::InsertOther) => {
                            // non-alnum char included, then group closes
                            return self.coalesce(entry, true);
                        }
                        (GroupMode::Deleting, EditDelta::DeleteOne) => {
                            return self.coalesce(entry, false);
                        }
                        _ => {}
                    }
                }
            }
            // Start a fresh group (the previous one, if any, is now closed).
            let group_id = self.fresh_group_id(&mut entry);
            let (mode, keep_open) = match delta {
                EditDelta::InsertAlnum => (Some(GroupMode::Typing), true),
                EditDelta::DeleteOne => (Some(GroupMode::Deleting), true),
                // A lone non-alnum insert (or non-coalescible edit) is its own
                // closed entry.
                _ => (None, false),
            };
            let _ = group_id;
            self.append(entry);
            self.open = if keep_open {
                mode.map(|m| OpenGroup { key, mode: m })
            } else {
                None
            };
            return;
        }

        // Structural / multi-field / non-text op: its own entry; closes group.
        self.fresh_group_id(&mut entry);
        self.append(entry);
        self.open = None;
    }

    /// Assign a fresh group id to `entry`, returning it.
    fn fresh_group_id(&mut self, entry: &mut UndoEntry) -> u64 {
        let id = self.next_group_id;
        self.next_group_id += 1;
        entry.group_id = id;
        id
    }

    /// Append a distinct entry, trimming to `max_size`.
    fn append(&mut self, entry: UndoEntry) {
        self.undo.push(entry);
        if self.undo.len() > self.max_size {
            self.undo.remove(0);
        }
    }

    /// Fold `entry` into the open (top) group: extend ops, advance the forward
    /// precondition; keep the original inverse + redo precondition. `close`
    /// marks the group closed afterwards (non-alnum boundary).
    fn coalesce(&mut self, entry: UndoEntry, close: bool) {
        let top = self
            .undo
            .last_mut()
            .expect("open group implies a top undo entry");
        top.ops.extend(entry.ops);
        top.precondition = entry.precondition;
        if close {
            self.open = None;
        }
    }

    /// Peek the entry that an undo would target (without removing it).
    pub fn peek_undo(&self) -> Option<&UndoEntry> {
        self.undo.last()
    }

    /// Peek the entry that a redo would target.
    pub fn peek_redo(&self) -> Option<&UndoEntry> {
        self.redo.last()
    }

    /// Remove the top undo entry after a *successful* inverse replay, moving it
    /// to the redo stack. Any open group is closed (its history point is
    /// spent).
    pub fn commit_undo(&mut self) -> Option<UndoEntry> {
        self.open = None;
        let entry = self.undo.pop()?;
        self.redo.push(entry.clone());
        Some(entry)
    }

    /// Drop the top undo entry WITHOUT moving it to redo — a stale entry whose
    /// inverse must not run. Any open group is closed.
    pub fn drop_undo(&mut self) -> Option<UndoEntry> {
        self.open = None;
        self.undo.pop()
    }

    /// Move the top redo entry back to the undo stack after a successful
    /// forward replay.
    pub fn commit_redo(&mut self) -> Option<UndoEntry> {
        self.open = None;
        let entry = self.redo.pop()?;
        self.undo.push(entry.clone());
        Some(entry)
    }

    /// Drop the top redo entry (stale) without re-applying.
    pub fn drop_redo(&mut self) -> Option<UndoEntry> {
        self.redo.pop()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn next_undo_display_name(&self) -> Option<&str> {
        self.undo.last().map(UndoEntry::undo_display_name)
    }

    pub fn next_redo_display_name(&self) -> Option<&str> {
        self.redo.last().map(UndoEntry::redo_display_name)
    }

    /// Number of distinct undo entries (a coalesced group counts once).
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A `HistoryOnly` delta (edge/junction field, e.g. `tags`) must be
    /// EXCLUDED from both preconditions — otherwise the `SqlUndoStateReader`
    /// would generate an invalid `SELECT tags FROM block_raw` (no such column).
    /// A `Readable` delta on the same batch is still fingerprinted.
    #[test]
    fn history_only_deltas_are_excluded_from_preconditions() {
        let changes = vec![
            FieldDelta::history_only(
                "block:x",
                "tags",
                Value::Null,
                Value::String("todo".to_string()),
            ),
            FieldDelta::new(
                "block:x",
                "content",
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ),
        ];
        let fwd = Precondition::forward(&changes);
        let inv = Precondition::inverse(&changes);
        assert_eq!(fwd.fields.len(), 1, "only the Readable content delta");
        assert_eq!(fwd.fields[0].field, "content");
        assert_eq!(fwd.fields[0].expected, Value::String("b".to_string()));
        assert_eq!(inv.fields.len(), 1);
        assert_eq!(inv.fields[0].expected, Value::String("a".to_string()));

        // A batch of only history_only deltas yields an empty precondition.
        let only_edge = vec![FieldDelta::history_only(
            "block:x",
            "tags",
            Value::Null,
            Value::String("todo".to_string()),
        )];
        assert!(Precondition::forward(&only_edge).is_empty());
        assert!(Precondition::inverse(&only_edge).is_empty());
    }

    fn set_field_op(id: &str, field: &str, value: &str) -> Operation {
        let mut p = HashMap::new();
        p.insert("id".to_string(), Value::String(id.to_string()));
        p.insert("field".to_string(), Value::String(field.to_string()));
        p.insert("value".to_string(), Value::String(value.to_string()));
        Operation::new("block", "set_field", "Edit", p)
    }

    /// Build a set_field User entry from old -> new content on one block/field.
    fn edit_entry(id: &str, field: &str, old: &str, new: &str) -> UndoEntry {
        let changes = vec![FieldDelta::new(
            id,
            field,
            Value::String(old.to_string()),
            Value::String(new.to_string()),
        )];
        UndoEntry {
            ops: vec![set_field_op(id, field, new)],
            inverse_ops: vec![set_field_op(id, field, old)],
            origin: OpOrigin::User,
            group_id: 0,
            precondition: Precondition::forward(&changes),
            redo_precondition: Precondition::inverse(&changes),
        }
    }

    fn structural_entry(id: &str) -> UndoEntry {
        let mut p = HashMap::new();
        p.insert("id".to_string(), Value::String(id.to_string()));
        UndoEntry {
            ops: vec![Operation::new("block", "indent", "Indent", p.clone())],
            inverse_ops: vec![Operation::new("block", "outdent", "Outdent", p)],
            origin: OpOrigin::User,
            group_id: 0,
            precondition: Precondition::default(),
            redo_precondition: Precondition::default(),
        }
    }

    /// Type a string one alnum char at a time as consecutive set_field entries.
    fn type_string(stack: &mut UndoStack, id: &str, field: &str, text: &str) {
        let mut acc = String::new();
        for ch in text.chars() {
            let old = acc.clone();
            acc.push(ch);
            stack.push(edit_entry(id, field, &old, &acc));
        }
    }

    #[test]
    fn typing_a_word_coalesces_into_one_group() {
        let mut stack = UndoStack::new();
        type_string(&mut stack, "b1", "content", "hello");
        assert_eq!(stack.undo_len(), 1, "hello = one group");
        // The single group's inverse restores the pre-group (empty) state.
        let entry = stack.peek_undo().unwrap();
        assert_eq!(entry.inverse_ops.len(), 1);
        assert_eq!(
            entry.inverse_ops[0]
                .params
                .get("value")
                .unwrap()
                .as_string_owned(),
            Some(String::new())
        );
        // Forward precondition advanced to the final content.
        assert_eq!(
            entry.precondition.fields[0].expected.as_string_owned(),
            Some("hello".to_string())
        );
    }

    #[test]
    fn space_closes_the_group_at_the_word_boundary() {
        let mut stack = UndoStack::new();
        // "hello world": space after "hello" closes group 1; "world" is group 2.
        type_string(&mut stack, "b1", "content", "hello");
        stack.push(edit_entry("b1", "content", "hello", "hello ")); // space closes
        type_string_from(&mut stack, "b1", "content", "hello ", "world");
        assert_eq!(
            stack.undo_len(),
            2,
            "two words -> two groups split at space"
        );
    }

    /// Continue typing appended chars starting from an existing accumulator.
    fn type_string_from(stack: &mut UndoStack, id: &str, field: &str, base: &str, text: &str) {
        let mut acc = base.to_string();
        for ch in text.chars() {
            let old = acc.clone();
            acc.push(ch);
            stack.push(edit_entry(id, field, &old, &acc));
        }
    }

    #[test]
    fn backspace_run_is_its_own_group() {
        let mut stack = UndoStack::new();
        type_string(&mut stack, "b1", "content", "abc"); // group 1 (typing)
        // delete c, b, a — one deletion run = one group
        stack.push(edit_entry("b1", "content", "abc", "ab"));
        stack.push(edit_entry("b1", "content", "ab", "a"));
        stack.push(edit_entry("b1", "content", "a", ""));
        assert_eq!(stack.undo_len(), 2, "typing group + deletion group");
    }

    #[test]
    fn structural_op_closes_the_group() {
        let mut stack = UndoStack::new();
        type_string(&mut stack, "b1", "content", "ab");
        stack.push(structural_entry("b1")); // closes typing group, own entry
        type_string(&mut stack, "b1", "content", "cd"); // NEW group, not coalesced
        assert_eq!(stack.undo_len(), 3);
    }

    #[test]
    fn different_block_does_not_coalesce() {
        let mut stack = UndoStack::new();
        stack.push(edit_entry("b1", "content", "", "a"));
        stack.push(edit_entry("b2", "content", "", "b"));
        assert_eq!(stack.undo_len(), 2);
    }

    #[test]
    fn roundtrip_serialization_preserves_entries() {
        let mut stack = UndoStack::new();
        type_string(&mut stack, "b1", "content", "hi");
        stack.push(structural_entry("b1"));
        let json = serde_json::to_string(&stack).unwrap();
        let restored: UndoStack = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.undo_len(), 2);
        // After restore the open group is closed: a new alnum edit is its own group.
        let mut restored = restored;
        restored.push(edit_entry("b1", "content", "hi", "hix"));
        assert_eq!(restored.undo_len(), 3);
    }

    #[test]
    fn classify_delta_cases() {
        assert_eq!(classify_delta("ab", "abc"), EditDelta::InsertAlnum);
        assert_eq!(classify_delta("ab", "ab "), EditDelta::InsertOther);
        assert_eq!(classify_delta("abc", "ab"), EditDelta::DeleteOne);
        assert_eq!(classify_delta("abc", "axc"), EditDelta::Other);
        assert_eq!(classify_delta("ab", "abcd"), EditDelta::Other);
    }
}
