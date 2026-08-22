//! A capability profile: what ONE durable FORMAT can carry without loss.
//!
//! Data, never a type parameter — datatypes are runtime-declared, so
//! capability cannot be monomorphized (design Fork 2).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::axes::AssetsAxis;
use crate::axes::ComputedAxis;
use crate::axes::ContentAxis;
use crate::axes::HierarchyAxis;
use crate::axes::HostedKind;
use crate::axes::IdentityAxis;
use crate::axes::MutationAxis;
use crate::axes::OrderingAxis;
use crate::axes::PropertyKeysAxis;
use crate::axes::PropertyValuesAxis;
use crate::clause::ClauseId;
use crate::clause::EnforcementMap;
use crate::clause::Marker;

/// The name of a durable format. Equality IS the profile's identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityProfileId(String);

impl CapabilityProfileId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CapabilityProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A profile's revision — the CONTENT HASH of its parsed axes, computed at
/// load.
///
/// Deliberately NOT a hand-written yaml field. An author who edits a clause
/// and forgets to bump a hand-maintained number leaves every in-flight witness
/// passing against a profile that no longer says what those values were
/// checked against, which silently re-admits content the new profile forbids —
/// the exact failure a revision exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileRevision(String);

impl ProfileRevision {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProfileRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The ten fidelity axes, in draft §1.2 order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FidelityAxes {
    pub hosted_kinds: BTreeSet<HostedKind>,
    pub content: ContentAxis,
    pub property_keys: PropertyKeysAxis,
    pub property_values: PropertyValuesAxis,
    pub ordering: OrderingAxis,
    pub hierarchy: HierarchyAxis,
    pub identity: IdentityAxis,
    pub computed: ComputedAxis,
    pub mutation: MutationAxis,
    pub assets: AssetsAxis,
}

/// The yaml shape, before the revision is derived.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    profile: CapabilityProfileId,
    /// Clauses this profile STATES that nothing drives yet.
    ///
    /// DATA, not a `#`-comment, because the certifier gates on it: a clause
    /// that is neither probed nor listed here is itself a finding. A comment
    /// could never do that job, and 2b.1 proved a comment does not.
    /// Each entry carries the REASON nothing drives it — a bare marker is the
    /// same defect as a deferral with no site.
    #[serde(default)]
    not_yet_certified: Vec<Marker>,
    /// WHO enforces each clause. Required to cover every clause exactly once —
    /// a clause with no stated owner is a load error, because a defaulted
    /// owner is how the layer dimension would rot back into invisibility.
    enforced_by: EnforcementMap,
    fidelity_axes: FidelityAxes,
}

/// What one durable format can carry, as parsed, checked data.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityProfile {
    id: CapabilityProfileId,
    revision: ProfileRevision,
    not_yet_certified: BTreeMap<ClauseId, String>,
    enforced_by: EnforcementMap,
    fidelity: FidelityAxes,
    /// Where the yaml came from, when it came from a file. Reported with every
    /// run: two valid profiles produce two valid-looking reports, and without
    /// the path nothing distinguishes them.
    source: Option<PathBuf>,
}

impl CapabilityProfile {
    /// Parse the profile at `path`, recording where it came from.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let yaml = std::fs::read_to_string(path)
            .with_context(|| format!("reading the capability profile {}", path.display()))?;
        let mut profile = Self::from_yaml(&yaml)
            .with_context(|| format!("parsing the capability profile {}", path.display()))?;
        profile.source = Some(path.to_path_buf());
        Ok(profile)
    }

    /// Parse a profile yaml. The ONLY constructor.
    ///
    /// `deny_unknown_fields` throughout means an unrecognised key is an error
    /// here rather than a silently ignored section: a profile can never name
    /// an AXIS the vocabulary does not model.
    ///
    /// That is a weaker guarantee than it looks, and the difference matters. It
    /// stops a profile naming an axis nothing understands; it does NOT stop a
    /// profile making a false claim within a modelled axis whose clause the
    /// certifier does not drive. In 2b.1 only three clauses are driven
    /// (`reserved_prefixes`, `types`, `empty_string`) — the rest are marked
    /// `# NOT YET CERTIFIED (2b.2)` in the yaml and are documentation until
    /// then.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let doc: ProfileDocument =
            serde_yaml::from_str(yaml).context("invalid capability-profile yaml")?;
        doc.enforced_by.check_total().map_err(|e| {
            anyhow::anyhow!("profile '{}': enforced_by is incomplete — {e}", doc.profile)
        })?;
        let revision = revision_of(&doc.fidelity_axes).with_context(|| {
            format!(
                "hashing the parsed axes of profile '{}' failed",
                doc.profile
            )
        })?;
        let mut markers: BTreeMap<ClauseId, String> = BTreeMap::new();
        for marker in doc.not_yet_certified {
            if marker.reason.trim().is_empty() {
                anyhow::bail!(
                    "profile '{}': the `not_yet_certified` marker on {} has no reason — a bare \
                     marker asserts nothing a reader can check",
                    doc.profile,
                    marker.clause
                );
            }
            if markers.insert(marker.clause, marker.reason).is_some() {
                anyhow::bail!(
                    "profile '{}': {} is marked `not_yet_certified` twice",
                    doc.profile,
                    marker.clause
                );
            }
        }
        Ok(Self {
            id: doc.profile,
            revision,
            not_yet_certified: markers,
            enforced_by: doc.enforced_by,
            fidelity: doc.fidelity_axes,
            source: None,
        })
    }

    /// The file this profile was read from, if it was read from one.
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    pub fn id(&self) -> &CapabilityProfileId {
        &self.id
    }

    pub fn revision(&self) -> &ProfileRevision {
        &self.revision
    }

    /// Clauses this profile admits nothing drives, each with its reason.
    /// Excused from the coverage law, and ONLY these.
    pub fn not_yet_certified(&self) -> &BTreeMap<ClauseId, String> {
        &self.not_yet_certified
    }

    /// Just the clause names, for the coverage law.
    pub fn marked_clauses(&self) -> BTreeSet<ClauseId> {
        self.not_yet_certified.keys().copied().collect()
    }

    /// Which layer enforces each clause.
    pub fn enforced_by(&self) -> &EnforcementMap {
        &self.enforced_by
    }

    pub fn property_keys(&self) -> &PropertyKeysAxis {
        &self.fidelity.property_keys
    }

    pub fn property_values(&self) -> &PropertyValuesAxis {
        &self.fidelity.property_values
    }

    pub fn hosted_kinds(&self) -> &BTreeSet<HostedKind> {
        &self.fidelity.hosted_kinds
    }

    pub fn content(&self) -> &ContentAxis {
        &self.fidelity.content
    }

    pub fn ordering(&self) -> &OrderingAxis {
        &self.fidelity.ordering
    }

    pub fn hierarchy(&self) -> &HierarchyAxis {
        &self.fidelity.hierarchy
    }

    pub fn identity(&self) -> &IdentityAxis {
        &self.fidelity.identity
    }

    pub fn computed(&self) -> &ComputedAxis {
        &self.fidelity.computed
    }

    pub fn mutation(&self) -> &MutationAxis {
        &self.fidelity.mutation
    }

    pub fn assets(&self) -> &AssetsAxis {
        &self.fidelity.assets
    }
}

