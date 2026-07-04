//! The **sans-IO admission decision** for an inbound sync envelope.
//!
//! [`admit`] is a pure function of `(envelope, receiver identity, clock,
//! verifier)`. It has no transport, no registry, and no async — so the SAME
//! function serves the in-process relay, an HTTPS blind relay, and the iroh
//! device mesh, and a PBT can drive every refusal path without a network.
//!
//! ## Refusals are decisions, not errors
//! A refusal is an EXPECTED outcome the caller must observe and record (the
//! plan's fail-loud taxonomy): transport breakage is an enriched `Err`, an
//! unauthorized envelope is a typed [`AdmitDecision`]. Nothing is silently
//! dropped — [`crate::sync::SyncReport`] carries every refusal back to the
//! caller.
//!
//! ## Whose membership is proved
//! [`holon_loro::sync_transport::MembershipProof::principal`] names the
//! **audience** principal the blob is destined for, and `chain` is the
//! owner→…→that-principal cert chain. So the receiver verifies *its own*
//! authorization at *its own* clock. That is what makes revocation work without
//! any online check: revocation is non-renewal, so the receiver's cert simply
//! lapses and every subsequent envelope is refused.

use holon_api::Clock;
use holon_loro::sync_transport::Envelope;
use holon_loro::sync_transport::MembershipProof;

use crate::lease::MembershipChain;
use crate::lease::MembershipError;
use crate::lease::verify_membership;
use crate::policy::Capabilities;
use crate::policy::Capability;
use crate::policy::Principal;
use crate::policy::VerifyingAuthority;
use crate::types::BlockId;

/// What the receiver decided about ONE envelope. Every non-`Import` variant
/// names the envelope and the reason, so a refusal is debuggable from the
/// report alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitDecision {
    /// Authorized: the caller may import `payload` into the container's doc.
    /// `capabilities` is the effective (chain-intersected) capability set.
    Import { capabilities: Capabilities },
    /// The blob signature is absent or does not match the envelope's canonical
    /// bytes — the relay (or anyone between) altered or injected content.
    RefuseSig { reason: String },
    /// The membership proof did not parse, or claims a principal this receiver
    /// is not. Structurally malformed input, refused at the boundary.
    RefuseMalformedProof { reason: String },
    /// The chain is structurally sound but does not prove membership NOW —
    /// lapsed lease, broken delegation, bad cert signature.
    RefuseLease {
        principal: Principal,
        error: MembershipError,
        now_millis: i64,
    },
    /// Membership holds but confers no read capability over the selector.
    RefuseCapability { principal: Principal },
}

impl AdmitDecision {
    pub fn is_import(&self) -> bool {
        matches!(self, Self::Import { .. })
    }
}

/// Everything the receiver knows that is NOT on the wire.
pub struct AcceptorContext<'a> {
    /// Who this receiver is. An envelope addressed to another principal is
    /// refused (it proves nothing about us).
    pub receiver: &'a Principal,
    /// The receiver's clock — every expiry decision reads this seam, never
    /// `SystemTime::now` (so a PBT drives revocation deterministically).
    pub clock: &'a dyn Clock,
    /// Verifies owner/delegate cert signatures.
    pub verifier: &'a dyn VerifyingAuthority,
}

/// The canonical bytes a [`holon_loro::sync_transport::BlobSig`] signs: the
/// tuple that binds payload to container, sender, and audience. Rebinding any
/// of them (a relay replaying a blob into another container) invalidates the
/// signature.
pub fn blob_canonical_bytes(env: &Envelope) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(env.container.as_str().as_bytes());
    hasher.update(&env.sender.0.to_le_bytes());
    hasher.update(env.auth.principal.as_bytes());
    hasher.update(env.auth.selector.as_bytes());
    hasher.update(&env.auth.epoch.to_le_bytes());
    hasher.update(&env.payload);
    hasher.finalize().as_bytes().to_vec()
}

