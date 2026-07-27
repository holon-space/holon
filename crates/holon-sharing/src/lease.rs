//! Lease-based membership + owner→delegate→peer delegation chains
//! (ADR 0028 D4 / H8 / A6).
//!
//! Membership is a **lease**, not a standing grant: a short-lived cert (days)
//! that must be actively renewed by the owner (or a delegate whose OWN
//! delegation cert is unexpired) before it lapses. **Revocation = non-renewal**
//! — there is deliberately no `revoke`; you stop renewing and the lease
//! expires. A peer proves membership by presenting an owner→delegate→peer chain
//! that verifies at the current instant; any broken link, expired lease, or bad
//! signature makes membership **unclaimable** (loud `Err`), never silently
//! granted.
//!
//! ## Clock discipline (senior-review amendment)
//! Every expiry decision reads the repo's injected [`holon_api::Clock`] seam —
//! **never `SystemTime::now()` inside this module**. That is what lets the
//! expiry/renewal PBTs drive time deterministically (`TestClock`) instead of
//! degrading to unit tests.

use holon_api::Clock;
use serde::Deserialize;
use serde::Serialize;

use crate::policy::Capabilities;
use crate::policy::Capability;
use crate::policy::Principal;
use crate::policy::VerifyingAuthority;
use crate::types::BlockId;
use crate::types::OwnerSig;
use crate::types::SigningAuthority;

/// A short-lived membership validity window, measured on the injected clock (ms
/// since the Unix epoch — never the ambient system clock). Half-open
/// `[issued_at, expires_at)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub issued_at_millis: i64,
    pub expires_at_millis: i64,
}

impl Lease {
    /// A lease starting at `now_millis`, valid for `ttl_millis` (must be > 0 —
    /// a zero/negative TTL is a caller bug, asserted loudly).
    pub fn starting_at(now_millis: i64, ttl_millis: i64) -> Self {
        assert!(
            ttl_millis > 0,
            "lease TTL must be positive, got {ttl_millis}"
        );
        Self {
            issued_at_millis: now_millis,
            expires_at_millis: now_millis + ttl_millis,
        }
    }

    /// True iff the lease is active at the clock's current instant
    /// (`issued_at <= now < expires_at`).
    pub fn is_active_at(&self, clock: &dyn Clock) -> bool {
        let now = clock.now_millis();
        self.issued_at_millis <= now && now < self.expires_at_millis
    }
}

/// Who signed a membership cert into being — the first structural gate of chain
/// verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Issuer {
    /// The owner-identity key — the root of every valid chain.
    Owner,
    /// A delegate principal, itself holding an unexpired cert with
    /// `delegation = true`.
    Delegate(Principal),
}

/// A signed membership cert: proof that `grantee` holds `capabilities` over the
/// subtree selected by `selector`, for the `lease` window, granted by `issuer`.
/// `delegation` = the grantee may itself issue sub-certs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipCert {
    pub selector: BlockId,
    pub grantee: Principal,
    pub issuer: Issuer,
    pub capabilities: Capabilities,
    pub delegation: bool,
    pub lease: Lease,
    pub sig: OwnerSig,
}

impl MembershipCert {
    /// Canonical bytes signed over — every field except the signature itself.
    fn canonical_bytes(
        selector: &BlockId,
        grantee: &Principal,
        issuer: &Issuer,
        capabilities: &Capabilities,
        delegation: bool,
        lease: &Lease,
    ) -> Vec<u8> {
        serde_json::to_vec(&(selector, grantee, issuer, capabilities, delegation, lease))
            .expect("cert body is serializable")
    }

