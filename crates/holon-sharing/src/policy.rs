//! Owner-signed policy objects + A7 disjointness (ADR 0028 §3 / D4 / H1 / A7).
//!
//! A [`Policy`] is a **durable authority object**: it is owner-signed (review
//! D4 — asymmetric authority through the H2 log, NOT symmetric CRDT merge) and
//! its selector binds a **stable block id, never a page name** (H6 — a rename
//! is an ordinary edit; a selector bound to a mutable name would silently move
//! content between audiences). Committing a policy enters a `PolicyEdit` in the
//! Inc-4 [`crate::log::CrossingLog`] (one log, one arbitration rule — review
//! A1) and enforces the **A7 disjointness invariant** (nested/overlapping
//! shares are forbidden in v1) as a fail-loud check *at commit*, not a runtime
//! scramble.
//!
//! ## Signing/verifying seam (OQ4) — DISCLOSED not-yet-verifiable
//! Signing rides [`crate::types::SigningAuthority`]; verifying rides
//! [`VerifyingAuthority`] here. Both are TRAITS — the REAL owner-identity key
//! backing is built by the parallel enrollment-ceremony lane. This module never
//! names a concrete authority. The [`UnverifiedVerifier`] stand-in mirrors
//! [`crate::types::UnverifiedAuthority`]'s blake3 hash so the policy/lease
//! LOGIC is exercisable end-to-end; it proves NO owner identity and MUST NOT be
//! treated as authorization.

use std::collections::BTreeSet;

use holon_api::Clock;
use serde::Deserialize;
use serde::Serialize;

use crate::lease::Lease;
use crate::lease::MembershipChain;
use crate::lease::verify_membership;
use crate::log::CrossingLog;
use crate::types::BlockId;
use crate::types::ContainerId;
use crate::types::LogEntryBody;
use crate::types::OwnerSig;
use crate::types::PolicyChange;
use crate::types::PolicyEdit;
use crate::types::PolicyEditId;
use crate::types::SigningAuthority;

/// A principal a policy grants to — an opaque **stable** identity string (a
/// peer key fingerprint / user id), NEVER a page name (H6: names are mutable,
/// so a principal or selector bound to one would silently move content between
/// audiences on rename).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Principal(pub String);

impl std::fmt::Display for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

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

/// Verification counterpart of [`crate::types::SigningAuthority`] (OQ4). The
/// REAL owner-identity key backing is built by the enrollment-ceremony lane;
/// this crate codes against the TRAIT so swapping in the real verifier needs no
/// change here.
pub trait VerifyingAuthority {
    /// True iff `sig` is a valid owner-identity signature over `payload`.
    fn verify(&self, payload: &[u8], sig: &OwnerSig) -> bool;
}

/// DISCLOSED stand-in mirroring [`crate::types::UnverifiedAuthority`]:
/// recompute the blake3 stand-in and compare. This does **not** prove owner
/// identity — any party can recompute the same hash — it only lets the
/// policy/lease LOGIC be exercised end-to-end until the real verifier lands.
/// MUST NOT be treated as authorization.
pub struct UnverifiedVerifier;

impl VerifyingAuthority for UnverifiedVerifier {
    fn verify(&self, payload: &[u8], sig: &OwnerSig) -> bool {
        OwnerSig(blake3::hash(payload).as_bytes().to_vec()) == *sig
    }
}

/// The durable authority object (ADR 0028 §3 / D4). Owner-signed via
/// [`SignedPolicy`]. `selector` binds a **stable block id** (H6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// The root block id of the shared subtree this policy governs (H6: id, not
    /// name).
    pub selector: BlockId,
    pub principals: BTreeSet<Principal>,
    pub capabilities: Capabilities,
    /// May the principals themselves issue sub-grants (delegation certs)?
    pub delegation: bool,
    /// The membership validity window (H8: membership is a lease, renewed or it
    /// lapses).
    pub lease: Lease,
}

/// A [`Policy`] plus its owner signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPolicy {
    pub policy: Policy,
    pub sig: OwnerSig,
}

