//! The known profiles, and the questions that need MORE THAN ONE of them.
//!
//! `CapabilityProfile::supports` stays a pure two-variant read of one profile
//! (2b.2). "Your home cannot do this, but that one can" is a different
//! question: it ranges over a SET of profiles, and a profile that knew its
//! peers would be a profile that changes meaning when a new format is added.
//! So re-homing lives here, on the registry, and nowhere else.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::diff::CapabilityLoss;
use crate::profile::CapabilityProfile;
use crate::profile::CapabilityProfileId;
use crate::supports::Feature;
use crate::supports::HomeSupport;
use crate::supports::Support;

/// Every profile this process knows, keyed by id.
#[derive(Debug, Clone, Default)]
pub struct ProfileRegistry {
    profiles: BTreeMap<CapabilityProfileId, CapabilityProfile>,
}

impl ProfileRegistry {
    /// Build a registry. A duplicate id is a LOAD ERROR: two profiles under one
    /// name means every later answer depends on which one won.
    pub fn new(profiles: impl IntoIterator<Item = CapabilityProfile>) -> Result<Self> {
        let mut map = BTreeMap::new();
        for profile in profiles {
            if let Some(previous) = map.insert(profile.id().clone(), profile) {
                anyhow::bail!(
                    "two profiles claim the id '{}' — a registry cannot answer for either",
                    previous.id()
                );
            }
        }
        Ok(Self { profiles: map })
    }

