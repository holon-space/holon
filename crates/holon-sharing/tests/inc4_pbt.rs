//! ADR 0028 Inc 4 PBT gates for the crossing log, driven against the REAL
//! [`CrossingLog`] type on the real Loro substrate (mirrors the Inc-0 two-owner
//! -device harness shape, now against the landed log rather than a spike).
//!
//! Gates:
//! 1. deterministic total order over ≥100 concurrent crossing pairs;
//! 2. the loud-rejected loser is returned as a keepable, materializable copy;
//! 3. a rejected loser NEVER enters the undo stack (reject → undo → unchanged);
//! 4. migration-journal crash-resume idempotency (+ no-op replay).
//!
//! ## Keystone obligation (Inc 2 §Step-4 caveat) — status
//! The composed keystone's transition alphabet has NO crossing transition, so
//! the C2 `inv-audience-never-over-approximates` oracle cannot yet be driven
//! end-to-end through the JSONL/ComposedSut. This file discharges the FEASIBLE
//! part tonight: a two-peer concurrent-crossing PBT at the `holon-sharing`
//! level that composes with the `move_storm_convergence`
//! offline-fork-then-merge shape (two owner devices author on the same block
//! offline, then merge).
//!
//! The remaining wiring is the EXPLICIT next increment, precisely:
//! extend `crates/holon-integration-tests/src/pbt/composed` transition alphabet
//! with `ShareSubtree{block,audience}` (widen: create-in-shared FIRST),
//! `NarrowShare{block,audience}` (narrow: delete-from-shared FIRST; a
//! wrong-order variant leaves the leak), and an epoch-barrier transition
//! (quiescent `effective == policy`), then route each through `CrossingLog` +
//! `arbitrate` so the Inc-2 oracle observes real migrations. That is a
//! ComposedSut wiring task (new `E2ETransition` variants + `apply` + ref-cap
//! updates), deferred here only because it exceeds tonight's crate scope.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_loro::share_peer_id::stable_peer_id;
use holon_sharing::BlockContent;
use holon_sharing::BlockId;
use holon_sharing::ContainerId;
use holon_sharing::Crossing;
use holon_sharing::CrossingId;
use holon_sharing::CrossingLog;
use holon_sharing::UndoError;
use holon_sharing::UnverifiedAuthority;
use holon_sharing::WitnessError;
use holon_sharing::arbitrate;
use holon_sharing::journal::JournalStep;
use holon_sharing::journal::MigrationError;
use holon_sharing::journal::MigrationJournal;
use holon_sharing::journal::MigrationOp;
use holon_sharing::types::LogEntryBody;
use holon_sharing::types::PolicyChange;
use holon_sharing::types::PolicyEdit;
use holon_sharing::types::PolicyEditId;
use holon_sharing::types::StablePeerId;
use iroh::SecretKey;
use proptest::prelude::*;

const SHARE: &str = "owner-log";

fn new_log(sk: &SecretKey) -> CrossingLog {
    CrossingLog::new(
        StablePeerId(stable_peer_id(sk, SHARE, 0)),
        Box::new(UnverifiedAuthority),
    )
}

/// Assert arbitration TOTALITY: every input crossing lands in exactly one of
/// the four disjoint outcomes (winner | loser | malformed | superseded), and
/// the union covers all `n` inputs with no id appearing twice.
fn assert_total(arb: &holon_sharing::Arbitration, n: usize) {
    assert_eq!(arb.accounted(), n, "not all inputs accounted for");
    let mut ids: Vec<&CrossingId> = Vec::new();
    ids.extend(arb.winners.iter().map(|c| &c.crossing_id));
    ids.extend(arb.losers.iter().map(|r| &r.crossing.crossing_id));
    ids.extend(arb.malformed.iter().map(|m| &m.crossing.crossing_id));
    ids.extend(arb.superseded.iter().map(|c| &c.crossing_id));
    assert_eq!(ids.len(), n, "outcome id count mismatch");
    let unique: std::collections::BTreeSet<&CrossingId> = ids.iter().copied().collect();
    assert_eq!(unique.len(), n, "an id appears in more than one outcome");
}

