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
//! A proof carries two independent facts:
//! [`holon_loro::sync_transport::MembershipProof::audience`] names the peer the
//! blob is destined for, and `chain` is an owner→…→grantee cert chain whose
//! terminal grantee is the **subject** — the party whose authorization the
//! envelope rests on. They are not the same principal in both directions, and
//! conflating them is what let a receiver→owner round be admitted under the
//! receiver's own read grant.
//!
//! ## Which capability an envelope needs
//! Every envelope is a CRDT update applied into the admitting peer's replica,
//! so the capability it needs follows from who the subject is relative to the
//! admitter:
//!
//! - **subject == admitter** — the owner is handing this peer state that peer
//!   is entitled to hold. That is a READ, and the admitter verifies *its own*
//!   authorization at *its own* clock. This is what makes revocation work with
//!   no online check: revocation is non-renewal, the cert lapses, every later
//!   envelope is refused.
//! - **subject != admitter** — a remote principal is writing into the
//!   admitter's container. That needs [`Capability::Write`] on the subject's
//!   chain, checked at the admitter's clock.
//!
//! That single rule is the ONLY capability gate on the inbound path;
//! [`crate::sync::pull_once`] imports whatever [`admit`] returns as `Import`
//! and re-checks nothing.
//!
//! ## What this layer does NOT prove
//! [`blob_canonical_bytes`] is an unkeyed hash, so it detects a mangled or
//! rebound blob but authenticates NO sender. The capability rule above is only
//! as strong as the transport's own peer authentication (the iroh QUIC
//! fingerprint recorded at enrollment). A relay-level attacker that can mint
//! envelopes can still replay a captured chain under a forged sender.

use holon_api::Clock;
use holon_loro::sync_transport::ContainerLogId;
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
    /// The membership proof did not parse. Structurally malformed input,
    /// refused at the boundary.
    RefuseMalformedProof { reason: String },
    /// The envelope is addressed to another peer. It may be perfectly valid
    /// there; it proves nothing here, so it is never imported.
    RefuseAudience {
        audience: Principal,
        admitter: Principal,
    },
    /// The proof covers a DIFFERENT object than the envelope rides on. A
    /// capability is granted over one container; spending it on another is not
    /// a lesser claim, it is a claim about something else entirely.
    RefuseContainer {
        container: ContainerLogId,
        selector: BlockId,
    },
    /// The chain is structurally sound but does not prove membership NOW —
    /// lapsed lease, broken delegation, bad cert signature.
    RefuseLease {
        principal: Principal,
        error: MembershipError,
        now_millis: i64,
    },
    /// Membership holds, but the subject's effective capabilities do not
    /// include the one this envelope exercises. Names the missing capability
    /// and what the chain actually conferred, so a misissued cert is
    /// diagnosable from the report alone.
    RefuseCapability {
        principal: Principal,
        missing: Capability,
        held: Capabilities,
    },
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
/// tuple that binds payload to container, sender, audience, and the cert chain.
/// Rebinding any of them — a relay replaying a blob into another container, or
/// swapping in a more capable chain — invalidates the signature.
pub fn blob_canonical_bytes(env: &Envelope) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(env.container.as_str().as_bytes());
    hasher.update(&env.sender.0.to_le_bytes());
    hasher.update(env.auth.audience.as_bytes());
    hasher.update(env.auth.selector.as_bytes());
    hasher.update(&env.auth.epoch.to_le_bytes());
    // The chain decides the subject, and the subject decides which capability is
    // required — so it must be covered or the requirement is forgeable.
    hasher.update(&env.auth.chain);
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
                env.auth.audience,
                env.auth.epoch,
                env.payload.len()
            ),
        };
    }

    let audience = Principal(env.auth.audience.clone());
    if audience != *ctx.receiver {
        return AdmitDecision::RefuseAudience {
            audience,
            admitter: ctx.receiver.clone(),
        };
    }

    // A capability is granted OVER an object. Verifying it against the selector
    // the sender chose, then importing into the container the envelope rode in
    // on, spends a grant for one object on another — so bind them here, once,
    // before anything is verified.
    let selector = BlockId(env.auth.selector.clone());
    if selector.0 != env.container.0 {
        return AdmitDecision::RefuseContainer {
            container: env.container.clone(),
            selector,
        };
    }

    // The subject comes OUT of the parse, not off an unsigned wire field, so
    // proof and claimant cannot disagree — and a chain with no terminal grantee
    // yields no subject to check instead of an unmet postcondition.
    let (chain, subject) = match parse_chain(&env.auth) {
        Ok(parsed) => parsed,
        Err(reason) => return AdmitDecision::RefuseMalformedProof { reason },
    };
    let required = required_capability(&subject, ctx.receiver);

    match verify_membership(&chain, &subject, &selector, ctx.clock, ctx.verifier) {
        Err(error) => AdmitDecision::RefuseLease {
            principal: subject,
            error,
            now_millis: ctx.clock.now_millis(),
        },
        Ok(capabilities) if !capabilities.contains(required) => AdmitDecision::RefuseCapability {
            principal: subject,
            missing: required,
            held: capabilities,
        },
        Ok(capabilities) => AdmitDecision::Import { capabilities },
    }
}

