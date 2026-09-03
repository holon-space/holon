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
use holon_integration_tests::pbt::composed::two_instance::boot_two_instances_on;
use holon_integration_tests::pbt::composed::two_instance_transport::TransportChoice;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::capabilities::SutReceiverBackend;
use holon_pbt_core::capabilities::SutTwoInstance;
use holon_pbt_core::capabilities::SyncRoundWitness;
use holon_pbt_core::capabilities::SyncTransportKind;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

prop_state_machine! {
    #![proptest_config(proptest::test_runner::Config {
        // Bounded: each case boots TWO full_headless sessions.
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(8),
        max_shrink_iters: 50,
        failure_persistence: None,
        .. proptest::test_runner::Config::default()
    })]
    #[test]
    fn two_instance_composed_pbt(sequential 1..8 => ComposedSut<TwoInstanceE2E>);
}

/// The executed-witness, in the vocabulary of whichever wire ran. Every
/// negative assertion in this file stands next to a call of this: "nothing
/// crossed" only means something beside proof the transport was consulted.
///
/// The two wires prove it differently and MUST NOT be conflated. The relay is
/// a store-and-forward log, so its evidence is consultations and the envelopes
/// it holds. A direct iroh link stores nothing, so its evidence is the QUIC
/// connections opened and the bytes they carried.
/// `expected` MUST be a literal kind (or one derived from the `TransportChoice`
/// the test asked for) — never `handle.transport_kind()`, which reads the kind
/// off the very transport under test and so compares the witness against
/// itself. A `build()` that returned a relay for `TransportChoice::Iroh` passed
/// that version of this assertion.
fn assert_transport_ran(w: &SyncRoundWitness, expected: SyncTransportKind) {
    assert_eq!(
        w.transport,
        expected,
        "the round ran on the {} wire, not the {} one this assertion is about",
        w.transport.as_str(),
        expected.as_str()
    );
    assert!(
        w.containers_visited > 0,
        "the round ran but walked ZERO containers — it never reached the replication set, so any \
         result below proves nothing"
    );
    assert!(
        w.transport_consultations > 0,
        "the {} transport was never consulted; the result is an absence of attempt, not a \
         decision",
        expected.as_str()
    );
    if expected == SyncTransportKind::Iroh {
        assert!(
            w.connections_opened > 0,
            "the iroh leg opened ZERO connections — `replicate_all` advertised nothing dialable, \
             so nothing about production was exercised"
        );
        // Bytes are read off the connection iroh hands back, which only a
        // dial that PASSED the enrollment gate produces. A refused round is
        // witnessed by the connection count and the refusal itself.
        if w.imported > 0 {
            assert!(
                w.bytes_on_wire > 0,
                "the iroh leg imported {} container(s) while moving ZERO bytes — the counters \
                 disagree with the wire",
                w.imported
            );
        }
    }
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
        let (caps, handle, _) = boot_two_instances(&resolver, &ref_state).await;

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
        assert_transport_ran(&witness, TransportChoice::from_env().kind());
        assert!(
            !witness.unauthorized.is_empty(),
            "an unshared round must report every container as UNAUTHORIZED (no membership proof \
             on the relay, no provable capability on iroh); none were reported, so authorization \
             was never evaluated"
        );
        assert_eq!(
            witness.pushed, 0,
            "an unshared vault published {} container(s) — state left the device under no \
             membership proof",
            witness.pushed
        );
        if TransportChoice::from_env().kind() == SyncTransportKind::Relay {
            assert_eq!(
                witness.transport_envelopes, 0,
                "the relay holds {} envelope(s) from an UNSHARED vault",
                witness.transport_envelopes
            );
        }
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

/// Inc1 gate over the ENV-SELECTED wire: share the whole vault, run one round,
/// and require the receiver to converge — store AND org — with both sharing
/// invariants proven to have RUN.
#[test]
fn one_way_share_converges_on_the_receiver() {
    one_way_share_converges_over(TransportChoice::from_env());
}

/// The SAME property pinned to PRODUCTION (D71.b): `replicate_all` over live
/// iroh endpoints. Pinned rather than env-selected so the landing gate covers
/// the shipping transport without anyone remembering to set a variable — the
/// whole point of parameterising the slice is that prod cannot drift out of
/// test coverage.
#[test]
fn one_way_share_converges_on_the_receiver_over_iroh() {
    one_way_share_converges_over(TransportChoice::Iroh);
}

fn one_way_share_converges_over(transport: TransportChoice) {
    let rt = rt();
    // `wide_e2e_ref` extracts the cap set by booting its OWN runtime, so it must
    // be built outside `block_on` — nested runtimes panic.
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, _) = boot_two_instances_on(&resolver, &ref_state, transport).await;

        let two = caps.expect::<dyn SutTwoInstance>();
        let recv = caps.expect::<dyn SutReceiverBackend>();

        two.share_container("holon_tree", "receiver").await;
        let witness = two.sync_now(true).await;
        assert_transport_ran(&witness, transport.kind());
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
    post_boot_create_reaches_the_receiver_over(TransportChoice::from_env());
}

/// The same layered localizer pinned to PRODUCTION (D71.b).
#[test]
fn post_boot_create_reaches_the_receiver_over_iroh() {
    post_boot_create_reaches_the_receiver_over(TransportChoice::Iroh);
}

fn post_boot_create_reaches_the_receiver_over(transport: TransportChoice) {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, _) = boot_two_instances_on(&resolver, &ref_state, transport).await;
        let two = caps.expect::<dyn SutTwoInstance>();
        let recv = caps.expect::<dyn SutReceiverBackend>();

        two.share_container("holon_tree", "receiver").await;
        two.sync_now(true).await;
        // Settle BOTH sides before the baseline snapshot. The production wire's
        // version-vector exchange is BIDIRECTIONAL — one round also carries the
        // receiver's own blocks back into the owner, and the owner's projection
        // materializes them a beat later. The relay's one-way `push_once` never
        // does, so a baseline taken mid-drain is stable there and stale here.
        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.owner(),
            std::time::Duration::from_secs(15),
        )
        .await;
        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.receiver(),
            std::time::Duration::from_secs(15),
        )
        .await;

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
        assert_eq!(
            created.len(),
            1,
            "expected exactly one new owner block, got {created:?}"
        );
        let created = created[0].clone();

        assert!(
            handle.loro_tree_ids(true).await.contains(created.as_str()),
            "the owner's own Loro tree does not carry {created} — the create never reached the \
             CRDT, so nothing downstream could have carried it"
        );

        let w = two.sync_now(true).await;
        assert_transport_ran(&w, transport.kind());
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

// ---------------------------------------------------------------------------
// Inc 0 — two-writer convergence under concurrent structure + text.
// ---------------------------------------------------------------------------
//
// ## The model
// Two peers over ONE replicated container (`holon_tree`). Each peer applies a
// script of production write ops to its own instance; sync rounds are
// interleaved at generated points, so some ops are authored while the peers are
// divergent. After the script the pair is driven to a sync fixed point.
//
// **Oracle (convergence law):** at the fixed point both peers' live Loro block
// trees are EQUAL — same ids, same parents, same sibling order, same text
// — and neither session panicked or poisoned. That is the only oracle available
// here: with two concurrent writers there is no single-writer reference state
// to reconcile against, and inventing one would mean re-implementing RGA
// tiebreaks in the model.
//
// **What is subtracted:** `receiver_boot_ids`. Both instances mint
// `block:root-layout`, `block:__default__` and the journals roots under the
// SAME fixed ids at boot, so they collide by construction on merge. That defect
// is Inc 1's; leaving it in would red every case for a reason this increment
// does not own.
//
// **Why this is not the composed slice's alphabet.** `TwoInstanceMachine`
// judges the OWNER against a `ReferenceState` that models one writer, and the
// composed harness treats EVERY invariant failure as hard. A receiver-authored
// block flowing back into the owner would red the owner-vs-oracle invariants
// for a reason unrelated to sharing. So the two-writer question gets its own
// oracle here, over the SAME production caps the keystone transitions drive.

use holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_receiver_caps;
use holon_pbt_core::capabilities::SutBlockCreate;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::composition::CapMap;
use proptest::prelude::*;

/// One production write, applied to whichever peer the step names. The
/// structural arms are exactly the ops D70 reports as fatal on a shallow share
/// when merged against a concurrent op; `Type` is the text op they must be
/// interleaved with.
#[derive(Debug, Clone)]
enum PairOp {
    Create,
    Type(String),
    Indent,
    Outdent,
    Join,
}

// `move_up` / `move_down` are deliberately ABSENT. Their driver asserts the
// Alt+Up / Alt+Down chord actually dispatched, which it does not when the
// target has no sibling in that direction — a driver precondition the keystone
// enforces in its reference model. This slice picks targets by observation, not
// by model, so a reorder arm would red on driver preconditions instead of on
// convergence. `Indent` already reparents, which is the tree-move shape D70
// reports as fatal.

#[derive(Debug, Clone)]
enum PairStep {
    /// `on_owner` picks the peer; `pick` selects a target block from that
    /// peer's own live id set (modulo). A step whose op has no applicable
    /// target is skipped rather than forced.
    Write {
        on_owner: bool,
        pick: usize,
        op: PairOp,
    },
    /// A sync round in one direction. Interleaved so writes land while the
    /// peers are divergent.
    Sync { owner_to_receiver: bool },
}

fn pair_op() -> impl Strategy<Value = PairOp> {
    prop_oneof![
        3 => Just(PairOp::Create),
        3 => "[a-z]{1,6}".prop_map(PairOp::Type),
        3 => Just(PairOp::Indent),
        2 => Just(PairOp::Outdent),
        1 => Just(PairOp::Join),
    ]
}

fn pair_step() -> impl Strategy<Value = PairStep> {
    prop_oneof![
        4 => (any::<bool>(), 0usize..64, pair_op())
            .prop_map(|(on_owner, pick, op)| PairStep::Write { on_owner, pick, op }),
        1 => any::<bool>().prop_map(|owner_to_receiver| PairStep::Sync { owner_to_receiver }),
    ]
}

/// Blocks a write op may target on one peer: everything the peer's live Loro
/// tree holds MINUS the ids the oracle subtracts. Writing to a subtracted block
/// would produce a write the oracle cannot see, so the non-vacuity count would
/// over-claim.
///
/// The candidate set is narrowed per op to the targets whose PRECONDITION
/// holds. `indent` and `join_block` both fold a block into its previous
/// sibling and the production op refuses when there is none; the keystone
/// enforces that in its reference model, and this slice — which picks by
/// observation, not by model — has to enforce it here or it reds on driver
/// preconditions instead of on convergence.
fn candidates(state: &TreeState, op: &PairOp) -> Vec<holon_api::EntityUri> {
    // A page ROOT has no parent block: `outdent` refuses it outright, and
    // `indent`/`join_block` refuse it even when other page roots sit beside it
    // under the tree sentinel.
    let needs_parent_block = matches!(op, PairOp::Indent | PairOp::Outdent | PairOp::Join);
    let needs_previous_sibling = matches!(op, PairOp::Indent | PairOp::Join);
    state
        .values()
        .filter(|b| {
            if needs_parent_block && !state.contains_key(b.block.parent_id.as_str()) {
                return false;
            }
            if !needs_previous_sibling {
                return true;
            }
            state.values().any(|other| {
                other.block.parent_id == b.block.parent_id
                    && other.block.id != b.block.id
                    && other.sort_key < b.sort_key
            })
        })
        .map(|b| b.block.id.clone())
        .collect()
}

/// Apply one production write to one peer through its OWN cap map.
///
/// Returns `false` when no target satisfies the op's precondition, so the
/// caller does not count a write that never happened.
async fn apply_pair_op(caps: &CapMap, state: &TreeState, op: &PairOp, pick: usize) -> bool {
    let mut targets = candidates(state, op);
    targets.sort();
    if targets.is_empty() {
        return false;
    }
    let target = targets[pick % targets.len()].clone();
    match op {
        PairOp::Create => {
            caps.expect::<dyn SutBlockCreate>()
                .apply_create_under_focus(&target, "pair", None)
                .await;
        }
        PairOp::Type(text) => {
            caps.expect::<dyn SutFocusWrite>()
                .apply_focus_editable_text(&target)
                .await;
            caps.expect::<dyn SutEditorMirrorWrite>()
                .apply_type_chars(text)
                .await;
        }
        PairOp::Indent => {
            caps.expect::<dyn SutBlockTreeWrite>()
                .apply_indent(&target)
                .await
        }
        PairOp::Outdent => {
            caps.expect::<dyn SutBlockTreeWrite>()
                .apply_outdent(&target)
                .await
        }
        PairOp::Join => {
            caps.expect::<dyn SutBlockTreeWrite>()
                .apply_join_block(&target)
                .await
        }
    }
    true
}

const PAIR_SETTLE: std::time::Duration = std::time::Duration::from_secs(15);

/// Drive rounds until neither side imports anything new — the sync fixed point
/// the oracle is stated at. Bounded: a pair that will not quiesce is a defect,
/// not a reason to loop forever.
async fn drive_to_sync_fixpoint(
    two: &std::sync::Arc<dyn SutTwoInstance>,
    handle: &std::sync::Arc<
        holon_integration_tests::pbt::composed::two_instance::TwoInstanceHandle,
    >,
) {
    for _ in 0..6 {
        let a = two.sync_now(true).await;
        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.receiver(),
            PAIR_SETTLE,
        )
        .await;
        let b = two.sync_now(false).await;
        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.owner(),
            PAIR_SETTLE,
        )
        .await;
        if a.imported == 0 && b.imported == 0 && a.pushed == 0 && b.pushed == 0 {
            return;
        }
    }
}

/// Run one generated script and return the two peers' final tree states.
///
/// `ref_state` is built by the CALLER, outside any runtime: `wide_e2e_ref`
/// boots its own runtime to extract the cap set, and a nested one panics.
type TreeState = std::collections::BTreeMap<String, holon_loro::loro_backend::SnapshotBlock>;