/// Decide whether this envelope may be imported. Pure; no IO, no async.
pub fn admit(env: &Envelope, ctx: &AcceptorContext<'_>) -> AdmitDecision {
    let expected = blob_canonical_bytes(env);
    if env.sig.0 != expected {
        return AdmitDecision::RefuseSig {
            reason: format!(
                "blob signature does not cover this envelope's (container={}, sender={}, \
                 audience={}, epoch={}, {}-byte payload) — the blob was altered or replayed into \
                 another container",
                env.container,
                env.sender.0,
                env.auth.principal,
                env.auth.epoch,
                env.payload.len()
            ),
        };
    }

    let claimant = Principal(env.auth.principal.clone());
    if claimant != *ctx.receiver {
        return AdmitDecision::RefuseMalformedProof {
            reason: format!(
                "envelope on container `{}` proves membership for `{claimant}`, but this receiver \
                 is `{}` — a chain for someone else authorizes nothing here",
                env.container, ctx.receiver
            ),
        };
    }

    let chain = match parse_chain(&env.auth) {
        Ok(chain) => chain,
        Err(reason) => return AdmitDecision::RefuseMalformedProof { reason },
    };
    let selector = BlockId(env.auth.selector.clone());

    match verify_membership(&chain, &claimant, &selector, ctx.clock, ctx.verifier) {
        Err(error) => AdmitDecision::RefuseLease {
            principal: claimant,
            error,
            now_millis: ctx.clock.now_millis(),
        },
        Ok(capabilities) if !capabilities.contains(Capability::Read) => {
            AdmitDecision::RefuseCapability {
                principal: claimant,
            }
        }
        Ok(capabilities) => AdmitDecision::Import { capabilities },
    }
}

fn parse_chain(auth: &MembershipProof) -> Result<MembershipChain, String> {
    if auth.chain.is_empty() {
        return Err(format!(
            "membership proof for `{}` over selector `{}` carries an EMPTY chain — an unproven \
             claim, not a claim to be trusted",
            auth.principal, auth.selector
        ));
    }
    serde_json::from_slice::<Vec<crate::lease::MembershipCert>>(&auth.chain)
        .map(MembershipChain::new)
        .map_err(|e| {
            format!(
                "membership chain for `{}` over selector `{}` is not a decodable cert list: {e}",
                auth.principal, auth.selector
            )
        })
}

/// Encode a chain into the wire form [`parse_chain`] expects.
pub fn encode_chain(chain: &MembershipChain) -> Vec<u8> {
    serde_json::to_vec(&chain.certs).expect("membership certs are serializable")
}

#[cfg(test)]
mod tests {
    use holon_api::TestClock;
    use holon_loro::sync_transport::BlobKind;
    use holon_loro::sync_transport::BlobSig;
    use holon_loro::sync_transport::ContainerLogId;
    use holon_loro::sync_transport::StablePeerId;

    use super::*;
    use crate::lease::Issuer;
    use crate::lease::Lease;
    use crate::lease::MembershipCert;
    use crate::policy::UnverifiedVerifier;
    use crate::types::UnverifiedAuthority;

    const SELECTOR: &str = "holon_tree";

    fn signed_envelope(principal: &str, chain: &MembershipChain) -> Envelope {
        let mut env = Envelope {
            container: ContainerLogId::root(),
            seq: None,
            kind: BlobKind::Update,
            sender: StablePeerId(11),
            payload: b"opaque".to_vec(),
            auth: MembershipProof {
                principal: principal.to_string(),
                selector: SELECTOR.to_string(),
                epoch: 0,
                chain: encode_chain(chain),
            },
            sig: BlobSig(Vec::new()),
            head: None,
        };
        env.sig = BlobSig(blob_canonical_bytes(&env));
        env
    }

    fn owner_chain(grantee: &str, start: i64, ttl: i64) -> MembershipChain {
        MembershipChain::direct(MembershipCert::issue(
            BlockId(SELECTOR.into()),
            Principal(grantee.into()),
            Issuer::Owner,
            Capabilities::read_only(),
            false,
            Lease::starting_at(start, ttl),
            &UnverifiedAuthority,
        ))
    }

    fn ctx<'a>(receiver: &'a Principal, clock: &'a TestClock) -> AcceptorContext<'a> {
        AcceptorContext {
            receiver,
            clock,
            verifier: &UnverifiedVerifier,
        }
    }

    #[test]
    fn valid_chain_at_a_live_lease_imports() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let env = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        assert!(admit(&env, &ctx(&receiver, &clock)).is_import());
    }

    #[test]
    fn lapsed_lease_refuses_after_the_clock_advances() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let env = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        clock.advance(20_000);
        assert!(matches!(
            admit(&env, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseLease { .. }
        ));
    }

    #[test]
    fn tampered_payload_refuses_on_signature() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let mut env = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        env.payload = b"relay-injected".to_vec();
        assert!(matches!(
            admit(&env, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseSig { .. }
        ));
    }

    #[test]
    fn chain_for_another_principal_refuses() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let env = signed_envelope("someone-else", &owner_chain("someone-else", 1_000, 10_000));
        assert!(matches!(
            admit(&env, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseMalformedProof { .. }
        ));
    }

    #[test]
    fn empty_chain_refuses() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let mut env = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        env.auth.chain = Vec::new();
        env.sig = BlobSig(blob_canonical_bytes(&env));
        assert!(matches!(
            admit(&env, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseMalformedProof { .. }
        ));
    }
}
