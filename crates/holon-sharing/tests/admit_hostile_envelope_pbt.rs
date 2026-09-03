//! Hostile-envelope fuzz over [`holon_sharing::admit`], the sans-IO admission
//! boundary.
//!
//! The two-instance transport slice only ever presents envelopes its own honest
//! publisher minted, so no case there rebinds a container, swaps in a more
//! capable chain, or ships a chain that decodes to zero certs. This file
//! generates those directly: structurally valid chains with mutated fields, and
//! arbitrary bytes.
//!
//! [`model_admits`] re-derives the admission predicate from the envelope alone,
//! including its own copy of the canonical blob bytes — a binding field dropped
//! from the signed tuple diverges here instead of hiding behind the production
//! hasher.

use holon_api::TestClock;
use holon_loro::sync_transport::BlobKind;
use holon_loro::sync_transport::BlobSig;
use holon_loro::sync_transport::ContainerLogId;
use holon_loro::sync_transport::Envelope;
use holon_loro::sync_transport::HeadHash;
use holon_loro::sync_transport::MembershipProof;
use holon_loro::sync_transport::StablePeerId;
use holon_sharing::AcceptorContext;
use holon_sharing::AdmitDecision;
use holon_sharing::BlockId;
use holon_sharing::Capabilities;
use holon_sharing::Capability;
use holon_sharing::Issuer;
use holon_sharing::Lease;
use holon_sharing::MembershipCert;
use holon_sharing::MembershipError;
use holon_sharing::Principal;
use holon_sharing::UnverifiedAuthority;
use holon_sharing::UnverifiedVerifier;
use holon_sharing::admit;
use proptest::bool::weighted;
use proptest::prelude::*;
use proptest::sample::select;

const PRINCIPALS: [&str; 4] = ["owner", "peer-a", "peer-b", "stranger"];
const OBJECTS: [&str; 3] = ["holon_tree", "private-journal", "shared-notes"];
/// The selector a cert carries when it is NOT bound to the proof's object, and
/// the value [`Tamper::MoveSelector`] rebinds one to. Never drawn as a
/// container, so a rebinding is always observable.
const OFF_SELECTOR: &str = "off-selector";
/// The delegator named by a cert whose link into the chain is broken.
const FORGED_ISSUER: &str = "forged-delegator";
const NOW: i64 = 1_700_000_000_000;

// -- the wire the receiver actually sees --------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseWindow {
    Live,
    Expired,
    NotYetValid,
    /// `issued == expires`: the half-open window admits no instant at all.
    Empty,
}

impl LeaseWindow {
    fn lease(self, now: i64) -> Lease {
        match self {
            Self::Live => Lease {
                issued_at_millis: now - 1_000,
                expires_at_millis: now + 60_000,
            },
            Self::Expired => Lease {
                issued_at_millis: now - 60_000,
                expires_at_millis: now - 1,
            },
            Self::NotYetValid => Lease {
                issued_at_millis: now + 1_000,
                expires_at_millis: now + 60_000,
            },
            Self::Empty => Lease {
                issued_at_millis: now,
                expires_at_millis: now,
            },
        }
    }
}

/// A mutation applied AFTER the cert is signed — the shape a relay or a peer
/// with a captured cert can produce. Each one must invalidate the signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tamper {
    None,
    FlipSig,
    InflateCaps,
    MoveSelector,
    ExtendLease,
}

fn apply_tamper(cert: &mut MembershipCert, tamper: Tamper) {
    match tamper {
        Tamper::None => {}
        Tamper::FlipSig => cert.sig.0.iter_mut().for_each(|b| *b ^= 0xff),
        Tamper::InflateCaps => {
            cert.capabilities =
                Capabilities::of([Capability::Read, Capability::Write, Capability::Share]);
        }
        Tamper::MoveSelector => cert.selector = BlockId(OFF_SELECTOR.into()),
        Tamper::ExtendLease => cert.lease.expires_at_millis += 10_000_000,
    }
}

#[derive(Debug, Clone)]
struct CertDraw {
    grantee: String,
    selector_matches_proof: bool,
    caps: Vec<Capability>,
    delegation: bool,
    link_correct: bool,
    lease: LeaseWindow,
    tamper: Tamper,
}