async fn run_pair_script(
    ref_state: &holon_integration_tests::pbt::reference_state::ReferenceState,
    script: &[PairStep],
) -> (TreeState, TreeState, usize, Vec<String>) {
    let resolver = IdResolver::default();
    let (owner_caps, receiver_caps, handle, _) =
        boot_two_instances_with_receiver_caps(&resolver, ref_state).await;
    let two = caps_two(&owner_caps);
    let recv = owner_caps.expect::<dyn SutReceiverBackend>();

    // One pairing gesture, read-write, then a first round so the receiver holds
    // the owner's tree — every later concurrent op is authored against a SHARED
    // ancestor, which is the merge shape the design turns on.
    two.share_container("holon_tree", "receiver").await;
    two.sync_now(true).await;
    holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
        handle.receiver(),
        PAIR_SETTLE,
    )
    .await;

    // The oracle's subtraction, resolved BEFORE the script so writes and the
    // comparison agree on what is observable.
    let exclude = recv.receiver_boot_block_ids().await;

    let mut writes = 0usize;
    for step in script {
        match step {
            PairStep::Write { on_owner, pick, op } => {
                let caps = if *on_owner {
                    &owner_caps
                } else {
                    &receiver_caps
                };
                let state = handle.loro_tree_state(*on_owner, &exclude).await;
                if !apply_pair_op(caps, &state, op, *pick).await {
                    continue;
                }
                let side = if *on_owner {
                    handle.owner()
                } else {
                    handle.receiver()
                };
                holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
                    side,
                    PAIR_SETTLE,
                )
                .await;
                writes += 1;
            }
            PairStep::Sync { owner_to_receiver } => {
                two.sync_now(*owner_to_receiver).await;
                let side = if *owner_to_receiver {
                    handle.receiver()
                } else {
                    handle.owner()
                };
                holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
                    side,
                    PAIR_SETTLE,
                )
                .await;
            }
        }
    }

    drive_to_sync_fixpoint(&two, &handle).await;

    let mut projection_lag = handle.sql_projection_lag(true, &exclude).await;
    projection_lag.extend(handle.sql_projection_lag(false, &exclude).await);
    (
        handle.loro_tree_state(true, &exclude).await,
        handle.loro_tree_state(false, &exclude).await,
        writes,
        projection_lag,
    )
}

fn caps_two(caps: &holon_pbt_core::composition::CapMap) -> std::sync::Arc<dyn SutTwoInstance> {
    caps.expect::<dyn SutTwoInstance>()
}

fn assert_converged(
    owner: &TreeState,
    receiver: &TreeState,
    writes: usize,
    projection_lag: &[String],
    label: &str,
) {
    // Peer-to-peer agreement is only half the claim. Both peers can hold the
    // same Loro tree while a projection pass rolled its batch back or withheld
    // an op, leaving SQL — everything the UI reads — behind. Judge that FIRST,
    // and before the `writes == 0` return below: boot alone drives dozens of
    // projection passes, so a script whose every precondition was unsatisfiable
    // still has a real SQL projection to judge. Skipping it there would let a
    // vacuous draw hide a lagging projection.
    assert!(
        projection_lag.is_empty(),
        "{label}: the peers' Loro trees may agree, but a side's SQL projection is BEHIND its \
         Loro tree after {writes} write(s) and a sync fixed point. {} divergence(s):\n{}",
        projection_lag.len(),
        projection_lag.join("\n")
    );
    if writes == 0 {
        // Every generated step's precondition was unsatisfiable. Nothing was
        // exercised, so there is nothing more to judge — and calling the
        // CONVERGENCE claim green would be a vacuous pass.
        return;
    }
    if owner == receiver {
        return;
    }
    let ids: BTreeSet<&String> = owner.keys().chain(receiver.keys()).collect();
    let divergent: Vec<String> = ids
        .into_iter()
        .filter(|id| owner.get(*id) != receiver.get(*id))
        .map(|id| {
            format!(
                "{id}\n  owner:    {:?}\n  receiver: {:?}",
                owner.get(id),
                receiver.get(id)
            )
        })
        .collect();
    panic!(
        "{label}: the two peers did NOT converge after {writes} write(s) and a sync fixed point. \
         {} block(s) differ:\n{}",
        divergent.len(),
        divergent.join("\n")
    );
}

/// The lag claim is judged even when the draw exercised nothing: booting the
/// pair drove dozens of projection passes, so there is a real SQL projection to
/// judge, and returning on `writes == 0` first would report such a draw green
/// over a projection that is behind.
#[test]
#[should_panic(expected = "SQL projection is BEHIND")]
fn a_draw_that_exercised_nothing_still_judges_the_projection_lag() {
    let empty = TreeState::new();
    assert_converged(
        &empty,
        &empty,
        0,
        &["receiver block:c2: held in Loro, ABSENT from block_raw".to_string()],
        "every precondition unsatisfiable",
    );
}

proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: std::env::var("PAIR_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(24),
        max_shrink_iters: 20,
        failure_persistence: None,
        .. proptest::test_runner::Config::default()
    })]

    /// **Inc 0 keystone.** Both peers author concurrently — structure and text
    /// — over a read-write pairing, and the pair converges.
    ///
    /// It drew the receiver-projection stall in roughly one run of five until
    /// the projection grounded an UPDATE's `parent_id` and the run loop
    /// re-drove a failed pass. Case count is `PAIR_CASES` (default 24).
    #[test]
    fn concurrent_two_writer_pair_converges(
        script in proptest::collection::vec(pair_step(), 1..7)
    ) {
        let rt = rt();
        let ref_state = wide_e2e_ref();
        let (owner, receiver, writes, lag) = rt.block_on(run_pair_script(&ref_state, &script));
        assert_converged(&owner, &receiver, writes, &lag, &format!("script {script:?}"));
    }
}

/// The worst shape, pinned deterministically: a text edit on the RECEIVER
/// concurrent with a structural create on the OWNER, then sync both ways. This
/// is the exact merge D70 reports as fatal on a shallow share
/// (`tree_state.rs:1198`); the replicate-all path exports full op lineage
/// (`ExportMode::updates_owned`), and this test is what says whether that
/// difference matters.
#[test]
fn edit_on_receiver_concurrent_with_create_on_owner_converges() {
    let script = vec![
        PairStep::Write {
            on_owner: false,
            pick: 1,
            op: PairOp::Type("phone".to_string()),
        },
        PairStep::Write {
            on_owner: true,
            pick: 1,
            op: PairOp::Create,
        },
        PairStep::Sync {
            owner_to_receiver: true,
        },
        PairStep::Sync {
            owner_to_receiver: false,
        },
    ];
    let rt = rt();
    let ref_state = wide_e2e_ref();
    let (owner, receiver, writes, lag) = rt.block_on(run_pair_script(&ref_state, &script));
    assert!(writes == 2, "worst shape: expected 2 writes, got {writes}");
    assert_converged(&owner, &receiver, writes, &lag, "worst shape");

    // Teeth: convergence on two peers that both lost the receiver's edit would
    // also be "equal". Require the phone-side text to be present on BOTH.
    for (side, state) in [("owner", &owner), ("receiver", &receiver)] {
        assert!(
            state.values().any(|b| b.block.content.contains("phone")),
            "{side} converged WITHOUT the receiver-authored text — the reverse leg carried \
             nothing, so this case proves only that two identical trees are identical"
        );
    }

    // Teeth: and the owner's concurrent create must have survived the merge.
    for (side, state) in [("owner", &owner), ("receiver", &receiver)] {
        assert!(
            state.values().any(|b| b.block.content == "pair"),
            "{side} converged WITHOUT the owner's concurrent create"
        );
    }
}

/// The structural half of the same shape: concurrent tree MOVES on both peers.
/// A move-move merge is the arm that reparents a node on both sides at once —
/// the loro tree state's hardest case.
#[test]
fn concurrent_structural_moves_on_both_peers_converge() {
    let script = vec![
        PairStep::Write {
            on_owner: true,
            pick: 2,
            op: PairOp::Create,
        },
        PairStep::Write {
            on_owner: false,
            pick: 2,
            op: PairOp::Create,
        },
        PairStep::Write {
            on_owner: true,
            pick: 3,
            op: PairOp::Indent,
        },
        PairStep::Write {
            on_owner: false,
            pick: 3,
            op: PairOp::Outdent,
        },
        PairStep::Sync {
            owner_to_receiver: true,
        },
        PairStep::Sync {
            owner_to_receiver: false,
        },
    ];
    let rt = rt();
    let ref_state = wide_e2e_ref();
    let (owner, receiver, writes, lag) = rt.block_on(run_pair_script(&ref_state, &script));
    assert!(
        writes == 4,
        "concurrent moves: expected 4 writes, got {writes}"
    );
    assert_converged(&owner, &receiver, writes, &lag, "concurrent moves");
}

