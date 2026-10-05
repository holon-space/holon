//! Load-time refusal of a vault profile that redeclares a typed computed field
//! (ruling D66.a).
//!
//! A typed field has one semantics on every seat (`Computation::eval`, the
//! planted SQL column, the live read seat). A vault profile's Rhai script under
//! the same name would replace it on the live seat alone.

use holon_api::TypeDefinition;
use holon_api::computation::Computation;

use crate::ParsedProfile;

#[derive(Debug, thiserror::Error)]
#[error(
    "profile '{profile}' for entity '{entity}' declares computed field '{field}', which the \
     type declares as a typed computation; a profile cannot replace it"
)]
pub struct TypedComputedFieldOverride {
    pub profile: String,
    pub entity: String,
    pub field: String,
}

/// Refuse `profile` (loaded from `profile_id`) if it declares a computed field
/// that `type_def` already declares as a typed computation. A field the type
/// serves by Rhai alone may be redeclared.
pub fn check_profile_computed_overrides(
    profile_id: &str,
    profile: &ParsedProfile,
    type_def: Option<&TypeDefinition>,
) -> Result<(), TypedComputedFieldOverride> {
    let Some(type_def) = type_def else {
        return Ok(());
    };
    for (name, spec) in type_def.computed_specs() {
        if profile.computed.contains_key(name)
            && !matches!(spec.computation(), Computation::Script(_))
        {
            return Err(TypedComputedFieldOverride {
                profile: profile_id.to_string(),
                entity: profile.entity_name.clone(),
                field: name.to_string(),
            });
        }
    }
    Ok(())
}
