//! Offer-time: what a home makes POSSIBLE, asked before the user has produced
//! anything.
//!
//! Deliberately NOT a check and deliberately witness-free (draft §4.3 point 6).
//! [`CapabilityProfile::check`] answers "is THIS content conforming" and costs
//! a round trip's worth of reasoning; `supports` answers "would content of this
//! shape be carried at all" and is a field read. Keeping them apart is what
//! stops the profile becoming a per-keystroke validator on the render path,
//! which the 200ms p95 SLO does not have room for.
//!
//! Per CV-A this layer stays DESCRIPTIVE: `Feature` names a capability of the
//! FORMAT, never an affordance of a frontend. The mapping from `Feature` to a
//! button lives in the consumer's `AffordanceTable`, because a certifier can
//! only falsify round-trip claims — a UI claim in the yaml would be an
//! untestable assertion.

use crate::axes::Attachments;
use crate::axes::BlockConstruct;
use crate::axes::ComputedPersisted;
use crate::axes::Extension;
use crate::axes::HostedKind;
use crate::axes::InlineConstruct;
use crate::axes::Reparent;
use crate::axes::SiblingOrder;
use crate::axes::ValueKind;
use crate::axes::WriteLeg;
use crate::profile::CapabilityProfile;

/// One thing a user might want to do, in the vocabulary of the FORMAT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feature {
    /// Home an entity of this shape here at all.
    Host(HostedKind),
    /// Write this inline mark.
    Inline(InlineConstruct),
    /// Write this block-level construct.
    Block(BlockConstruct),
    /// Store a property whose value has this kind.
    PropertyValue(ValueKind),
    /// Store a property under this exact key.
    PropertyKey(String),
    /// Reorder siblings DURABLY — a reorder the format cannot persist is worse
    /// than one it refuses, because the user sees it work and then lose it.
    ReorderSiblings,
    /// Move an entity to a different parent.
    Reparent,
    /// Persist a computed field of this result kind.
    ComputedPersisted(ValueKind),
    /// Mutate anything at all.
    Mutate,
    /// Attach a file with this extension.
    Attach(Extension),
}

impl Feature {
    /// Whether this feature MUTATES. `write_leg: absent` un-offers exactly
    /// this set, which is the axis-9 assertion.
    pub fn is_mutating(&self) -> bool {
        match self {
            Self::Mutate
            | Self::ReorderSiblings
            | Self::Reparent
            | Self::Inline(_)
            | Self::Block(_)
            | Self::PropertyValue(_)
            | Self::PropertyKey(_)
            | Self::ComputedPersisted(_)
            | Self::Attach(_) => true,
            // Asking whether a shape can LIVE here reads nothing and writes
            // nothing.
            Self::Host(_) => false,
        }
    }
}

/// The answer to an offer-time question.
///
/// TWO variants, not the draft's three. `OfferedViaRehoming { target }` needs a
/// target profile to name, and naming one requires `CapabilityProfile::diff`,
/// which arrives with the second and third yamls in 2b.3. Shipping an
/// unconstructible variant would be a promise nothing can keep — the same
/// defect as a clause nothing drives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Offered,
    NotOffered { reason: String },
}

impl Support {
    pub fn is_offered(&self) -> bool {
        matches!(self, Self::Offered)
    }

    fn no(reason: impl Into<String>) -> Self {
        Self::NotOffered {
            reason: reason.into(),
        }
    }
}

impl CapabilityProfile {
    /// What this home makes possible. A pure read of the profile — no witness,
    /// no round trip, no I/O.
    pub fn supports(&self, feature: &Feature) -> Support {
        // A format with no write leg un-offers every mutating feature, whatever
        // the other axes say. Checked FIRST so a read-only home cannot be
        // talked into offering a write by a permissive content axis.
        if feature.is_mutating() && self.mutation().write_leg == WriteLeg::Absent {
            return Support::no("this home has no write leg — it is read-only");
        }

        match feature {
            Feature::Host(kind) => {
                if self.hosted_kinds().contains(kind) {
                    Support::Offered
                } else {
                    Support::no(format!("this home cannot host a {kind:?} entity"))
                }
            }
            Feature::Inline(c) => {
                if self.content().inline_constructs.contains(c) {
                    Support::Offered
                } else {
                    Support::no(format!("{c:?} is not carried by this format"))
                }
            }
            Feature::Block(c) => {
                if self.content().block_constructs.contains(c) {
                    Support::Offered
                } else {
                    Support::no(format!("{c:?} is not carried by this format"))
                }
            }
            Feature::PropertyValue(kind) => {
                if self.property_values().types.contains(kind) {
                    Support::Offered
                } else {
                    Support::no(format!(
                        "a {kind:?} property value has no representation here"
                    ))
                }
            }
            Feature::PropertyKey(key) => {
                if self.property_keys().is_prefix_reserved(key) {
                    Support::no(format!("`{key}` carries a prefix this format erases"))
                } else if self.property_keys().is_owned(key) {
                    Support::no(format!("`{key}` is owned by the format itself"))
                } else {
                    Support::Offered
                }
            }
            Feature::ReorderSiblings => match self.ordering().sibling_order {
                SiblingOrder::Unordered => {
                    Support::no("this format does not model sibling order at all")
                }
                _ => Support::Offered,
            },
            Feature::Reparent => match self.hierarchy().reparent {
                Reparent::None => Support::no("this format does not allow reparenting"),
                // `Constrained` still OFFERS: the constraints are per-move, and
                // refusing the whole affordance because SOME move is illegal
                // would hide a feature that mostly works.
                Reparent::Free | Reparent::Constrained => Support::Offered,
            },
            Feature::ComputedPersisted(kind) => match &self.computed().computed_persisted {
                ComputedPersisted::None => {
                    Support::no("this home cannot persist a computed field at all")
                }
                ComputedPersisted::FullAlgebra => Support::Offered,
                ComputedPersisted::StringOnly => {
                    if *kind == ValueKind::String {
                        Support::Offered
                    } else {
                        Support::no(format!(
                            "this home persists only string-valued computed fields, not {kind:?}"
                        ))
                    }
                }
                ComputedPersisted::TypedSubset { types } => {
                    if types.contains(kind) {
                        Support::Offered
                    } else {
                        Support::no(format!("this home cannot persist a computed {kind:?}"))
                    }
                }
            },
            // Reached only when the write leg is present (guarded above).
            Feature::Mutate => Support::Offered,
            Feature::Attach(ext) => match self.assets().attachments {
                Attachments::None => Support::no("this format carries no attachments"),
                Attachments::InlineReference | Attachments::ManagedStore => {
                    if self.assets().extensions.contains(ext) {
                        Support::Offered
                    } else {
                        Support::no(format!(
                            "`{}` is not among the extensions this format carries",
                            ext.as_str()
                        ))
                    }
                }
            },
        }
    }
}