fn content(text: &str) -> BlockContent {
    BlockContent {
        text: text.to_string(),
        props: BTreeMap::new(),
    }
}

fn crossing(id: &str, block: &str, src: &str, tgt: &str, widens: bool, text: &str) -> Crossing {
    Crossing {
        crossing_id: CrossingId(id.to_string()),
        block: BlockId(block.to_string()),
        source: ContainerId(src.to_string()),
        target: ContainerId(tgt.to_string()),
        widens_audience: widens,
        content: content(text),
        inverse_of: None,
        supersedes: None,
    }
}

fn commit_crossing(log: &CrossingLog, c: &Crossing) {
    log.append_entry(LogEntryBody::CrossingBegin(c.clone()));
    log.append_entry(LogEntryBody::CrossingCommit {
        crossing_id: c.crossing_id.clone(),
    });
}

fn warmup_policy_edit(i: u32) -> LogEntryBody {
    LogEntryBody::PolicyEdit(PolicyEdit {
        edit_id: PolicyEditId(format!("edit-{i}")),
        container: ContainerId("c-src".into()),
        principal: format!("p{i}"),
        change: PolicyChange::Grant,
    })
}

/// Build a merged pair of logs where both owner devices authored a crossing on
/// the SAME block to DIFFERENT targets, offline, after `warmup_a`/`warmup_b`
/// unrelated policy edits (which vary the Lamport heights and so sometimes
/// force the peer-id tiebreak). Returns the two merged replicas and the two
/// authored crossings.
fn merged_divergent_pair(
    warmup_a: u32,
    warmup_b: u32,
    text_a: &str,
    text_b: &str,
) -> (CrossingLog, CrossingLog, Crossing, Crossing) {
    let sk_a = SecretKey::generate(&mut rand::rng());
    let sk_b = SecretKey::generate(&mut rand::rng());
    let a = new_log(&sk_a);
    let b = new_log(&sk_b);
    for i in 0..warmup_a {
        a.append_entry(warmup_policy_edit(i));
    }
    for i in 0..warmup_b {
        b.append_entry(warmup_policy_edit(i));
    }
    let ca = crossing("A", "shared", "c-src", "c-tgt-a", false, text_a);
    let cb = crossing("B", "shared", "c-src", "c-tgt-b", false, text_b);
    commit_crossing(&a, &ca);
    commit_crossing(&b, &cb);
    a.merge(&b);
    (a, b, ca, cb)
}

