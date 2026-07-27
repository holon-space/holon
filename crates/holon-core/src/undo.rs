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

/// Merge per-`(entity, field)` fingerprints across a composite group's buffered
/// entries into one [`Precondition`], excluding derived positional columns
/// ([`is_derived_positional_field`]). `last_wins=true` keeps the LAST
/// contributor per field (the post-group forward precondition, checked before
/// undo); `false` keeps the FIRST (the pre-group redo precondition, checked
/// before redo). Output is deterministic (sorted by `(entity_id, field)`).
fn merge_fingerprints<'a>(
    preconditions: impl Iterator<Item = &'a Precondition>,
    last_wins: bool,
) -> Precondition {
    use std::collections::BTreeMap;
    use std::collections::btree_map::Entry;

    let mut by_key: BTreeMap<(String, String), Value> = BTreeMap::new();
    for pre in preconditions {
        for fp in &pre.fields {
            if is_derived_positional_field(&fp.field) {
                continue;
            }
            match by_key.entry((fp.entity_id.clone(), fp.field.clone())) {
                Entry::Vacant(v) => {
                    v.insert(fp.expected.clone());
                }
                Entry::Occupied(mut o) => {
                    if last_wins {
                        o.insert(fp.expected.clone());
                    }
                }
            }
        }
    }
    Precondition {
        fields: by_key
            .into_iter()
            .map(|((entity_id, field), expected)| FieldFingerprint {
                entity_id,
                field,
                expected,
            })
            .collect(),
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

/// An open composite-undo transaction: while present, [`UndoStack::push`]
/// buffers pushed entries here instead of appending them, so `begin_group` …
/// `end_group` collapses N sub-ops into ONE composite [`UndoEntry`]. `depth`
/// tracks nesting: nested `begin_group` deepens it and only the OUTERMOST
/// `end_group` materializes the composite (flatten semantics — a composite is a
/// flat entry, and the product law is one gesture ⇒ one undo).
#[derive(Debug, Clone)]
struct GroupBuffer {
    depth: u32,
    entries: Vec<UndoEntry>,
}

/// Whether `field` is a DERIVED positional column (`depth`, `sort_key`) that
/// structural ops RECOMPUTE from the live tree rather than restore to a
/// captured value — so its post-replay value is a function of the current
/// parent chain, not the pre-op value. Fingerprinting it makes a legitimate
/// undo→redo trip spuriously "stale". Excluded from every composite
/// [`Precondition`] here; the engine-level `convert_block_to_page` compound
/// applies the SAME rule to its hand-assembled entry (single-sourced so the two
/// cannot drift).
pub fn is_derived_positional_field(field: &str) -> bool {
    field == "depth" || field == "sort_key"
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
    /// Transient composite-group buffer; never serialized — a restart abandons
    /// any in-flight group (its sub-op writes already landed; only the undo
    /// bookkeeping is dropped, exactly as a crash mid-instantiation would).
    #[serde(skip)]
    group: Option<GroupBuffer>,
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
            group: None,
        }
    }

    /// Push a freshly-executed User entry, applying word-boundary grouping.
    ///
    /// Coalescing extends the open entry's `ops` and advances its forward
    /// precondition, keeping the ORIGINAL `inverse_ops` and `redo_precondition`
    /// so one undo restores the pre-group state. Any new push clears redo.
    pub fn push(&mut self, mut entry: UndoEntry) {
        self.redo.clear();

        // Inside an open composite group: buffer raw, in execution order. No
        // word-boundary coalescing applies (the whole group is one gesture); the
        // buffered entries are folded into ONE composite entry at `end_group`.
        if let Some(group) = self.group.as_mut() {
            group.entries.push(entry);
            return;
        }

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

    /// Open a composite-undo group. Every User-origin [`push`](Self::push)
    /// until the matching [`end_group`](Self::end_group) is buffered and
    /// folded into ONE composite [`UndoEntry`], so a multi-op operation
    /// (e.g. template instantiation) is ONE undo gesture. Nestable: a
    /// nested `begin_group` just deepens the counter (flatten — see
    /// [`GroupBuffer`]). Opening a group closes any open word-boundary
    /// typing run (a structural boundary).
    pub fn begin_group(&mut self) {
        match self.group.as_mut() {
            Some(group) => group.depth += 1,
            None => {
                self.open = None;
                self.group = Some(GroupBuffer {
                    depth: 1,
                    entries: Vec::new(),
                });
            }
        }
    }

    /// Close the innermost composite-undo group. Loud on imbalance (an
    /// `end_group` with no open group is a programming error, never silent). At
    /// depth 0 the buffered sub-ops materialize as ONE composite entry: forward
    /// `ops` concatenated in execution order (redo replays forward); inverse
    /// `ops` concatenated in REVERSE entry order with each entry's internal
    /// inverse order preserved (undo replays leaf-first / FK-safe). An empty
    /// group (no User pushes) materializes nothing.
    pub fn end_group(&mut self) {
        let group = self
            .group
            .as_mut()
            .expect("end_group without a matching begin_group (unbalanced undo group)");
        assert!(group.depth > 0, "undo group depth underflow");
        group.depth -= 1;
        if group.depth > 0 {
            return; // inner close: flatten, keep buffering
        }
        let entries = self.group.take().expect("group present above").entries;
        if entries.is_empty() {
            return;
        }
        let mut composite = Self::compose(entries);
        self.fresh_group_id(&mut composite);
        self.append(composite);
        self.open = None;
    }

    /// Fold N buffered single-step entries into ONE composite [`UndoEntry`].
    ///
    /// - `ops`: every entry's forward ops, in execution order (redo replays
    ///   forward).
    /// - `inverse_ops`: entries in REVERSE order, each entry's own inverse
    ///   order preserved (leaf-first / FK-safe — a create-then-child group
    ///   undoes child-then-parent).
    /// - `precondition` (checked BEFORE undo ⇒ must equal the POST-group
    ///   state): per (entity, field) the LAST writer's forward fingerprint
    ///   wins.
    /// - `redo_precondition` (checked BEFORE redo ⇒ must equal the PRE-group
    ///   state): per (entity, field) the FIRST writer's inverse fingerprint
    ///   wins. Derived positional columns (`depth`/`sort_key`) are excluded
    ///   from both (see [`is_derived_positional_field`]).
    fn compose(entries: Vec<UndoEntry>) -> UndoEntry {
        let mut ops: Vec<Operation> = Vec::new();
        for e in &entries {
            ops.extend(e.ops.iter().cloned());
        }
        let mut inverse_ops: Vec<Operation> = Vec::new();
        for e in entries.iter().rev() {
            inverse_ops.extend(e.inverse_ops.iter().cloned());
        }
        UndoEntry {
            ops,
            inverse_ops,
            origin: OpOrigin::User,
            group_id: 0,
            precondition: merge_fingerprints(entries.iter().map(|e| &e.precondition), true),
            redo_precondition: merge_fingerprints(
                entries.iter().map(|e| &e.redo_precondition),
                false,
            ),
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

    // ── Composite-undo grouping (Inc1) ──────────────────────────────────

    /// A create-shaped entry: forward `create{id}`, inverse `delete{id}`.
    fn create_entry(id: &str) -> UndoEntry {
        let mut p = HashMap::new();
        p.insert("id".to_string(), Value::String(id.to_string()));
        UndoEntry {
            ops: vec![Operation::new("block", "create", "Create", p.clone())],
            inverse_ops: vec![Operation::new("block", "delete", "Delete", p)],
            origin: OpOrigin::User,
            group_id: 0,
            precondition: Precondition::default(),
            redo_precondition: Precondition::default(),
        }
    }

    /// The `id` param of each op, in order.
    fn ids_of(ops: &[Operation]) -> Vec<String> {
        ops.iter()
            .map(|o| {
                o.params
                    .get("id")
                    .and_then(Value::as_string_owned)
                    .expect("op has an id param")
            })
            .collect()
    }

    fn op_names(ops: &[Operation]) -> Vec<String> {
        ops.iter().map(|o| o.op_name.clone()).collect()
    }

    #[test]
    fn group_of_n_creates_is_one_composite_entry_inverse_reversed() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.push(create_entry("block:a"));
        stack.push(create_entry("block:b"));
        stack.push(create_entry("block:c"));
        stack.end_group();

        assert_eq!(stack.undo_len(), 1, "the whole group is ONE undo entry");
        let e = stack.peek_undo().unwrap();
        // Forward ops in execution order (redo replays forward).
        assert_eq!(op_names(&e.ops), vec!["create", "create", "create"]);
        assert_eq!(ids_of(&e.ops), vec!["block:a", "block:b", "block:c"]);
        // Inverse ops reversed (leaf-first / FK-safe: delete c, b, a).
        assert_eq!(op_names(&e.inverse_ops), vec!["delete", "delete", "delete"]);
        assert_eq!(
            ids_of(&e.inverse_ops),
            vec!["block:c", "block:b", "block:a"]
        );
    }

    /// Amendment 1: when two ops touch the SAME (entity, field), the
    /// composite's forward `precondition` (checked before undo ⇒ POST-group
    /// state) keeps the LAST writer's new value, and its
    /// `redo_precondition` (checked before redo ⇒ PRE-group state) keeps
    /// the FIRST writer's old value.
    #[test]
    fn composite_precondition_merges_first_pre_and_last_post() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.push(edit_entry("b1", "content", "A", "B")); // A -> B
        stack.push(edit_entry("b1", "content", "B", "C")); // B -> C
        stack.end_group();

        assert_eq!(stack.undo_len(), 1);
        let e = stack.peek_undo().unwrap();

        // Forward precondition = LAST op's post-state (C).
        assert_eq!(e.precondition.fields.len(), 1, "one merged field");
        assert_eq!(e.precondition.fields[0].entity_id, "b1");
        assert_eq!(e.precondition.fields[0].field, "content");
        assert_eq!(
            e.precondition.fields[0].expected.as_string_owned(),
            Some("C".to_string()),
            "precondition = last-post"
        );

        // Redo precondition = FIRST op's pre-state (A).
        assert_eq!(e.redo_precondition.fields.len(), 1);
        assert_eq!(
            e.redo_precondition.fields[0].expected.as_string_owned(),
            Some("A".to_string()),
            "redo_precondition = first-pre"
        );

        // The two set_field forwards survive in order; inverses reversed.
        assert_eq!(e.ops.len(), 2);
        assert_eq!(e.inverse_ops.len(), 2);
    }

    /// Derived positional columns never enter a composite precondition (they
    /// are recomputed from the live tree, not restored).
    #[test]
    fn composite_precondition_excludes_depth_and_sort_key() {
        assert!(is_derived_positional_field("depth"));
        assert!(is_derived_positional_field("sort_key"));
        assert!(!is_derived_positional_field("content"));

        let with_depth = |id: &str, field: &str, val: &str| {
            let changes = vec![FieldDelta::new(
                id,
                field,
                Value::Null,
                Value::String(val.to_string()),
            )];
            UndoEntry {
                ops: vec![set_field_op(id, field, val)],
                inverse_ops: vec![set_field_op(id, field, "")],
                origin: OpOrigin::User,
                group_id: 0,
                precondition: Precondition::forward(&changes),
                redo_precondition: Precondition::inverse(&changes),
            }
        };
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.push(with_depth("b1", "content", "x"));
        stack.push(with_depth("b1", "depth", "3"));
        stack.push(with_depth("b1", "sort_key", "A0"));
        stack.end_group();

        let e = stack.peek_undo().unwrap();
        assert_eq!(
            e.precondition.fields.len(),
            1,
            "only the content field is fingerprinted; depth/sort_key excluded"
        );
        assert_eq!(e.precondition.fields[0].field, "content");
    }

    #[test]
    fn nested_begin_end_flattens_into_one_entry() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.push(create_entry("block:a"));
        stack.begin_group(); // nested: just deepens
        stack.push(create_entry("block:b"));
        stack.end_group(); // inner close: still buffering
        assert_eq!(stack.undo_len(), 0, "inner end does not materialize");
        stack.push(create_entry("block:c"));
        stack.end_group(); // outer close: materialize ONE entry

        assert_eq!(stack.undo_len(), 1);
        let e = stack.peek_undo().unwrap();
        assert_eq!(ids_of(&e.ops), vec!["block:a", "block:b", "block:c"]);
        assert_eq!(
            ids_of(&e.inverse_ops),
            vec!["block:c", "block:b", "block:a"]
        );
    }

    #[test]
    fn empty_group_materializes_nothing() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.end_group();
        assert_eq!(stack.undo_len(), 0);
    }

    #[test]
    #[should_panic(expected = "end_group without a matching begin_group")]
    fn end_group_without_begin_is_loud() {
        let mut stack = UndoStack::new();
        stack.end_group();
    }

    #[test]
    fn a_group_boundary_closes_the_open_typing_run() {
        let mut stack = UndoStack::new();
        type_string(&mut stack, "b1", "content", "ab"); // one typing group
        assert_eq!(stack.undo_len(), 1);

        stack.begin_group(); // closes the typing run
        stack.push(create_entry("block:x"));
        stack.end_group();
        assert_eq!(stack.undo_len(), 2);

        // A further alnum edit must NOT coalesce back into the pre-group typing
        // run — the boundary closed it.
        stack.push(edit_entry("b1", "content", "ab", "abc"));
        assert_eq!(stack.undo_len(), 3);
    }

    #[test]
    fn composite_entry_survives_serialization_roundtrip() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.push(create_entry("block:a"));
        stack.push(create_entry("block:b"));
        stack.end_group();

        let json = serde_json::to_string(&stack).unwrap();
        let restored: UndoStack = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.undo_len(), 1);
        let e = restored.peek_undo().unwrap();
        assert_eq!(ids_of(&e.ops), vec!["block:a", "block:b"]);
        assert_eq!(ids_of(&e.inverse_ops), vec!["block:b", "block:a"]);
    }

    /// An in-flight (open) group is transient: it is never serialized, so a
    /// restart abandons its buffered entries (their writes already landed; only
    /// the undo bookkeeping is dropped).
    #[test]
    fn an_open_group_is_not_serialized() {
        let mut stack = UndoStack::new();
        stack.begin_group();
        stack.push(create_entry("block:a")); // buffered, not yet materialized
        assert_eq!(stack.undo_len(), 0);

        let json = serde_json::to_string(&stack).unwrap();
        let mut restored: UndoStack = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.undo_len(), 0, "buffered group entry not persisted");
        // No open group after restore: the next push is its own normal entry.
        restored.push(create_entry("block:b"));
        assert_eq!(restored.undo_len(), 1);
    }
}