/// **Regression pin — the cross-peer indent+join CLASS.**
///
/// An `indent` and a `join_block` applied across the two peers used to stall
/// the RECEIVER's Loro→SQL projection permanently. The CRDT layer was never at
/// fault — the trees merge and the peers agree. The projection emitted
///
/// ```text
/// update:block:fe-target<-block:fe-blocked
/// ```
///
/// re-parenting onto the block the `join` had just deleted from `block_raw`, so
/// the deferred `parent_id` self-FK failed at COMMIT and rolled the whole batch
/// back. `LoroSyncController::run_loop` is wake-driven, so with nothing to
/// re-drive the failed batch that ONE failure was permanent and
/// `converge_projections` never reached its fixed point.
///
/// Fixed in two halves: the projection grounds an UPDATE's `parent_id` the same
/// way it already grounded a CREATE's, and the run loop re-drives a failed or
/// incomplete pass a bounded number of times before raising a degraded banner.
///
/// Two shapes are pinned because the defect is a CLASS, not one interleaving:
/// this one puts the `indent` on the receiver, its sibling below puts four of
/// five writes on the owner.
#[test]
fn cross_peer_indent_then_join_stalls_the_receiver_projection() {
    let script = vec![
        PairStep::Write {
            on_owner: false,
            pick: 3,
            op: PairOp::Indent,
        },
        PairStep::Write {
            on_owner: true,
            pick: 2,
            op: PairOp::Join,
        },
    ];
    let rt = rt();
    let ref_state = wide_e2e_ref();
    let (owner, receiver, writes, lag) = rt.block_on(run_pair_script(&ref_state, &script));
    assert_eq!(writes, 2);
    assert_converged(&owner, &receiver, writes, &lag, "indent+join");
}

/// The same class, owner-heavy: the shrunk counterexample the lane's verifier
/// reached independently, with four of its five writes on the OWNER. Pinned
/// beside its sibling so a fix that only handles a receiver-side `indent`
/// cannot look complete.
/// It is also the reproducer for the batch delete-cascade loss recorded as
/// `docs/Testing/bugfunnel/entries/
/// 2026-09-02-receiver-sql-loses-a-block-its-loro-tree-still-holds.md`: the
/// `join` batch carries `update:block:c2` (reparenting it off `block:c1`) ahead
/// of `delete:block:c1`, and the delete's descendant walk — which reads the
/// database as it stood BEFORE the batch — cascaded onto `block:c2` anyway.
#[test]
fn owner_heavy_indent_then_join_stalls_the_receiver_projection() {
    let script = vec![
        PairStep::Write {
            on_owner: true,
            pick: 1,
            op: PairOp::Indent,
        },
        PairStep::Write {
            on_owner: false,
            pick: 1,
            op: PairOp::Indent,
        },
        PairStep::Write {
            on_owner: true,
            pick: 2,
            op: PairOp::Indent,
        },
        PairStep::Write {
            on_owner: true,
            pick: 3,
            op: PairOp::Indent,
        },
        PairStep::Write {
            on_owner: true,
            pick: 2,
            op: PairOp::Join,
        },
    ];
    let rt = rt();
    let ref_state = wide_e2e_ref();
    let (owner, receiver, writes, lag) = rt.block_on(run_pair_script(&ref_state, &script));
    assert_eq!(writes, 5);
    assert_converged(&owner, &receiver, writes, &lag, "owner-heavy indent+join");
}

/// The REVERSE leg, which the generator never draws (`SyncNow` always draws
/// `owner_to_receiver: true`) and which therefore has no property coverage.
///
/// It pins the per-direction audience end to end: a receiver→owner round is
/// addressed to the OWNER, and the owner admits it against its OWN identity. A
/// harness that reuses one constant audience for both legs makes the owner
/// refuse every envelope and import nothing.
#[test]
fn the_reverse_leg_reaches_the_owner_under_the_owners_own_audience() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, _) = boot_two_instances(&resolver, &ref_state).await;
        let two = caps.expect::<dyn SutTwoInstance>();
        let recv = caps.expect::<dyn SutReceiverBackend>();

        two.share_container("holon_tree", "receiver").await;
        two.sync_now(true).await;

        let before = recv.owner_block_ids().await;
        let witness = two.sync_now(false).await;
        holon_integration_tests::pbt::composed::wide_e2e::converge_handle(
            handle.owner(),
            std::time::Duration::from_secs(15),
        )
        .await;

        assert!(
            witness.imported >= 1,
            "the receiver→owner round imported nothing (pushed={} refusals={:?}) — with a \
             read-write pairing cert the owner must admit the receiver's delta",
            witness.pushed,
            witness.refusals
        );
        assert!(
            !witness
                .refusals
                .iter()
                .any(|r| r.contains("RefuseCapability")),
            "the pairing cert must confer Write, but the owner refused on capability: {:?}",
            witness.refusals
        );
        let after = recv.owner_block_ids().await;
        assert!(
            after.len() > before.len(),
            "the owner's store did not grow across the reverse round ({} -> {}) — the \
             receiver-authored state never landed",
            before.len(),
            after.len()
        );
    });
}

// ---------------------------------------------------------------------------
// The two-writer reference model, driven deterministically (D76.b).
// ---------------------------------------------------------------------------

use holon_integration_tests::pbt::composed::harness::ComposedSlice;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::sharing_state::RECEIVER_PRINCIPAL;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::ReceiverCreateBlock;
use holon_integration_tests::pbt::transitions::ShareContainer;
use holon_integration_tests::pbt::transitions::SyncNow;
use holon_integration_tests::pbt::transitions::share_container::ROOT_SELECTOR;
use holon_pbt_core::capabilities::RefSharedView;
use holon_pbt_core::composition::RunReport;
use holon_pbt_core::invariant::InvariantResult;
use proptest_state_machine::ReferenceStateMachine;

type Sut = ComposedSut<TwoInstanceE2E>;
type Machine = <TwoInstanceE2E as ComposedSlice>::Machine;

