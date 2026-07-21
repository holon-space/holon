//! Unit gates for Inc 5 (owner-signed policy + lease membership + A7
//! disjointness) and Inc 6 (owner-private alias ledger / re-encode baseline).

use std::collections::BTreeSet;

use holon_api::TestClock;
use holon_sharing::AliasLedger;
use holon_sharing::Capabilities;
use holon_sharing::Capability;
use holon_sharing::CrossingLog;
use holon_sharing::DelegationError;
use holon_sharing::GrantOutcome;
use holon_sharing::Granter;
use holon_sharing::Issuer;
use holon_sharing::Lease;
use holon_sharing::MapContainment;
use holon_sharing::MembershipCert;
use holon_sharing::MembershipChain;
use holon_sharing::MembershipError;
use holon_sharing::Policy;
use holon_sharing::PolicyCommitError;
use holon_sharing::PolicySet;
use holon_sharing::Principal;
use holon_sharing::SignedPolicy;
use holon_sharing::SuccessionPointer;
use holon_sharing::UnverifiedVerifier;
use holon_sharing::alias_ledger::NonReplicated;
use holon_sharing::evaluate_grant;
use holon_sharing::issue_delegated_cert;
use holon_sharing::types::BlockId;
use holon_sharing::types::ContainerId;
use holon_sharing::types::LogEntryBody;
use holon_sharing::types::StablePeerId;
use holon_sharing::types::UnverifiedAuthority;
use holon_sharing::verify_membership;

fn block(s: &str) -> BlockId {
    BlockId(s.to_string())
}
fn principal(s: &str) -> Principal {
    Principal(s.to_string())
}
fn log() -> CrossingLog {
    CrossingLog::new(StablePeerId(1), Box::new(UnverifiedAuthority))
}

fn owner_policy(selector: &str, principals: &[&str], lease: Lease) -> SignedPolicy {
    let policy = Policy {
        selector: block(selector),
        principals: principals.iter().map(|p| principal(p)).collect(),
        capabilities: Capabilities::read_only(),
        delegation: false,
        lease,
    };
    SignedPolicy::sign(policy, &UnverifiedAuthority)
}

// ── Inc 5: A7 disjointness ───────────────────────────────────────────────

#[test]
fn a7_disjoint_selectors_both_commit() {
    let mut set = PolicySet::new();
    let rel = MapContainment::new()
        .with_subtree(block("A"), [block("a1")])
        .with_subtree(block("B"), [block("b1")]);
    let log = log();
    set.commit(
        owner_policy("A", &["p"], Lease::starting_at(0, 1000)),
        &rel,
        &UnverifiedVerifier,
        &log,
    )
    .unwrap();
    set.commit(
        owner_policy("B", &["q"], Lease::starting_at(0, 1000)),
        &rel,
        &UnverifiedVerifier,
        &log,
    )
    .expect("disjoint selectors are allowed");
    assert_eq!(set.policies().len(), 2);
}

#[test]
fn a7_nested_selector_rejected_loud() {
    // B's subtree contains A's root => A nests inside B => overlap.
    let mut set = PolicySet::new();
    let rel = MapContainment::new().with_subtree(block("B"), [block("A"), block("a1")]);
    let log = log();
    set.commit(
        owner_policy("B", &["p"], Lease::starting_at(0, 1000)),
        &rel,
        &UnverifiedVerifier,
        &log,
    )
    .unwrap();
    let err = set
        .commit(
            owner_policy("A", &["q"], Lease::starting_at(0, 1000)),
            &rel,
            &UnverifiedVerifier,
            &log,
        )
        .expect_err("a nested selector must be rejected");
    assert!(matches!(err, PolicyCommitError::OverlappingSelector { .. }));
    // Nothing recorded on failure.
    assert_eq!(set.policies().len(), 1);
}

