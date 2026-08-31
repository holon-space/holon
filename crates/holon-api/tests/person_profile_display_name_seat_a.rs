//! The `person_profile.display_name` computed field — read from the REAL
//! shipped yaml, not a copied string — must parse into the typed `Computation`
//! subset and lower to SQL, so it is served from a planted matview column
//! (seat A) rather than a per-row Rhai call (seat B).

use std::path::PathBuf;

use holon_api::computation::Computation;
use holon_api::computation::DerivedField;
use holon_api::computation::DerivedFieldPlan;
use holon_api::computation::FieldIdent;
use holon_api::computation::FieldKind;
use holon_api::computation::FieldTypes;
use holon_api::expr_parser;

/// The shipped `assets/default/types/person_profile.yaml`, read from disk.
fn display_name_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/default/types/person_profile.yaml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("must read {}: {e}", path.display()));
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("must parse yaml: {e}"));
    doc["computed"]["display_name"]["expr"]
        .as_str()
        .expect("computed.display_name.expr must be a string")
        .to_string()
}

/// `person`'s declared columns: `role` and `email` are both TEXT.
fn person_types() -> FieldTypes {
    let mut types = FieldTypes::new();
    types.insert("role", FieldKind::Text);
    types.insert("email", FieldKind::Text);
    types
}

#[test]
fn display_name_parses_into_the_typed_subset() {
    let src = display_name_source();
    let comp = expr_parser::parse_typed(&src, &person_types())
        .unwrap_or_else(|e| panic!("subset parser must accept `{src}`: {e}"));
    assert!(
        !matches!(comp, Computation::Script(_)),
        "display_name must be a TYPED computation, not a Rhai Script"
    );
}

#[test]
fn display_name_lowers_to_sql_and_plants_to_seat_a() {
    let src = display_name_source();
    let comp = expr_parser::parse_typed(&src, &person_types())
        .unwrap_or_else(|e| panic!("must parse `{src}`: {e}"));
    let frag = comp
        .compile_sql()
        .unwrap_or_else(|e| panic!("compile_sql must succeed for `{src}`: {e}"));
    println!("display_name SQL: {}", frag.sql);

    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(
        FieldIdent::parse("display_name").expect("identifier"),
        comp,
    )]);
    assert_eq!(
        plan.sql_planted.len(),
        1,
        "display_name must be SQL-planted; stage={:?}",
        plan.stage_evaluated
    );
    println!("planted column: {}", plan.sql_planted[0].sql);
}
