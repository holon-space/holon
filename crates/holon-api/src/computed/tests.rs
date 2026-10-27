use super::*;
use crate::CompiledExpr;

fn field(name: &str, expr: &str) -> CompiledComputedField {
    CompiledComputedField::script(
        name,
        CompiledExpr::compile(&RhaiEngine::new(), expr).unwrap(),
    )
}

#[test]
fn basic_computed_field() {
    let mut ctx = HashMap::new();
    ctx.insert("priority".to_string(), Value::Integer(3));

    let fields = vec![field("priority_score", "priority * 10")];

    resolve_computed_fields(&fields, &mut ctx);
    assert_eq!(ctx["priority_score"], Value::Integer(30));
}

#[test]
fn chained_computed_fields() {
    let mut ctx = HashMap::new();
    ctx.insert("base".to_string(), Value::Float(2.0));

    let fields = vec![
        field("doubled", "base * 2.0"),
        field("quadrupled", "doubled * 2.0"),
    ];

    resolve_computed_fields(&fields, &mut ctx);
    assert_eq!(ctx["doubled"], Value::Float(4.0));
    assert_eq!(ctx["quadrupled"], Value::Float(8.0));
}

fn huge_base_row() -> HashMap<String, Value> {
    HashMap::from([("base".to_string(), Value::Float(1e308))])
}

#[test]
fn a_non_finite_rhai_result_is_refused_naming_its_source() {
    let err = dynamic_to_value(&rhai::Dynamic::from(f64::INFINITY), "base * 10.0")
        .expect_err("a non-finite float has no Value");
    assert!(
        err.to_string()
            .contains("non-finite float inf from base * 10.0"),
        "{err}"
    );
}

#[test]
fn an_overflowing_computed_field_is_refused_at_the_enrich_seat() {
    let mut ctx = huge_base_row();
    let fields = vec![field("ratio", "base * 10.0")];

    resolve_computed_fields(&fields, &mut ctx);
    assert_eq!(ctx["ratio"], Value::Null, "an infinite result is refused");
}

#[test]
fn an_overflowing_computed_field_is_refused_at_the_render_seat() {
    let profile = crate::entity_profile::EntityProfile {
        entity_name: crate::EntityName::new("thing"),
        variants: Vec::new(),
        computed_fields: vec![field("ratio", "base * 10.0")],
        virtual_child: None,
        declared_columns: BTreeSet::new(),
        render_requirements: crate::render_requirements::RenderRequirements::default(),
    };
    let computed = profile.compute_fields_only(
        &huge_base_row(),
        &RhaiEngine::new(),
        &crate::render_requirements::RenderRequirements::default(),
    );
    assert_eq!(
        computed["ratio"],
        Value::Null,
        "an infinite result is refused"
    );
}

#[test]
fn missing_required_column_is_unbound_null_not_evaluated() {
    // Type-aware binding: a required column absent from scope makes the field
    // UNBOUND — rhai is never invoked (no "Variable not found"), output is Null.
    let mut ctx = HashMap::new();
    let fields = vec![field("bad", "nonexistent_var + 1")];

    resolve_computed_fields(&fields, &mut ctx);
    assert_eq!(ctx["bad"], Value::Null);
}

#[test]
fn unbound_field_does_not_poison_a_dependent_boolean_field() {
    // The Seat-B cascade root: `is_page_row` unbound (tags absent) must NOT
    // become a `unit` that type-errors a dependent `&&`. It must stay
    // unbound so the dependent is ALSO unbound (no genuine eval error).
    let engine = RhaiEngine::new();
    let mut scope = Scope::new();
    let mut ctx = HashMap::new();
    let fields = vec![
        field("is_page_row", "tags != ()"),
        field("embedded", "is_page_row && content_type == \"text\""),
    ];
    let declared = BTreeSet::new();
    resolve_computed_fields_with_scope(&engine, &mut scope, &fields, &mut ctx, &declared);
    // Both unbound → Null, and crucially NO panic / no genuine error.
    assert_eq!(ctx.get("is_page_row"), Some(&Value::Null));
    assert_eq!(ctx.get("embedded"), Some(&Value::Null));
    // Neither was pushed into scope (absence = unbound).
    assert!(!scope.contains("is_page_row"));
    assert!(!scope.contains("embedded"));
}

#[test]
fn present_columns_still_evaluate_and_a_prior_field_binds_dependents() {
    let engine = RhaiEngine::new();
    let mut scope = Scope::new();
    scope.push(
        "content_type".to_string(),
        value_to_dynamic(&Value::String("text".into())),
    );
    let mut ctx = HashMap::new();
    let fields = vec![
        field("is_source", "content_type == \"source\""),
        field("shown", "!is_source"),
    ];
    let declared = BTreeSet::new();
    resolve_computed_fields_with_scope(&engine, &mut scope, &fields, &mut ctx, &declared);
    assert_eq!(ctx["is_source"], Value::Boolean(false));
    assert_eq!(ctx["shown"], Value::Boolean(true));
}