    /// Build a registry straight from profile yaml, `label` naming each
    /// document in a parse error.
    ///
    /// The parse belongs here so that assembling a registry — the shipped set
    /// below, or a bespoke one a consumer needs — never obliges a caller to
    /// hold a `CapabilityProfile` value of its own.
    pub fn from_yaml<'a>(documents: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self> {
        let mut profiles = Vec::new();
        for (label, yaml) in documents {
            profiles.push(
                CapabilityProfile::from_yaml(yaml).map_err(|e| {
                    anyhow::anyhow!("parsing the `{label}` capability profile: {e}")
                })?,
            );
        }
        Self::new(profiles)
    }

    pub fn get(&self, id: &CapabilityProfileId) -> Option<&CapabilityProfile> {
        self.profiles.get(id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &CapabilityProfileId> {
        self.profiles.keys()
    }

    /// What `home` makes possible, and — when it does not — whether ANOTHER
    /// known home does.
    ///
    /// An unknown `home` is an error, never a `NotOffered`: answering "no" for
    /// a profile the registry has never seen would turn a wiring mistake into
    /// a capability statement.
    pub fn supports(&self, home: &CapabilityProfileId, feature: &Feature) -> Result<Support> {
        let profile = self.profiles.get(home).ok_or_else(|| {
            anyhow::anyhow!("no profile registered under '{home}' — cannot answer for it")
        })?;
        match profile.supports(feature) {
            HomeSupport::Offered => Ok(Support::Offered),
            HomeSupport::NotOffered { reason } => {
                // The FIRST other home that offers it, in id order — a stable
                // answer rather than whichever profile happened to load first.
                match self
                    .profiles
                    .values()
                    .find(|p| p.id() != home && p.supports(feature).is_offered())
                {
                    Some(other) => Ok(Support::OfferedViaRehoming {
                        target: other.id().clone(),
                        reason,
                    }),
                    None => Ok(Support::NotOffered { reason }),
                }
            }
        }
    }

    /// What re-homing from `home` to `target` would COST.
    ///
    /// The price tag that belongs beside every re-homing offer: a suggestion to
    /// move without one is an invitation to lose data quietly.
    pub fn rehoming_cost(
        &self,
        home: &CapabilityProfileId,
        target: &CapabilityProfileId,
    ) -> Result<Vec<CapabilityLoss>> {
        let from = self
            .profiles
            .get(home)
            .ok_or_else(|| anyhow::anyhow!("no profile registered under '{home}'"))?;
        let to = self
            .profiles
            .get(target)
            .ok_or_else(|| anyhow::anyhow!("no profile registered under '{target}'"))?;
        Ok(from.diff(to))
    }
}

/// The profiles this build ships, embedded like the seed assets so a packaged
/// binary carries them.
///
/// Every id [`crate::profile_of`] can return must parse, so a home that cannot
/// be priced fails at startup rather than at the first move.
pub fn shipped_profiles() -> Result<ProfileRegistry> {
    ProfileRegistry::from_yaml([
        (
            "holon-native",
            include_str!("../../../assets/default/capability/holon-native.yaml"),
        ),
        ("org", include_str!("../../holon-org-format/profile.yaml")),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::MINIMAL;
    use crate::fixture::minimal_with;

    fn named(id: &str, yaml: &str) -> CapabilityProfile {
        CapabilityProfile::from_yaml(&yaml.replace("profile: test", &format!("profile: {id}")))
            .expect("fixture parses")
    }

    fn read_only(id: &str) -> CapabilityProfile {
        named(id, &minimal_with("write_leg: file", "write_leg: absent"))
    }

    #[test]
    fn a_duplicate_id_is_a_load_error() {
        let err = ProfileRegistry::new([named("twin", MINIMAL), named("twin", MINIMAL)])
            .expect_err("two profiles under one id must not build a registry")
            .to_string();
        assert!(err.contains("twin"), "the error must name the id: {err}");
    }

    /// An unknown home is an ERROR, not a "no". Answering for a profile nobody
    /// registered would report a wiring mistake as a capability fact.
    #[test]
    fn an_unknown_home_is_an_error_not_a_refusal() {
        let registry = ProfileRegistry::new([named("known", MINIMAL)]).expect("registry builds");
        let err = registry
            .supports(&CapabilityProfileId::new("ghost"), &Feature::Mutate)
            .expect_err("an unregistered home must not be answered for")
            .to_string();
        assert!(err.contains("ghost"), "the error must name it: {err}");
    }

    /// The re-homing answer, and the reason the home said no travels WITH it —
    /// otherwise the offer replaces the explanation.
    #[test]
    fn a_feature_the_home_refuses_is_offered_via_a_home_that_does_not() {
        let registry = ProfileRegistry::new([read_only("archive"), named("live", MINIMAL)])
            .expect("registry builds");
        let answer = registry
            .supports(&CapabilityProfileId::new("archive"), &Feature::Mutate)
            .expect("a registered home answers");
        match answer {
            Support::OfferedViaRehoming { target, reason } => {
                assert_eq!(target, CapabilityProfileId::new("live"));
                assert!(
                    !reason.is_empty(),
                    "the home's own reason must survive the offer"
                );
            }
            other => panic!("a writable peer exists, so re-homing must be offered: {other:?}"),
        }
    }

    /// With no peer that offers it, the answer stays a plain refusal — the
    /// registry must not invent a target.
    #[test]
    fn with_no_capable_peer_the_answer_is_still_no() {
        let registry = ProfileRegistry::new([read_only("archive"), read_only("cold")])
            .expect("registry builds");
        let answer = registry
            .supports(&CapabilityProfileId::new("archive"), &Feature::Mutate)
            .expect("a registered home answers");
        assert!(
            matches!(answer, Support::NotOffered { .. }),
            "no peer offers it, so there is nothing to re-home to: {answer:?}"
        );
    }

    /// Every offer must be able to state its price.
    #[test]
    fn re_homing_reports_what_the_move_would_cost() {
        let wide = named(
            "wide",
            &minimal_with("types: [string]", "types: [string, integer]"),
        );
        let registry = ProfileRegistry::new([wide, named("narrow", MINIMAL)]).expect("builds");
        let cost = registry
            .rehoming_cost(
                &CapabilityProfileId::new("wide"),
                &CapabilityProfileId::new("narrow"),
            )
            .expect("both homes are registered");
        assert!(
            cost.iter()
                .any(|l| l.clause == crate::clause::ClauseId::PropertyValuesTypes),
            "the price tag must name the clause that pays: {cost:?}"
        );
    }
}
