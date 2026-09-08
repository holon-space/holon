//! The capability vocabulary of the sharing authorization boundary.
//!
//! Both admission legs decide the same question — "may this peer's update enter
//! my replica?" — and they must answer it in the same words. The relay leg
//! (`holon_sharing::acceptor::admit`) intersects a cert chain down to a
//! [`Capabilities`]; the iroh leg (`holon_loro::peer_import`) carries one out
//! of the enrollment roster. `holon-sharing` depends on `holon-loro`, so
//! neither can own the type: it lives here, in the layer under both, and
//! travels from the decision to the enforcement point as a value — never
//! re-parsed from a string at the point of use.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

/// One capability a membership confers. Declaration order is the canonical
/// (`Ord`) order, so a `BTreeSet<Capability>` serializes deterministically for
/// signing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Read,
    Write,
    Share,
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Share => "share",
        })
    }
}

/// A deterministically-ordered capability set (canonical for signing).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities(pub BTreeSet<Capability>);

impl Capabilities {
    pub fn of(caps: impl IntoIterator<Item = Capability>) -> Self {
        Self(caps.into_iter().collect())
    }

    pub fn read_only() -> Self {
        Self::of([Capability::Read])
    }

    /// What an own-device peer holds: it both reads the vault and authors into
    /// it. Distinct from [`Self::read_only`] because a third-party share must
    /// never confer `Write` by default.
    pub fn read_write() -> Self {
        Self::of([Capability::Read, Capability::Write])
    }

    pub fn contains(&self, cap: Capability) -> bool {
        self.0.contains(&cap)
    }

    /// The intersection — a delegate can never confer MORE than it itself
    /// holds, so effective capabilities down a chain are the running
    /// intersection.
    pub fn intersect(&self, other: &Self) -> Self {
        Self(self.0.intersection(&other.0).copied().collect())
    }

    /// True iff every capability in `self` is also in `other` (`self ⊆ other`).
    pub fn is_subset_of(&self, other: &Self) -> bool {
        self.0.is_subset(&other.0)
    }
}

/// Renders as the set a refusal message must name, e.g. `{read}` — so a
/// misissued grant is diagnosable from the log line alone.
impl std::fmt::Display for Capabilities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("{")?;
        for (i, cap) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{cap}")?;
        }
        f.write_str("}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_does_not_confer_write() {
        assert!(!Capabilities::read_only().contains(Capability::Write));
        assert!(Capabilities::read_write().contains(Capability::Write));
    }

    #[test]
    fn intersect_never_grows_a_grant() {
        let held = Capabilities::read_only();
        let asked = Capabilities::read_write();
        assert_eq!(held.intersect(&asked), Capabilities::read_only());
        assert!(held.intersect(&asked).is_subset_of(&held));
    }

    #[test]
    fn a_refusal_can_name_the_set_it_refused() {
        assert_eq!(Capabilities::read_write().to_string(), "{read, write}");
        assert_eq!(Capabilities::default().to_string(), "{}");
    }
}
