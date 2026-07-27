//! **The two-instance composed PBT** — the keystone's alphabet, catalog, and
//! kernel driven over TWO real Holon instances with a relay between them.
//!
//! Own binary (not a case of the keystone) because every case boots two full
//! sessions: bounded case count, short sequences, and a failure here localizes
//! to sharing rather than to the keystone's whole surface.
//!
//! @pbt kind slice
//! @pbt covers two-instance-share, two-instance-sync — one-way share and
//!   convergence over `SyncTransport` between two `compose_sut(full_headless)`
//!   sessions.
//!
//! ## The two red gates this file owns
//! - `two_instances_boot_with_distinct_peer_ids` (Inc0): the peer-id injection
//!   seam. Before it, both sessions read the process-global
//!   `HOLON_LORO_PEER_ID`, authored under the SAME peer id, and their CRDT
//!   histories could not converge. It also asserts the NEGATIVE: an unshared
//!   vault transports nothing — beside an executed-witness proving `sync_once`
//!   RAN and walked the replication set, so "nothing crossed" is a decision,
//!   not an absence of attempt.
//! - `one_way_share_converges_on_the_receiver` (Inc1): after a share and one
//!   round, every owner-exclusive block is in the receiver's store AND its org
//!   files, and both invariants RAN (they are in `report.ran`) rather than
//!   silently deselecting.
//! - `post_boot_create_reaches_the_receiver_on_the_next_round`: the layered
//!   localizer. It asserts at the owner's Loro tree, the receiver's Loro tree,
//!   and the receiver's SQL store, so a convergence regression names the side
//!   that lost the block instead of only reporting that it is gone.

use std::collections::BTreeSet;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::two_instance::TwoInstanceE2E;
use holon_integration_tests::pbt::composed::two_instance::boot_two_instances;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutTwoInstance;
use proptest_state_machine::prop_state_machine;

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        // Bounded: each case boots TWO full_headless sessions.
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(8),
        max_shrink_iters: 50,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn two_instance_composed_pbt(sequential 1..8 => ComposedSut<TwoInstanceE2E>);
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("multi-thread runtime for two booted sessions")
}

/// Inc0 gate. Two halves, both of which must hold:
/// 1. the two instances mint DISTINCT Loro peer ids (the injection seam);
/// 2. an UNSHARED vault transports nothing — asserted beside the executed
///    witness, so it cannot pass because nothing was attempted.
#[test]
fn two_instances_boot_with_distinct_peer_ids_and_transport_nothing_unshared() {
    let rt = rt();
    // `wide_e2e_ref` extracts the cap set by booting its OWN runtime, so it must
    // be built outside `block_on` — nested runtimes panic.
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, _handle, _) = boot_two_instances(&resolver, &ref_state).await;

        let two = caps.expect::<dyn SutTwoInstance>();
        let (owner_peer, receiver_peer) = two.instance_peer_ids().await;
        assert_ne!(
            owner_peer, receiver_peer,
            "the two instances booted under the SAME Loro peer id — their CRDT histories cannot \
             converge, and every sharing result below would be meaningless"
        );
        let (owner_doc_peer, receiver_doc_peer) = two.live_doc_peer_ids().await;
        assert_eq!(
            (owner_doc_peer, receiver_doc_peer),
            (owner_peer, receiver_peer),
            "the claimed peer ids must be the ids the LIVE documents actually carry — otherwise \
             the injection seam is reporting an intention it never applied"
        );

        // Drive rounds over an UNSHARED vault.
        for _ in 0..3 {
            two.sync_now(true).await;
        }
        let witness = two.sync_witness().await;

        // Executed witness FIRST: without it the assertions below are vacuous.
        assert_eq!(witness.rounds_run, 3, "the slice must have driven 3 rounds");
        assert!(
            witness.containers_visited > 0,
            "sync_once ran but walked ZERO containers — it never reached the replication set, so \
             'nothing crossed' proves nothing"
        );
        assert!(
            witness.transport_consultations > 0,
            "sync_once never consulted the transport; the negative result is an absence of \
             attempt, not a refusal"
        );
        assert!(
            !witness.unauthorized.is_empty(),
            "an unshared round must report every container as UNAUTHORIZED (no membership proof \
             to attach); none were reported, so the orchestrator did not evaluate authorization"
        );
        assert_eq!(
            witness.pushed, 0,
            "an unshared vault published {} container(s) to the relay — state left the device \
             under no membership proof",
            witness.pushed
        );
        assert_eq!(
            witness.transport_envelopes, 0,
            "the relay holds {} envelope(s) from an UNSHARED vault",
            witness.transport_envelopes
        );
        assert_eq!(
            witness.imported, 0,
            "the receiver imported {} envelope(s) from an UNSHARED vault",
            witness.imported
        );

        let recv = caps.expect::<dyn SutReceiverBackend>();
        let owner = recv.owner_block_ids().await;
        let boot = recv.receiver_boot_block_ids().await;
        let receiver = recv.receiver_block_ids().await;
        let exclusive: BTreeSet<_> = owner.difference(&boot).cloned().collect();
        assert!(
            !exclusive.is_empty(),
            "the owner holds no block the receiver lacked at boot, so this test could not detect a \
             leak — the two seeds are not disjoint enough"
        );
        let leaked: Vec<_> = exclusive.intersection(&receiver).collect();
        assert!(
            leaked.is_empty(),
            "owner-exclusive blocks {leaked:?} reached the receiver with NO share in force"
        );
    });
}