/// The one rule that turns "who is this envelope from" into "what must they
/// hold": ingesting state proved for MYSELF is a read; ingesting state proved
/// for someone else is that someone else writing into my replica.
fn required_capability(subject: &Principal, admitter: &Principal) -> Capability {
    if subject == admitter {
        Capability::Read
    } else {
        Capability::Write
    }
}

/// Decode the wire chain AND the subject it terminates at. Returning the two
/// together is what makes the subject's existence a parse result rather than an
/// assumption: a chain that carries no terminal grantee — zero bytes, or the
/// two bytes `[]`, which decode to a well-formed EMPTY list — cannot produce
/// one, so there is no caller-side unwrap to be wrong about.
fn parse_chain(auth: &MembershipProof) -> Result<(MembershipChain, Principal), String> {
    let unproven = || {
        format!(
            "membership proof addressed to `{}` over selector `{}` carries an EMPTY chain — an \
             unproven claim, not a claim to be trusted",
            auth.audience, auth.selector
        )
    };
    if auth.chain.is_empty() {
        return Err(unproven());
    }
    let certs: Vec<crate::lease::MembershipCert> =
        serde_json::from_slice(&auth.chain).map_err(|e| {
            format!(
                "membership chain addressed to `{}` over selector `{}` is not a decodable cert \
                 list: {e}",
                auth.audience, auth.selector
            )
        })?;
    let subject = certs.last().ok_or_else(unproven)?.grantee.clone();
    Ok((MembershipChain::new(certs), subject))
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
    use holon_loro::sync_transport::StablePeerId;

    use super::*;
    use crate::lease::Issuer;
    use crate::lease::Lease;
    use crate::lease::MembershipCert;
    use crate::policy::UnverifiedVerifier;
    use crate::types::UnverifiedAuthority;

    const SELECTOR: &str = "holon_tree";

    fn signed_envelope(audience: &str, chain: &MembershipChain) -> Envelope {
        let mut envelope = Envelope {
            container: ContainerLogId::root(),
            seq: None,
            kind: BlobKind::Update,
            sender: StablePeerId(11),
            payload: b"opaque".to_vec(),
            auth: MembershipProof {
                audience: audience.to_string(),
                selector: SELECTOR.to_string(),
                epoch: 0,
                chain: encode_chain(chain),
            },
            sig: BlobSig(Vec::new()),
            head: None,
        };
        envelope.sig = BlobSig(blob_canonical_bytes(&envelope));
        envelope
    }

    fn owner_chain(grantee: &str, start: i64, ttl: i64) -> MembershipChain {
        granting(grantee, Capabilities::read_only(), start, ttl)
    }

    fn granting(grantee: &str, caps: Capabilities, start: i64, ttl: i64) -> MembershipChain {
        MembershipChain::direct(MembershipCert::issue(
            BlockId(SELECTOR.into()),
            Principal(grantee.into()),
            Issuer::Owner,
            caps,
            false,
            Lease::starting_at(start, ttl),
            &UnverifiedAuthority,
        ))
    }

    fn read_write() -> Capabilities {
        Capabilities::of([Capability::Read, Capability::Write])
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
        let envelope = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        assert!(admit(&envelope, &ctx(&receiver, &clock)).is_import());
    }

    #[test]
    fn lapsed_lease_refuses_after_the_clock_advances() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let envelope = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        clock.advance(20_000);
        assert!(matches!(
            admit(&envelope, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseLease { .. }
        ));
    }

    #[test]
    fn tampered_payload_refuses_on_signature() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let mut envelope = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        envelope.payload = b"relay-injected".to_vec();
        assert!(matches!(
            admit(&envelope, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseSig { .. }
        ));
    }

    // -- the reverse leg: a peer writing into the ADMITTER's store --------------

    #[test]
    fn a_read_only_peers_write_into_the_owners_store_is_refused() {
        // The receiver holds an owner-issued READ-ONLY cert and pushes its own
        // delta to the owner. Ingesting it is the receiver WRITING into the
        // owner's replica, which its cert does not confer.
        let owner = Principal("owner".into());
        let clock = TestClock::new(1_000);
        let envelope = signed_envelope(
            "owner",
            &granting("receiver", Capabilities::read_only(), 1_000, 10_000),
        );
        let decision = admit(&envelope, &ctx(&owner, &clock));
        assert_eq!(
            decision,
            AdmitDecision::RefuseCapability {
                principal: Principal("receiver".into()),
                missing: Capability::Write,
                held: Capabilities::read_only(),
            },
            "a read-only peer's update must be refused by the owner, naming the missing \
             capability; got {decision:?}"
        );
    }

    #[test]
    fn a_read_write_peers_write_into_the_owners_store_is_admitted() {
        let owner = Principal("owner".into());
        let clock = TestClock::new(1_000);
        let envelope = signed_envelope("owner", &granting("receiver", read_write(), 1_000, 10_000));
        assert_eq!(
            admit(&envelope, &ctx(&owner, &clock)),
            AdmitDecision::Import {
                capabilities: read_write()
            }
        );
    }

    #[test]
    fn an_envelope_addressed_to_the_receiver_is_refused_by_the_owner() {
        // The reverse-leg audience defect in one assertion: reusing the receiver
        // as audience on a round the OWNER admits must authorize nothing.
        let owner = Principal("owner".into());
        let clock = TestClock::new(1_000);
        let envelope = signed_envelope(
            "receiver",
            &granting("receiver", read_write(), 1_000, 10_000),
        );
        assert_eq!(
            admit(&envelope, &ctx(&owner, &clock)),
            AdmitDecision::RefuseAudience {
                audience: Principal("receiver".into()),
                admitter: owner.clone(),
            }
        );
    }

    #[test]
    fn a_third_partys_read_only_chain_cannot_write_into_my_store() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let envelope = signed_envelope("receiver", &owner_chain("someone-else", 1_000, 10_000));
        assert!(matches!(
            admit(&envelope, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseCapability {
                missing: Capability::Write,
                ..
            }
        ));
    }

    // -- the capability must be scoped to the container it is spent on --------

    #[test]
    fn a_write_cert_for_one_container_is_refused_on_another_containers_log() {
        // A peer legitimately shared into `holon_tree`, holding a GENUINE
        // owner-issued read+write cert for it, presents that cert on a different
        // container's log. No forgery: the capability is simply being spent on an
        // object it was never granted over.
        let owner = Principal("owner".into());
        let clock = TestClock::new(1_000);
        let mut envelope =
            signed_envelope("owner", &granting("receiver", read_write(), 1_000, 10_000));
        envelope.container = ContainerLogId("private-journal".into());
        envelope.sig = BlobSig(blob_canonical_bytes(&envelope));
        let decision = admit(&envelope, &ctx(&owner, &clock));
        assert_eq!(
            decision,
            AdmitDecision::RefuseContainer {
                container: ContainerLogId("private-journal".into()),
                selector: BlockId(SELECTOR.into()),
            },
            "a cert proved over one container must not admit a delta on another \
             container's log; got {decision:?}"
        );
    }

    #[test]
    fn a_cert_for_the_containers_own_selector_is_admitted() {
        // The positive half: the binding must not refuse the honest path, where
        // the proof's selector and the envelope's container are the same object.
        let owner = Principal("owner".into());
        let clock = TestClock::new(1_000);
        let envelope = signed_envelope("owner", &granting("receiver", read_write(), 1_000, 10_000));
        assert_eq!(envelope.container.as_str(), SELECTOR);
        assert!(admit(&envelope, &ctx(&owner, &clock)).is_import());
    }

    #[test]
    fn a_swapped_in_more_capable_chain_refuses_on_signature() {
        // The chain decides the subject, and the subject decides the required
        // capability, so the chain must be covered by the canonical bytes: a
        // relay upgrading read-only to read-write must not produce an admissible
        // envelope.
        let owner = Principal("owner".into());
        let clock = TestClock::new(1_000);
        let mut envelope = signed_envelope(
            "owner",
            &granting("receiver", Capabilities::read_only(), 1_000, 10_000),
        );
        envelope.auth.chain = encode_chain(&granting("receiver", read_write(), 1_000, 10_000));
        assert!(matches!(
            admit(&envelope, &ctx(&owner, &clock)),
            AdmitDecision::RefuseSig { .. }
        ));
    }

    #[test]
    fn a_chain_that_decodes_to_zero_certs_refuses_instead_of_panicking() {
        // Two bytes, `[]`, decode to a well-formed EMPTY cert list. The
        // byte-level emptiness check does not catch it, and the subject is taken
        // from the chain's last cert — so this reached the boundary with no
        // cert, no key and no lease and panicked there. Only the container and
        // the audience must match, and both are attacker-chosen.
        let owner = Principal("owner".into());
        let clock = TestClock::new(1_000);
        let mut envelope =
            signed_envelope("owner", &granting("receiver", read_write(), 1_000, 10_000));
        envelope.auth.chain = b"[]".to_vec();
        envelope.sig = BlobSig(blob_canonical_bytes(&envelope));
        assert!(matches!(
            admit(&envelope, &ctx(&owner, &clock)),
            AdmitDecision::RefuseMalformedProof { .. }
        ));
    }

    #[test]
    fn empty_chain_refuses() {
        let receiver = Principal("receiver".into());
        let clock = TestClock::new(1_000);
        let mut envelope = signed_envelope("receiver", &owner_chain("receiver", 1_000, 10_000));
        envelope.auth.chain = Vec::new();
        envelope.sig = BlobSig(blob_canonical_bytes(&envelope));
        assert!(matches!(
            admit(&envelope, &ctx(&receiver, &clock)),
            AdmitDecision::RefuseMalformedProof { .. }
        ));
    }
}
