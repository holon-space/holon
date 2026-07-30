//! **Two-peer OFFLINE move-storm convergence gate (ADR 0028 §C4).**
//!
//! ADR 0028 §C4 names concurrent structural moves a HAZARDOUS path: production
//! has cycle *prevention* (`holon-api/src/block_mutation.rs:44` UI path +
//! `holon-loro/src/loro_backend.rs:3781` native `tree.mov` re-check) and cycle
//! *detection* (`inv-no-parent-cycles`), but **no repair path** — once two
//! offline peers commit conflicting reparents, LoroTree's convergent merge is
//! the *only* safety net that keeps every replica on one acyclic tree. Device
//! sync makes such concurrent moves routine. Martin's ruling: "test the hell
//! out of this."
//!
//! This gate drives that net directly through the SAME `tree.mov` /
//! export-import merge primitives production uses (`holon_loro::multi_peer`,
//! the peer-mesh substrate the `LoroSyncController` sits on), with **directed**
//! crossing storms (A-under-B ∥ B-under-A; deep cross-subtree cycles;
//! undo-across-a-crossing per §7) plus a randomized fuzz rung. After merge it
//! asserts the four §C4 properties: convergence (every replica identical),
//! no cycles, no orphans, sibling-order sanity.
//!
//! Scope boundary (see the module tail comment): this covers the CRDT safety
//! net itself. The full-stack keystone projection (Turso block hierarchy + org
//! render + ViewModel) after a converged move-storm is the documented next
//! increment — `PeerEditOp` has no `Move` arm today, so `ComposedSut<WideE2E>`
//! structurally cannot express a peer reparent (see the report's Inc plan).

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon::sync::multi_peer;
use holon::sync::multi_peer::TREE_NAME;
use holon_loro_testing::peer_ops;
use loro::LoroDoc;
use loro::TreeID;
use loro::TreeParentId;
use loro::UndoManager;
use proptest::prelude::*;

// ── Structural snapshots (stable-id keyed, replica-independent) ──────────────

/// `(parent_stable_id, content)` keyed by stable id — order-independent
/// structural identity of a replica's alive tree.
fn structure(doc: &LoroDoc) -> BTreeMap<String, (Option<String>, String)> {
    peer_ops::peer_alive_blocks(doc)
        .into_iter()
        .map(|b| (b.stable_id, (b.parent_stable_id, b.content)))
        .collect()
}

/// Ordered child stable-ids per parent (`None` = root), in the tree's own
/// sibling order — the "sibling-order sanity" observable.
fn sibling_order(doc: &LoroDoc) -> BTreeMap<Option<String>, Vec<String>> {
    let tree = doc.get_tree(TREE_NAME);
    let sids = |nodes: Vec<TreeID>| -> Vec<String> {
        nodes
            .into_iter()
            .filter_map(|n| peer_ops::read_node_stable_id(doc, n))
            .collect()
    };
    let mut out: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    out.insert(
        None,
        sids(tree.children(TreeParentId::Root).unwrap_or_default()),
    );
    for node in tree.get_nodes(false) {
        if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
            continue;
        }
        let Some(sid) = peer_ops::read_node_stable_id(doc, node.id) else {
            continue;
        };
        out.insert(
            Some(sid),
            sids(
                tree.children(TreeParentId::Node(node.id))
                    .unwrap_or_default(),
            ),
        );
    }
    out
}

// ── §C4 invariant assertions ────────────────────────────────────────────────

/// No orphans: every alive node's parent is itself alive (or root).
fn assert_no_orphans(doc: &LoroDoc, ctx: &str) {
    let s = structure(doc);
    let alive: BTreeSet<&String> = s.keys().collect();
    for (sid, (parent, _)) in &s {
        if let Some(p) = parent {
            assert!(
                alive.contains(p),
                "{ctx}: ORPHAN — node {sid} has non-alive parent {p}\ntree: {s:?}"
            );
        }
    }
}