fn assemble_certs(draws: &[CertDraw], proof_selector: &str, now: i64) -> Vec<MembershipCert> {
    let mut certs: Vec<MembershipCert> = Vec::with_capacity(draws.len());
    for (index, draw) in draws.iter().enumerate() {
        let issuer = match (index, draw.link_correct) {
            (0, true) => Issuer::Owner,
            (_, false) => Issuer::Delegate(Principal(FORGED_ISSUER.into())),
            (_, true) => Issuer::Delegate(certs[index - 1].grantee.clone()),
        };
        let selector = BlockId(if draw.selector_matches_proof {
            proof_selector.to_string()
        } else {
            OFF_SELECTOR.to_string()
        });
        let mut cert = MembershipCert::issue(
            selector,
            Principal(draw.grantee.clone()),
            issuer,
            Capabilities::of(draw.caps.iter().copied()),
            draw.delegation,
            draw.lease.lease(now),
            &UnverifiedAuthority,
        );
        apply_tamper(&mut cert, draw.tamper);
        certs.push(cert);
    }
    certs
}

#[derive(Debug, Clone)]
enum ChainWire {
    Certs(Vec<CertDraw>),
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SigMode {
    Covering,
    Corrupt,
    Absent,
}

#[derive(Debug, Clone)]
struct Scenario {
    receiver: String,
    container: String,
    selector_matches_container: bool,
    other_selector: String,
    audience_is_receiver: bool,
    other_audience: String,
    terminal_is_receiver: bool,
    wire: ChainWire,
    sig: SigMode,
    sender: u64,
    snapshot: bool,
    payload: Vec<u8>,
    head: Option<[u8; 32]>,
    epoch: u64,
}

impl Scenario {
    fn receiver(&self) -> Principal {
        Principal(self.receiver.clone())
    }

    fn envelope(&self) -> Envelope {
        let selector = if self.selector_matches_container {
            self.container.clone()
        } else {
            self.other_selector.clone()
        };
        let audience = if self.audience_is_receiver {
            self.receiver.clone()
        } else {
            self.other_audience.clone()
        };
        let chain = match &self.wire {
            ChainWire::Raw(bytes) => bytes.clone(),
            ChainWire::Certs(draws) => {
                let mut draws = draws.clone();
                if self.terminal_is_receiver {
                    draws
                        .last_mut()
                        .expect("the chain strategy draws at least one cert")
                        .grantee = self.receiver.clone();
                }
                serde_json::to_vec(&assemble_certs(&draws, &selector, NOW))
                    .expect("membership certs serialize")
            }
        };
        let mut envelope = Envelope {
            container: ContainerLogId(self.container.clone()),
            seq: None,
            kind: if self.snapshot {
                BlobKind::Snapshot
            } else {
                BlobKind::Update
            },
            sender: StablePeerId(self.sender),
            payload: self.payload.clone(),
            auth: MembershipProof {
                audience,
                selector,
                epoch: self.epoch,
                chain,
            },
            sig: BlobSig(Vec::new()),
            head: self.head.map(HeadHash),
        };
        envelope.sig = BlobSig(match self.sig {
            SigMode::Covering => model_blob_bytes(&envelope),
            SigMode::Corrupt => model_blob_bytes(&envelope)
                .into_iter()
                .map(|b| b ^ 0x5a)
                .collect(),
            SigMode::Absent => Vec::new(),
        });
        envelope
    }
}

// -- the model ----------------------------------------------------------------

/// The bytes a covering blob signature is taken over. Written out here rather
/// than called from the acceptor: the binding of payload to container, sender,
/// audience, selector, epoch and chain is exactly what the fuzz is testing.
fn model_blob_bytes(env: &Envelope) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(env.container.0.as_bytes());
    hasher.update(&env.sender.0.to_le_bytes());
    hasher.update(env.auth.audience.as_bytes());
    hasher.update(env.auth.selector.as_bytes());
    hasher.update(&env.auth.epoch.to_le_bytes());
    hasher.update(&env.auth.chain);
    hasher.update(&env.payload);
    hasher.finalize().as_bytes().to_vec()
}