proptest! {
    /// Gate 1 + 2: both devices independently compute the SAME total order and
    /// the SAME winner/loser after merge; the loser is a keepable, materializable
    /// copy carrying the losing device's own content and intended target.
    #[test]
    fn concurrent_divergent_crossings_arbitrate_deterministically(
        warmup_a in 0u32..6,
        warmup_b in 0u32..6,
        text_a in "[a-z]{1,6}",
        text_b in "[a-z]{1,6}",
    ) {
        let (a, b, _ca, _cb) = merged_divergent_pair(warmup_a, warmup_b, &text_a, &text_b);

        // Both replicas converged to the identical, totally-ordered entry list.
        prop_assert_eq!(a.entries(), b.entries());

        let arb_a = arbitrate(&a.entries());
        let arb_b = arbitrate(&b.entries());
        prop_assert_eq!(&arb_a, &arb_b);

        prop_assert_eq!(arb_a.winners.len(), 1, "exactly one winner per contended block");
        prop_assert_eq!(arb_a.losers.len(), 1, "exactly one loud-rejected loser");

        let loser = &arb_a.losers[0];
        // Winner's begin key strictly beats the loser (max wins).
        prop_assert!(loser.lost_to > loser_begin_key(&a.entries(), &loser.crossing.crossing_id));
        // Keepable copy is a real value carrying the loser's own content/target.
        let expected_text = if loser.crossing.crossing_id.0 == "A" { &text_a } else { &text_b };
        prop_assert_eq!(&loser.keepable_copy.content.text, expected_text);
        prop_assert_eq!(&loser.keepable_copy.intended_target, &loser.crossing.target);
        prop_assert_eq!(&loser.keepable_copy.block, &loser.crossing.block);
        prop_assert!(!loser.reason.is_empty(), "rejection is loud");
    }

    /// Gate 3: a rejected loser NEVER enters the undo stack. reject → undo →
    /// state unchanged; the winner, by contrast, IS undoable.
    #[test]
    fn rejected_loser_never_enters_undo_stack(
        warmup_a in 0u32..6,
        warmup_b in 0u32..6,
    ) {
        let (a, _b, _ca, _cb) = merged_divergent_pair(warmup_a, warmup_b, "xx", "yy");
        let arb = arbitrate(&a.entries());
        let loser_id = arb.losers[0].crossing.crossing_id.clone();
        let winner_id = arb.winners[0].crossing_id.clone();

        let before = a.entries();
        let before_len = before.len();
        let res = a.undo_crossing(&loser_id);
        prop_assert!(
            matches!(res, Err(UndoError::WasRejectedLoser(_))),
            "undo of a rejected loser must be refused loudly, got {res:?}"
        );
        // State unchanged: no inverse appended.
        prop_assert_eq!(a.entries(), before);

        // The winner is undoable (appends an inverse crossing).
        prop_assert!(a.undo_crossing(&winner_id).is_ok());
        prop_assert!(a.entries().len() > before_len);
    }

    /// Gate 4: the migration journal is idempotent + resumable. A crash at any
    /// step boundary, followed by a serde round-trip (persistence) and resume,
    /// reaches the SAME world as a crash-free run; a replay of a complete journal
    /// is a no-op.
    #[test]
    fn journal_crash_resume_is_idempotent(
        widens in any::<bool>(),
        crash_at in 0usize..3,
    ) {
        let c = crossing("X", "blk", "c-src", "c-tgt", widens, "hi");
        let plan = MigrationJournal::plan(&c);

        // Crash-free reference run.
        let mut world_full: BTreeSet<(ContainerId, BlockId)> =
            BTreeSet::from([(c.source.clone(), c.block.clone())]);
        let mut j_full = plan.clone();
        j_full
            .resume(|s| { apply_step(s, &c, &mut world_full); Ok(()) })
            .expect("crash-free run completes");
        prop_assert!(j_full.is_complete());

        // Crash run: fail exactly at step index `crash_at` (before it mutates).
        let mut world: BTreeSet<(ContainerId, BlockId)> =
            BTreeSet::from([(c.source.clone(), c.block.clone())]);
        let mut j = plan.clone();
        let mut idx = 0usize;
        let _ = j.resume(|s| {
            if idx == crash_at {
                return Err(MigrationError {
                    crossing_id: s.crossing_id.0.clone(),
                    op: s.op,
                    reason: "simulated crash".into(),
                });
            }
            apply_step(s, &c, &mut world);
            idx += 1;
            Ok(())
        });

        // Persist across the crash (serde round-trip) then resume.
        let persisted = serde_json::to_string(&j).unwrap();
        let mut reloaded: MigrationJournal = serde_json::from_str(&persisted).unwrap();
        reloaded
            .resume(|s| { apply_step(s, &c, &mut world); Ok(()) })
            .expect("resume completes");
        prop_assert!(reloaded.is_complete());
        prop_assert_eq!(&world, &world_full, "crash-resume reaches the crash-free world");

        // Replay of a complete journal is a no-op (Done steps are skipped).
        let snapshot = world.clone();
        reloaded
            .resume(|_| -> Result<(), MigrationError> {
                panic!("no step should re-run on a complete journal")
            })
            .expect("no-op replay");
        prop_assert_eq!(&world, &snapshot);
    }
}