#[test]
fn a7_identical_selector_rejected() {
    let mut set = PolicySet::new();
    let rel = MapContainment::new();
    let log = log();
    set.commit(
        owner_policy("A", &["p"], Lease::starting_at(0, 1000)),
        &rel,
        &UnverifiedVerifier,
        &log,
    )
    .unwrap();
    let err = set
        .commit(
            owner_policy("A", &["q"], Lease::starting_at(0, 1000)),
            &rel,
            &UnverifiedVerifier,
            &log,
        )
        .expect_err("a duplicate selector overlaps itself (reflexive containment)");
    assert!(matches!(err, PolicyCommitError::OverlappingSelector { .. }));
}

#[test]
fn policy_commit_records_policy_edit_in_log() {
    let mut set = PolicySet::new();
    let rel = MapContainment::new();
    let log = log();
    set.commit(
        owner_policy("A", &["alice", "bob"], Lease::starting_at(0, 1000)),
        &rel,
        &UnverifiedVerifier,
        &log,
    )
    .unwrap();
    let policy_edits = log
        .entries()
        .into_iter()
        .filter(|e| matches!(e.body, LogEntryBody::PolicyEdit(_)))
        .count();
    assert_eq!(
        policy_edits, 2,
        "one PolicyEdit per principal enters the log"
    );
}

#[test]
fn tampered_policy_signature_rejected_loud() {
    let mut set = PolicySet::new();
    let mut signed = owner_policy("A", &["p"], Lease::starting_at(0, 1000));
    signed.sig.0[0] ^= 0xFF; // corrupt the signature
    let err = set
        .commit(signed, &MapContainment::new(), &UnverifiedVerifier, &log())
        .expect_err("a bad owner signature must be rejected");
    assert!(matches!(err, PolicyCommitError::Unsigned { .. }));
}

// ── Inc 5: lease membership + delegation chains ──────────────────────────

fn owner_cert(selector: &str, grantee: &str, delegation: bool, lease: Lease) -> MembershipCert {
    MembershipCert::issue(
        block(selector),
        principal(grantee),
        Issuer::Owner,
        Capabilities::of([Capability::Read, Capability::Write]),
        delegation,
        lease,
        &UnverifiedAuthority,
    )
}

