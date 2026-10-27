use super::*;
use crate::render_types::OperationParam;

fn descriptor(entity: &str, op: &str, params: Vec<OperationParam>) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: entity.into(),
        entity_short_name: entity.to_string(),
        id_column: "id".to_string(),
        name: op.to_string(),
        display_name: op.to_string(),
        description: op.to_string(),
        required_params: params,
        optional_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: crate::TargetScope::Block,
        boundary_behavior: crate::BoundaryBehavior::Unclassified,
        menu_exposure: crate::MenuExposure::NotListed {
            surface: crate::NonMenuSurface::Test,
        },
        trigger: None,
        bound_params: Default::default(),
        marking_delta: crate::marking::MarkingDelta::Undeclared,
        guard: crate::pattern::OpGuard::None,
        arcs: crate::arcs::TransitionArcs::Undeclared,
    }
}

fn param(name: &str, hint: TypeHint) -> OperationParam {
    OperationParam {
        name: name.to_string(),
        type_hint: hint,
        description: name.to_string(),
    }
}

fn entity_id(entity: &str) -> TypeHint {
    TypeHint::EntityId {
        entity_name: EntityName::new(entity),
    }
}

#[test]
fn the_subject_and_role_names_are_references_and_a_value_name_is_not() {
    assert!(names_an_entity_reference("id"));
    assert!(names_an_entity_reference("parent_id"));
    assert!(names_an_entity_reference("after_id"));
    assert!(!names_an_entity_reference("_id"));
    assert!(!names_an_entity_reference("field"));
    assert!(!names_an_entity_reference("region"));
}

/// Two providers advertise `block/indent`; the pair's references must come
/// out the same whichever was registered first, because the dispatcher
/// scans the providers in registration order.
#[test]
fn references_do_not_depend_on_registration_order() {
    let authority = descriptor(
        "block",
        "indent",
        vec![
            param("id", entity_id("block")),
            param("field", TypeHint::String),
        ],
    );
    let mirror = descriptor(
        "block",
        "indent",
        vec![
            param("id", entity_id("block")),
            param("after_id", entity_id("block")),
        ],
    );

    let forward_order = [authority.clone(), mirror.clone()];
    let reverse_order = [mirror, authority];
    let forward = entity_reference_params(&forward_order, "block", "indent", |_| None)
        .expect("agreeing declarations");
    let reverse = entity_reference_params(&reverse_order, "block", "indent", |_| None)
        .expect("agreeing declarations");

    let names = |v: Vec<EntityReferenceParam<'_>>| {
        let mut n: Vec<String> = v.into_iter().map(|p| p.name.to_string()).collect();
        n.sort();
        n
    };
    assert_eq!(
        names(forward),
        vec!["after_id".to_string(), "id".to_string()]
    );
    assert_eq!(
        names(reverse),
        vec!["after_id".to_string(), "id".to_string()]
    );
}

/// Disagreement is refused by name rather than resolved in favour of
/// whichever descriptor the scan reached first — in either order.
#[test]
fn disagreeing_declarations_are_refused_by_name_in_both_orders() {
    let typed = descriptor("block", "indent", vec![param("id", entity_id("block"))]);
    let untyped = descriptor("block", "indent", vec![param("id", TypeHint::String)]);

    for (first, second) in [(&typed, &untyped), (&untyped, &typed)] {
        let error =
            entity_reference_params(&[first.clone(), second.clone()], "block", "indent", |_| {
                None
            })
            .expect_err("the two declarations disagree");
        assert!(error.contains("block/indent"), "{error}");
        assert!(error.contains("'id'"), "{error}");
    }
}

/// The registration gate: a reference-named parameter left
/// `TypeHint::String` would reach the provider unparsed, so the
/// descriptor set is refused where it arrives.
#[test]
fn a_string_typed_entity_reference_is_refused_at_registration() {
    let error = validate_entity_references(&[descriptor(
        "project",
        "set_field",
        vec![param("id", TypeHint::String)],
    )])
    .expect_err("a String-typed `id` is a declaration hole");
    assert!(error.contains("project/set_field"), "{error}");
    assert!(error.contains("TypeHint::EntityId"), "{error}");
}

/// The root is legal only where a parameter declares it, and the
/// declaration travels with the parameter rather than with its name.
#[test]
fn only_a_root_admitting_position_reports_admits_root() {
    let d = descriptor(
        "block",
        "create",
        vec![
            param("id", entity_id("block")),
            param(
                "parent_id",
                TypeHint::EntityIdOrRoot {
                    entity_name: EntityName::new("block"),
                },
            ),
        ],
    );
    let found = entity_reference_params(std::slice::from_ref(&d), "block", "create", |_| None)
        .expect("both positions are declared");
    let admits = |name: &str| {
        found
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("{name} is parsed as a reference"))
            .admits_root
    };
    assert!(!admits("id"), "a write's subject may never be the root");
    assert!(admits("parent_id"), "a parent position may be the root");
}

#[test]
fn an_optional_param_declared_an_entity_id_is_a_reference() {
    let mut d = descriptor("block", "move_block", vec![param("id", entity_id("block"))]);
    d.optional_params = vec![param("after_block_id", entity_id("block"))];
    let found = entity_reference_params(std::slice::from_ref(&d), "block", "move_block", |_| None)
        .expect("both are declared");
    let names: Vec<_> = found.iter().map(|p| p.name).collect();
    assert_eq!(names, ["id", "after_block_id"]);
}

