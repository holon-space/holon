//! A capability profile: what ONE durable FORMAT can carry without loss.
//!
//! Data, never a type parameter — datatypes are runtime-declared, so
//! capability cannot be monomorphized (design Fork 2).

use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

use crate::axes::PropertyKeysAxis;
use crate::axes::PropertyValuesAxis;

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

/// The ten fidelity axes. Increment 2b.1 carries two of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FidelityAxes {
    pub property_keys: PropertyKeysAxis,
    pub property_values: PropertyValuesAxis,
}

/// The yaml shape, before the revision is derived.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    profile: CapabilityProfileId,
    fidelity_axes: FidelityAxes,
}

/// What one durable format can carry, as parsed, checked data.
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilityProfile {
    id: CapabilityProfileId,
    revision: ProfileRevision,
    fidelity: FidelityAxes,
}

impl CapabilityProfile {
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
        let revision = revision_of(&doc.fidelity_axes).with_context(|| {
            format!(
                "hashing the parsed axes of profile '{}' failed",
                doc.profile
            )
        })?;
        Ok(Self {
            id: doc.profile,
            revision,
            fidelity: doc.fidelity_axes,
        })
    }

    pub fn id(&self) -> &CapabilityProfileId {
        &self.id
    }

    pub fn revision(&self) -> &ProfileRevision {
        &self.revision
    }

    pub fn property_keys(&self) -> &PropertyKeysAxis {
        &self.fidelity.property_keys
    }

    pub fn property_values(&self) -> &PropertyValuesAxis {
        &self.fidelity.property_values
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

    const MINIMAL: &str = r#"
profile: test
fidelity_axes:
  property_keys:
    charset: no_whitespace
    case: sensitive
    reserved_prefixes: ["_"]
    reserved_keys: [ID]
    collision: last_wins
    schema_required: open
  property_values:
    types: [string]
    empty_string: representable
    null: dropped
    multi_value:
      kind: none
    reference_values: by_id
"#;

    #[test]
    fn a_yaml_key_the_vocabulary_does_not_know_is_a_load_error() {
        let with_unknown_axis = MINIMAL.replace(
            "  property_values:",
            "  ordering:\n    sibling_order: file_position\n  property_values:",
        );
        let err = CapabilityProfile::from_yaml(&with_unknown_axis)
            .expect_err("an axis the code does not check must not load")
            .to_string();
        assert!(
            err.contains("capability-profile yaml"),
            "the refusal must name what failed; got: {err}"
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

        let widened = MINIMAL.replace("types: [string]", "types: [string, integer]");
        let other = CapabilityProfile::from_yaml(&widened).expect("widened profile parses");
        assert_ne!(
            base.revision(),
            other.revision(),
            "widening a declared value space MUST move the revision, or witnesses checked \
             against the narrow profile would silently survive it"
        );
    }
}
