//! Property-based tests for Inc 5 (lease membership + delegation + A7) and
//! Inc 6 (alias ledger). Time is driven through the injected `TestClock` seam
//! (no `SystemTime::now()` anywhere in policy code), so expiry/renewal are real
//! temporal properties, not disguised unit tests.

use holon_api::Clock;
use holon_api::TestClock;
use holon_sharing::AliasLedger;
use holon_sharing::Capabilities;
use holon_sharing::Capability;
use holon_sharing::CrossingLog;
use holon_sharing::GrantOutcome;
use holon_sharing::Granter;
use holon_sharing::Issuer;
use holon_sharing::Lease;
use holon_sharing::MapContainment;
use holon_sharing::MembershipCert;
use holon_sharing::MembershipChain;
use holon_sharing::Policy;
use holon_sharing::PolicyCommitError;
use holon_sharing::PolicySet;
use holon_sharing::Principal;
use holon_sharing::SignedPolicy;
use holon_sharing::UnverifiedVerifier;
use holon_sharing::evaluate_grant;
use holon_sharing::types::BlockId;
use holon_sharing::types::ContainerId;
use holon_sharing::types::StablePeerId;
use holon_sharing::types::UnverifiedAuthority;
use holon_sharing::verify_membership;
use proptest::prelude::*;

/// A capability set from a (read, write, share) triple.
fn caps(read: bool, write: bool, share: bool) -> Capabilities {
    let mut v = Vec::new();
    if read {
        v.push(Capability::Read);
    }
    if write {
        v.push(Capability::Write);
    }
    if share {
        v.push(Capability::Share);
    }
    Capabilities::of(v)
}

fn owner_cert(selector: &str, grantee: &str, delegation: bool, lease: Lease) -> MembershipCert {
    MembershipCert::issue(
        BlockId(selector.into()),
        Principal(grantee.into()),
        Issuer::Owner,
        Capabilities::of([Capability::Read, Capability::Write]),
        delegation,
        lease,
        &UnverifiedAuthority,
    )
}