    /// Issue (and sign) a fresh cert.
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        selector: BlockId,
        grantee: Principal,
        issuer: Issuer,
        capabilities: Capabilities,
        delegation: bool,
        lease: Lease,
        authority: &dyn SigningAuthority,
    ) -> Self {
        let sig = authority.sign(&Self::canonical_bytes(
            &selector,
            &grantee,
            &issuer,
            &capabilities,
            delegation,
            &lease,
        ));
        Self {
            selector,
            grantee,
            issuer,
            capabilities,
            delegation,
            lease,
            sig,
        }
    }

    fn signature_valid(&self, verifier: &dyn VerifyingAuthority) -> bool {
        verifier.verify(
            &Self::canonical_bytes(
                &self.selector,
                &self.grantee,
                &self.issuer,
                &self.capabilities,
                self.delegation,
                &self.lease,
            ),
            &self.sig,
        )
    }

    /// **Auto-renew** (H8): re-issue this cert with a fresh lease window,
    /// keeping the same selector/grantee/issuer/capabilities/delegation. The
    /// caller supplies the authority — the owner may always renew; a delegate
    /// may renew only while its own delegation cert is unexpired (the caller
    /// verifies that first via [`verify_membership`]). **Not calling this is
    /// how revocation happens** (revocation = non-renewal).
    pub fn renew(
        &self,
        now_millis: i64,
        ttl_millis: i64,
        authority: &dyn SigningAuthority,
    ) -> Self {
        Self::issue(
            self.selector.clone(),
            self.grantee.clone(),
            self.issuer.clone(),
            self.capabilities.clone(),
            self.delegation,
            Lease::starting_at(now_millis, ttl_millis),
            authority,
        )
    }
}

/// An ordered owner→…→grantee delegation chain. `certs[0]` must be
/// [`Issuer::Owner`]; each subsequent cert's `issuer` must be
/// `Delegate(prev.grantee)` and `prev.delegation` must be true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipChain {
    pub certs: Vec<MembershipCert>,
}

impl MembershipChain {
    pub fn new(certs: Vec<MembershipCert>) -> Self {
        Self { certs }
    }

    /// A single owner-issued cert — the common one-hop case.
    pub fn direct(cert: MembershipCert) -> Self {
        Self { certs: vec![cert] }
    }
}

/// Why a membership claim is UNCLAIMABLE. Every variant is loud (never a silent
/// deny) so a misissued or lapsed chain is debuggable.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum MembershipError {
    #[error("empty chain proves no membership")]
    Empty,
    #[error("chain root must be owner-issued, got a delegate cert for `{grantee}`")]
    RootNotOwner { grantee: String },
    #[error(
        "broken delegation link at cert {index}: issuer does not match \
         delegator `{expected}`"
    )]
    BrokenLink { index: usize, expected: String },
    #[error("delegator `{delegator}` (cert {index}) lacks delegation rights")]
    NoDelegationRight { index: usize, delegator: String },
    #[error("cert {index} for `{grantee}` has an invalid owner/delegate signature")]
    BadSignature { index: usize, grantee: String },
    #[error(
        "cert {index} for `{grantee}` lease is inactive at {now}ms \
         (expired or not yet valid) — membership lapsed"
    )]
    LeaseInactive {
        index: usize,
        grantee: String,
        now: i64,
    },
    #[error(
        "cert {index} binds selector `{selector}` but the chain is for \
         `{expected}` — a delegate cannot grant outside the owner's selector"
    )]
    SelectorMismatch {
        index: usize,
        selector: String,
        expected: String,
    },
    #[error("chain terminates at `{end}`, not the claimant `{claimant}`")]
    ClaimantMismatch { claimant: String, end: String },
}