impl SignedPolicy {
    /// Canonical bytes signed over — the whole policy, deterministically
    /// serialized (`BTreeSet`s + declaration-ordered enums make this stable).
    fn canonical_bytes(policy: &Policy) -> Vec<u8> {
        serde_json::to_vec(policy).expect("policy is serializable")
    }

    /// Owner-sign a policy.
    pub fn sign(policy: Policy, authority: &dyn SigningAuthority) -> Self {
        let sig = authority.sign(&Self::canonical_bytes(&policy));
        Self { policy, sig }
    }

    /// Verify the owner signature over the policy.
    pub fn verify(&self, verifier: &dyn VerifyingAuthority) -> bool {
        verifier.verify(&Self::canonical_bytes(&self.policy), &self.sig)
    }
}

/// The subtree-containment relation A7 disjointness is checked against. A
/// selector is the root block id of a shared subtree; two selectors **overlap**
/// iff one subtree contains the other's root (nesting — which includes
/// equality). The policy layer does not own the block tree, so the caller
/// supplies this relation (the container registry, Inc 3, will provide the real
/// one; tests use [`MapContainment`]).
pub trait SubtreeContainment {
    /// Does the subtree rooted at `selector` contain `block` (reflexive:
    /// `contains(x, x)` is always true)?
    fn contains(&self, selector: &BlockId, block: &BlockId) -> bool;
}

/// Two selectors are DISJOINT unless one nests inside the other. Reflexive
/// containment means equal selectors overlap.
fn selectors_overlap(a: &BlockId, b: &BlockId, rel: &dyn SubtreeContainment) -> bool {
    rel.contains(a, b) || rel.contains(b, a)
}

/// Loud rejection at policy-commit (A7 / fail-loud signing). Nothing is
/// recorded when this fires.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PolicyCommitError {
    #[error(
        "policy selector `{new}` overlaps existing shared selector `{existing}` \
         — nested/overlapping shares are forbidden in v1 (ADR 0028 A7); \
         selectors must be disjoint"
    )]
    OverlappingSelector { new: String, existing: String },
    #[error("policy for selector `{selector}` is not validly owner-signed")]
    Unsigned { selector: String },
}

/// The owner's committed policy overlay. Enforces A7 disjointness across all
/// live selectors and mirrors every commit into the crossing log.
#[derive(Debug, Default)]
pub struct PolicySet {
    policies: Vec<SignedPolicy>,
}

impl PolicySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn policies(&self) -> &[SignedPolicy] {
        &self.policies
    }

    pub fn selector_is_shared(&self, selector: &BlockId) -> bool {
        self.policies.iter().any(|p| &p.policy.selector == selector)
    }

    /// Commit a policy: verify the owner signature, enforce A7 disjointness
    /// against every existing selector, then record one `PolicyEdit` per
    /// principal in the crossing log AND retain the policy. Fail-loud on a bad
    /// signature or an overlapping selector — nothing is recorded on failure.
    pub fn commit(
        &mut self,
        signed: SignedPolicy,
        rel: &dyn SubtreeContainment,
        verifier: &dyn VerifyingAuthority,
        log: &CrossingLog,
    ) -> Result<(), PolicyCommitError> {
        if !signed.verify(verifier) {
            return Err(PolicyCommitError::Unsigned {
                selector: signed.policy.selector.0.clone(),
            });
        }
        for existing in &self.policies {
            if selectors_overlap(&signed.policy.selector, &existing.policy.selector, rel) {
                return Err(PolicyCommitError::OverlappingSelector {
                    new: signed.policy.selector.0.clone(),
                    existing: existing.policy.selector.0.clone(),
                });
            }
        }
        let container = ContainerId(signed.policy.selector.0.clone());
        for principal in &signed.policy.principals {
            let edit = PolicyEdit {
                edit_id: PolicyEditId(format!("grant:{}:{}", container.0, principal.0)),
                container: container.clone(),
                principal: principal.0.clone(),
                change: PolicyChange::Grant,
            };
            log.append_entry(LogEntryBody::PolicyEdit(edit));
        }
        self.policies.push(signed);
        Ok(())
    }
}

