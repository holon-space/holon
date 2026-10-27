use super::*;

fn block(tags: &[&str], requires: &[&str]) -> Block {
    let mut b = Block::new_text(
        EntityUri::parse_owned("block:b1".to_string()).unwrap(),
        EntityUri::no_parent(),
        "x",
    );
    b.tags = Tags::from(tags.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    b.requires = requires
        .iter()
        .map(|s| EntityUri::parse_owned(format!("block:{s}")).unwrap())
        .collect();
    b
}

#[test]
fn edge_field_closed_enum_surface() {
    assert_eq!(EdgeField::Tags.column(), "tags");
    assert_eq!(EdgeField::Requires.column(), "requires");
    assert_eq!(EdgeField::AdviceSuppressed.column(), "advice_suppressed");
    assert_eq!(EdgeField::ContributesTo.column(), "contributes_to");

    assert!(EdgeField::is_edge_column("tags"));
    assert!(EdgeField::is_edge_column("requires"));
    assert!(EdgeField::is_edge_column("advice_suppressed"));
    assert!(EdgeField::is_edge_column("contributes_to"));
    assert!(!EdgeField::is_edge_column("content"));
    assert!(!EdgeField::is_edge_column("tag"));

    let empty = block(&[], &[]);
    let tagged = block(&["a"], &[]);
    let requiring = block(&[], &["dep"]);

    assert!(EdgeField::Tags.is_empty(&empty));
    assert!(!EdgeField::Tags.is_empty(&tagged));
    assert!(EdgeField::Requires.is_empty(&tagged));
    assert!(!EdgeField::Requires.is_empty(&requiring));

    assert!(EdgeField::Tags.differs(&empty, &tagged));
    assert!(!EdgeField::Tags.differs(&empty, &requiring));
    assert!(EdgeField::Requires.differs(&empty, &requiring));
    assert!(!EdgeField::Requires.differs(&empty, &tagged));

    assert_eq!(
        EdgeField::Tags.param_value(&tagged),
        Value::Array(vec![Value::String("a".to_string())])
    );
    assert_eq!(
        EdgeField::Requires.param_value(&requiring),
        Value::Array(vec![Value::String("block:dep".to_string())])
    );
    assert_eq!(EdgeField::Tags.param_value(&empty), Value::Array(vec![]));

    assert_eq!(
        EdgeFieldUpdate::Tags(Tags::default()).field(),
        EdgeField::Tags
    );
    assert_eq!(
        EdgeFieldUpdate::Requires(vec![]).field(),
        EdgeField::Requires
    );
    assert_eq!(EdgeFieldUpdate::Tags(Tags::default()).field_name(), "tags");
    assert_eq!(EdgeFieldUpdate::Requires(vec![]).field_name(), "requires");
}

/// Every junction keys on `(source, target)` and the provider emits one
/// plain `INSERT` per element, so a repeated target fails the whole block
/// write on the primary key. `Vec<EntityUri>` cannot express that
/// constraint, so this builder folds the repeat out — keeping the FIRST
/// occurrence, which holds the surviving targets in authored order.
///
/// Bug-funnel `2026-08-30-edge-field-duplicate-target-wedges-write`.
#[test]
fn param_value_folds_a_repeated_target_keeping_authored_order() {
    let b = block(&[], &["late", "early", "late"]);
    assert_eq!(
        EdgeField::Requires.param_value(&b),
        Value::Array(vec![
            Value::String("block:late".to_string()),
            Value::String("block:early".to_string()),
        ]),
        "the repeat is dropped at its SECOND occurrence, so the surviving targets keep the \
             order they were authored in — sorting instead would churn org write-back bytes"
    );
}
