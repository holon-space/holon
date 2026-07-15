//! Widget specification types for backend-driven UI
//!
//! WidgetSpec is the unified return type for all rendered widgets.
//! The backend executes queries and returns both the render spec and data.
//! The frontend just renders using RenderInterpreter.

use std::collections::HashMap;

use crate::streaming::Change;
use crate::EntityUri;
use crate::Value;

/// A single row of query result data (may or may not be enriched).
pub type DataRow = HashMap<String, Value>;

/// Parse the canonical `EntityUri` out of a matview/CDC `DataRow`'s `"id"`
/// column. This is the single typed-id boundary for the reactive row
/// pipeline: the row's `"id"` is the stringly-typed matview representation,
/// and every downstream consumer must thread the resulting `EntityUri` rather
/// than re-parsing the string. Returns `None` when the row has no `"id"`.
pub fn data_row_entity_uri(row: &DataRow) -> Option<EntityUri> {
    row.get("id")
        .and_then(|v| v.as_string())
        .map(entity_uri_from_id_str)
}

/// Typed accessor for the matview `parent_id` column of a row.
///
/// Boundary read — routes through the centralized [`entity_uri_from_id_str`]
/// helper so the column name and the bare-vs-schemed canonicalisation live in
/// one place. Empty / missing → `None`.
pub fn data_row_parent_id(row: &DataRow) -> Option<EntityUri> {
    row.get("parent_id")
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .map(entity_uri_from_id_str)
}

/// Typed accessor for the sibling-ordering key of a row.
///
/// `sort_key` (the Loro/Turso fractional index) is the authority: it can
/// represent an insert *between* two siblings, which the legacy integer
/// `sequence` cannot. `split_block` gives its new block a fractional `sort_key`
/// between the source and the next sibling but no `sequence` — preferring
/// `sequence` here sorted that new block to the end (its absent `sequence`
/// lost to the existing 0,1,2…). Prefer `sort_key`, falling back to `sequence`
/// only for rows that predate the fractional index.
pub fn data_row_sort_key(row: &DataRow) -> String {
    let v = row.get("sort_key").or_else(|| row.get("sequence"));
    crate::render_eval::sort_value(v)
}

/// Parse a boundary id string (matview row id column, CDC `Change` id /
/// `entity_id`) into the canonical `EntityUri`. The single `from_raw` seam
/// for the reactive row pipeline.
pub fn entity_uri_from_id_str(id: &str) -> EntityUri {
    // ALLOW(entity_uri_from_raw): matview/CDC row id column is the typed-id
    // boundary
    EntityUri::from_raw(id)
}

/// Deterministic content hash of a value-shaped row.
///
/// Stable across incremental matview recompute: a matview `_rowid` is NOT
/// guaranteed stable when the view is recomputed, but the row's column/value
/// content is. Same content → same hash → same identity → no spurious
/// re-render churn (Martin ruling 2026-07-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowContentHash(u64);

impl RowContentHash {
    /// FNV-1a over the canonical (sorted-key, recursively-canonicalized)
    /// serialization of the row. A fixed algorithm (not `DefaultHasher`, whose
    /// output is not guaranteed stable across std versions) so the identity is
    /// reproducible.
    ///
    /// Generic over the key type so both `DataRow` (`String` keys) and
    /// `StorageEntity` (`Arc<str>` keys) hash to the SAME identity for equal
    /// content — the prod row set and the keystone twin agree.
    pub fn of_row<K>(row: &HashMap<K, Value>) -> Self
    where
        K: std::borrow::Borrow<str> + std::hash::Hash + Eq,
    {
        let mut hasher = Fnv1a::new();
        let mut keys: Vec<&str> = row.keys().map(|k| k.borrow()).collect();
        keys.sort_unstable();
        for k in keys {
            hasher.write(k.as_bytes());
            hasher.write(&[0x1f]); // unit separator: key/value boundary
            canonicalize_value(&row[k], &mut hasher);
            hasher.write(&[0x1e]); // record separator: end of column
        }
        Self(hasher.finish())
    }