/// Inc1 gate: share the whole vault, run one round, and require the receiver to
/// converge — store AND org — with both sharing invariants proven to have RUN.
#[test]
fn one_way_share_converges_on_the_receiver() {
    let rt = rt();
    // `wide_e2e_ref` extracts the cap set by booting its OWN runtime, so it must
    // be built outside `block_on` — nested runtimes panic.
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, _) = boot_two_instances(&resolver, &ref_state).await;

        let two = caps.expect::<dyn SutTwoInstance>();
        let recv = caps.expect::<dyn SutReceiverBackend>();

        two.share_container("holon_tree", "receiver").await;
        let witness = two.sync_now(true).await;
        assert!(
            witness.pushed > 0,
            "a shared vault with content pushed NOTHING (refusals: {:?})",
            witness.refusals
        );
        assert!(
            witness.imported > 0,
            "the receiver admitted NOTHING from an authorized push; acceptor refusals: {:?}",
            witness.refusals
        );

        // Let the receiver's CDC + org writeback drain before reading them.
        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.receiver(),
            std::time::Duration::from_secs(10),
        )
        .await;

        let owner = recv.owner_block_ids().await;
        let boot = recv.receiver_boot_block_ids().await;
        let receiver = recv.receiver_block_ids().await;
        let exclusive: BTreeSet<_> = owner.difference(&boot).cloned().collect();
        let missing: Vec<_> = exclusive.difference(&receiver).collect();
        assert!(
            missing.is_empty(),
            "after one authorized round the receiver's store is missing {} of {} owner-exclusive \
             block(s): {missing:?}",
            missing.len(),
            exclusive.len()
        );

        let owner_org = recv.owner_org_block_ids().await;
        let receiver_org = recv.receiver_org_block_ids().await;
        let expected_org: BTreeSet<_> = owner_org.intersection(&exclusive).cloned().collect();
        assert!(
            !expected_org.is_empty(),
            "the owner's own org files carry NONE of the shared blocks, so the receiver-side \
             writeback half of convergence would be vacuous"
        );
        let missing_org: Vec<_> = expected_org.difference(&receiver_org).collect();
        assert!(
            missing_org.is_empty(),
            "the receiver's store converged but its ORG files are missing {missing_org:?} — \
             received state that never reaches disk is lost on restart"
        );

        assert_eq!(
            recv.crdt_converged().await,
            Some(true),
            "the two instances' Loro documents do not reach a common state under a pairwise \
             fork-and-sync fixed point"
        );
    });
}