/// Verify that `claimant` holds membership over `selector`, at the clock's
/// current instant, through this owner→delegate→peer chain. Returns the
/// **effective capabilities** (the running intersection down the chain — a
/// delegate never confers more than it holds) on success. Any structural break,
/// expired lease, or bad signature makes membership **unclaimable** (loud
/// `Err`) — never silently granted.
pub fn verify_membership(
    chain: &MembershipChain,
    claimant: &Principal,
    selector: &BlockId,
    clock: &dyn Clock,
    verifier: &dyn VerifyingAuthority,
) -> Result<Capabilities, MembershipError> {
    let certs = &chain.certs;
    if certs.is_empty() {
        return Err(MembershipError::Empty);
    }

    let mut effective: Option<Capabilities> = None;
    for (index, cert) in certs.iter().enumerate() {
        if cert.selector != *selector {
            return Err(MembershipError::SelectorMismatch {
                index,
                selector: cert.selector.0.clone(),
                expected: selector.0.clone(),
            });
        }
        if !cert.signature_valid(verifier) {
            return Err(MembershipError::BadSignature {
                index,
                grantee: cert.grantee.0.clone(),
            });
        }
        if !cert.lease.is_active_at(clock) {
            return Err(MembershipError::LeaseInactive {
                index,
                grantee: cert.grantee.0.clone(),
                now: clock.now_millis(),
            });
        }

        match (index, &cert.issuer) {
            (0, Issuer::Owner) => {}
            (0, Issuer::Delegate(_)) => {
                return Err(MembershipError::RootNotOwner {
                    grantee: cert.grantee.0.clone(),
                });
            }
            (_, issuer) => {
                let delegator = &certs[index - 1];
                let expected = Issuer::Delegate(delegator.grantee.clone());
                if *issuer != expected {
                    return Err(MembershipError::BrokenLink {
                        index,
                        expected: delegator.grantee.0.clone(),
                    });
                }
                if !delegator.delegation {
                    return Err(MembershipError::NoDelegationRight {
                        index,
                        delegator: delegator.grantee.0.clone(),
                    });
                }
            }
        }

        effective = Some(match effective {
            None => cert.capabilities.clone(),
            Some(acc) => acc.intersect(&cert.capabilities),
        });
    }

    let end = &certs[certs.len() - 1].grantee;
    if end != claimant {
        return Err(MembershipError::ClaimantMismatch {
            claimant: claimant.0.clone(),
            end: end.0.clone(),
        });
    }

    Ok(effective.expect("non-empty chain always sets effective caps"))
}

/// Why a delegated sub-cert cannot be issued (fail-loud at the SOURCE —
/// parse-don't-validate: an inflated cert is never even constructed through the
/// checked path).
#[derive(Debug, thiserror::Error)]
pub enum DelegationError {
    #[error("delegator's own chain does not verify: {0}")]
    DelegatorUnverified(#[from] MembershipError),
    #[error("delegator `{delegator}` holds no delegation right and cannot issue sub-certs")]
    NotDelegable { delegator: String },
    #[error(
        "delegator `{delegator}` cannot confer capabilities it does not hold — \
         requested {requested:?} exceeds its effective {effective:?}"
    )]
    CapsExceedDelegator {
        delegator: String,
        requested: Vec<Capability>,
        effective: Vec<Capability>,
    },
}

/// Issue (and sign) a **delegated sub-cert**, rejecting at the SOURCE any
/// attempt to confer more than the delegator itself holds
/// (parse-don't-validate: the inflated cert is never constructed). The
/// delegator proves its own rights via `delegator_chain`; the sub-cert must (a)
/// come from a delegator carrying `delegation`, and (b) request capabilities
/// that are a subset of the delegator's **effective** (intersected) caps. The
/// sub-cert binds the same selector and appends `Delegate(delegator)` as its
/// issuer.
///
/// This is the checked counterpart to the raw [`MembershipCert::issue`]
/// (which, like `CrossingLog::append_entry`, models an entry that may arrive
/// from a buggy/malicious device); [`crate::policy::evaluate_grant`]'s
/// intersection check is the defense-in-depth backstop for anything that
/// bypasses this path.
#[allow(clippy::too_many_arguments)]
pub fn issue_delegated_cert(
    delegator_chain: &MembershipChain,
    new_grantee: Principal,
    new_capabilities: Capabilities,
    new_delegation: bool,
    lease: Lease,
    clock: &dyn Clock,
    verifier: &dyn VerifyingAuthority,
    authority: &dyn SigningAuthority,
) -> Result<MembershipCert, DelegationError> {
    let leaf = delegator_chain.certs.last().ok_or(MembershipError::Empty)?;
    let delegator = leaf.grantee.clone();
    let selector = leaf.selector.clone();

    let effective = verify_membership(delegator_chain, &delegator, &selector, clock, verifier)?;

    if !leaf.delegation {
        return Err(DelegationError::NotDelegable {
            delegator: delegator.0.clone(),
        });
    }
    if !new_capabilities.is_subset_of(&effective) {
        return Err(DelegationError::CapsExceedDelegator {
            delegator: delegator.0.clone(),
            requested: new_capabilities.0.iter().copied().collect(),
            effective: effective.0.iter().copied().collect(),
        });
    }

    Ok(MembershipCert::issue(
        selector,
        new_grantee,
        Issuer::Delegate(delegator),
        new_capabilities,
        new_delegation,
        lease,
        authority,
    ))
}
