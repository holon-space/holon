//! Entity references at the operation surface.
//!
//! An operation parameter that addresses an entity — the subject `id`, or a
//! role name like `parent_id` / `block_id` — is declared
//! [`TypeHint::EntityId`], and the operation
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
use crate::Value;
use crate::entity::TypeDefinition;
use crate::render_types::OperationDescriptor;
use crate::render_types::OperationParam;
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

/// Whether a parameter is id-like yet declares no role for the reference —
/// exactly the declaration the registration rule refuses.
///
/// The ONE composition of the two readings and the role check: the rule
/// ([`entity_reference_params`]) refuses on it, and a schema-driven synthesizer
/// (the MCP tool mapper, which builds parameters no Holon author writes)
/// declares [`TypeHint::RowKey`] on it, so a parameter the rule would refuse is
/// a parameter it converts. Re-deriving either condition at either site is the
/// drift this closes: the name reading is UNCONDITIONAL, and only the
/// description reading is `String`-scoped.
pub fn id_like_but_undeclared(name: &str, description: &str, hint: &TypeHint) -> bool {
    let declares_its_role = matches!(
        hint,
        TypeHint::EntityId { .. } | TypeHint::EntityIdOrRoot { .. } | TypeHint::RowKey
    );
    !declares_its_role
        && (names_an_entity_reference(name)
            || (*hint == TypeHint::String && description_calls_it_an_id(description)))
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
/// descriptor that advertises it, required and optional alike. A
/// [`TypeHint::FieldValue`] param is a reference exactly when the field that
/// `param` (the dispatched params, by name) names is declared one.
///
/// The PAIR is the unit, not one descriptor: `block/set_field` is advertised
/// both by the block authority and by a `SqlOperationProvider`, so reading the
/// first match would make the boundary's behaviour depend on the order the
/// providers were registered in. Two descriptors that name the same parameter
/// must mean the same thing by it; when they do not, the disagreement is
/// refused by name instead of being resolved in favour of whichever the scan
/// reached first.
pub fn entity_reference_params<'a, 'p>(
    descriptors: &'a [OperationDescriptor],
    entity_name: &str,
    op_name: &str,
    param: impl Fn(&str) -> Option<&'p Value>,
) -> Result<Vec<EntityReferenceParam<'a>>, String> {
    let mut hints: Vec<(&str, &TypeHint)> = Vec::new();
    for descriptor in descriptors
        .iter()
        .filter(|op| op.entity_name == entity_name && op.name == op_name)
    {
        for param in descriptor
            .required_params
            .iter()
            .chain(&descriptor.optional_params)
        {
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
                    // than passed through. The description is the second
                    // reading of the same question, reaching the references the
                    // name rule cannot see (`target`, `canonical`, `from`).
                    if id_like_but_undeclared(&param.name, &param.description, &param.type_hint) {
                        let said = if names_an_entity_reference(&param.name) {
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
        .filter_map(|(name, hint)| reference(name, hint, &param))
        .collect())
}

/// The reference `name` declared `hint` is, if it is one.
fn reference<'a, 'p>(
    name: &'a str,
    hint: &'a TypeHint,
    param: &impl Fn(&str) -> Option<&'p Value>,
) -> Option<EntityReferenceParam<'a>> {
    match hint {
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
        TypeHint::FieldValue {
            field_param,
            fields,
        } => {
            let field = param(field_param)?.as_string()?;
            let declared = fields.iter().find(|f| f.name == field)?;
            reference(name, &declared.type_hint, param)
        }
        _ => None,
    }
}

/// The fields of `type_def`'s rows that reference an entity, as the params a
/// `create`, `update` or `set_field` carries them in. A field referencing the
/// type itself is a tree edge, and the root is a legal value there.
///
/// Refuses a target spelled as a Rust type name (`Project`) that is not the
/// type's own: only the self-reference has a known entity behind that name.
pub fn row_field_references(type_def: &TypeDefinition) -> Result<Vec<OperationParam>, String> {
    type_def
        .fields
        .iter()
        .filter(|field| !field.primary_key)
        .filter_map(|field| Some((field, field.reference_target.as_deref()?)))
        .map(|(field, target)| {
            // `#[reference(Block)]` spells the target as the Rust type.
            let type_hint = if target.eq_ignore_ascii_case(&type_def.name) {
                TypeHint::EntityIdOrRoot {
                    entity_name: EntityName::new(type_def.name.clone()),
                }
            } else if target.starts_with(|c: char| c.is_ascii_uppercase()) {
                return Err(format!(
                    "field '{}' of '{}' references '{target}', a type name rather than an \
                     entity name: name the referenced entity in lower case",
                    field.name, type_def.name
                ));
            } else {
                TypeHint::EntityId {
                    entity_name: EntityName::parse_named(target).map_err(|e| {
                        format!(
                            "field '{}' of '{}' references '{target}': {e}",
                            field.name, type_def.name
                        )
                    })?,
                }
            };
            Ok(OperationParam {
                name: field.name.clone(),
                type_hint,
                description: format!("Reference to a {target}"),
            })
        })
        .collect()
}

/// The entity references a `create` of `type_def` may carry: the new row's
/// own key, when the caller picks it, and its reference fields.
pub fn create_references(type_def: &TypeDefinition) -> Result<Vec<OperationParam>, String> {
    let own = OperationParam {
        name: type_def.primary_key.to_string(),
        type_hint: TypeHint::EntityId {
            entity_name: EntityName::new(type_def.name.clone()),
        },
        description: "Entity ID".to_string(),
    };
    Ok(std::iter::once(own)
        .chain(row_field_references(type_def)?)
        .collect())
}

/// The hint of `set_field`'s `value` on `type_def`: a reference exactly when
/// the written field is one.
pub fn set_field_value_hint(type_def: &TypeDefinition) -> Result<TypeHint, String> {
    Ok(TypeHint::FieldValue {
        field_param: "field".to_string(),
        fields: row_field_references(type_def)?,
    })
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
        entity_reference_params(descriptors, entity_name, op_name, |_| None)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