/// The `UnverifiedAuthority` stand-in signs a cert body with an unkeyed blake3
/// hash, so the model verifies one the same way the receiver's verifier does.
fn model_cert_signature_holds(cert: &MembershipCert) -> bool {
    let body = serde_json::to_vec(&(
        &cert.selector,
        &cert.grantee,
        &cert.issuer,
        &cert.capabilities,
        cert.delegation,
        &cert.lease,
    ))
    .expect("cert body serializes");
    blake3::hash(&body).as_bytes().as_slice() == cert.sig.0.as_slice()
}

/// `Some(effective capabilities)` iff this envelope is admissible at `now`.
/// Independent of the acceptor: everything it reads comes off the envelope.
fn model_admits(env: &Envelope, receiver: &Principal, now: i64) -> Option<Capabilities> {
    if env.sig.0 != model_blob_bytes(env) {
        return None;
    }
    if env.auth.audience != receiver.0 {
        return None;
    }
    if env.auth.selector != env.container.0 {
        return None;
    }
    let Ok(certs) = serde_json::from_slice::<Vec<MembershipCert>>(&env.auth.chain) else {
        return None;
    };
    if certs.is_empty() {
        return None;
    }

    let mut effective: Option<Capabilities> = None;
    for (index, cert) in certs.iter().enumerate() {
        if cert.selector.0 != env.auth.selector {
            return None;
        }
        if !model_cert_signature_holds(cert) {
            return None;
        }
        if !(cert.lease.issued_at_millis <= now && now < cert.lease.expires_at_millis) {
            return None;
        }
        match (index, &cert.issuer) {
            (0, Issuer::Owner) => {}
            (0, Issuer::Delegate(_)) => return None,
            (_, issuer) => {
                let delegator = &certs[index - 1];
                if *issuer != Issuer::Delegate(delegator.grantee.clone()) {
                    return None;
                }
                if !delegator.delegation {
                    return None;
                }
            }
        }
        effective = Some(match effective {
            None => cert.capabilities.clone(),
            Some(acc) => acc.intersect(&cert.capabilities),
        });
    }

    let subject = &certs[certs.len() - 1].grantee;
    let required = required_capability(subject, receiver);
    let effective = effective.expect("a non-empty chain sets effective capabilities");
    effective.contains(required).then_some(effective)
}

/// Ingesting state proved for the receiver itself is a read; ingesting state
/// proved for anyone else is that party writing into the receiver's replica.
fn required_capability(subject: &Principal, receiver: &Principal) -> Capability {
    if subject == receiver {
        Capability::Read
    } else {
        Capability::Write
    }
}

// -- generators ---------------------------------------------------------------

fn capability_set() -> impl Strategy<Value = Vec<Capability>> {
    prop::collection::vec(
        select(vec![Capability::Read, Capability::Write, Capability::Share]),
        0..4,
    )
}

fn lease_window() -> impl Strategy<Value = LeaseWindow> {
    prop_oneof![
        6 => Just(LeaseWindow::Live),
        1 => Just(LeaseWindow::Expired),
        1 => Just(LeaseWindow::NotYetValid),
        1 => Just(LeaseWindow::Empty),
    ]
}

fn tamper_strategy() -> impl Strategy<Value = Tamper> {
    prop_oneof![
        8 => Just(Tamper::None),
        1 => Just(Tamper::FlipSig),
        1 => Just(Tamper::InflateCaps),
        1 => Just(Tamper::MoveSelector),
        1 => Just(Tamper::ExtendLease),
    ]
}

prop_compose! {
    fn cert_draw()(
        grantee in select(PRINCIPALS.to_vec()),
        selector_matches_proof in weighted(0.85),
        caps in capability_set(),
        delegation in weighted(0.8),
        link_correct in weighted(0.85),
        lease in lease_window(),
        tamper in tamper_strategy(),
    ) -> CertDraw {
        CertDraw {
            grantee: grantee.to_string(),
            selector_matches_proof,
            caps,
            delegation,
            link_correct,
            lease,
            tamper,
        }
    }
}

fn chain_wire() -> impl Strategy<Value = ChainWire> {
    prop_oneof![
        7 => prop::collection::vec(cert_draw(), 1..4).prop_map(ChainWire::Certs),
        1 => prop::collection::vec(any::<u8>(), 0..24).prop_map(ChainWire::Raw),
        1 => select(vec![
                Vec::new(),
                b"[]".to_vec(),
                b"  [ ]  ".to_vec(),
                b"null".to_vec(),
                b"{}".to_vec(),
                b"[{}]".to_vec(),
                b"[[]]".to_vec(),
            ]).prop_map(ChainWire::Raw),
    ]
}