/// Who is attempting a grant (D4 authorization gate).
pub enum Granter {
    /// The owner — always authorized (subject to A7 at commit).
    Owner,
    /// A non-owner delegate, identified by a membership chain proving its own
    /// rights. It may re-grant only if its own leaf cert carries `delegation`
    /// and the requested capabilities are a subset of what it holds.
    Delegate(MembershipChain),
}

/// A grant a non-owner tried to make without delegation rights: recorded as a
/// request awaiting **owner** approval (ADR 0028 D4), NOT an effective grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    pub selector: BlockId,
    pub requested_by: Option<Principal>,
    pub principal: Principal,
    pub capabilities: Capabilities,
}

/// The verdict of [`evaluate_grant`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantOutcome {
    /// Authorized — the caller may proceed to build + commit the policy.
    Committed,
    /// Not authorized as an effective grant — becomes a pending owner request.
    Pending(PendingRequest),
}

/// Decide whether `granter` may effect a grant of `caps` over `selector` to
/// `grantee`, or whether it becomes a **pending request** (D4: a non-owner
/// without delegation rights). The owner is always authorized; a delegate is
/// authorized only when its own chain verifies at the clock's instant, its leaf
/// cert carries `delegation`, and `caps` is a subset of what it holds.
pub fn evaluate_grant(
    granter: &Granter,
    selector: &BlockId,
    grantee: &Principal,
    caps: &Capabilities,
    clock: &dyn Clock,
    verifier: &dyn VerifyingAuthority,
) -> GrantOutcome {
    match granter {
        Granter::Owner => GrantOutcome::Committed,
        Granter::Delegate(chain) => {
            let Some(leaf) = chain.certs.last() else {
                return pending(selector, None, grantee, caps);
            };
            let delegate = leaf.grantee.clone();
            // Authorize against the EFFECTIVE (intersected) caps the chain
            // actually confers — the value `verify_membership` returns — NOT
            // `leaf.capabilities`, which a delegate self-declares and could
            // inflate. A delegate re-grants only capabilities it truly holds.
            // (Defense-in-depth: `lease::issue_delegated_cert` already rejects
            // an inflated sub-cert at the source; this is the backstop for a
            // raw/malicious cert that bypassed that path.)
            match verify_membership(chain, &delegate, selector, clock, verifier) {
                Ok(effective) if leaf.delegation && caps.is_subset_of(&effective) => {
                    GrantOutcome::Committed
                }
                _ => pending(selector, Some(delegate), grantee, caps),
            }
        }
    }
}

fn pending(
    selector: &BlockId,
    requested_by: Option<Principal>,
    grantee: &Principal,
    caps: &Capabilities,
) -> GrantOutcome {
    GrantOutcome::Pending(PendingRequest {
        selector: selector.clone(),
        requested_by,
        principal: grantee.clone(),
        capabilities: caps.clone(),
    })
}

/// A concrete [`SubtreeContainment`] over explicit subtree membership, for
/// tests and small owner-local sets. `descendants[root]` lists every block in
/// that subtree (the root itself is always contained — reflexive).
#[derive(Debug, Default)]
pub struct MapContainment {
    subtrees: std::collections::BTreeMap<BlockId, BTreeSet<BlockId>>,
}

impl MapContainment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare that `root`'s subtree contains exactly `members` (plus `root`).
    pub fn with_subtree(
        mut self,
        root: BlockId,
        members: impl IntoIterator<Item = BlockId>,
    ) -> Self {
        let set: BTreeSet<BlockId> = members
            .into_iter()
            .chain(std::iter::once(root.clone()))
            .collect();
        self.subtrees.insert(root, set);
        self
    }
}

impl SubtreeContainment for MapContainment {
    fn contains(&self, selector: &BlockId, block: &BlockId) -> bool {
        if selector == block {
            return true;
        }
        self.subtrees
            .get(selector)
            .is_some_and(|s| s.contains(block))
    }
}