/// The two-writer chain: share, one forward round, a block authored on the
/// RECEIVER under an owner-authored parent, one reverse round.
///
/// Driven through `StateMachineTest::apply`, so the harness's per-tick
/// synthetic↔real reconcile judges every step exactly as it does inside the
/// property — that reconcile is the seam the authorship scoping exists for, and
/// without it step 4 panics on the 1:1 `synthetic`/`real_new` guard.
fn drive_two_writer_chain() -> (Sut, ReferenceState) {
    let mut ref_state = wide_e2e_ref();
    let mut sut = <Sut as StateMachineTest>::init_test(&ref_state);

    let mut step = |sut: Sut, ref_state: &mut ReferenceState, t: E2ETransition| -> Sut {
        assert!(
            Machine::preconditions(ref_state, &t),
            "two-writer chain: {t:?} violates its precondition against the booted oracle — the \
             chain is malformed, which is a finding about the model, not a reason to skip a step"
        );
        *ref_state = Machine::apply(ref_state.clone(), &t);
        let sut = <Sut as StateMachineTest>::apply(sut, ref_state, t);
        // A second settle window. The receiver's org write-back can outlast one
        // `CONVERGE_BUDGET` on a loaded host — the load-sensitive red `pair-inc0`
        // recorded on pristine `main` — and this chain would then red on that
        // instead of on the two-writer question. Same production settle, one
        // more budget; a write-back that is stalled rather than slow still reds.
        sut.settle_projections();
        sut
    };

    sut = step(
        sut,
        &mut ref_state,
        E2ETransition::ShareContainer(ShareContainer {
            selector: ROOT_SELECTOR.to_string(),
            principal: RECEIVER_PRINCIPAL.to_string(),
        }),
    );
    sut = step(
        sut,
        &mut ref_state,
        E2ETransition::SyncNow(SyncNow {
            owner_to_receiver: true,
        }),
    );

    // The parent the receiver writes under: an owner-authored block the model
    // knows the forward round carried. Picked from the MODEL, not by observing
    // the receiver — a parent chosen by observation would make the oracle's
    // parent claim a restatement of what it read.
    let parent = ref_state
        .blocks_delivered_to_receiver()
        .into_iter()
        .next()
        .expect(
            "one owner→receiver round must leave the model holding at least one delivered block; \
             with none, a peer write has nothing to parent under and this chain proves nothing",
        );
    sut = step(
        sut,
        &mut ref_state,
        E2ETransition::ReceiverCreateBlock(ReceiverCreateBlock {
            parent,
            content: "phone".to_string(),
            id: holon_api::EntityUri::block("pair-0"),
        }),
    );
    sut = step(
        sut,
        &mut ref_state,
        E2ETransition::SyncNow(SyncNow {
            owner_to_receiver: false,
        }),
    );
    (sut, ref_state)
}

/// The two-writer oracle's verdict, demanded rather than read.
///
/// Fails on absence (the peer-authored partition would be scoped out of the
/// owner's comparison and judged NOWHERE) and on `Skipped` (the oracle declined
/// to look at the only thing the chain exists to check). This is the assertion
/// the first version of these tests could never reach: it sat after a full
/// `check_invariants`, which panics on the fixed-id boot collision — a defect
/// this model does not own — so the guard was unreachable code.
fn assert_two_writer_oracle_engaged(report: &RunReport, ref_state: &ReferenceState) {
    assert!(
        !ref_state.peer_writes_delivered().is_empty(),
        "the model records no delivered peer write, so this assertion would judge nothing — the \
         chain did not reach the state it exists to create"
    );
    let verdict = report
        .ran
        .iter()
        .find(|(id, _)| id.0 == "inv-two-writer-peer-writes-land")
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| {
            panic!(
                "`inv-two-writer-peer-writes-land` did not run against a two-instance CapMap — \
                 the peer-authored partition of the owner's store is scoped out of the \
                 owner-vs-oracle comparison and judged NOWHERE. Ran: {:?}, deselected: {:?}",
                report.ran_ids(),
                report.deselected
            )
        });
    match verdict {
        InvariantResult::Ok => {}
        InvariantResult::Skipped(reason) => panic!(
            "the two-writer oracle SKIPPED after a receiver-authored block and a reverse round \
             ({reason}) — it declined to look at the only thing this chain exists to check"
        ),
        other => panic!("the two-writer oracle failed: {other:?}"),
    }
}

/// **The two-writer model, asserted.** Runs the chain and demands the
/// two-writer oracle ENGAGE and pass — nothing else.
///
/// Not `#[ignore]`d, and deliberately narrower than its sibling below: it runs
/// the harness reconcile over a second writer and judges the peer-authored
/// partition, without also demanding the whole catalog be green over a merged
/// pair of independently-seeded vaults (which the fixed-id boot collision makes
/// impossible until the layout-container increment lands). Without this test
/// the two-writer oracle engages in NO runnable test and could sit at `Skipped`
/// forever.
#[test]
fn a_receiver_authored_block_reaches_the_owner_and_the_two_writer_oracle_engages() {
    let (sut, ref_state) = drive_two_writer_chain();
    // Authorship of every id in the owner's tree: a red is then triaged against
    // provenance rather than presence — an id listing BOTH peers is one both
    // instances minted, not one that crossed.
    for line in sut.runtime().block_on(sut.handle().authorship_dump(true)) {
        eprintln!("[authorship owner] {line}");
    }
    assert_two_writer_oracle_engaged(&sut.run_report_now(&ref_state), &ref_state);
}

/// The SAME chain, judged by the WHOLE catalog — the state the property will be
/// in once the two-writer alphabet is drawable by default.
///
/// ## OPEN, and why it is `#[ignore]`d
/// It reds on the FIXED-ID BOOT COLLISION, not on the two-writer model. Once a
/// receiver→owner round runs, the owner's tree holds two nodes for every id
/// both instances seed independently:
///
/// ```text
/// [inv-blocks-match-ref/loro] actual holds DUPLICATE ids
///   ["block:368857d2-…", "block:journals", "block:journals::action::0",
///    "block:journals::auto-create"]
/// [inv-birth-contract-satisfied] 4 of 42 visible block(s) hold no minted
///   position … block:structural-page … block:receiver-root
/// [inv-viewmodel-entity-ids-subset-of-data] phantom entity … block:receiver-root
/// ```
///
/// The `[authorship owner]` dump names the cause of the first: those ids list
/// `peers {1, 2}` while `block:pair-0` and `block:receiver-root` list
/// `peers {2}` and `block:parent` lists `peers {1}`. Both instances minted
/// them; nothing crossed that should not have. That is the layout-container
/// increment's defect, and subtracting the receiver's boot ids to make it pass
/// is exactly the subtraction that increment exists to remove.
///
/// The other two are a SECOND, separate gap: `inv-birth-contract-satisfied` and
/// `inv-viewmodel-entity-ids-subset-of-data` read the store directly and never
/// consult the seed / unmodeled set, so they judge `block:receiver-root` — a
/// pure `peers {2}` foreign block — as if the owner's model should account for
/// it. The scoping hook cannot reach them. Over-strict, never a false green,
/// and it needs those two bodies to respect the partition.
///
/// The two-writer oracle is asserted FIRST here too, so this test can only ever
/// fail for a reason its sibling above has already cleared.
#[test]
#[ignore = "OPEN (D78): reds on the journals half of the fixed-id boot collision (duplicate block:journals* nodes after a reverse round); the layout half is closed by the device-local layout doc"]
fn the_two_writer_chain_is_green_across_the_whole_catalog() {
    let (sut, ref_state) = drive_two_writer_chain();
    assert_two_writer_oracle_engaged(&sut.run_report_now(&ref_state), &ref_state);
    <Sut as StateMachineTest>::check_invariants(&sut, &ref_state);
}

// ─── The fixed-id boot collision (plan v3 Inc 3) ───────────────────────

/// Boot a pair, pair it read-write, and drive both directions to a sync fixed
/// point. NO writes: the collision is a property of booting and syncing alone.
///
/// Returns each side's live-node counts plus the owner's tree, which is what
/// names the `block:__default__` subtree.
async fn pair_and_settle(
    ref_state: &holon_integration_tests::pbt::reference_state::ReferenceState,
) -> (
    std::collections::BTreeMap<String, usize>,
    std::collections::BTreeMap<String, usize>,
    TreeState,
) {
    let resolver = IdResolver::default();
    let (owner_caps, _receiver_caps, handle, _) =
        boot_two_instances_with_receiver_caps(&resolver, ref_state).await;
    let two = caps_two(&owner_caps);
    two.share_container("holon_tree", "receiver").await;
    drive_to_sync_fixpoint(&two, &handle).await;
    (
        handle.live_node_counts(true).await,
        handle.live_node_counts(false).await,
        handle.loro_tree_state(true, &BTreeSet::new()).await,
    )
}