/// No cycles: the parent chain from every node terminates at root without
/// revisiting a node. This is the property the merge's convergent tie-break
/// exists to preserve — prod has no repair path if it ever failed.
fn assert_acyclic(doc: &LoroDoc, ctx: &str) {
    let s = structure(doc);
    for start in s.keys() {
        let mut seen = BTreeSet::new();
        let mut cur = Some(start.clone());
        while let Some(c) = cur {
            assert!(
                seen.insert(c.clone()),
                "{ctx}: CYCLE reachable from {start} (revisited {c})\ntree: {s:?}"
            );
            cur = s.get(&c).and_then(|(p, _)| p.clone());
        }
    }
}

/// Convergence: every replica agrees on structure AND sibling order.
fn assert_converged(docs: &[(&str, &LoroDoc)], ctx: &str) {
    let (base_name, base_doc) = docs[0];
    let base_struct = structure(base_doc);
    let base_order = sibling_order(base_doc);
    for (name, doc) in &docs[1..] {
        assert_eq!(
            structure(doc),
            base_struct,
            "{ctx}: structural divergence — {name} vs {base_name}"
        );
        assert_eq!(
            sibling_order(doc),
            base_order,
            "{ctx}: sibling-order divergence — {name} vs {base_name}"
        );
    }
}

// ── Peer-mesh helpers (production `multi_peer` primitives) ───────────────────

/// Fork a peer off `primary` (shares seed history/stable-ids) and pin a
/// distinct peer id so concurrent ops carry distinct op-ids — exactly
/// `ShadowMesh::fork_peer` / `LoroSut::apply_add_peer`.
fn fork(primary: &LoroDoc, peer_id: u64) -> LoroDoc {
    let p = primary.fork();
    p.set_peer_id(peer_id).expect("set_peer_id");
    p
}

/// Reparent `sid` under `new_parent` (`None` = root) on one replica. Panics on
/// a LOCAL cyclic-move rejection: the directed setups below only ever issue
/// locally-valid moves — the cycle emerges solely from the concurrent MERGE.
fn mv(doc: &LoroDoc, sid: &str, new_parent: Option<&str>) {
    let node = peer_ops::find_node_by_stable_id(doc, sid)
        .unwrap_or_else(|| panic!("mv: node {sid} not found"));
    let parent = new_parent.map(|p| {
        peer_ops::find_node_by_stable_id(doc, p)
            .unwrap_or_else(|| panic!("mv: parent {p} not found"))
    });
    multi_peer::move_block(doc, node, parent)
        .unwrap_or_else(|e| panic!("mv {sid}->{new_parent:?} rejected locally: {e}"));
}

/// Sync every replica pairwise to a fixed point — the offline peers all
/// exchange deltas (device sync catch-up), so afterward they must converge.
fn merge_all(docs: &[&LoroDoc]) {
    for _round in 0..3 {
        for i in 0..docs.len() {
            for j in (i + 1)..docs.len() {
                multi_peer::sync_docs_direct(docs[i], docs[j]);
            }
        }
    }
}

/// Flat tree: three root blocks A, B, C.
fn seed_flat() -> LoroDoc {
    let d = multi_peer::init_doc(1);
    for id in ["A", "B", "C"] {
        peer_ops::peer_create_block(&d, None, id, id);
    }
    d
}

/// Two-level tree: roots A, B; A1 under A; B1 under B.
fn seed_deep() -> LoroDoc {
    let d = multi_peer::init_doc(1);
    peer_ops::peer_create_block(&d, None, "A", "A");
    peer_ops::peer_create_block(&d, None, "B", "B");
    peer_ops::peer_create_block(&d, Some("A"), "A1", "A1");
    peer_ops::peer_create_block(&d, Some("B"), "B1", "B1");
    d
}

// ── Directed storms ─────────────────────────────────────────────────────────