/// Non-vacuity guard for the slice: the two sharing invariants must actually be
/// SELECTED and RUN by the composed catalog against a two-instance CapMap. A
/// silently deselected invariant would let every case above pass while checking
/// nothing.
#[test]
fn both_sharing_invariants_are_selected_and_run() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    let (caps, _handle, scaffold, resolver) = rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, scaffold) = boot_two_instances(&resolver, &ref_state).await;
        (caps, handle, scaffold, resolver)
    });

    let report = rt.block_on(
        <TwoInstanceE2E as holon_integration_tests::pbt::composed::harness::ComposedSlice>::run_report(
            &caps,
            &resolver,
            &Default::default(),
            &Default::default(),
            &scaffold,
            &ref_state,
        ),
    );
    let ran = report.ran_ids();
    for id in ["inv-two-instance-convergence", "inv-boundary-respected"] {
        assert!(
            ran.contains(&id),
            "`{id}` did not run against a two-instance CapMap — it deselected, so the slice \
             proves nothing. Ran: {ran:?}, deselected: {:?}",
            report.deselected
        );
    }
    assert!(
        report.failures().is_empty(),
        "two-instance catalog run failed: {:?}",
        report.failures()
    );
}

/// A block created on the owner AFTER the share must reach the receiver on the
/// NEXT round — through the Loro tree AND the SQL projection. Asserts at both
/// layers so a future regression localizes itself: absent from the receiver's
/// Loro tree means the owner's incremental export since the last round dropped
/// it; present there but absent from `block_raw` means the receiver's Loro→SQL
/// projection did not materialize an IMPORTED node.
#[test]
fn post_boot_create_reaches_the_receiver_on_the_next_round() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, _) = boot_two_instances(&resolver, &ref_state).await;
        let two = caps.expect::<dyn SutTwoInstance>();
        let recv = caps.expect::<dyn SutReceiverBackend>();

        two.share_container("holon_tree", "receiver").await;
        two.sync_now(true).await;

        // Post-boot create on the OWNER, through the production create path.
        let before = recv.owner_block_ids().await;
        let parent = before
            .iter()
            .find(|id| id.as_str().contains("structural-page"))
            .cloned()
            .expect("owner seed carries the structural page root");
        caps.expect::<dyn holon_pbt_core::capabilities::SutBlockCreate>()
            .apply_create_under_focus(&parent, "post-boot content", None)
            .await;

        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.owner(),
            std::time::Duration::from_secs(15),
        )
        .await;
        let after = recv.owner_block_ids().await;
        let created: Vec<_> = after.difference(&before).cloned().collect();
        assert_eq!(created.len(), 1, "expected exactly one new owner block, got {created:?}");
        let created = created[0].clone();

        assert!(
            handle.loro_tree_ids(true).await.contains(created.as_str()),
            "the owner's own Loro tree does not carry {created} — the create never reached the \
             CRDT, so nothing downstream could have carried it"
        );

        let w = two.sync_now(true).await;
        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.receiver(),
            std::time::Duration::from_secs(15),
        )
        .await;

        assert!(
            handle.loro_tree_ids(false).await.contains(created.as_str()),
            "{created} did not reach the receiver's LORO TREE (round: pushed={} imported={} \
             refusals={:?}) — the owner's incremental export since the last round dropped it",
            w.pushed,
            w.imported,
            w.refusals
        );
        assert!(
            recv.receiver_block_ids().await.contains(&created),
            "{created} reached the receiver's Loro tree but NOT its SQL store — the receiver's \
             Loro→SQL projection did not materialize an IMPORTED node (a locally-authored write \
             would have projected)"
        );
    });
}
