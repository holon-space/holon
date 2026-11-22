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

/// What ONE home answers. Two variants, and the type says so.
///
/// A profile cannot name another home — it does not know its peers, and one
/// that did would change meaning whenever a format was added. Re-homing is
/// therefore not merely "never produced here", it is UNREPRESENTABLE here:
/// [`Support`] is the registry's wider answer, and the two are different types
/// so no `match` has to carry an arm for a case that cannot occur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeSupport {
    Offered,
    NotOffered { reason: String },
}

impl HomeSupport {
    pub fn is_offered(&self) -> bool {
        matches!(self, Self::Offered)
    }

    fn no(reason: impl Into<String>) -> Self {
        Self::NotOffered {
            reason: reason.into(),
        }
    }
}

/// What the REGISTRY answers: a home's own verdict, plus the one thing only a
/// set of profiles can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Support {
    Offered,
    NotOffered {
        reason: String,
    },
    /// This home cannot, but `target` can. `reason` is the HOME's refusal,
    /// carried along so the offer never replaces the explanation.
    OfferedViaRehoming {
        target: crate::profile::CapabilityProfileId,
        reason: String,
    },
}

impl Support {
    pub fn is_offered(&self) -> bool {
        matches!(self, Self::Offered | Self::OfferedViaRehoming { .. })
    }
}

impl From<HomeSupport> for Support {
    fn from(home: HomeSupport) -> Self {
        match home {
            HomeSupport::Offered => Self::Offered,
            HomeSupport::NotOffered { reason } => Self::NotOffered { reason },
        }
    }
}