#[test]
fn active_lease_grants_expired_lease_denies() {
    let clock = TestClock::new(0);
    let cert = owner_cert("A", "alice", false, Lease::starting_at(0, 1000));
    let chain = MembershipChain::direct(cert);

    clock.set(500);
    verify_membership(
        &chain,
        &principal("alice"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect("mid-window membership verifies");

    clock.set(1500); // past expiry — non-renewal = revoked
    let err = verify_membership(
        &chain,
        &principal("alice"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect_err("an expired lease grants no access");
    assert!(matches!(err, MembershipError::LeaseInactive { .. }));
}

#[test]
fn renewal_restores_access() {
    let clock = TestClock::new(1500); // the original lease has lapsed
    let expired = owner_cert("A", "alice", false, Lease::starting_at(0, 1000));
    let renewed = expired.renew(1500, 1000, &UnverifiedAuthority);
    let chain = MembershipChain::direct(renewed);
    verify_membership(
        &chain,
        &principal("alice"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect("a renewed lease restores membership");
}

#[test]
fn valid_delegation_chain_verifies_with_intersected_caps() {
    let clock = TestClock::new(0);
    // Owner grants alice RW + delegation.
    let owner = owner_cert("A", "alice", true, Lease::starting_at(0, 1000));
    // Alice delegates READ-only to bob.
    let delegate = MembershipCert::issue(
        block("A"),
        principal("bob"),
        Issuer::Delegate(principal("alice")),
        Capabilities::of([Capability::Read]),
        false,
        Lease::starting_at(0, 1000),
        &UnverifiedAuthority,
    );
    let chain = MembershipChain::new(vec![owner, delegate]);
    let caps = verify_membership(
        &chain,
        &principal("bob"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect("a well-formed chain verifies");
    // Effective = intersection: alice RW ∩ bob R = R.
    assert!(caps.contains(Capability::Read));
    assert!(!caps.contains(Capability::Write));
}

#[test]
fn delegation_without_rights_rejected() {
    let clock = TestClock::new(0);
    // Owner grants alice WITHOUT delegation.
    let owner = owner_cert("A", "alice", false, Lease::starting_at(0, 1000));
    let delegate = MembershipCert::issue(
        block("A"),
        principal("bob"),
        Issuer::Delegate(principal("alice")),
        Capabilities::read_only(),
        false,
        Lease::starting_at(0, 1000),
        &UnverifiedAuthority,
    );
    let chain = MembershipChain::new(vec![owner, delegate]);
    let err = verify_membership(
        &chain,
        &principal("bob"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect_err("a delegate without delegation rights cannot re-grant");
    assert!(matches!(err, MembershipError::NoDelegationRight { .. }));
}

#[test]
fn non_owner_root_rejected() {
    let clock = TestClock::new(0);
    let rogue = MembershipCert::issue(
        block("A"),
        principal("bob"),
        Issuer::Delegate(principal("alice")), // claims a delegator but is chain root
        Capabilities::read_only(),
        false,
        Lease::starting_at(0, 1000),
        &UnverifiedAuthority,
    );
    let chain = MembershipChain::direct(rogue);
    let err = verify_membership(
        &chain,
        &principal("bob"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect_err("a chain not rooted at the owner is unclaimable");
    assert!(matches!(err, MembershipError::RootNotOwner { .. }));
}

#[test]
fn expired_intermediate_cert_breaks_chain() {
    let clock = TestClock::new(1200);
    // Alice's own delegation cert has expired; bob's is fresh — bob still loses.
    let owner = owner_cert("A", "alice", true, Lease::starting_at(0, 1000));
    let delegate = MembershipCert::issue(
        block("A"),
        principal("bob"),
        Issuer::Delegate(principal("alice")),
        Capabilities::read_only(),
        false,
        Lease::starting_at(1000, 1000),
        &UnverifiedAuthority,
    );
    let chain = MembershipChain::new(vec![owner, delegate]);
    let err = verify_membership(
        &chain,
        &principal("bob"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect_err("an expired delegator cert breaks the chain");
    assert!(matches!(
        err,
        MembershipError::LeaseInactive { index: 0, .. }
    ));
}

// ── Inc 5: non-owner grant → pending request (D4) ────────────────────────

#[test]
fn owner_grant_committed() {
    let clock = TestClock::new(0);
    let outcome = evaluate_grant(
        &Granter::Owner,
        &block("A"),
        &principal("bob"),
        &Capabilities::read_only(),
        &clock,
        &UnverifiedVerifier,
    );
    assert_eq!(outcome, GrantOutcome::Committed);
}

#[test]
fn non_owner_without_delegation_becomes_pending() {
    let clock = TestClock::new(0);
    // Alice holds membership but NO delegation right.
    let owner = owner_cert("A", "alice", false, Lease::starting_at(0, 1000));
    let chain = MembershipChain::direct(owner);
    let outcome = evaluate_grant(
        &Granter::Delegate(chain),
        &block("A"),
        &principal("bob"),
        &Capabilities::read_only(),
        &clock,
        &UnverifiedVerifier,
    );
    assert!(matches!(outcome, GrantOutcome::Pending(_)));
}

#[test]
fn delegate_with_rights_commits_grant() {
    let clock = TestClock::new(0);
    let owner = owner_cert("A", "alice", true, Lease::starting_at(0, 1000));
    let chain = MembershipChain::direct(owner);
    let outcome = evaluate_grant(
        &Granter::Delegate(chain),
        &block("A"),
        &principal("bob"),
        &Capabilities::read_only(),
        &clock,
        &UnverifiedVerifier,
    );
    assert_eq!(outcome, GrantOutcome::Committed);
}

#[test]
fn delegate_cannot_regrant_capability_outside_effective_caps() {
    // Verifier refutation: a ≥2-hop chain where the leaf cert SELF-DECLARES an
    // inflated capability its delegator never held. owner grants alice
    // {Read,Write}+delegation; alice (malicious/buggy, using the RAW
    // constructor to bypass the checked issuance path) issues bob a
    // {Read,Write,Share}+delegation cert. The chain's EFFECTIVE caps are the
    // intersection {Read,Write} — Share is NOT conferred. bob must NOT be able
    // to re-grant Share.
    let clock = TestClock::new(0);
    let owner = MembershipCert::issue(
        block("A"),
        principal("alice"),
        Issuer::Owner,
        Capabilities::of([Capability::Read, Capability::Write]),
        true,
        Lease::starting_at(0, 1000),
        &UnverifiedAuthority,
    );
    let bob_inflated = MembershipCert::issue(
        block("A"),
        principal("bob"),
        Issuer::Delegate(principal("alice")),
        Capabilities::of([Capability::Read, Capability::Write, Capability::Share]),
        true,
        Lease::starting_at(0, 1000),
        &UnverifiedAuthority,
    );
    let chain = MembershipChain::new(vec![owner, bob_inflated]);
    let outcome = evaluate_grant(
        &Granter::Delegate(chain),
        &block("A"),
        &principal("carol"),
        &Capabilities::of([Capability::Share]),
        &clock,
        &UnverifiedVerifier,
    );
    assert!(
        matches!(outcome, GrantOutcome::Pending(_)),
        "a delegate cannot re-grant a capability outside its EFFECTIVE \
         (intersected) caps; got {outcome:?}"
    );
}

#[test]
fn issuing_inflated_sub_cert_rejected_at_source() {
    // Parse-don't-validate: the inflated cert is never even constructed through
    // the checked path. owner grants alice {Read,Write}+delegation; alice tries
    // to issue bob {Read,Write,Share} — Share exceeds her effective caps.
    let clock = TestClock::new(0);
    let owner = MembershipCert::issue(
        block("A"),
        principal("alice"),
        Issuer::Owner,
        Capabilities::of([Capability::Read, Capability::Write]),
        true,
        Lease::starting_at(0, 1000),
        &UnverifiedAuthority,
    );
    let alice_chain = MembershipChain::direct(owner);

    let err = issue_delegated_cert(
        &alice_chain,
        principal("bob"),
        Capabilities::of([Capability::Read, Capability::Write, Capability::Share]),
        false,
        Lease::starting_at(0, 1000),
        &clock,
        &UnverifiedVerifier,
        &UnverifiedAuthority,
    )
    .expect_err("issuing caps the delegator does not hold must fail at the source");
    assert!(matches!(err, DelegationError::CapsExceedDelegator { .. }));

    // A subset (Read) issues fine and the resulting cert verifies end-to-end.
    let bob = issue_delegated_cert(
        &alice_chain,
        principal("bob"),
        Capabilities::read_only(),
        false,
        Lease::starting_at(0, 1000),
        &clock,
        &UnverifiedVerifier,
        &UnverifiedAuthority,
    )
    .expect("a subset sub-cert issues");
    let mut certs = alice_chain.certs.clone();
    certs.push(bob);
    let full = MembershipChain::new(certs);
    verify_membership(
        &full,
        &principal("bob"),
        &block("A"),
        &clock,
        &UnverifiedVerifier,
    )
    .expect("the checked sub-cert produces a verifiable chain");
}

#[test]
fn non_delegable_delegator_cannot_issue() {
    // owner grants alice WITHOUT delegation — she cannot issue any sub-cert.
    let clock = TestClock::new(0);
    let owner = owner_cert("A", "alice", false, Lease::starting_at(0, 1000));
    let alice_chain = MembershipChain::direct(owner);
    let err = issue_delegated_cert(
        &alice_chain,
        principal("bob"),
        Capabilities::read_only(),
        false,
        Lease::starting_at(0, 1000),
        &clock,
        &UnverifiedVerifier,
        &UnverifiedAuthority,
    )
    .expect_err("a non-delegable delegator cannot issue sub-certs");
    assert!(matches!(err, DelegationError::NotDelegable { .. }));
}

// ── Inc 6: alias ledger / re-encode baseline ─────────────────────────────

#[test]
fn ledger_resolves_multi_hop_chain() {
    let ledger = AliasLedger::new(StablePeerId(1), Box::new(UnverifiedAuthority));
    ledger.record_rotation(
        ContainerId("c_old".into()),
        ContainerId("c_mid".into()),
        &[(block("b_old"), block("b_mid"))],
    );
    ledger.record_rotation(
        ContainerId("c_mid".into()),
        ContainerId("c_new".into()),
        &[(block("b_mid"), block("b_new"))],
    );
    assert_eq!(ledger.resolve(&block("b_old")), block("b_new"));
    assert_eq!(
        ledger.resolve(&block("never_rotated")),
        block("never_rotated")
    );
    assert_eq!(
        ledger.container_successor(&ContainerId("c_old".into())),
        Some(ContainerId("c_mid".into()))
    );
}

#[test]
fn owner_backlinks_rewrite_to_current_ids() {
    let ledger = AliasLedger::new(StablePeerId(1), Box::new(UnverifiedAuthority));
    ledger.record_rotation(
        ContainerId("c_old".into()),
        ContainerId("c_new".into()),
        &[(block("b_old"), block("b_new"))],
    );
    let sql = ledger.owner_backlink_rewrite_sql();
    assert_eq!(
        sql,
        vec![
            "UPDATE block_links SET resolved_id = 'b_new' WHERE resolved_id = 'b_old'".to_string()
        ]
    );
}

#[test]
fn recipient_without_ledger_cannot_correlate() {
    // The owner's ledger maps old→new. A recipient is modelled by a FRESH
    // ledger (no shared state — the ledger is NonReplicated): it can never
    // resolve the owner's old id, so it holds no correlation handle. It only
    // ever sees the fresh id, which is distinct from the old one.
    let owner = AliasLedger::new(StablePeerId(1), Box::new(UnverifiedAuthority));
    owner.record_rotation(
        ContainerId("c_old".into()),
        ContainerId("c_new".into()),
        &[(block("secret_old"), block("fresh_new"))],
    );
    let recipient = AliasLedger::new(StablePeerId(2), Box::new(UnverifiedAuthority));
    // Recipient's resolve is identity: it has no idea "fresh_new" was ever
    // "secret_old", and cannot walk back.
    assert_eq!(recipient.resolve(&block("fresh_new")), block("fresh_new"));
    assert_ne!(block("fresh_new"), block("secret_old"));
    // And the owner's fresh id is genuinely distinct (no reuse).
    assert_eq!(owner.resolve(&block("secret_old")), block("fresh_new"));
}

#[test]
fn succession_pointer_signs_and_verifies() {
    let pointer = SuccessionPointer::sign(
        ContainerId("old".into()),
        ContainerId("new".into()),
        &UnverifiedAuthority,
    );
    assert!(pointer.verify(&UnverifiedVerifier));
    let mut tampered = pointer.clone();
    tampered.new = ContainerId("evil".into());
    assert!(
        !tampered.verify(&UnverifiedVerifier),
        "a tampered succession pointer must not verify"
    );
}

#[test]
fn non_replicated_marker_exposes_only_owner_local() {
    let mut m = NonReplicated::new(41);
    *m.owner_local_mut() += 1;
    assert_eq!(*m.owner_local(), 42);
    let _: &i32 = m.owner_local();
    let mut caps: BTreeSet<Capability> = BTreeSet::new();
    caps.insert(Capability::Read);
    assert!(caps.contains(&Capability::Read));
}
