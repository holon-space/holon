//! Entity references at the operation surface.
//!
//! An operation parameter that addresses an entity — the subject `id`, or a
//! role name like `parent_id` / `block_id` — is declared
//! [`TypeHint::EntityId`](crate::TypeHint::EntityId), and the operation
//! boundary parses it into an [`EntityUri`](crate::EntityUri) once. These two
//! gates keep that declaration honest: [`validate_entity_references`] refuses
//! a descriptor set at registration, and
//! [`entity_reference_params`] is what the boundary parses with.
//!
//! The names below are the surface's spelling for "addresses an entity". The
//! macro that generates descriptors from Rust signatures reads the same names
//! (`crates/holon-macros/src/operations_trait.rs`); a drift between the two is
//! a registration error, not a silent hole.

use crate::EntityName;
use crate::render_types::OperationDescriptor;
use crate::render_types::TypeHint;

/// Whether a parameter's own PROSE calls it an id.
///
/// The name rule is blind to a reference spelled `target`, `canonical`, `from`
/// or `to`; what those parameters do have is a description saying "id". The two
/// readings together catch a `TypeHint::String` the boundary would pass through
/// unparsed, and the answer to either is the same: declare what the parameter
/// is.
///
/// Two narrowings keep it from reading things that are not claims about this
/// parameter. Word boundaries, so "invalid", "identity" and "width" say
/// nothing. And the two bracket kinds that sketch a payload are skipped: a
/// description like `JSON snapshot of [{system, foreign_id, confidence}]` names
/// fields of something else, and rewording it to quiet a check is how a
/// description stops being one. A parenthesis is not a sketch — `(id)` is prose
/// about this parameter and is read.
pub fn description_calls_it_an_id(description: &str) -> bool {
    let mut depth = 0usize;
    let prose: String = description
        .chars()
        .map(|c| match c {
            '[' | '{' => {
                depth += 1;
                ' '
            }
            ']' | '}' => {
                depth = depth.saturating_sub(1);
                ' '
            }
            c if depth > 0 => {
                let _ = c;
                ' '
            }
            c => c,
        })
        .collect();
    prose
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| {
            word.eq_ignore_ascii_case("id")
                || word.eq_ignore_ascii_case("ids")
                || word.eq_ignore_ascii_case("identifier")
        })
}

/// Whether `param_name` names an entity reference rather than a value: the
/// subject `id`, or a `<role>_id` role name (`parent_id`, `anchor_id`).
///
/// A reference to a DIFFERENT entity is stated, never inferred from the role
/// name — the operation macro spells that `#[entity_ref("project")]`.
pub fn names_an_entity_reference(param_name: &str) -> bool {
    param_name == "id"
        || param_name
            .strip_suffix("_id")
            .is_some_and(|role| !role.is_empty())
}

/// One entity-reference parameter of a dispatched operation, as its
/// descriptors declare it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityReferenceParam<'a> {
    /// The parameter name (`id`, `parent_id`, `target`, …).
    pub name: &'a str,
    /// The entity its value must name.
    pub entity_name: &'a EntityName,
    /// Whether the ROOT sentinel is a legal value HERE. True only for a
    /// position declared [`TypeHint::EntityIdOrRoot`]: elsewhere the sentinel
    /// names no row, so a write addressed by it would report success having
    /// changed nothing.
    pub admits_root: bool,
}

