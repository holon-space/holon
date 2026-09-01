//! CV-E: does a home actually offer what a type DECLARES it will store?
//!
//! CV-E's law (ruling 2026-08-22) is coerce iff the round trip is lossless,
//! else REFUSE. For a `computed_persisted` field the round trip is the planted
//! matview column, so the question is whether the home persists a computed
//! field of the kind the computation produces — answered by
//! [`Feature::ComputedPersisted`] against the home's own profile.
//!
//! One check, two seats ([`HomeSeat`]), because the two resolve the home
//! DIFFERENTLY and a shared entry point that guessed which one it was in would
//! be the bug this module exists to prevent. Declaration-time homes are
//! defaults a field may override; a re-home applies the DESTINATION to every
//! field, where a field-level default has no say.

use std::fmt;

use holon_api::ComputedTier;
use holon_api::FieldLifetime;
use holon_api::TypeDefinition;
use holon_api::computation::FieldKind;

use crate::axes::ValueKind;
use crate::profile::CapabilityProfileId;
use crate::registry::ProfileRegistry;
use crate::supports::Feature;
use crate::supports::Support;

/// Which seat is asking, and therefore how a field's home is resolved.
#[derive(Debug, Clone)]
pub enum HomeSeat {
    /// `declare_type`. Precedence: the field's own `home`, else the type's,
    /// else a refusal — a type declaring a `computed_persisted` field with no
    /// home has nothing to check against, and defaulting it would make the
    /// whole check vacuous.
    Declaration,
    /// A re-home. The destination governs EVERY field: declaration-time homes
    /// are defaults for where a type lands, not exemptions a field carries with
    /// it into a home that cannot hold it.
    Destination(CapabilityProfileId),
}

/// A field whose declared home cannot store it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeRefusal {
    pub type_name: String,
    pub field: String,
    pub home: CapabilityProfileId,
    pub reason: String,
}

impl fmt::Display for HomeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}.{}` cannot be a persisted computed field in home `{}`: {}",
            self.type_name, self.field, self.home, self.reason
        )
    }
}

impl std::error::Error for HomeRefusal {}

/// Refuse every `computed_persisted` field of `type_def` whose home cannot
/// persist the kind the computation produces.
///
/// Fields of any other tier are not consulted: nothing is planted for them, so
/// there is no home claim to break.
pub fn check_computed_persisted(
    registry: &ProfileRegistry,
    type_def: &TypeDefinition,
    seat: &HomeSeat,
) -> Result<(), HomeRefusal> {
    for field in &type_def.fields {
        let FieldLifetime::Computed { spec } = &field.lifetime else {
            continue;
        };
        if spec.tier() != ComputedTier::ComputedPersisted {
            continue;
        }

        let home = match seat {
            HomeSeat::Destination(destination) => destination.clone(),
            HomeSeat::Declaration => {
                let declared = field.home.as_ref().or(type_def.home.as_ref());
                match declared {
                    Some(id) => CapabilityProfileId::new(id.as_str()),
                    None => {
                        return Err(HomeRefusal {
                            type_name: type_def.name.clone(),
                            field: field.name.clone(),
                            home: CapabilityProfileId::new("<none>"),
                            reason: "the declaration names no home, and a persisted computed \
                                     field cannot be accepted against a home nobody stated"
                                .to_string(),
                        });
                    }
                }
            }
        };

        let kind = spec.result_kind().map_err(|e| HomeRefusal {
            type_name: type_def.name.clone(),
            field: field.name.clone(),
            home: home.clone(),
            reason: format!("its result type cannot be inferred, so no home can vouch for it: {e}"),
        })?;

        if let Some(reason) = unoffered_reason(registry, &home, kind)? {
            return Err(HomeRefusal {
                type_name: type_def.name.clone(),
                field: field.name.clone(),
                home,
                reason,
            });
        }
    }
    Ok(())
}

/// Why `home` does not offer a persisted computed field of `kind`, or `None`
/// when it does.
///
/// `Numeric` is asked TWICE. [`FieldKind`] does not separate integers from
/// floats and cannot: SQLite division makes an integer expression yield a
/// float, so the declaration genuinely admits both. A home must therefore
/// offer both to be safe for the column — asking only one would let a
/// `TypedSubset` home that carries `Integer` alone accept a column that can
/// hold a float.
fn unoffered_reason(
    registry: &ProfileRegistry,
    home: &CapabilityProfileId,
    kind: FieldKind,
) -> Result<Option<String>, HomeRefusal> {
    let value_kinds: &[ValueKind] = match kind {
        FieldKind::Text => &[ValueKind::String],
        FieldKind::Boolean => &[ValueKind::Boolean],
        FieldKind::Numeric => &[ValueKind::Integer, ValueKind::Float],
    };
    for value_kind in value_kinds {
        let support = registry
            .supports(home, &Feature::ComputedPersisted(*value_kind))
            .map_err(|e| HomeRefusal {
                type_name: String::new(),
                field: String::new(),
                home: home.clone(),
                reason: e.to_string(),
            })?;
        match support {
            Support::Offered => {}
            Support::NotOffered { reason } | Support::OfferedViaRehoming { reason, .. } => {
                return Ok(Some(reason));
            }
        }
    }
    Ok(None)
}

/// Refuse a `home:` that names no profile this build ships.
///
/// Runs for ANY declared home, whether or not the type has a computed field:
/// a typo must fail on the day it is authored, not on the day someone adds the
/// first `computed_persisted` field to that type.
pub fn check_declared_homes_exist(
    registry: &ProfileRegistry,
    type_def: &TypeDefinition,
) -> Result<(), UnknownHome> {
    let sites = std::iter::once((None, type_def.home.as_ref())).chain(
        type_def
            .fields
            .iter()
            .map(|f| (Some(f.name.as_str()), f.home.as_ref())),
    );
    for (field, home) in sites {
        let Some(home) = home else { continue };
        let id = CapabilityProfileId::new(home.as_str());
        if registry.get(&id).is_none() {
            return Err(UnknownHome {
                type_name: type_def.name.clone(),
                field: field.map(str::to_string),
                home: id,
                known: registry.ids().map(ToString::to_string).collect(),
            });
        }
    }
    Ok(())
}

/// A declared `home:` no registered profile answers for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHome {
    pub type_name: String,
    pub field: Option<String>,
    pub home: CapabilityProfileId,
    pub known: Vec<String>,
}

impl fmt::Display for UnknownHome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let site = match &self.field {
            Some(field) => format!("`{}.{field}`", self.type_name),
            None => format!("`{}`", self.type_name),
        };
        write!(
            f,
            "{site} declares home `{}`, which no registered capability profile answers for \
             (known: {})",
            self.home,
            self.known.join(", ")
        )
    }
}

impl std::error::Error for UnknownHome {}