/// The canonical crossing: peer1 puts A under B while peer2 puts B under A.
/// Merge must keep exactly ONE reparent (acyclic), and all replicas agree on
/// which one.
#[test]
fn crossing_a_under_b_vs_b_under_a_converges_acyclic() {
    let primary = seed_flat();
    let p1 = fork(&primary, 100);
    let p2 = fork(&primary, 101);

    mv(&p1, "A", Some("B")); // peer1: A under B
    mv(&p2, "B", Some("A")); // peer2: B under A

    merge_all(&[&primary, &p1, &p2]);

    let ctx = "crossing";
    assert_converged(&[("primary", &primary), ("p1", &p1), ("p2", &p2)], ctx);
    assert_acyclic(&primary, ctx);
    assert_no_orphans(&primary, ctx);

    let s = structure(&primary);
    assert!(
        s.contains_key("A") && s.contains_key("B") && s.contains_key("C"),
        "{ctx}: no block may vanish — {s:?}"
    );
    let a_under_b = s["A"].0.as_deref() == Some("B");
    let b_under_a = s["B"].0.as_deref() == Some("A");
    assert!(
        a_under_b ^ b_under_a,
        "{ctx}: exactly one reparent must survive; A_parent={:?} B_parent={:?}",
        s["A"].0,
        s["B"].0
    );
}

/// Same block, different target on each peer: X moved under P on peer1 and
/// under Q on peer2. Merge must land X under exactly one of {P, Q}.
#[test]
fn same_block_different_parent_converges() {
    let primary = seed_flat(); // A, B, C at root; move C.
    let p1 = fork(&primary, 100);
    let p2 = fork(&primary, 101);

    mv(&p1, "C", Some("A")); // peer1: C under A
    mv(&p2, "C", Some("B")); // peer2: C under B

    merge_all(&[&primary, &p1, &p2]);

    let ctx = "same-block-diff-parent";
    assert_converged(&[("primary", &primary), ("p1", &p1), ("p2", &p2)], ctx);
    assert_acyclic(&primary, ctx);
    assert_no_orphans(&primary, ctx);

    let parent_of_c = structure(&primary)["C"].0.clone();
    assert!(
        matches!(parent_of_c.as_deref(), Some("A") | Some("B")),
        "{ctx}: C must resolve to exactly one target, got {parent_of_c:?}"
    );
}

/// Deep cross-subtree cycle: peer1 grafts the A subtree under B's child, peer2
/// grafts the B subtree under A's child — a two-hop cycle that only the
/// converging merge can break.
#[test]
fn deep_cross_subtree_move_storm_converges_acyclic() {
    let primary = seed_deep();
    let p1 = fork(&primary, 100);
    let p2 = fork(&primary, 101);

    mv(&p1, "A", Some("B1")); // peer1: A (carrying A1) under B1 (under B)
    mv(&p2, "B", Some("A1")); // peer2: B (carrying B1) under A1 (under A)

    merge_all(&[&primary, &p1, &p2]);

    let ctx = "deep-cross-subtree";
    assert_converged(&[("primary", &primary), ("p1", &p1), ("p2", &p2)], ctx);
    assert_acyclic(&primary, ctx);
    assert_no_orphans(&primary, ctx);
    let s = structure(&primary);
    for id in ["A", "B", "A1", "B1"] {
        assert!(s.contains_key(id), "{ctx}: {id} must survive — {s:?}");
    }
}