/// The entity-reference parameters of one `(entity, op)` pair, over EVERY
/// descriptor that advertises it.
///
/// The PAIR is the unit, not one descriptor: `block/set_field` is advertised
/// both by the block authority and by a `SqlOperationProvider`, so reading the
/// first match would make the boundary's behaviour depend on the order the
/// providers were registered in. Two descriptors that name the same parameter
/// must mean the same thing by it; when they do not, the disagreement is
/// refused by name instead of being resolved in favour of whichever the scan
/// reached first.
pub fn entity_reference_params<'a>(
    descriptors: &'a [OperationDescriptor],
    entity_name: &str,
    op_name: &str,
) -> Result<Vec<EntityReferenceParam<'a>>, String> {
    let mut hints: Vec<(&str, &TypeHint)> = Vec::new();
    for descriptor in descriptors
        .iter()
        .filter(|op| op.entity_name == entity_name && op.name == op_name)
    {
        for param in &descriptor.required_params {
            match hints.iter().find(|(name, _)| *name == param.name.as_str()) {
                Some((_, hint)) if *hint != &param.type_hint => {
                    return Err(format!(
                        "operation '{entity_name}/{op_name}': parameter '{}' carries two \
                         different type hints across the descriptors that advertise this \
                         operation ({hint:?} and {:?}). The boundary parses entity references \
                         from the declaration, so which one wins would decide whether the \
                         parameter is parsed at all — make the declarations agree.",
                        param.name, param.type_hint
                    ));
                }
                Some(_) => {}
                None => {
                    // A reference-named parameter says one of exactly two
                    // things, and it says it HERE: `EntityId` (the boundary
                    // parses it) or `RowKey` (it names a row in this provider's
                    // own table). Any other hint under such a name leaves the
                    // boundary nothing to decide by, so it is refused rather
                    // than passed through.
                    let declares_its_role = matches!(
                        param.type_hint,
                        TypeHint::EntityId { .. }
                            | TypeHint::EntityIdOrRoot { .. }
                            | TypeHint::RowKey
                    );
                    let by_name = names_an_entity_reference(&param.name);
                    // The description is the second reading of the same
                    // question, and it reaches the references the name rule
                    // cannot see (`target`, `canonical`, `from`). Only
                    // `TypeHint::String` is read this way: a parameter that
                    // already declares its role has answered.
                    let by_description = param.type_hint == TypeHint::String
                        && description_calls_it_an_id(&param.description);
                    if (by_name || by_description) && !declares_its_role {
                        let said = if by_name {
                            "is named as an entity reference"
                        } else {
                            "is described as an id"
                        };
                        return Err(format!(
                            "operation '{entity_name}/{op_name}': parameter '{}' {said} but \
                             declared {:?}, so the operation boundary would pass it through \
                             unparsed. Declare `TypeHint::EntityId {{ entity_name: … }}`, \
                             `TypeHint::EntityIdOrRoot` if the ROOT is a legal value there, or \
                             `TypeHint::RowKey` if it keys a row in this provider's own table \
                             rather than naming an entity. If it is none of those, the \
                             description is what is wrong.",
                            param.name, param.type_hint
                        ));
                    }
                    hints.push((param.name.as_str(), &param.type_hint));
                }
            }
        }
    }
    Ok(hints
        .into_iter()
        .filter_map(|(name, hint)| match hint {
            TypeHint::EntityId { entity_name } => Some(EntityReferenceParam {
                name,
                entity_name,
                admits_root: false,
            }),
            TypeHint::EntityIdOrRoot { entity_name } => Some(EntityReferenceParam {
                name,
                entity_name,
                admits_root: true,
            }),
            _ => None,
        })
        .collect())
}

/// Registration gate over a whole descriptor set: [`entity_reference_params`]
/// for every `(entity, op)` pair in it.
///
/// A provider registered at runtime is a new declaration of operations the
/// boundary will parse, so the declaration is checked where it arrives rather
/// than when a caller first dispatches it.
pub fn validate_entity_references(descriptors: &[OperationDescriptor]) -> Result<(), String> {
    let mut pairs: Vec<(&str, &str)> = descriptors
        .iter()
        .map(|d| (d.entity_name.as_str(), d.name.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    for (entity_name, op_name) in pairs {
        entity_reference_params(descriptors, entity_name, op_name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
        let forward = entity_reference_params(&forward_order, "block", "indent")
            .expect("agreeing declarations");
        let reverse = entity_reference_params(&reverse_order, "block", "indent")
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
                entity_reference_params(&[first.clone(), second.clone()], "block", "indent")
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
        let found = entity_reference_params(std::slice::from_ref(&d), "block", "create")
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
}