/// The `block:__default__` subtree — every block the bundled `index.org` layout
/// contributes, closed over `parent_id`. This is the set D68.b rules
/// DEVICE-LOCAL, so it is the set the layout container has to carry.
fn layout_subtree(owner_tree: &TreeState) -> BTreeSet<String> {
    let mut closure: BTreeSet<String> = BTreeSet::new();
    closure.insert(holon_api::DEFAULT_DOC_BLOCK_ID.to_string());
    // The tree map is in id order, not parent order, so grow to a fixed point
    // rather than assuming a single pass reaches the leaves.
    loop {
        let before = closure.len();
        for (id, snap) in owner_tree {
            if closure.contains(snap.block.parent_id.as_str()) {
                closure.insert(id.clone());
            }
        }
        if closure.len() == before {
            return closure;
        }
    }
}

/// Ids carried by MORE than one live tree node, optionally restricted to a set.
fn duplicate_ids(
    counts: &std::collections::BTreeMap<String, usize>,
    restrict: Option<&BTreeSet<String>>,
) -> Vec<String> {
    counts
        .iter()
        .filter(|(id, n)| **n > 1 && restrict.is_none_or(|r| r.contains(*id)))
        .map(|(id, n)| format!("{id} ×{n}"))
        .collect()
}

/// **The layout half of the fixed-id boot collision.**
///
/// Both devices seed the bundled `index.org` layout under the SAME fixed ids
/// (`block:root-layout`, `block:__default__`, the sidebars), so once a round
/// carries the receiver's state to the owner every one of those ids names TWO
/// live tree nodes. Every other oracle in this file keys by stable id and so
/// cannot see it — `live_node_counts` counts nodes.
///
/// D68.b rules the layout DEVICE-LOCAL, so the fix is structural: the layout
/// lives in its own `LoroDoc` that never enters the replication set, and this
/// assertion is that increment's green criterion. Deterministic in ~10 s; 25
/// duplicated ids at the time of writing.
#[test]
fn the_device_local_layout_ids_resolve_to_one_live_node_after_a_round() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    let (owner, receiver, owner_tree) = rt.block_on(pair_and_settle(&ref_state));

    let layout = layout_subtree(&owner_tree);
    assert!(
        layout.len() > 1,
        "the owner's tree holds no `{}` subtree, so this test asserts nothing — the layout seed \
         did not run",
        holon_api::DEFAULT_DOC_BLOCK_ID
    );

    for (side, counts) in [("owner", &owner), ("receiver", &receiver)] {
        let dups = duplicate_ids(counts, Some(&layout));
        assert!(
            dups.is_empty(),
            "{side}: {} device-local layout id(s) resolve to MORE than one live Loro node after a \
             round — both devices minted the bundled layout independently and replication carried \
             both mintings:\n  {}",
            dups.len(),
            dups.join("\n  ")
        );
    }
}

/// **The whole fixed-id boot collision, layout and replicated alike.**
///
/// The strict statement of the same law over EVERY id: after a whole-vault
/// round, no block id may name two live tree nodes on either side. It is
/// strictly stronger than the layout test above, and it stays red on the
/// families a device-local layout container cannot reach — `block:journals`
/// and its machinery, and the rule-minted journal day block, all of which are
/// replicated content that both devices mint independently at boot.
///
/// Those families need a boot-ORDER answer, not a container answer: the seed
/// is already idempotent against a node the tree holds
/// (`BlockCellRegistry::create_entity` skips the create when
/// `resolve_to_tree_id` resolves), so a receiver that bootstrapped from the
/// owner's snapshot BEFORE its first seed would never mint the second node.
/// Un-ignore once that order is ruled and wired.
#[test]
#[ignore = "OPEN (D78): the replicated fixed-id roots — block:journals + its machinery and the \
            rule-minted journal day block — are minted independently on both devices before the \
            first sync; they need a boot-ORDER answer, not a container answer"]
fn every_fixed_boot_id_resolves_to_one_live_node_after_a_round() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    let (owner, receiver, _) = rt.block_on(pair_and_settle(&ref_state));

    for (side, counts) in [("owner", &owner), ("receiver", &receiver)] {
        let dups = duplicate_ids(counts, None);
        assert!(
            dups.is_empty(),
            "{side}: {} block id(s) resolve to MORE than one live Loro node after a round:\n  {}",
            dups.len(),
            dups.join("\n  ")
        );
    }
}

// ─── Production whole-store pairing ─────────────────────────────────────────

/// The entity and operation names the receiver-side pairing operation is
/// dispatched under. Named here so the test drives the SHIPPING path — a
/// pairing that only the harness can start proves nothing about a device.
const PAIRING_ENTITY: &str = "device";
const PAIR_OFFER_OP: &str = "pair_offer";
const PAIR_ACCEPT_OP: &str = "pair_with_owner";

/// Dispatch one operation on one side's production engine. The `Err` is
/// returned rather than raised so a caller can put a missing operation INTO
/// the oracle's failure message instead of masking the oracle with a panic.
async fn dispatch_pairing_op(
    handle: &holon_integration_tests::pbt::composed::wide_e2e::WideHandle,
    side: &str,
    op: &str,
    params: holon_api::StorageEntity,
) -> anyhow::Result<holon_api::OpOutcome> {
    let engine = handle.engine().unwrap_or_else(|| {
        panic!("the {side} instance has no backend engine; pairing dispatches through it")
    });
    let entity: holon_api::EntityName = PAIRING_ENTITY.to_string().into();
    engine
        .execute_operation(&entity, op, params, holon_api::OpOrigin::User)
        .await
}

/// Read the invite out of `pair_offer`'s response — a JSON object carried as a
/// string, the shape `share_subtree` returns its ticket in. `Err` carries the
/// diagnostic for a response that holds no invite, kept separate so an oracle
/// can quote it without quoting a live invite.
fn invite_from_response(v: &holon_api::Value) -> Result<String, String> {
    let Some(text) = v.as_string() else {
        return Err(format!(
            "`{PAIR_OFFER_OP}` returned a non-string response: {v:?}"
        ));
    };
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(json) => json["invite"].as_str().map(str::to_string).ok_or_else(|| {
            format!("`{PAIR_OFFER_OP}` returned a response with no `invite` field: {text}")
        }),
        Err(e) => Err(format!(
            "`{PAIR_OFFER_OP}` returned a response that is not JSON ({e}): {text}"
        )),
    }
}

/// What a failing pairing oracle may print about the offer. An invite is a
/// bearer credential over every container for its whole TTL, so the fingerprint
/// — never the invite — is what reaches the log.
fn describe_offer(offer: &Result<String, String>) -> String {
    match offer {
        Ok(invite) => holon_loro::device_pairing_op::invite_fingerprint(invite),
        Err(diagnostic) => diagnostic.clone(),
    }
}

