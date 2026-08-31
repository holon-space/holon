//! Declared field types decide the `+` operator: over TEXT columns it
//! concatenates, over numeric ones it adds. Syntax alone cannot tell them
//! apart.

use holon_api::computation::Computation;
use holon_api::computation::FieldKind;
use holon_api::computation::FieldTypes;
use holon_api::expr_parser;

fn types(pairs: &[(&str, FieldKind)]) -> FieldTypes {
    let mut t = FieldTypes::new();
    for (name, kind) in pairs {
        t.insert(*name, *kind);
    }
    t
}

#[test]
fn two_declared_text_columns_concatenate() {
    let t = types(&[
        ("first_name", FieldKind::Text),
        ("last_name", FieldKind::Text),
    ]);
    let comp = expr_parser::parse_typed("first_name + last_name", &t).expect("in the subset");
    assert!(
        matches!(comp, Computation::Concat { .. }),
        "expected Concat, got {comp:?}"
    );
    assert_eq!(
        comp.compile_sql().expect("lowers").sql,
        "(first_name || last_name)"
    );
}

#[test]
fn two_declared_numeric_columns_add() {
    let t = types(&[
        ("weight", FieldKind::Numeric),
        ("bonus", FieldKind::Numeric),
    ]);
    let comp = expr_parser::parse_typed("weight + bonus", &t).expect("in the subset");
    assert!(
        matches!(comp, Computation::Arith { .. }),
        "expected Arith, got {comp:?}"
    );
    assert_eq!(comp.compile_sql().expect("lowers").sql, "(weight + bonus)");
}

#[test]
fn an_undeclared_column_carries_no_type_evidence() {
    let comp = expr_parser::parse_typed("first_name + last_name", &FieldTypes::new())
        .expect("in the subset");
    assert!(
        matches!(comp, Computation::Arith { .. }),
        "without declared types `+` has only its numeric reading, got {comp:?}"
    );
}

#[test]
fn a_declared_boolean_column_is_an_and_operand() {
    let t = types(&[
        ("is_source", FieldKind::Boolean),
        ("is_focused", FieldKind::Boolean),
    ]);
    let comp = expr_parser::parse_typed("is_source && is_focused", &t)
        .expect("declared BOOLEAN columns are boolean operands");
    assert!(matches!(comp, Computation::And { .. }), "got {comp:?}");

    expr_parser::parse_typed("is_source && is_focused", &FieldTypes::new())
        .expect_err("without declared types the operands carry no boolean evidence");
}

/// A serialized declaration carries the types it was resolved against, so a
/// round-trip cannot silently re-read `role + email` as arithmetic.
#[test]
fn a_serde_round_trip_preserves_the_resolved_operator() {
    let t = types(&[("role", FieldKind::Text), ("email", FieldKind::Text)]);
    let spec = holon_api::ComputedSpec::parse(
        "who",
        "role + email",
        holon_api::ComputedTier::ComputedPersisted,
        &t,
        &rhai::Engine::new(),
    )
    .expect("declares cleanly");
    assert!(matches!(spec.computation(), Computation::Concat { .. }));

    let json = serde_json::to_string(&spec).expect("serializes");
    let back: holon_api::ComputedSpec = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(
        back.computation().compile_sql().expect("lowers").sql,
        "(role || email)",
        "round-trip changed the operator; json was {json}"
    );
    assert_eq!(spec, back);
}

/// The tier's promise is enforced wherever a declaration is built, not only on
/// the registry path.
#[test]
fn deserializing_a_persisted_field_that_cannot_lower_is_refused() {
    let json = r#"{"name":"tagged","tier":"computed_persisted","source":"role.contains(\"x\")","field_types":{}}"#;
    let err = serde_json::from_str::<holon_api::ComputedSpec>(json)
        .expect_err("a persisted spec outside the SQL subset must be refused");
    assert!(
        format!("{err}").contains("tagged"),
        "the error must name the field, got: {err}"
    );
}

/// Omitting the resolved types would let the same source re-read `+` as
/// arithmetic, so the machine form requires them.
#[test]
fn deserializing_without_field_types_is_refused() {
    let err = serde_json::from_str::<holon_api::ComputedSpec>(
        r#"{"name":"who","tier":"computed_persisted","source":"role + email"}"#,
    )
    .expect_err("a spec without its resolved types must be refused");
    assert!(
        format!("{err}").contains("field_types"),
        "the error must name the missing field, got: {err}"
    );
}