/// Hash the PARSED axes, not the yaml bytes: a comment edit or a reflow must
/// not invalidate witnesses, and two spellings of the same declaration must
/// not produce two revisions.
fn revision_of(fidelity: &FidelityAxes) -> Result<ProfileRevision> {
    let canonical =
        serde_yaml::to_string(fidelity).context("re-serializing the parsed axes failed")?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(ProfileRevision(hex::encode(&hasher.finalize()[..8])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::MINIMAL;
    use crate::fixture::minimal_with;

    #[test]
    fn a_yaml_key_the_vocabulary_does_not_know_is_a_load_error() {
        let with_unknown_axis = minimal_with(
            "  property_values:",
            "  nonexistent_axis:\n    whatever: 1\n  property_values:",
        );
        let err = CapabilityProfile::from_yaml(&with_unknown_axis)
            .expect_err("an axis the code does not check must not load")
            .to_string();
        assert!(
            err.contains("capability-profile yaml"),
            "the refusal must name what failed; got: {err}"
        );
    }

    /// The retired `vector_of_refs` must fail LOUDLY and say where the concept
    /// went — a profile written against the old vocabulary must not load and
    /// must not leave its author guessing.
    #[test]
    fn the_retired_vector_of_refs_names_the_axis_that_took_it_over() {
        let old = minimal_with("reference_values: none", "reference_values: vector_of_refs");
        // `{:#}` — the serde message is the CAUSE; the outer context only says
        // which file failed to parse.
        let err = format!(
            "{:#}",
            CapabilityProfile::from_yaml(&old)
                .expect_err("a retired vocabulary value must not load")
        );
        assert!(
            err.contains("multi_value"),
            "the refusal must point at the axis that governs cardinality; got: {err}"
        );
    }

    /// A marker with no reason is the same defect as a deferral with no site:
    /// it excuses a clause from the coverage law while asserting nothing.
    #[test]
    fn a_not_yet_certified_marker_without_a_reason_is_a_load_error() {
        let blank = minimal_with(
            "  - clause: hosted_kinds\n    reason: no stub in this crate drives it; the org \
             harness does",
            "  - clause: hosted_kinds\n    reason: \"  \"",
        );
        let err = CapabilityProfile::from_yaml(&blank)
            .expect_err("a marker with a blank reason must not load")
            .to_string();
        assert!(
            err.contains("has no reason"),
            "the refusal must say the marker carries no reason; got: {err}"
        );
    }

    /// The whole point of a content hash: an edit to a CLAUSE moves it, and a
    /// comment or a reflow does not.
    #[test]
    fn the_revision_tracks_the_clauses_and_ignores_the_formatting() {
        let base = CapabilityProfile::from_yaml(MINIMAL).expect("minimal profile parses");

        let commented = format!("# a comment nobody should hash\n{MINIMAL}");
        let same = CapabilityProfile::from_yaml(&commented).expect("commented profile parses");
        assert_eq!(
            base.revision(),
            same.revision(),
            "a comment is not a clause — it must not invalidate witnesses"
        );

        let widened = minimal_with("types: [string]", "types: [string, integer]");
        let other = CapabilityProfile::from_yaml(&widened).expect("widened profile parses");
        assert_ne!(
            base.revision(),
            other.revision(),
            "widening a declared value space MUST move the revision, or witnesses checked \
             against the narrow profile would silently survive it"
        );
    }
}