/// Run the production pairing: the owner mints an invite, the receiver consumes
/// it. Returns the two dispatch results verbatim — including their errors,
/// which the oracles quote.
async fn run_production_pairing(
    handle: &std::sync::Arc<
        holon_integration_tests::pbt::composed::two_instance::TwoInstanceHandle,
    >,
) -> (Result<String, String>, Result<(), String>) {
    let offer = mint_pairing_invite(handle, "write").await;
    let invite = match offer {
        Ok(invite) => invite,
        Err(diagnostic) => {
            // The offer's error is returned on BOTH legs: a test that only
            // reads the accept result must still see why the pair never
            // started.
            return (Err(diagnostic.clone()), Err(diagnostic));
        }
    };
    let accepted = consume_pairing_invite(handle, &invite).await;
    (Ok(invite), accepted)
}

/// Mint one invite on the owner. `Err` carries why no invite exists.
async fn mint_pairing_invite(
    handle: &std::sync::Arc<
        holon_integration_tests::pbt::composed::two_instance::TwoInstanceHandle,
    >,
    capability: &str,
) -> Result<String, String> {
    let mut params = holon_api::StorageEntity::new();
    params.insert(
        "capability".into(),
        holon_api::Value::String(capability.into()),
    );
    match dispatch_pairing_op(handle.owner(), "owner", PAIR_OFFER_OP, params).await {
        Ok(outcome) => match outcome.response {
            Some(v) => invite_from_response(&v),
            None => Err(format!("`{PAIR_OFFER_OP}` returned no response payload")),
        },
        Err(e) => Err(format!("`{PAIR_OFFER_OP}` failed: {e:#}")),
    }
}

async fn consume_pairing_invite(
    handle: &std::sync::Arc<
        holon_integration_tests::pbt::composed::two_instance::TwoInstanceHandle,
    >,
    invite: &str,
) -> Result<(), String> {
    let mut params = holon_api::StorageEntity::new();
    params.insert(
        "invite".into(),
        holon_api::Value::String(invite.to_string()),
    );
    dispatch_pairing_op(handle.receiver(), "receiver", PAIR_ACCEPT_OP, params)
        .await
        .map(|_| ())
        .map_err(|e| format!("`{PAIR_ACCEPT_OP}` failed: {e:#}"))
}

/// **D68.b + D71.b.** The production pairing operation replicates the WHOLE
/// store: after one pairing and a sync fixed point, every container the owner
/// advertises is converged on the receiver.
///
/// This is deliberately not `pair_and_settle`, which mints the membership cert
/// inside the harness. Here the cert, the advertiser and the dial all come from
/// the operation under test, so a production path that stops granting — or
/// stops replicating past the root container — fails here.
#[test]
fn production_pairing_replicates_the_whole_store() {
    production_pairing_replicates_over(TransportChoice::from_env());
}

/// The same property pinned to the SHIPPING wire, so the landing gate covers
/// iroh whether or not anyone sets the environment variable.
#[test]
fn production_pairing_replicates_the_whole_store_over_iroh() {
    production_pairing_replicates_over(TransportChoice::Iroh);
}

fn production_pairing_replicates_over(transport: TransportChoice) {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, _) = holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, transport).await;
        let two = caps_two(&caps);

        let (invite, accepted) = run_production_pairing(&handle).await;
        drive_to_sync_fixpoint(&two, &handle).await;

        let owner_tree = handle.loro_tree_state(true, &BTreeSet::new()).await;
        let device_local: BTreeSet<holon_api::EntityUri> = layout_subtree(&owner_tree)
            .iter()
            .map(|id| {
                holon_api::EntityUri::parse(id)
                    .unwrap_or_else(|e| panic!("the layout subtree holds a non-URI id {id:?}: {e}"))
            })
            .collect();
        let owner = handle.loro_tree_state(true, &device_local).await;
        let receiver = handle.loro_tree_state(false, &device_local).await;

        assert!(
            !owner.is_empty(),
            "the owner's replicated tree is empty outside the device-local layout, so convergence \
             here would be vacuous"
        );
        let missing: Vec<&String> = owner
            .keys()
            .filter(|id| receiver.get(*id) != owner.get(*id))
            .collect();
        assert!(
            missing.is_empty(),
            "after `{PAIRING_ENTITY}.{PAIR_OFFER_OP}` + `{PAIRING_ENTITY}.{PAIR_ACCEPT_OP}` and a \
             sync fixed point on the {} wire, {} of {} replicated block(s) have not converged on \
             the receiver: {missing:?}\n  offer: {}\n  accept: {accepted:?}",
            transport.kind().as_str(),
            missing.len(),
            owner.len(),
            describe_offer(&invite),
        );
    });
}

/// **D73.a.** A receiver that already holds a per-subtree MOUNT must be refused
/// — loudly, naming each mount — rather than pairing into a store whose mounts
/// the owner's snapshot knows nothing about.
#[test]
fn production_pairing_refuses_a_receiver_that_holds_mounts() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (_caps, handle, _) =
            holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, TransportChoice::Relay).await;

        let mount = mount_the_owners_subtree_on_the_receiver(&handle).await;
        let (_, accepted) = run_production_pairing(&handle).await;

        let refusal = accepted.expect_err(
            "pairing a receiver that holds a mount SUCCEEDED; the mount's shared doc is outside \
             the owner's replication set, so the pair silently drops it",
        );
        assert!(
            refusal.contains(&mount),
            "the refusal must name the mount that caused it so the user can unshare it; got: \
             {refusal}"
        );
    });
}

/// **D72.a.** A receiver paired under a READ-only grant cannot write back: the
/// owner's acceptor refuses the reverse leg, naming the missing capability.
/// The capability decision must be the acceptor's — a receiver that merely
/// declines to push would pass a weaker version of this.
///
/// The read/write rule lives in `holon_sharing::acceptor::admit`, which the
/// production iroh leg never reaches: that leg authorizes in
/// `share_enrollment::acceptor_enroll`, which proves possession of a
/// `CapabilitySecret` and has no Read/Write dimension at all. So
/// `Capability::Read` is unenforceable over the shipping wire, and closing that
/// is an architecture decision, not a fix this test can drive.
#[test]
#[ignore = "OPEN (D86): `pair_offer` refuses a read grant outright, because the production iroh \
            leg authorizes via CapabilitySecret enrollment and carries no Read/Write dimension — \
            `acceptor::admit`, where the rule lives, has no caller on that path. Un-ignore \
            together with that refusal once D86 lands the enforcing gate"]
fn a_read_only_pairing_cannot_write_back_to_the_owner() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (caps, handle, _) =
            holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, TransportChoice::Relay).await;
        let two = caps_two(&caps);

        let invite = mint_pairing_invite(&handle, "read")
            .await
            .expect("a read-only pair offer must be mintable");
        consume_pairing_invite(&handle, &invite).await.expect(
            "accepting a read-only invite must succeed — the grant is valid, it is the write \
             that is not",
        );
        drive_to_sync_fixpoint(&two, &handle).await;

        let reverse = two.sync_now(false).await;
        assert!(
            !reverse.unauthorized.is_empty(),
            "the owner ADMITTED a read-only peer's write-back. Refusals: {:?}",
            reverse.refusals
        );
    });
}

/// A note the user jots into today's journal is the likeliest content on an
/// otherwise-fresh device, and it is not the app's: pairing must refuse and
/// name it, because adopting the owner's store drops it.
#[test]
fn production_pairing_refuses_a_receiver_that_holds_a_journal_note() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (_caps, handle, _) =
            holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, TransportChoice::Relay).await;

        let day = receiver_day_block(&handle).await;
        let note = holon_api::EntityUri::block("receiver-journal-note");
        handle
            .receiver_create_block(&day, "bought milk", &note)
            .await;

        let (_, accepted) = run_production_pairing(&handle).await;

        let refusal = accepted.expect_err(&format!(
            "pairing a receiver that holds a note under its journal day block {day} SUCCEEDED; \
             the note is outside the owner's store, so the pair drops it"
        ));
        assert!(
            refusal.contains(note.as_str()),
            "the refusal must name the note so the user knows what pairing would drop; got: \
             {refusal}"
        );
    });
}