prop_compose! {
    fn scenario()(
        receiver in select(PRINCIPALS.to_vec()),
        container in select(OBJECTS.to_vec()),
        selector in (weighted(0.6), select(OBJECTS.to_vec())),
        audience in (weighted(0.6), select(PRINCIPALS.to_vec())),
        terminal_is_receiver in weighted(0.5),
        wire in chain_wire(),
        sig in prop_oneof![7 => Just(SigMode::Covering), 2 => Just(SigMode::Corrupt), 1 => Just(SigMode::Absent)],
        transport in (any::<u64>(), any::<bool>(), prop::collection::vec(any::<u8>(), 0..6), prop::option::of(any::<[u8; 32]>()), any::<u64>()),
    ) -> Scenario {
        Scenario {
            receiver: receiver.to_string(),
            container: container.to_string(),
            selector_matches_container: selector.0,
            other_selector: selector.1.to_string(),
            audience_is_receiver: audience.0,
            other_audience: audience.1.to_string(),
            terminal_is_receiver,
            wire,
            sig,
            sender: transport.0,
            snapshot: transport.1,
            payload: transport.2,
            head: transport.3,
            epoch: transport.4,
        }
    }
}

// -- honest baselines the targeted properties perturb -------------------------

const CONTAINER: &str = "holon_tree";

/// An owner→…→`subject` chain with every link wired, every lease live and every
/// hop carrying `caps`.
fn honest_certs(hops: usize, caps: &Capabilities, subject: &str) -> Vec<MembershipCert> {
    let mut grantees: Vec<String> = (1..hops).map(|i| format!("delegate-{i}")).collect();
    grantees.push(subject.to_string());
    grantees
        .iter()
        .enumerate()
        .map(|(index, grantee)| {
            let issuer = if index == 0 {
                Issuer::Owner
            } else {
                Issuer::Delegate(Principal(grantees[index - 1].clone()))
            };
            MembershipCert::issue(
                BlockId(CONTAINER.into()),
                Principal(grantee.clone()),
                issuer,
                caps.clone(),
                true,
                LeaseWindow::Live.lease(NOW),
                &UnverifiedAuthority,
            )
        })
        .collect()
}

fn envelope_over(certs: &[MembershipCert], audience: &str) -> Envelope {
    let mut envelope = Envelope {
        container: ContainerLogId(CONTAINER.into()),
        seq: None,
        kind: BlobKind::Update,
        sender: StablePeerId(7),
        payload: b"opaque".to_vec(),
        auth: MembershipProof {
            audience: audience.to_string(),
            selector: CONTAINER.to_string(),
            epoch: 0,
            chain: serde_json::to_vec(certs).expect("membership certs serialize"),
        },
        sig: BlobSig(Vec::new()),
        head: None,
    };
    envelope.sig = BlobSig(model_blob_bytes(&envelope));
    envelope
}

fn decide(envelope: &Envelope, receiver: &Principal) -> AdmitDecision {
    let clock = TestClock::new(NOW);
    admit(
        envelope,
        &AcceptorContext {
            receiver,
            clock: &clock,
            verifier: &UnverifiedVerifier,
        },
    )
}