fn apply_step(step: &JournalStep, c: &Crossing, world: &mut BTreeSet<(ContainerId, BlockId)>) {
    match step.op {
        MigrationOp::CreateInTarget => {
            world.insert((c.target.clone(), c.block.clone()));
        }
        MigrationOp::DeleteFromSource => {
            world.remove(&(c.source.clone(), c.block.clone()));
        }
    }
}

/// The begin key of the crossing with `id`, read straight from the merged log.
fn loser_begin_key(
    entries: &[holon_sharing::LogEntry],
    id: &CrossingId,
) -> holon_sharing::CrossingKey {
    entries
        .iter()
        .find_map(|e| match &e.body {
            LogEntryBody::CrossingBegin(c) if &c.crossing_id == id => Some(e.key),
            _ => None,
        })
        .expect("loser has a begin entry")
}

/// RED-first counterexample (verifier): a wrong-block supersedes witness on
/// block1's crossing R must NOT silently drop block2's crossing Q. Q must
/// remain accounted (winner of its own block); R must be rejected as malformed.
#[test]
fn wrong_block_witness_must_not_drop_another_blocks_crossing() {
    let sk = SecretKey::generate(&mut rand::rng());
    let log = new_log(&sk);
    let p = crossing("P", "block1", "c-src", "c-x", false, "p");
    let q = crossing("Q", "block2", "c-src", "c-y", false, "q");
    let mut r = crossing("R", "block1", "c-x", "c-z", false, "r");
    r.supersedes = Some(CrossingId("Q".into())); // WRONG block (Q is block2).
    // Raw appends simulate malformed entries arriving via merge from a peer
    // that did not validate at its source.
    log.append_entry(LogEntryBody::CrossingBegin(p));
    log.append_entry(LogEntryBody::CrossingBegin(q));
    log.append_entry(LogEntryBody::CrossingBegin(r));

    let arb = arbitrate(&log.entries());
    let q_id = CrossingId("Q".into());
    let q_accounted = arb.winners.iter().any(|c| c.crossing_id == q_id)
        || arb.losers.iter().any(|x| x.crossing.crossing_id == q_id)
        || arb.malformed.iter().any(|x| x.crossing.crossing_id == q_id)
        || arb.superseded.iter().any(|c| c.crossing_id == q_id);
    assert!(
        q_accounted,
        "Q vanished from arbitration (silent data loss)"
    );
    assert!(arb.is_winner(&q_id), "Q must win its own uncontended block");
    assert!(
        arb.is_malformed(&CrossingId("R".into())),
        "R (wrong-block witness) must be rejected malformed"
    );
    assert_total(&arb, 3);
}

/// A legitimate sequential re-move (author observed the prior crossing and set
/// `supersedes`) must NOT be mistaken for a concurrent divergent crossing: only
/// the latest crossing is live, and there is no loser.
#[test]
fn sequential_re_move_is_not_a_rejected_loser() {
    let sk = SecretKey::generate(&mut rand::rng());
    let log = new_log(&sk);
    let first = crossing("m1", "blk", "c-src", "c-x", false, "hi");
    commit_crossing(&log, &first);
    // Second move X->Y, authored AFTER observing the first (supersedes it).
    let mut second = crossing("m2", "blk", "c-x", "c-y", false, "hi");
    second.supersedes = Some(first.crossing_id.clone());
    commit_crossing(&log, &second);

    let arb = arbitrate(&log.entries());
    assert_eq!(arb.winners.len(), 1, "only the latest crossing is live");
    assert_eq!(arb.winners[0].crossing_id.0, "m2");
    assert!(arb.losers.is_empty(), "a sequential re-move is not a loser");
}