proptest! {
    // P1 — a lease grants access at exactly the instants inside its half-open
    // window and NOWHERE else. The single source of correctness for "expired
    // lease → no access".
    #[test]
    fn lease_active_exactly_within_window(
        issued in 0i64..1_000_000,
        ttl in 1i64..1_000_000,
        now in 0i64..2_000_000,
    ) {
        let clock = TestClock::new(now);
        let cert = owner_cert("A", "alice", false, Lease { issued_at_millis: issued, expires_at_millis: issued + ttl });
        let chain = MembershipChain::direct(cert);
        let verified = verify_membership(
            &chain, &Principal("alice".into()), &BlockId("A".into()), &clock, &UnverifiedVerifier,
        ).is_ok();
        let in_window = issued <= now && now < issued + ttl;
        prop_assert_eq!(verified, in_window,
            "membership verified={} but in_window={} (issued={}, ttl={}, now={})",
            verified, in_window, issued, ttl, now);
    }

    // P2 — non-renewal = revocation; renewal restores. After the window, the
    // ORIGINAL cert is dead; a renewal issued at `now` is alive iff we are
    // still inside the renewed window.
    #[test]
    fn non_renewal_revokes_renewal_restores(
        ttl in 1i64..100_000,
        elapsed in 1i64..200_000,
    ) {
        let expiry = ttl;
        let clock = TestClock::new(expiry + elapsed); // strictly after original expiry
        let original = owner_cert("A", "alice", false, Lease::starting_at(0, ttl));
        let dead = MembershipChain::direct(original.clone());
        prop_assert!(
            verify_membership(&dead, &Principal("alice".into()), &BlockId("A".into()), &clock, &UnverifiedVerifier).is_err(),
            "an un-renewed lease must be revoked after expiry");

        // Renew at the current instant for a fresh full ttl.
        let now = clock.now_millis();
        let renewed = original.renew(now, ttl, &UnverifiedAuthority);
        let alive = MembershipChain::direct(renewed);
        prop_assert!(
            verify_membership(&alive, &Principal("alice".into()), &BlockId("A".into()), &clock, &UnverifiedVerifier).is_ok(),
            "a lease renewed at `now` is active at `now`");
    }

    // P3 — A7: two selectors are accepted together iff they are disjoint;
    // nesting (either direction, incl. equality) is rejected loudly.
    #[test]
    fn a7_accepts_iff_disjoint(nested in any::<bool>()) {
        let rel = if nested {
            MapContainment::new().with_subtree(BlockId("B".into()), [BlockId("A".into())])
        } else {
            MapContainment::new()
                .with_subtree(BlockId("A".into()), [])
                .with_subtree(BlockId("B".into()), [])
        };
        let mut set = PolicySet::new();
        let log = CrossingLog::new(StablePeerId(1), Box::new(UnverifiedAuthority));
        let mk = |sel: &str| {
            SignedPolicy::sign(Policy {
                selector: BlockId(sel.into()),
                principals: [Principal("p".into())].into_iter().collect(),
                capabilities: Capabilities::read_only(),
                delegation: false,
                lease: Lease::starting_at(0, 1000),
            }, &UnverifiedAuthority)
        };
        set.commit(mk("B"), &rel, &UnverifiedVerifier, &log).unwrap();
        let second = set.commit(mk("A"), &rel, &UnverifiedVerifier, &log);
        let overlapped = matches!(second, Err(PolicyCommitError::OverlappingSelector { .. }));
        if nested {
            prop_assert!(overlapped, "nested selectors must be rejected loud");
        } else {
            prop_assert!(second.is_ok(), "disjoint selectors must be accepted");
        }
    }

    // P4 — delegation chain: an owner→delegate→peer chain verifies iff the
    // delegator carries delegation rights; strip that flag and it must reject.
    #[test]
    fn delegation_requires_delegator_rights(delegator_may_delegate in any::<bool>()) {
        let clock = TestClock::new(0);
        let owner = owner_cert("A", "alice", delegator_may_delegate, Lease::starting_at(0, 1000));
        let delegate = MembershipCert::issue(
            BlockId("A".into()), Principal("bob".into()), Issuer::Delegate(Principal("alice".into())),
            Capabilities::read_only(), false, Lease::starting_at(0, 1000), &UnverifiedAuthority,
        );
        let chain = MembershipChain::new(vec![owner, delegate]);
        let verified = verify_membership(
            &chain, &Principal("bob".into()), &BlockId("A".into()), &clock, &UnverifiedVerifier,
        ).is_ok();
        prop_assert_eq!(verified, delegator_may_delegate);
    }

    // P4b — escalation safety over a ≥2-hop chain with RANDOM per-hop cap
    // subsets (certs built raw, so a hop may self-declare inflated caps). The
    // core property: whenever evaluate_grant COMMITS, the granted caps are a
    // subset of the chain's EFFECTIVE (intersected) caps — a delegate can never
    // re-grant a capability the chain does not actually confer.
    #[test]
    fn granted_caps_are_subset_of_effective(
        or in 0u8..8,   // owner cert caps bitmask (read/write/share)
        ar in 0u8..8,   // alice's self-declared caps (may exceed owner's)
        req in 0u8..8,   // bob requests to grant these caps to carol
        alice_deleg in any::<bool>(),
    ) {
        let bit = |m: u8, i: u8| (m >> i) & 1 == 1;
        let owner_caps = caps(bit(or, 0), bit(or, 1), bit(or, 2));
        let alice_caps = caps(bit(ar, 0), bit(ar, 1), bit(ar, 2));
        let requested = caps(bit(req, 0), bit(req, 1), bit(req, 2));

        let clock = TestClock::new(0);
        // owner → alice (delegation), alice → bob (delegation) — bob is the leaf.
        let owner = MembershipCert::issue(
            BlockId("A".into()), Principal("alice".into()), Issuer::Owner,
            owner_caps.clone(), true, Lease::starting_at(0, 1000), &UnverifiedAuthority);
        let bob = MembershipCert::issue(
            BlockId("A".into()), Principal("bob".into()), Issuer::Delegate(Principal("alice".into())),
            alice_caps.clone(), alice_deleg, Lease::starting_at(0, 1000), &UnverifiedAuthority);
        let chain = MembershipChain::new(vec![owner, bob]);

        // Effective = owner ∩ alice-declared (what verify_membership returns).
        let effective = verify_membership(
            &chain, &Principal("bob".into()), &BlockId("A".into()), &clock, &UnverifiedVerifier)
            .expect("both leases active, chain well-linked");

        let outcome = evaluate_grant(
            &Granter::Delegate(chain), &BlockId("A".into()), &Principal("carol".into()),
            &requested, &clock, &UnverifiedVerifier);

        if let GrantOutcome::Committed = outcome {
            // The load-bearing invariant: granted ⊆ effective, ALWAYS.
            prop_assert!(requested.is_subset_of(&effective),
                "committed a grant of caps NOT in the effective set (escalation): \
                 requested={:?} effective={:?}", requested, effective);
            // A commit additionally implies the leaf could delegate.
            prop_assert!(alice_deleg, "committed without delegation right");
        }
    }

    // P5 (Inc 6) — after a re-encode rotation the owner's old id resolves to
    // the fresh id through the ledger, while a recipient (no ledger) sees only
    // the fresh id and can never correlate it back to the old one.
    #[test]
    fn rotation_owner_resolves_recipient_cannot_correlate(
        n in 1usize..6,
    ) {
        let owner = AliasLedger::new(StablePeerId(1), Box::new(UnverifiedAuthority));
        let remap: Vec<(BlockId, BlockId)> = (0..n)
            .map(|i| (BlockId(format!("old{i}")), BlockId(format!("new{i}"))))
            .collect();
        owner.record_rotation(ContainerId("c_old".into()), ContainerId("c_new".into()), &remap);

        let recipient = AliasLedger::new(StablePeerId(2), Box::new(UnverifiedAuthority));
        for (old, new) in &remap {
            // Owner stitches old → new.
            prop_assert_eq!(&owner.resolve(old), new);
            // Fresh ids are genuinely distinct (no correlation by reuse).
            prop_assert_ne!(old, new);
            // Recipient has no handle: resolve is identity on the fresh id and
            // it cannot resolve the old id to anything meaningful either.
            prop_assert_eq!(&recipient.resolve(new), new);
            prop_assert_eq!(&recipient.resolve(old), old);
        }
    }
}