// -- properties ---------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// P1 + P2 + P3 + P4 over the hostile corpus. `admit` is total, so reaching
    /// the assertions at all is the refuse-or-import half of P1; the model is
    /// the exact oracle for which of the two.
    #[test]
    fn every_envelope_yields_a_decision_bound_to_the_container_audience_and_capability(
        scenario in scenario(),
    ) {
        let receiver = scenario.receiver();
        let envelope = scenario.envelope();
        let decision = decide(&envelope, &receiver);

        if let AdmitDecision::Import { capabilities } = &decision {
            prop_assert_eq!(
                &envelope.auth.selector, &envelope.container.0,
                "P2: imported an envelope whose proof covers another container"
            );
            prop_assert_eq!(
                &envelope.auth.audience, &receiver.0,
                "P3: imported an envelope addressed to another peer"
            );

            let certs: Vec<MembershipCert> = serde_json::from_slice(&envelope.auth.chain)
                .expect("an imported envelope's chain decodes");
            let subject = &certs
                .last()
                .expect("an imported envelope's chain terminates at a subject")
                .grantee;
            for cert in &certs {
                prop_assert!(
                    capabilities.is_subset_of(&cert.capabilities),
                    "P4: conferred {capabilities:?} through a cert holding only {:?}",
                    cert.capabilities
                );
            }
            prop_assert!(
                capabilities.contains(required_capability(subject, &receiver)),
                "P4: imported without the capability the envelope exercises \
                 (subject `{subject}`, receiver `{receiver}`, held {capabilities:?})"
            );
        }

        match (&decision, &model_admits(&envelope, &receiver, NOW)) {
            (AdmitDecision::Import { capabilities }, Some(expected)) => {
                prop_assert_eq!(
                    capabilities, expected,
                    "imported with capabilities the chain does not confer"
                );
            }
            (AdmitDecision::Import { capabilities }, None) => prop_assert!(
                false,
                "admitted an inadmissible envelope with {capabilities:?}: {envelope:?}"
            ),
            (refusal, Some(expected)) => prop_assert!(
                false,
                "refused an admissible envelope ({expected:?} conferred): {refusal:?}"
            ),
            (_, None) => {}
        }
    }

    /// The generator's admissible branch is reachable, so the properties above
    /// are not passing vacuously on a corpus that only ever refuses.
    #[test]
    fn an_honest_chain_terminating_at_the_receiver_imports(
        hops in 1usize..4,
        shares in any::<bool>(),
    ) {
        let receiver = Principal("peer-a".into());
        let caps = if shares {
            Capabilities::of([Capability::Read, Capability::Share])
        } else {
            Capabilities::read_only()
        };
        let envelope = envelope_over(&honest_certs(hops, &caps, &receiver.0), &receiver.0);
        prop_assert_eq!(
            decide(&envelope, &receiver),
            AdmitDecision::Import { capabilities: caps }
        );
    }

    /// P2, targeted: a capability proved over one container, spent on another
    /// container's log, is a claim about a different object.
    #[test]
    fn a_proof_for_one_container_is_refused_on_another_containers_log(
        hops in 1usize..4,
        other in select(vec!["private-journal", "shared-notes"]),
    ) {
        let receiver = Principal("peer-a".into());
        let mut envelope = envelope_over(
            &honest_certs(hops, &Capabilities::read_only(), &receiver.0),
            &receiver.0,
        );
        envelope.container = ContainerLogId(other.to_string());
        envelope.sig = BlobSig(model_blob_bytes(&envelope));
        prop_assert_eq!(
            decide(&envelope, &receiver),
            AdmitDecision::RefuseContainer {
                container: ContainerLogId(other.to_string()),
                selector: BlockId(CONTAINER.into()),
            }
        );
    }

    /// P3, targeted: an envelope addressed to another peer proves nothing here,
    /// however sound its chain.
    #[test]
    fn an_envelope_addressed_to_another_peer_is_refused(
        hops in 1usize..4,
        addressee in select(vec!["owner", "peer-b", "stranger"]),
    ) {
        let receiver = Principal("peer-a".into());
        let envelope = envelope_over(
            &honest_certs(hops, &Capabilities::read_write(), &receiver.0),
            addressee,
        );
        prop_assert_eq!(
            decide(&envelope, &receiver),
            AdmitDecision::RefuseAudience {
                audience: Principal(addressee.into()),
                admitter: receiver.clone(),
            }
        );
    }

    /// P4, targeted: a third party's chain writing into MY replica needs
    /// `Write`, and a chain that carries only `Read` never supplies it.
    #[test]
    fn a_third_partys_chain_writes_into_my_replica_only_with_write(
        hops in 1usize..4,
        grants_write in any::<bool>(),
    ) {
        let receiver = Principal("peer-a".into());
        let caps = if grants_write {
            Capabilities::read_write()
        } else {
            Capabilities::of([Capability::Read, Capability::Share])
        };
        let envelope = envelope_over(&honest_certs(hops, &caps, "stranger"), &receiver.0);
        let decision = decide(&envelope, &receiver);
        if grants_write {
            prop_assert_eq!(decision, AdmitDecision::Import { capabilities: caps });
        } else {
            prop_assert_eq!(
                decision,
                AdmitDecision::RefuseCapability {
                    principal: Principal("stranger".into()),
                    missing: Capability::Write,
                    held: caps,
                }
            );
        }
    }

    /// P5: one broken link anywhere in an otherwise honest chain refuses, and
    /// names the cert that broke.
    #[test]
    fn a_chain_with_one_broken_link_is_refused(
        hops in 1usize..4,
        victim in 0usize..3,
        tamper in prop_oneof![
            Just(Tamper::FlipSig),
            Just(Tamper::InflateCaps),
            Just(Tamper::MoveSelector),
            Just(Tamper::ExtendLease),
        ],
    ) {
        let receiver = Principal("peer-a".into());
        let index = victim % hops;
        let mut certs = honest_certs(hops, &Capabilities::read_only(), &receiver.0);
        apply_tamper(&mut certs[index], tamper);
        let envelope = envelope_over(&certs, &receiver.0);

        let expected = if tamper == Tamper::MoveSelector {
            MembershipError::SelectorMismatch {
                index,
                selector: OFF_SELECTOR.into(),
                expected: CONTAINER.into(),
            }
        } else {
            MembershipError::BadSignature {
                index,
                grantee: certs[index].grantee.0.clone(),
            }
        };
        prop_assert_eq!(
            decide(&envelope, &receiver),
            AdmitDecision::RefuseLease {
                principal: receiver.clone(),
                error: expected,
                now_millis: NOW,
            }
        );
    }

    /// P6: a chain carrying no terminal grantee is a claim about nobody. Both
    /// the byte-empty form and the forms that DECODE to an empty cert list are
    /// refused as malformed.
    #[test]
    fn a_chain_with_no_terminal_grantee_refuses_as_malformed(
        chain in select(vec![
            Vec::new(),
            b"[]".to_vec(),
            b"  [ ]  ".to_vec(),
            b"[\n]".to_vec(),
        ]),
    ) {
        let receiver = Principal("peer-a".into());
        let mut envelope = envelope_over(
            &honest_certs(1, &Capabilities::read_only(), &receiver.0),
            &receiver.0,
        );
        envelope.auth.chain = chain;
        envelope.sig = BlobSig(model_blob_bytes(&envelope));
        let decision = decide(&envelope, &receiver);
        prop_assert!(
            matches!(decision, AdmitDecision::RefuseMalformedProof { .. }),
            "a chain proving nobody must be refused as malformed; got {decision:?}"
        );
    }
}