    /// Lowercase hex serialization (the `value:` scheme's path component).
    pub fn to_hex(self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Fold a `Value` into the hasher in a canonical, key-order-independent way.
fn canonicalize_value(v: &Value, hasher: &mut Fnv1a) {
    match v {
        Value::String(s) => {
            hasher.write(b"s");
            hasher.write(s.as_bytes());
        }
        Value::Integer(i) => {
            hasher.write(b"i");
            hasher.write(&i.to_le_bytes());
        }
        Value::Float(f) => {
            hasher.write(b"f");
            hasher.write(&f.to_bits().to_le_bytes());
        }
        Value::Boolean(b) => {
            hasher.write(b"b");
            hasher.write(&[*b as u8]);
        }
        Value::DateTime(s) => {
            hasher.write(b"d");
            hasher.write(s.as_bytes());
        }
        Value::Json(s) => {
            hasher.write(b"j");
            hasher.write(s.as_bytes());
        }
        Value::Array(items) => {
            hasher.write(b"a");
            for it in items {
                canonicalize_value(it, hasher);
                hasher.write(&[0x1d]);
            }
        }
        Value::Object(map) => {
            hasher.write(b"o");
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                hasher.write(k.as_bytes());
                hasher.write(&[0x1f]);
                canonicalize_value(&map[k], hasher);
                hasher.write(&[0x1d]);
            }
        }
        Value::Null => hasher.write(b"n"),
    }
}

/// Minimal FNV-1a 64-bit hasher — a fixed, dependency-free algorithm so the
/// content hash is deterministic and stable across builds.
struct Fnv1a(u64);

impl Fnv1a {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// Identity of a query result row at the reactive keying layer.
///
/// Every row is one of two shapes (Martin ruling 2026-07-11), and the shape is
/// a legitimate display case — not an error:
///
/// - [`RowIdentity::Entity`] — the row carries a real, entity-shaped `id`
///   column. It resolves to an entity profile / entity templates, and
///   entity-id-dependent interactions (click-to-navigate) are meaningful.
/// - [`RowIdentity::Value`] — a synthetic-identity row (an aggregate, a
///   rule-trigger result, a future table row). It has NO entity to resolve;
///   identity is the deterministic [`RowContentHash`] of its content, so the
///   same row keeps the same identity across incremental matview recompute. It
///   renders as a plain value row.
///
/// **Parse, don't validate**: this enum is the MODEL. The `value:` (and, for
/// entities, `block:`/`doc:`/…) URI *scheme* is only how the identity is
/// *serialized* into the `EntityUri` used as the row-store key — it is not the
/// model. Downstream that reasons about identity matches on the enum.
///
/// **Collision**: two value rows with byte-identical content share one
/// identity and collapse to a single store entry. This is an inherent,
/// documented limitation of content-hash identity (distinct content → distinct
/// identity). A future refinement can disambiguate identical rows by an
/// occurrence index within a batch; today identical value rows collapse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowIdentity {
    Entity(EntityUri),
    Value(RowContentHash),
}

impl RowIdentity {
    /// Classify a row into its identity shape: a row carrying a non-empty
    /// string `id` column is entity-shaped, keyed through
    /// [`entity_uri_from_id_str`] — the pipeline's `from_raw` boundary that
    /// normalizes bare ids like `b` to `block:b`, the SAME normalization the
    /// `Updated`/`Deleted`/`FieldsChanged` CDC arms apply to their id strings,
    /// so `Created` and `Updated` agree on the store key. A row with no usable
    /// `id` is value-shaped, keyed on its content hash. (Deliberately NOT the
    /// profile resolver's strict [`crate::row_id`] parse: that rejects bare
    /// ids, which the CDC pipeline legitimately carries.)
    pub fn of_row<K>(row: &HashMap<K, Value>) -> Self
    where
        K: std::borrow::Borrow<str> + std::hash::Hash + Eq,
    {
        match row
            .get("id")
            .and_then(|v| v.as_string())
            .filter(|s| !s.is_empty())
        {
            Some(id) => RowIdentity::Entity(entity_uri_from_id_str(id)),
            None => RowIdentity::Value(RowContentHash::of_row(row)),
        }
    }

    /// True for value-shaped rows (no resolvable entity).
    pub fn is_value(&self) -> bool {
        matches!(self, RowIdentity::Value(_))
    }