/// A page created while the focus is on the layout root is the user's. The
/// bundled layout is a closed set of ids, so anything else under it is content
/// pairing would drop.
#[test]
fn production_pairing_refuses_a_receiver_that_holds_a_page_under_the_layout_root() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (_caps, handle, _) =
            holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, TransportChoice::Relay).await;

        let layout_root = holon_api::EntityUri::parse(holon_api::ROOT_LAYOUT_BLOCK_ID)
            .expect("the layout root id is a URI");
        let page = holon_api::EntityUri::block("receiver-layout-page");
        handle
            .receiver_create_block(&layout_root, "my notes", &page)
            .await;

        let (_, accepted) = run_production_pairing(&handle).await;

        let refusal = accepted.expect_err(
            "pairing a receiver that holds a page under the layout root SUCCEEDED; the layout \
             closure swallowed user content",
        );
        assert!(
            refusal.contains(page.as_str()),
            "the refusal must name the page so the user knows what pairing would drop; got: \
             {refusal}"
        );
    });
}

/// **D86.** A read grant nothing downstream enforces is refused at the offer,
/// not minted and reported as granted.
#[test]
fn a_read_only_pair_offer_is_refused_until_the_wire_can_enforce_it() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (_caps, handle, _) =
            holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, TransportChoice::Relay).await;

        let offer = mint_pairing_invite(&handle, "read").await;
        let Err(refusal) = offer else {
            panic!("a read-only pair offer was MINTED, and the iroh leg grants write");
        };
        assert!(
            refusal.contains("D86"),
            "the refusal must name the open decision so the user can find out when read pairing \
             arrives; got: {refusal}"
        );
    });
}

/// A second offer while one is live is refused: minting again would strand the
/// live offer's advertisements, which no invite then names and `pair_cancel`
/// can no longer reach.
#[test]
fn a_second_pair_offer_while_one_is_live_is_refused() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (_caps, handle, _) =
            holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, TransportChoice::Relay).await;

        mint_pairing_invite(&handle, "write")
            .await
            .expect("the first offer mints");
        let second = mint_pairing_invite(&handle, "write").await;
        let Err(refusal) = second else {
            panic!("a second offer was minted while the first was still advertising");
        };
        assert!(
            refusal.contains("pair_cancel"),
            "the refusal must name the way out; got: {refusal}"
        );
    });
}

/// `pair_cancel` withdraws the offer: the invite it minted no longer pairs, and
/// the containers it advertised can be offered again.
#[test]
fn pairing_after_pair_cancel_is_refused() {
    let rt = rt();
    let ref_state = wide_e2e_ref();
    rt.block_on(async {
        let resolver = IdResolver::default();
        let (_caps, handle, _) =
            holon_integration_tests::pbt::composed::two_instance::boot_two_instances_with_an_empty_receiver_on(&resolver, &ref_state, TransportChoice::Relay).await;

        let invite = mint_pairing_invite(&handle, "write")
            .await
            .expect("the offer mints");
        dispatch_pairing_op(
            handle.owner(),
            "owner",
            "pair_cancel",
            holon_api::StorageEntity::new(),
        )
        .await
        .expect("pair_cancel withdraws the live offer");

        let accepted = consume_pairing_invite(&handle, &invite).await;
        assert!(
            accepted.is_err(),
            "a cancelled invite still paired; cancellation is the only revocation this offer has"
        );
        mint_pairing_invite(&handle, "write")
            .await
            .expect("after a cancel the containers are free to be offered again");
    });
}

/// The receiver's journal day block — the one its own auto-create rule minted
/// at boot, and the only child of `block:journals` that is not machinery.
async fn receiver_day_block(
    handle: &std::sync::Arc<
        holon_integration_tests::pbt::composed::two_instance::TwoInstanceHandle,
    >,
) -> holon_api::EntityUri {
    const JOURNALS: &str = "block:journals";
    let tree = handle.loro_tree_state(false, &BTreeSet::new()).await;
    let days: Vec<&String> = tree
        .iter()
        .filter(|(id, snap)| {
            snap.block.parent_id.as_str() == JOURNALS && !id.starts_with("block:journals::")
        })
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        days.len(),
        1,
        "the receiver must hold exactly the one day block its auto-create rule mints; got {days:?}"
    );
    holon_api::EntityUri::parse(days[0])
        .unwrap_or_else(|e| panic!("the day block id {:?} is not a URI: {e}", days[0]))
}

/// Give the receiver a mount by accepting a share of one of the owner's
/// subtrees — the state D73.a refuses to pair over. Returns the mount block id.
async fn mount_the_owners_subtree_on_the_receiver(
    handle: &std::sync::Arc<
        holon_integration_tests::pbt::composed::two_instance::TwoInstanceHandle,
    >,
) -> String {
    let owner_tree = handle.loro_tree_state(true, &BTreeSet::new()).await;
    let shareable = owner_tree
        .keys()
        .find(|id| id.as_str() != holon_api::DEFAULT_DOC_BLOCK_ID)
        .expect("the owner's seeded tree holds a shareable block")
        .clone();

    let mut share = holon_api::StorageEntity::new();
    share.insert("id".into(), holon_api::Value::String(shareable));
    share.insert("retention".into(), holon_api::Value::String("none".into()));
    let shared = dispatch_tree_op(handle.owner(), "owner", "share_subtree", share)
        .await
        .expect("share_subtree")
        .response
        .and_then(|v| v.as_string().map(str::to_string))
        .expect("share_subtree returns a response");
    let ticket = serde_json::from_str::<serde_json::Value>(&shared)
        .expect("share_subtree's response is JSON")["ticket"]
        .as_str()
        .expect("share_subtree's response carries a ticket")
        .to_string();

    let mut accept = holon_api::StorageEntity::new();
    accept.insert(
        "parent_id".into(),
        holon_api::Value::String(holon_api::ROOT_LAYOUT_BLOCK_ID.to_string()),
    );
    accept.insert("ticket".into(), holon_api::Value::String(ticket));
    dispatch_tree_op(
        handle.receiver(),
        "receiver",
        "accept_shared_subtree",
        accept,
    )
    .await
    .expect("accept_shared_subtree")
    .response
    .and_then(|v| v.as_string().map(str::to_string))
    .map(|json| {
        serde_json::from_str::<serde_json::Value>(&json)
            .expect("accept_shared_subtree's response is JSON")["mount_block_id"]
            .as_str()
            .expect("accept_shared_subtree's response carries the mount block id")
            .to_string()
    })
    .expect("accept_shared_subtree returns the mount block id")
}

async fn dispatch_tree_op(
    handle: &holon_integration_tests::pbt::composed::wide_e2e::WideHandle,
    side: &str,
    op: &str,
    params: holon_api::StorageEntity,
) -> anyhow::Result<holon_api::OpOutcome> {
    let engine = handle
        .engine()
        .unwrap_or_else(|| panic!("the {side} instance has no backend engine"));
    let entity: holon_api::EntityName = "tree".to_string().into();
    engine
        .execute_operation(&entity, op, params, holon_api::OpOrigin::User)
        .await
}