impl CapabilityProfile {
    /// What this home makes possible. A pure read of the profile — no witness,
    /// no round trip, no I/O.
    pub fn supports(&self, feature: &Feature) -> HomeSupport {
        // A format with no write leg un-offers every mutating feature, whatever
        // the other axes say. Checked FIRST so a read-only home cannot be
        // talked into offering a write by a permissive content axis.
        if feature.is_mutating() && self.mutation().write_leg == WriteLeg::Absent {
            return HomeSupport::no("this home has no write leg — it is read-only");
        }

        match feature {
            Feature::Host(kind) => {
                if self.hosted_kinds().contains(kind) {
                    HomeSupport::Offered
                } else {
                    HomeSupport::no(format!("this home cannot host a {kind:?} entity"))
                }
            }
            Feature::Inline(c) => {
                if self.content().inline_constructs.contains(c) {
                    HomeSupport::Offered
                } else {
                    HomeSupport::no(format!("{c:?} is not carried by this format"))
                }
            }
            Feature::Block(c) => {
                if self.content().block_constructs.contains(c) {
                    HomeSupport::Offered
                } else {
                    HomeSupport::no(format!("{c:?} is not carried by this format"))
                }
            }
            Feature::PropertyValue(kind) => {
                if self.property_values().types.contains(kind) {
                    HomeSupport::Offered
                } else {
                    HomeSupport::no(format!(
                        "a {kind:?} property value has no representation here"
                    ))
                }
            }
            Feature::PropertyKey(key) => {
                if self.property_keys().is_prefix_reserved(key) {
                    HomeSupport::no(format!("`{key}` carries a prefix this format erases"))
                } else if self.property_keys().is_owned(key) {
                    HomeSupport::no(format!("`{key}` is owned by the format itself"))
                } else if self.property_keys().is_engine_owned(key) {
                    HomeSupport::no(format!(
                        "`{key}` is minted by the format, which refuses an authored one"
                    ))
                } else {
                    HomeSupport::Offered
                }
            }
            Feature::ReorderSiblings => match self.ordering().sibling_order {
                SiblingOrder::Unordered => {
                    HomeSupport::no("this format does not model sibling order at all")
                }
                SiblingOrder::FilePosition
                | SiblingOrder::FractionalIndex
                | SiblingOrder::ExplicitInteger
                | SiblingOrder::LinkedList => HomeSupport::Offered,
            },
            Feature::Reparent => match self.hierarchy().reparent {
                Reparent::None => HomeSupport::no("this format does not allow reparenting"),
                // `Constrained` still OFFERS: the constraints are per-move, and
                // refusing the whole affordance because SOME move is illegal
                // would hide a feature that mostly works.
                Reparent::Free | Reparent::Constrained => HomeSupport::Offered,
            },
            Feature::ComputedPersisted(kind) => match &self.computed().computed_persisted {
                ComputedPersisted::None => {
                    HomeSupport::no("this home cannot persist a computed field at all")
                }
                ComputedPersisted::FullAlgebra => HomeSupport::Offered,
                ComputedPersisted::StringOnly => {
                    if *kind == ValueKind::String {
                        HomeSupport::Offered
                    } else {
                        HomeSupport::no(format!(
                            "this home persists only string-valued computed fields, not {kind:?}"
                        ))
                    }
                }
                ComputedPersisted::TypedSubset { types } => {
                    if types.contains(kind) {
                        HomeSupport::Offered
                    } else {
                        HomeSupport::no(format!("this home cannot persist a computed {kind:?}"))
                    }
                }
            },
            // Reached only when the write leg is present (guarded above).
            Feature::Mutate => HomeSupport::Offered,
            Feature::Attach(ext) => match self.assets().attachments {
                Attachments::None => HomeSupport::no("this format carries no attachments"),
                Attachments::InlineReference | Attachments::ManagedStore => {
                    if self.assets().extensions.contains(ext) {
                        HomeSupport::Offered
                    } else {
                        HomeSupport::no(format!(
                            "`{}` is not among the extensions this format carries",
                            ext.as_str()
                        ))
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::test_runner::Config;
    use proptest::test_runner::TestRunner;

    use super::*;
    use crate::fixture::KeyLists;
    use crate::fixture::strategies::lists_and_key;

    fn keys(members: &[&str]) -> Vec<String> {
        members.iter().map(|m| m.to_string()).collect()
    }

    /// `supports(PropertyKey)` offers exactly the keys no list refuses, and a
    /// refusal names the key it is about.
    fn offer_matches_the_refusal_set(lists: &KeyLists, key: &str) -> Result<(), String> {
        let profile = CapabilityProfile::from_yaml(&lists.minimal_profile_yaml())
            .map_err(|e| format!("a drawn key-list set must load: {e:#}"))?;
        let refused = lists
            .reserved_prefixes
            .iter()
            .any(|p| key.starts_with(p.as_str()))
            || lists.reserved_keys.iter().any(|k| k == key)
            || lists.engine_owned_keys.iter().any(|k| k == key);

        match (
            profile.supports(&Feature::PropertyKey(key.to_string())),
            refused,
        ) {
            (HomeSupport::Offered, false) => Ok(()),
            (HomeSupport::Offered, true) => Err(format!("`{key}` is refused but offered")),
            (HomeSupport::NotOffered { .. }, false) => {
                Err(format!("`{key}` is not refused but not offered"))
            }
            (HomeSupport::NotOffered { reason }, true) => reason
                .contains(&format!("`{key}`"))
                .then_some(())
                .ok_or(format!("the refusal does not name `{key}`: {reason}")),
        }
    }

    /// The property is checked on fixed cases first, then on drawn ones.
    /// Fixed cases are stored as explicit lists, not as seeds, so a change to
    /// the generator cannot turn them into other cases.
    #[test]
    fn a_property_key_is_offered_iff_no_key_list_refuses_it() {
        let fixed = [
            (
                KeyLists {
                    engine_owned_keys: keys(&["_provenance"]),
                    ..KeyLists::default()
                },
                "_provenance",
            ),
            (
                KeyLists {
                    reserved_prefixes: keys(&["_"]),
                    reserved_keys: keys(&["id"]),
                    engine_owned_keys: keys(&["_provenance"]),
                },
                "title",
            ),
        ];
        for (lists, key) in &fixed {
            offer_matches_the_refusal_set(lists, key).expect("fixed case");
        }

        let only_engine_owned = std::cell::Cell::new(0usize);
        TestRunner::new(Config::default())
            .run(&lists_and_key(), |(lists, key)| {
                let prefixed = lists
                    .reserved_prefixes
                    .iter()
                    .any(|p| key.starts_with(p.as_str()));
                only_engine_owned.set(
                    only_engine_owned.get()
                        + usize::from(
                            lists.engine_owned_keys.contains(&key)
                                && !lists.reserved_keys.contains(&key)
                                && !prefixed,
                        ),
                );
                offer_matches_the_refusal_set(&lists, &key)
                    .map_err(proptest::test_runner::TestCaseError::fail)
            })
            .expect("the offer agrees with the refusal set");
        assert!(
            only_engine_owned.get() > 0,
            "no drawn key was refused by the engine-owned list alone"
        );
    }
}