    /// Serialize the identity into the `EntityUri` used as the row-store key.
    /// Entity rows key on their own URI; value rows key on the `value:` scheme
    /// carrying the hex content hash. This is the serialization boundary — the
    /// enum is the model, the scheme is its wire form.
    pub fn to_store_key(&self) -> EntityUri {
        match self {
            RowIdentity::Entity(uri) => uri.clone(),
            RowIdentity::Value(hash) => EntityUri::new("value", &hash.to_hex()),
        }
    }
}

/// A row that has been through the enrichment pipeline (`flatten_properties` +
/// computed fields from entity profile resolution).
///
/// **Parse, don't validate**: The only way to obtain an `EnrichedRow` is
/// through the enrichment pipeline — there is no public constructor.  This
/// makes it a compile error to feed raw storage data into the reactive
/// pipeline.
///
/// `Deref<Target = HashMap>` lets read-only code (`.get("task_state")`, etc.)
/// work unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct EnrichedRow(HashMap<String, Value>);

impl std::ops::Deref for EnrichedRow {
    type Target = HashMap<String, Value>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for EnrichedRow {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl EnrichedRow {
    /// Enrich a raw storage row: flatten `properties` JSON to top-level keys
    /// and inject caller-provided computed fields.
    ///
    /// This is the **only** way to create an `EnrichedRow`.  The
    /// `computed_fields` closure receives the flattened row and returns
    /// additional key-value pairs (typically from entity profile
    /// resolution).
    pub fn from_raw(
        data: HashMap<String, Value>,
        computed_fields: impl FnOnce(&HashMap<String, Value>) -> HashMap<String, Value>,
    ) -> Self {
        let mut row = Self::flatten_properties(data);
        for (key, value) in computed_fields(&row) {
            row.insert(key, value);
        }
        Self(row)
    }

    /// Enrich a raw `StorageEntity` row (Arc<str> keys). Re-keys to the
    /// String-keyed row shape once, at the enrichment boundary.
    pub fn from_storage(
        data: crate::StorageEntity,
        computed_fields: impl FnOnce(&HashMap<String, Value>) -> HashMap<String, Value>,
    ) -> Self {
        let rekeyed = data.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        Self::from_raw(rekeyed, computed_fields)
    }

    /// Convert back to a plain `DataRow` when crossing into code that hasn't
    /// been migrated to `EnrichedRow` yet.  Prefer removing these call sites
    /// over adding new ones.
    pub fn into_inner(self) -> HashMap<String, Value> {
        self.0
    }

    /// Promote fields from the `properties` JSON object to top-level row keys.
    fn flatten_properties(mut data: HashMap<String, Value>) -> HashMap<String, Value> {
        if let Some(Value::Object(props)) = data.get("properties") {
            for (key, value) in props.clone() {
                data.entry(key).or_insert(value);
            }
        }
        data
    }
}

/// Keyed collection of data rows with CDC change support.
///
/// Replaces the pattern of maintaining a `Vec<DataRow>` and doing linear scans
/// to apply Created/Updated/Deleted/FieldsChanged events. Uses a HashMap keyed
/// by the "id" column for O(1) lookups.
#[derive(Debug, Clone)]
pub struct DataRowAccumulator {
    rows: HashMap<String, DataRow>,
}

impl DataRowAccumulator {
    pub fn new() -> Self {
        Self {
            rows: HashMap::new(),
        }
    }

    pub fn from_rows(rows: Vec<DataRow>) -> Self {
        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            if let Some(id) = row.get("id").and_then(|v| v.as_string()) {
                map.insert(id.to_string(), row);
            }
        }
        Self { rows: map }
    }

    pub fn apply_change(&mut self, change: Change<DataRow>) {
        match change {
            Change::Created { data, .. } => {
                if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
                    self.rows.insert(id.to_string(), data);
                }
            }
            Change::Updated { ref id, data, .. } => {
                self.rows.insert(id.clone(), data);
            }
            Change::Deleted { ref id, .. } => {
                self.rows.remove(id);
            }
            Change::FieldsChanged {
                ref entity_id,
                ref fields,
                ..
            } => {
                if let Some(row) = self.rows.get_mut(entity_id) {
                    for (name, _old, new) in fields {
                        row.insert(name.clone(), new.clone());
                    }
                }
            }
        }
    }

    pub fn apply_batch(&mut self, changes: impl IntoIterator<Item = Change<DataRow>>) {
        for change in changes {
            self.apply_change(change);
        }
    }

    /// Export as Vec<DataRow> for interpretation.
    pub fn to_vec(&self) -> Vec<DataRow> {
        self.rows.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl Default for DataRowAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::ChangeOrigin;

    fn origin() -> ChangeOrigin {
        ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        }
    }

    fn row(id: &str, content: &str) -> DataRow {
        HashMap::from([
            ("id".into(), Value::String(id.into())),
            ("content".into(), Value::String(content.into())),
        ])
    }

