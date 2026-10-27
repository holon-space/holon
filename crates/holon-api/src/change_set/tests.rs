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
            assert_eq!(parent_id.to_string(), "block:root");
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
fn create_without_parent_decodes_to_no_parent_sentinel() {
    let ops = ops(vec![(
        "create",
        vec![("id", s("block:a")), ("content", s("root"))],
    )]);
    let cs = ChangeSet::from_ops(&ops, Provenance::default());
    match &cs.ops[0] {
        ChangeOp::Create { parent_id, .. } => {
            assert_eq!(*parent_id, EntityUri::no_parent());
        }
        other => panic!("expected Create, got {other:?}"),
    }
}

#[test]
fn relocate_parent_is_typed() {
    let ops = ops(vec![(
        "update",
        vec![
            ("id", s("block:a")),
            ("parent_id", s("block:newparent")),
            ("sort_key", s("A1")),
        ],
    )]);
    let cs = ChangeSet::from_ops(&ops, Provenance::default());
    match &cs.ops[0] {
        ChangeOp::Relocate { parent, .. } => {
            assert_eq!(
                parent.as_ref().map(|u| u.to_string()),
                Some("block:newparent".into())
            );
        }
        other => panic!("expected Relocate, got {other:?}"),
    }
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
    assert!(
        cs.ops
            .iter()
            .any(|o| matches!(o, ChangeOp::Relocate { .. }))
    );
    assert!(
        cs.ops
            .iter()
            .any(|o| matches!(o, ChangeOp::SetField { field, .. } if field == "content"))
    );
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
