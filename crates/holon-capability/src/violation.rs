//! What the certifier reports when a profile's declaration is not true.

use holon_api::Value;

use crate::axes::ValueKind;
use crate::profile::CapabilityProfileId;
use crate::profile::ProfileRevision;

/// Which fidelity axis the failing declaration belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    PropertyKeys,
    PropertyValues,
}

impl std::fmt::Display for Axis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::PropertyKeys => "property_keys",
            Self::PropertyValues => "property_values",
        })
    }
}

/// The specific clause that turned out not to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clause {
    /// The key was not covered by `reserved_prefixes`/`reserved_keys`, so the
    /// profile claims it survives as an ordinary property.
    KeyNotReserved,
    /// `property_values.types` lists this kind.
    TypeDeclared(ValueKind),
    /// `property_values.empty_string` says this.
    EmptyString,
}

impl std::fmt::Display for Clause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyNotReserved => write!(f, "the key is not declared reserved"),
            Self::TypeDeclared(kind) => write!(f, "property_values.types lists {kind:?}"),
            Self::EmptyString => write!(f, "property_values.empty_string"),
        }
    }
}

/// WHICH code path lost the value.
///
/// A format with more than one property carrier fails DIFFERENTLY per carrier
/// — org drops a non-string on its flat leg and stringifies it on its
/// `org_properties` JSON leg — so a violation that does not name the leg sends
/// the reader to the wrong function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leg(pub &'static str);

impl std::fmt::Display for Leg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// How the round trip broke the declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The value did not come back at all.
    Dropped,
    /// The value came back as something else — the accept-then-quietly-change
    /// outcome that is always red.
    Changed { got: Value },
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dropped => write!(f, "DROPPED"),
            Self::Changed { got } => write!(f, "CHANGED to {got:?}"),
        }
    }
}

/// One declared-but-broken finding: the profile said this survives, and it did
/// not.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub profile: CapabilityProfileId,
    pub rev: ProfileRevision,
    pub axis: Axis,
    pub clause: Clause,
    pub leg: Leg,
    pub key: String,
    pub sent: Value,
    pub outcome: Outcome,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}@{}] axis={} leg={} key={:?} sent={:?} -> {} (clause: {})",
            self.profile,
            self.rev,
            self.axis,
            self.leg,
            self.key,
            self.sent,
            self.outcome,
            self.clause,
        )
    }
}

/// One works-but-undeclared finding: the format is BETTER than the profile
/// admits.
///
/// Never a failure (CV-C) — it is a tightening prompt, and it reaches a human
/// through the ledger, never by failing a gate.
#[derive(Debug, Clone, PartialEq)]
pub struct TighteningPrompt {
    pub profile: CapabilityProfileId,
    pub axis: Axis,
    pub leg: Leg,
    pub key: String,
    pub sent: Value,
    pub note: String,
}

impl std::fmt::Display for TighteningPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] axis={} leg={} key={:?} sent={:?}: {}",
            self.profile, self.axis, self.leg, self.key, self.sent, self.note,
        )
    }
}