    #[test]
    fn accumulator_from_rows_and_to_vec() {
        let acc = DataRowAccumulator::from_rows(vec![row("a", "hello"), row("b", "world")]);
        assert_eq!(acc.len(), 2);
        let v = acc.to_vec();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn accumulator_apply_created() {
        let mut acc = DataRowAccumulator::new();
        acc.apply_change(Change::Created {
            data: row("x", "new"),
            origin: origin(),
        });
        assert_eq!(acc.len(), 1);
        let v = acc.to_vec();
        assert_eq!(v[0].get("content").unwrap().as_string().unwrap(), "new");
    }

    #[test]
    fn accumulator_apply_updated() {
        let mut acc = DataRowAccumulator::from_rows(vec![row("a", "old")]);
        acc.apply_change(Change::Updated {
            id: "a".into(),
            data: row("a", "updated"),
            origin: origin(),
        });
        assert_eq!(acc.len(), 1);
        let v = acc.to_vec();
        assert_eq!(v[0].get("content").unwrap().as_string().unwrap(), "updated");
    }

    #[test]
    fn accumulator_apply_deleted() {
        let mut acc = DataRowAccumulator::from_rows(vec![row("a", "bye")]);
        acc.apply_change(Change::Deleted {
            id: "a".into(),
            origin: origin(),
        });
        assert!(acc.is_empty());
    }

    #[test]
    fn accumulator_apply_fields_changed() {
        let mut acc = DataRowAccumulator::from_rows(vec![row("a", "old")]);
        acc.apply_change(Change::FieldsChanged {
            entity_id: "a".into(),
            fields: vec![(
                "content".into(),
                Value::String("old".into()),
                Value::String("patched".into()),
            )],
            origin: origin(),
        });
        let v = acc.to_vec();
        assert_eq!(v[0].get("content").unwrap().as_string().unwrap(), "patched");
    }

    #[test]
    fn accumulator_apply_batch() {
        let mut acc = DataRowAccumulator::new();
        acc.apply_batch([
            Change::Created {
                data: row("a", "first"),
                origin: origin(),
            },
            Change::Created {
                data: row("b", "second"),
                origin: origin(),
            },
            Change::Deleted {
                id: "a".into(),
                origin: origin(),
            },
        ]);
        assert_eq!(acc.len(), 1);
        assert!(acc
            .to_vec()
            .iter()
            .any(|r| { r.get("id").unwrap().as_string().unwrap() == "b" }));
    }

    fn value_row(name: &str) -> DataRow {
        HashMap::from([
            ("_rowid".into(), Value::Integer(7)),
            ("name".into(), Value::String(name.into())),
        ])
    }

    #[test]
    fn entity_row_identity_is_the_entity_uri() {
        let r = row("block:abc", "hi");
        match RowIdentity::of_row(&r) {
            RowIdentity::Entity(uri) => assert_eq!(uri.as_str(), "block:abc"),
            other => panic!("entity-shaped row must be Entity, got {other:?}"),
        }
        assert!(!RowIdentity::of_row(&r).is_value());
    }

    #[test]
    fn value_row_identity_is_content_hash() {
        let r = value_row("2026-07-10");
        let id = RowIdentity::of_row(&r);
        assert!(id.is_value(), "id-less row must be value-shaped");
        // Serializes under the `value:` scheme.
        assert_eq!(id.to_store_key().scheme(), "value");
    }

    #[test]
    fn value_row_identity_is_stable_across_recompute() {
        // Two independent constructions of the same row content (simulating an
        // incremental matview recompute that re-emits the row) hash to the SAME
        // identity — identity is content, so a recompute does not churn the row.
        let a = value_row("2026-07-10");
        let b = value_row("2026-07-10");
        assert_eq!(RowIdentity::of_row(&a), RowIdentity::of_row(&b));
    }

    #[test]
    fn distinct_value_rows_get_distinct_identity() {
        let a = value_row("2026-07-10");
        let b = value_row("2026-07-11");
        assert_ne!(RowIdentity::of_row(&a), RowIdentity::of_row(&b));
    }

    #[test]
    fn content_hash_is_key_order_independent() {
        let mut a: DataRow = HashMap::new();
        a.insert("x".into(), Value::Integer(1));
        a.insert("y".into(), Value::String("q".into()));
        let mut b: DataRow = HashMap::new();
        b.insert("y".into(), Value::String("q".into()));
        b.insert("x".into(), Value::Integer(1));
        assert_eq!(RowContentHash::of_row(&a), RowContentHash::of_row(&b));
    }

    #[test]
    fn value_row_round_trips_through_accumulator_with_stable_key() {
        // A value row flows through the same Created path an aggregate takes;
        // re-applying the identical content (recompute) keys to the same slot,
        // so the row set does not churn.
        let mut acc = DataRowAccumulator::new();
        let key = RowIdentity::of_row(&value_row("2026-07-10"))
            .to_store_key()
            .as_str()
            .to_string();
        // Mimic the reactive store keying: insert under the identity key.
        acc.rows.insert(key.clone(), value_row("2026-07-10"));
        assert_eq!(acc.len(), 1);
        // Recompute re-emits the same content → same key → still one row.
        let key2 = RowIdentity::of_row(&value_row("2026-07-10"))
            .to_store_key()
            .as_str()
            .to_string();
        assert_eq!(key, key2);
        acc.rows.insert(key2, value_row("2026-07-10"));
        assert_eq!(
            acc.len(),
            1,
            "stable identity ⇒ no duplicate row on recompute"
        );
    }
}