/// The hostile corpus reaches every decision the boundary can make. A generator
/// that stopped reaching one of these would leave the property above asserting
/// nothing about that branch.
#[test]
fn the_corpus_reaches_every_decision_variant() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = scenario();
    let mut imports = 0usize;
    let mut sig = 0usize;
    let mut malformed = 0usize;
    let mut audience = 0usize;
    let mut container = 0usize;
    let mut lease = 0usize;
    let mut capability = 0usize;

    for _ in 0..4_000 {
        let scenario = strategy
            .new_tree(&mut runner)
            .expect("the scenario strategy produces a value")
            .current();
        let receiver = scenario.receiver();
        match decide(&scenario.envelope(), &receiver) {
            AdmitDecision::Import { .. } => imports += 1,
            AdmitDecision::RefuseSig { .. } => sig += 1,
            AdmitDecision::RefuseMalformedProof { .. } => malformed += 1,
            AdmitDecision::RefuseAudience { .. } => audience += 1,
            AdmitDecision::RefuseContainer { .. } => container += 1,
            AdmitDecision::RefuseLease { .. } => lease += 1,
            AdmitDecision::RefuseCapability { .. } => capability += 1,
        }
    }

    let counts = [
        imports, sig, malformed, audience, container, lease, capability,
    ]
    .map(|c| c as f64 / 40.0);
    println!(
        "corpus reach (import, sig, malformed, audience, container, lease, capability) = \
         {counts:?} percent over 4000 draws"
    );
    assert!(
        imports >= 40
            && sig > 0
            && malformed > 0
            && audience > 0
            && container > 0
            && lease > 0
            && capability > 0,
        "the corpus must reach every decision; got (import, sig, malformed, audience, \
         container, lease, capability) = {counts:?} percent over 4000 draws"
    );
}
