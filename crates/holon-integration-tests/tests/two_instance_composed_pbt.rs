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
) -> (TreeState, TreeState, usize) {
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

    (
        handle.loro_tree_state(true, &exclude).await,
        handle.loro_tree_state(false, &exclude).await,
        writes,
    )
}

fn caps_two(caps: &holon_pbt_core::composition::CapMap) -> std::sync::Arc<dyn SutTwoInstance> {
    caps.expect::<dyn SutTwoInstance>()
}

fn assert_converged(owner: &TreeState, receiver: &TreeState, writes: usize, label: &str) {
    if writes == 0 {
        // Every generated step's precondition was unsatisfiable. Nothing was
        // exercised, so there is nothing to judge — and calling it green would
        // be a vacuous pass.
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
    /// `#[ignore]`d while
    /// `cross_peer_indent_then_join_stalls_the_receiver_projection` is
    /// OPEN: roughly one run in five draws that shape and reds on it, which
    /// would make every landing gate flaky on a defect this increment does not
    /// own. Measured 2026-09-02: 4 of 5 runs at 24 cases green, the fifth red
    /// on case 20. Run it with
    /// `PAIR_CASES=24 cargo nextest run --run-ignored all -E 'test(concurrent_two_writer_pair_converges)'`.
    #[test]
    #[ignore = "reds ~1 run in 5 on the OPEN receiver-projection stall; see cross_peer_indent_then_join_stalls_the_receiver_projection"]
    fn concurrent_two_writer_pair_converges(
        script in proptest::collection::vec(pair_step(), 1..7)
    ) {
        let rt = rt();
        let ref_state = wide_e2e_ref();
        let (owner, receiver, writes) = rt.block_on(run_pair_script(&ref_state, &script));
        assert_converged(&owner, &receiver, writes, &format!("script {script:?}"));
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
    let (owner, receiver, writes) = rt.block_on(run_pair_script(&ref_state, &script));
    assert!(writes == 2, "worst shape: expected 2 writes, got {writes}");
    assert_converged(&owner, &receiver, writes, "worst shape");

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
    let (owner, receiver, writes) = rt.block_on(run_pair_script(&ref_state, &script));
    assert!(
        writes == 4,
        "concurrent moves: expected 4 writes, got {writes}"
    );
    assert_converged(&owner, &receiver, writes, "concurrent moves");
}

/// **OPEN DEFECT, pinned — the cross-peer indent+join CLASS.**
///
/// An `indent` and a `join_block` applied across the two peers stall the
/// RECEIVER's Loro→SQL projection. The CRDT layer is fine — the trees merge and
/// the peers agree — but the receiver's outbound reconcile fails with
///
/// ```text
/// [TursoBackend::Actor] Commit failed, rolling back: deferred foreign key constraint failed on commit
/// [LoroSyncController] Outbound reconcile failed: BlockConsolidator sink write failed: ...
///   (ops[N]: create:block:receiver-root<-sentinel:no_parent,
///            create:block:<uuid><-block:receiver-root,
///            update:block:fe-target<-block:fe-blocked, ...)
/// ```
///
/// and is never retried. `LoroSyncController::run_loop`
/// (`crates/holon-loro/src/loro_sync_controller.rs:438-451`) is wake-driven: a
/// failed reconcile bumps `error_count`, logs, and waits for the NEXT doc
/// change. Nothing re-drives the failed batch, so ONE failure is permanent and
/// `converge_projections` never reaches its fixed point. A STALL, not a
/// livelock — the single-case log carries exactly one reconcile failure.
///
/// The loro fork emits `WARN loro_internal::state: Missing in parent's
/// children` on the same tick, which is the D70 neighbourhood surfacing as a
/// warning rather than the `tree_state.rs` panic — worth a look from the
/// fork-rebase lane.
///
/// Two shapes are pinned because the defect is a CLASS, not one interleaving:
/// this one puts the `indent` on the receiver, its sibling below puts four of
/// five writes on the owner. Both stall the same way.
///
/// `#[ignore]`d because the defect is OPEN and belongs to the Loro→SQL
/// projection, not to this increment: Inc 0 answers whether the replicate-all
/// path converges in the CRDT, and it does. Un-ignore when the projection both
/// surfaces the failure and re-drives it.
#[test]
#[ignore = "OPEN: a failed reconcile after a cross-peer indent+join is never re-driven, so the receiver projection stalls"]
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
    let (owner, receiver, writes) = rt.block_on(run_pair_script(&ref_state, &script));
    assert_eq!(writes, 2);
    assert_converged(&owner, &receiver, writes, "indent+join");
}

/// The same class, owner-heavy: the shrunk counterexample the lane's verifier
/// reached independently, with four of its five writes on the OWNER. Pinned
/// beside its sibling so a fix that only handles a receiver-side `indent`
/// cannot look complete.
#[test]
#[ignore = "OPEN: same class — a failed reconcile after a cross-peer indent+join is never re-driven"]
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
    let (owner, receiver, writes) = rt.block_on(run_pair_script(&ref_state, &script));
    assert_eq!(writes, 5);
    assert_converged(&owner, &receiver, writes, "owner-heavy indent+join");
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
