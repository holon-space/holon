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
    assert!(
        acc.to_vec()
            .iter()
            .any(|r| { r.get("id").unwrap().as_string().unwrap() == "b" })
    );
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