/// Undo-across-a-crossing (ADR 0028 §7 — Loro undo is per-doc): peer1 crosses
/// (A under B) then undoes it locally; peer2 concurrently crosses (B under A).
///
/// PRIMARY GATE — the four §C4 safety properties still hold: all replicas
/// converge to ONE acyclic, orphan-free tree with a consistent sibling order.
/// The CRDT safety net does its job even when an undo interleaves a crossing.
///
/// CANARY (characterized finding F-undo, see the report's BugFunnel candidate):
/// contrary to the naive expectation that "peer1 withdrew its move, so only
/// peer2's B-under-A survives", the merge lands **B back at root** — peer1's
/// per-doc undo of its OWN crossing (A→B, A→root) silently reverts peer2's
/// concurrent, UNRELATED reparent of B. Convergence is preserved, but a
/// concurrent structural edit is lost. This is exactly the hazard ADR §7's
/// "inverse-crossings through the H2 log" must reckon with. The assertion pins
/// the observed loro-1.11.1 outcome so a behavior change (loro upgrade or a
/// fix) trips here loudly instead of passing silently.
#[test]
fn undo_across_crossing_then_merge_converges() {
    let primary = seed_flat();
    let p1 = fork(&primary, 100);
    let p2 = fork(&primary, 101);

    // peer1: cross, then undo the cross (per-doc undo).
    let mut undo = UndoManager::new(&p1);
    mv(&p1, "A", Some("B"));
    undo.record_new_checkpoint().expect("checkpoint");
    assert!(undo.undo().expect("undo"), "undo must revert the crossing");
    p1.commit();
    // Localize: the undo really put A back at root on peer1 BEFORE any merge.
    assert_eq!(
        structure(&p1)["A"].0,
        None,
        "undo-precheck: A must return to root on p1"
    );
    assert_eq!(
        structure(&p2)["A"].0,
        None,
        "undo-precheck: p2 has not touched A"
    );

    // peer2: concurrent crossing that is NOT withdrawn.
    mv(&p2, "B", Some("A"));
    assert_eq!(
        structure(&p2)["B"].0.as_deref(),
        Some("A"),
        "undo-precheck: peer2 locally holds B under A before merge"
    );

    merge_all(&[&primary, &p1, &p2]);

    let ctx = "undo-across-crossing";
    // §C4 safety properties — the actual gate.
    assert_converged(&[("primary", &primary), ("p1", &p1), ("p2", &p2)], ctx);
    assert_acyclic(&primary, ctx);
    assert_no_orphans(&primary, ctx);

    // CANARY: pin the surprising-but-convergent outcome (finding F-undo).
    let s = structure(&primary);
    assert_eq!(
        s["A"].0, None,
        "{ctx}: A stays at root (peer1's crossing was undone) — {s:?}"
    );
    assert_eq!(
        s["B"].0, None,
        "{ctx}: CANARY F-undo — peer1's undo dropped peer2's concurrent B→A \
         reparent; B lands back at root. If this trips, loro's undo/merge \
         interaction changed — reassess ADR §7 inverse-crossings. — {s:?}"
    );
}

// ── Randomized fuzz rung ─────────────────────────────────────────────────────

const FUZZ_IDS: [&str; 4] = ["A", "B", "C", "D"];

/// Apply a script of `(block_idx, parent_idx)` reparents to one replica. A
/// `parent_idx` of `FUZZ_IDS.len()` means root. A move that Loro rejects
/// LOCALLY (would create an immediate cycle) is a no-op on this replica — the
/// exact parity of production's native `tree.mov` cyclic-move rejection, not a
/// swallowed error.
fn apply_script(doc: &LoroDoc, script: &[(usize, usize)]) {
    for &(bi, pi) in script {
        let sid = FUZZ_IDS[bi];
        let parent = FUZZ_IDS.get(pi).copied();
        if parent == Some(sid) {
            continue; // self-parent is never a legal intent
        }
        let node = peer_ops::find_node_by_stable_id(doc, sid).expect("fuzz node");
        let pnode = parent.map(|p| peer_ops::find_node_by_stable_id(doc, p).expect("fuzz parent"));
        // Err == native cyclic-move rejection (defense-in-depth); replica unchanged.
        let _rejected_locally = multi_peer::move_block(doc, node, pnode);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Fuzz the storm: two peers each apply a random reparent script offline,
    /// then merge. The four §C4 properties must hold for every generated storm.
    #[test]
    fn random_two_peer_move_storm_converges(
        script1 in prop::collection::vec((0usize..4, 0usize..5), 0..6),
        script2 in prop::collection::vec((0usize..4, 0usize..5), 0..6),
    ) {
        let primary = {
            let d = multi_peer::init_doc(1);
            for id in FUZZ_IDS {
                peer_ops::peer_create_block(&d, None, id, id);
            }
            d
        };
        let p1 = fork(&primary, 100);
        let p2 = fork(&primary, 101);

        apply_script(&p1, &script1);
        apply_script(&p2, &script2);

        merge_all(&[&primary, &p1, &p2]);

        let ctx = "fuzz";
        assert_converged(&[("primary", &primary), ("p1", &p1), ("p2", &p2)], ctx);
        assert_acyclic(&primary, ctx);
        assert_no_orphans(&primary, ctx);
    }
}