/// Explicit ≥100-pair determinism sweep (mirrors the Inc-0 note's 100
/// randomized concurrent pairs), independent of proptest case counts.
#[test]
fn one_hundred_concurrent_pairs_are_deterministic() {
    for n in 0..100u32 {
        let (a, b, _ca, _cb) = merged_divergent_pair(n % 4, (n / 4) % 4, "aa", "bb");
        assert_eq!(a.entries(), b.entries(), "pair {n}: replicas diverged");
        let arb_a = arbitrate(&a.entries());
        let arb_b = arbitrate(&b.entries());
        assert_eq!(arb_a, arb_b, "pair {n}: arbitration diverged");
        assert_eq!(arb_a.winners.len(), 1);
        assert_eq!(arb_a.losers.len(), 1);
        assert_total(&arb_a, 2);
    }
}

/// Deterministically build a mixed crossing set over 3 blocks with every
/// witness shape: `None`, honest same-block, wrong-block, and unknown-id.
fn build_mixed(specs: &[(u8, u8)]) -> Vec<Crossing> {
    let blocks = ["b1", "b2", "b3"];
    let mut out: Vec<Crossing> = Vec::new();
    for (i, (b_idx, kind)) in specs.iter().enumerate() {
        let block = blocks[(*b_idx as usize) % 3];
        let id = format!("c{i}");
        let mut c = crossing(&id, block, "src", &format!("t{i}"), i % 2 == 0, "x");
        if i > 0 {
            match kind % 4 {
                0 => {} // None
                1 => {
                    // honest same-block: latest earlier crossing on this block.
                    if let Some(prev) = out.iter().rev().find(|p| p.block.0 == block) {
                        c.supersedes = Some(prev.crossing_id.clone());
                    }
                }
                2 => {
                    // wrong-block: latest earlier crossing on a DIFFERENT block.
                    if let Some(prev) = out.iter().rev().find(|p| p.block.0 != block) {
                        c.supersedes = Some(prev.crossing_id.clone());
                    }
                }
                _ => c.supersedes = Some(CrossingId("ghost".into())), // unknown
            }
        }
        out.push(c);
    }
    out
}

proptest! {
    /// TOTALITY + determinism over MIXED scenarios (concurrent contention,
    /// honest sequential chains, wrong-block witnesses, unknown ids). One device
    /// authors the whole set, merges to a second; both arbitrate to the SAME
    /// outcome and EVERY input is accounted for (no silent drop). The totality
    /// assertion inside `arbitrate` would panic on any drop; `assert_total`
    /// re-checks disjoint coverage here.
    #[test]
    fn arbitration_is_total_and_deterministic_over_mixed(
        specs in prop::collection::vec((0u8..3, 0u8..4), 3..12),
    ) {
        let crossings = build_mixed(&specs);
        let n = crossings.len();
        let a = new_log(&SecretKey::generate(&mut rand::rng()));
        let b = new_log(&SecretKey::generate(&mut rand::rng()));
        for c in &crossings {
            // Raw appends: authored on A, replicated to B via merge, so both see
            // identical stamped entries (real cross-device determinism).
            a.append_entry(LogEntryBody::CrossingBegin(c.clone()));
        }
        a.merge(&b);

        let arb_a = arbitrate(&a.entries());
        let arb_b = arbitrate(&b.entries());
        prop_assert_eq!(&arb_a, &arb_b, "arbitration diverged across replicas");
        // TOTALITY: no crossing vanished.
        prop_assert_eq!(arb_a.accounted(), n);
        // Every malformed carries a keepable copy.
        for m in &arb_a.malformed {
            prop_assert!(!m.reason.is_empty());
            prop_assert_eq!(&m.keepable_copy.block, &m.crossing.block);
        }
        // Every wrong-block / unknown witness IS rejected malformed (never honored).
        for c in &crossings {
            if let Some(sid) = &c.supersedes {
                let target = crossings.iter().find(|t| &t.crossing_id == sid);
                let malformed_expected = match target {
                    None => true,                       // unknown
                    Some(t) => t.block != c.block,      // wrong block
                };
                if malformed_expected {
                    prop_assert!(
                        arb_a.is_malformed(&c.crossing_id),
                        "crossing {} with bad witness was not rejected malformed",
                        c.crossing_id.0
                    );
                }
            }
        }
    }
}