#[test]
fn a_description_saying_id_is_read_as_a_reference_and_a_substring_is_not() {
    assert!(description_calls_it_an_id("The surviving block id"));
    // `_` is a word boundary, so a column name spelled `resolved_id`
    // reads as an id — which is what caught `rewrite_link_resolution`.
    assert!(description_calls_it_an_id("Current resolved_id to rewrite"));
    assert!(description_calls_it_an_id("Captured ids to restore"));
    assert!(!description_calls_it_an_id("An invalid width"));
    assert!(!description_calls_it_an_id("Identity of the author"));
    // A payload sketch names fields of something else, not this parameter.
    assert!(!description_calls_it_an_id(
        "JSON snapshot of [{system, foreign_id, confidence}]"
    ));
    assert!(description_calls_it_an_id(
        "Integration row id, 'integration:<provider>'"
    ));
}

/// `identifier` names an id the same way `id` and `ids` do. `identity` and
/// `invalid` merely begin with the same letters and say nothing.
#[test]
fn identifier_names_an_id_and_identity_and_invalid_do_not() {
    assert!(description_calls_it_an_id("The block id"));
    assert!(description_calls_it_an_id("The captured ids"));
    assert!(description_calls_it_an_id("The identifier of the block"));
    assert!(!description_calls_it_an_id("Identity of the author"));
    assert!(!description_calls_it_an_id("An invalid width"));
}

/// A parenthesis is prose, not a payload sketch. `(id)` is a claim about
/// THIS parameter and trips the rule; only the sketch kinds are skipped.
#[test]
fn a_parenthesised_id_is_read_and_a_bracketed_payload_sketch_is_not() {
    assert!(description_calls_it_an_id("Origin block to convert (id)"));
    assert!(description_calls_it_an_id("The siblings (ids) to renumber"));
    assert!(!description_calls_it_an_id(
        "JSON snapshot of [{system, foreign_id, confidence}]"
    ));
    assert!(!description_calls_it_an_id("Field map {parent_id: uuid}"));
}

/// A reference the NAME rule cannot see — `target` is not `id` or `*_id` —
/// is still refused when its own description calls it an id.
#[test]
fn a_string_described_as_an_id_is_refused_even_under_a_value_name() {
    let error = validate_entity_references(&[descriptor(
        "block",
        "merge_blocks_plan",
        vec![OperationParam {
            name: "canonical".to_string(),
            type_hint: TypeHint::String,
            description: "The surviving block id".to_string(),
        }],
    )])
    .expect_err("a String described as an id is a declaration hole");
    assert!(error.contains("'canonical'"), "{error}");
    assert!(error.contains("described as an id"), "{error}");
}

#[test]
fn a_value_typed_parameter_passes_registration() {
    validate_entity_references(&[descriptor(
        "project",
        "set_field",
        vec![
            param("id", entity_id("project")),
            param("value", TypeHint::String),
        ],
    )])
    .expect("references declared, values left alone");
}

/// `#[reference(Block)]` names the block type itself, so `parent_id` is a
/// tree edge whose root is legal; `set_field`'s value is that reference
/// only when it writes `parent_id`.
#[test]
fn a_block_create_and_set_field_declare_the_parent_edge() {
    let block = crate::Block::type_definition();
    let parent = param(
        "parent_id",
        TypeHint::EntityIdOrRoot {
            entity_name: EntityName::new("block"),
        },
    );
    assert_eq!(
        create_references(&block)
            .expect("Block's references")
            .into_iter()
            .map(|p| (p.name, p.type_hint))
            .collect::<Vec<_>>(),
        vec![
            ("id".to_string(), entity_id("block")),
            (parent.name.clone(), parent.type_hint.clone()),
        ]
    );
    let set_field = descriptor(
        "block",
        "set_field",
        vec![
            param("id", entity_id("block")),
            param("field", TypeHint::String),
            param(
                "value",
                set_field_value_hint(&block).expect("Block's references"),
            ),
        ],
    );
    let ops = [set_field];
    let parent_value = Value::String("block:p".to_string());
    let content_field = Value::String("content".to_string());
    let parent_field = Value::String("parent_id".to_string());
    let names = |field: &Value| {
        entity_reference_params(&ops, "block", "set_field", |k| match k {
            "field" => Some(field),
            "value" => Some(&parent_value),
            _ => None,
        })
        .expect("the declaration is complete")
        .into_iter()
        .map(|r| (r.name, r.admits_root))
        .collect::<Vec<_>>()
    };
    assert_eq!(names(&content_field), vec![("id", false)]);
    assert_eq!(names(&parent_field), vec![("id", false), ("value", true)]);
}

/// A declared type's JSON can name a foreign target as a Rust type; that
/// is a refusal of the declaration, not a panic inside the op.
#[test]
fn a_foreign_type_name_target_is_refused() {
    let task = TypeDefinition::new(
        "task",
        vec![
            crate::FieldSchema::new("id", "TEXT").primary_key(),
            crate::FieldSchema::new("project", "TEXT").reference_target("Project"),
        ],
    );
    let err = row_field_references(&task).expect_err("`Project` names no entity");
    assert!(err.contains("'project' of 'task'"), "{err}");
    assert!(create_references(&task).is_err());
    assert!(set_field_value_hint(&task).is_err());
}
#[test]
fn an_unnameable_target_is_refused() {
    let task = TypeDefinition::new(
        "task",
        vec![
            crate::FieldSchema::new("id", "TEXT").primary_key(),
            crate::FieldSchema::new("project", "TEXT").reference_target("my project"),
        ],
    );
    let err = row_field_references(&task).expect_err("`my project` names no entity");
    assert!(err.contains("'project' of 'task'"), "{err}");
    assert!(err.contains("is not a valid entity name"), "{err}");
}