/// A supersedes CYCLE must be rejected malformed (both members), never causing
/// silent total loss.
#[test]
fn supersedes_cycle_is_rejected_malformed_not_dropped() {
    let log = new_log(&SecretKey::generate(&mut rand::rng()));
    let mut a = crossing("A", "blk", "src", "x", false, "aa");
    let mut b = crossing("B", "blk", "src", "y", false, "bb");
    a.supersedes = Some(CrossingId("B".into()));
    b.supersedes = Some(CrossingId("A".into()));
    log.append_entry(LogEntryBody::CrossingBegin(a));
    log.append_entry(LogEntryBody::CrossingBegin(b));

    let arb = arbitrate(&log.entries());
    assert!(arb.is_malformed(&CrossingId("A".into())));
    assert!(arb.is_malformed(&CrossingId("B".into())));
    assert!(
        arb.winners.is_empty(),
        "no winner when the whole block is a cycle"
    );
    assert!(
        arb.malformed
            .iter()
            .all(|m| !m.keepable_copy.content.text.is_empty())
    );
    assert_total(&arb, 2);
}

/// An unknown-id witness is rejected malformed (with a keepable copy), never
/// silently dropped.
#[test]
fn unknown_supersedes_is_rejected_malformed() {
    let log = new_log(&SecretKey::generate(&mut rand::rng()));
    let mut c = crossing("C", "blk", "src", "x", false, "cc");
    c.supersedes = Some(CrossingId("nope".into()));
    log.append_entry(LogEntryBody::CrossingBegin(c));

    let arb = arbitrate(&log.entries());
    assert!(arb.is_malformed(&CrossingId("C".into())));
    assert!(arb.winners.is_empty());
    assert_total(&arb, 1);
}

/// SOURCE-side guard: `append_crossing` rejects a wrong-block witness loudly
/// and never lets it into the log.
#[test]
fn append_crossing_rejects_wrong_block_at_source() {
    let log = new_log(&SecretKey::generate(&mut rand::rng()));
    let q = crossing("Q", "block2", "src", "y", false, "q");
    log.append_crossing(q).unwrap();
    let mut r = crossing("R", "block1", "src", "z", false, "r");
    r.supersedes = Some(CrossingId("Q".into())); // wrong block
    let err = log.append_crossing(r).unwrap_err();
    assert!(
        matches!(err, WitnessError::WrongBlock { .. }),
        "got {err:?}"
    );
    // R never entered the log.
    let has_r = log
        .entries()
        .iter()
        .any(|e| matches!(&e.body, LogEntryBody::CrossingBegin(c) if c.crossing_id.0 == "R"));
    assert!(!has_r, "malformed R must not be persisted at the source");
}

/// SOURCE-side guard: `append_crossing` rejects an unknown witness loudly.
#[test]
fn append_crossing_rejects_unknown_at_source() {
    let log = new_log(&SecretKey::generate(&mut rand::rng()));
    let mut c = crossing("C", "blk", "src", "x", false, "c");
    c.supersedes = Some(CrossingId("ghost".into()));
    let err = log.append_crossing(c).unwrap_err();
    assert!(
        matches!(err, WitnessError::UnknownTarget { .. }),
        "got {err:?}"
    );
}

/// SOURCE-side guard accepts a valid same-block witness; arbitration then
/// treats the predecessor as superseded history and the successor as the
/// winner.
#[test]
fn append_crossing_accepts_valid_same_block_witness() {
    let log = new_log(&SecretKey::generate(&mut rand::rng()));
    let first = crossing("m1", "blk", "src", "x", false, "hi");
    log.append_crossing(first).unwrap();
    let mut second = crossing("m2", "blk", "x", "y", false, "hi");
    second.supersedes = Some(CrossingId("m1".into()));
    log.append_crossing(second).unwrap();

    let arb = arbitrate(&log.entries());
    assert!(arb.is_winner(&CrossingId("m2".into())));
    assert!(arb.is_superseded(&CrossingId("m1".into())));
    assert_total(&arb, 2);
}
