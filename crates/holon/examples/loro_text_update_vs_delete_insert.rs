//! Focused repro: production primary uses `text.update(new)` (myers diff,
//! minimal RGA ops), but test-peer `multi_peer::update_block` uses
//! `delete(0, old_len) + insert(0, new)` (full rewrite). When concurrent
//! edits merge, the two op patterns produce different RGA orderings —
//! which manifests as an inv-backend-blocks-match-ref SQL/Loro divergence in
//! the TUI PBT (`PROPTEST_SEED=3`).
//!
//! The PBT's reference model (`loro_merge_text` in
//! `crates/holon-integration-tests/src/pbt/state_machine.rs`) uses
//! `text.update()` for both sides, so it predicts what production primary
//! would do — but production peer (test infrastructure) uses delete+insert,
//! and the actual merge result diverges from the prediction.
//!
//! This example demonstrates the divergence. Run with:
//!     cargo run -p holon --example loro_text_update_vs_delete_insert

use loro::ExportMode;
use loro::LoroDoc;

const TEXT_KEY: &str = "content";

fn make_doc(peer_id: u64, baseline: &str) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(peer_id).unwrap();
    let text = doc.get_text(TEXT_KEY);
    text.update(baseline, Default::default()).unwrap();
    doc.commit();
    doc
}

fn snapshot(doc: &LoroDoc) -> Vec<u8> {
    doc.export(ExportMode::Snapshot).unwrap()
}

fn import_snapshot(peer_id: u64, snapshot: &[u8]) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(peer_id).unwrap();
    doc.import(snapshot).unwrap();
    doc
}

/// Apply a content update via `text.update(new, _)` — myers diff, minimal ops.
/// Mirrors production primary's `update_text_field` in
/// `crates/holon/src/api/loro_backend.rs:421`.
fn update_via_text_update(doc: &LoroDoc, new: &str) {
    let text = doc.get_text(TEXT_KEY);
    text.update(new, Default::default()).unwrap();
    doc.commit();
}

/// Apply a content update via `delete(0, old_len) + insert(0, new)` — full
/// rewrite. Mirrors the test-peer path in
/// `crates/holon/src/sync/multi_peer.rs:326` (`update_block`).
fn update_via_delete_insert(doc: &LoroDoc, new: &str) {
    let text = doc.get_text(TEXT_KEY);
    let old_len = text.len_unicode();
    if old_len > 0 {
        text.delete(0, old_len).unwrap();
    }
    text.insert(0, new).unwrap();
    doc.commit();
}

/// Merge `peer`'s delta (relative to `primary`'s vv) into `primary`.
/// Mirrors `apply_merge_from_peer` in
/// `crates/holon-integration-tests/src/pbt/sut.rs:2340`.
fn merge_into(primary: &LoroDoc, peer: &LoroDoc) {
    let primary_vv = primary.oplog_vv();
    let delta = peer.export(ExportMode::updates(&primary_vv)).unwrap();
    if !delta.is_empty() {
        primary.import(&delta).unwrap();
    }
}

fn read(doc: &LoroDoc) -> String {
    doc.get_text(TEXT_KEY).to_string()
}

/// Reproduces seed=3 inputs: baseline = "tQKsaFj" (whatever block:-940o
/// was after the bulk-add), primary edits to "wfbqU 66ZW tQKsaFj" (some
/// UI mutation), peer edits to nothing-shared so both sides race at
/// position 0. The exact strings don't matter — what matters is that
/// `text.update()` finds shared chars and `delete+insert` doesn't.
fn run_case_with_ids(
    label: &str,
    baseline: &str,
    primary_new: &str,
    peer_new: &str,
    primary_id: u64,
    peer_id: u64,
) {
    println!("=== {label}  [primary_id={primary_id:#x}, peer_id={peer_id:#x}] ===");
    println!("  baseline:    {baseline:?}");
    println!("  primary new: {primary_new:?}");
    println!("  peer new:    {peer_new:?}");

    let ancestor = make_doc(0, baseline);
    let snap = snapshot(&ancestor);

    let primary_a = import_snapshot(primary_id, &snap);
    let peer_a = import_snapshot(peer_id, &snap);
    update_via_text_update(&primary_a, primary_new);
    update_via_text_update(&peer_a, peer_new);
    merge_into(&primary_a, &peer_a);
    let result_a = read(&primary_a);

    let primary_b = import_snapshot(primary_id, &snap);
    let peer_b = import_snapshot(peer_id, &snap);
    update_via_text_update(&primary_b, primary_new);
    update_via_delete_insert(&peer_b, peer_new);
    merge_into(&primary_b, &peer_b);
    let result_b = read(&primary_b);

    let primary_c = import_snapshot(primary_id, &snap);
    let peer_c = import_snapshot(peer_id, &snap);
    update_via_delete_insert(&primary_c, primary_new);
    update_via_delete_insert(&peer_c, peer_new);
    merge_into(&primary_c, &peer_c);
    let result_c = read(&primary_c);

    println!("    A (both text.update — REF):             {result_a:?}");
    println!(
        "    B (primary update, peer del+ins — SUT): {result_b:?}{}",
        if result_a == result_b {
            ""
        } else {
            "  ← differs from REF"
        }
    );
    println!("    C (both del+ins):                       {result_c:?}");
    println!();
}

fn run_case(label: &str, baseline: &str, primary_new: &str, peer_new: &str) {
    // Reference model: primary_id=1, peer_id=2 (`loro_merge_text`).
    run_case_with_ids(label, baseline, primary_new, peer_new, 1, 2);
    // Production-like: primary_id=large random u64, peer_id=peer_idx+100.
    run_case_with_ids(
        &format!("{label} [production-like ids]"),
        baseline,
        primary_new,
        peer_new,
        0x1234_5678_90AB_CDEF,
        100,
    );
}

fn main() {
    // Case 1: PBT seed=3 observed values.
    // The actual baseline at the time of MergeFromPeer for block:-940o
    // isn't logged, but we know:
    //   - primary's content after ApplyMutation: "U 66ZW tQKsaFj" (ref model
    //     expects this from set_field("content", "..."))
    //   - peer's content after PeerEdit: random `[a-z]{4,8}` => "wfbq"
    //     (peer_edit.rs generator strategy, length 4-8 lowercase)
    //   - merge result (production):     "wfbqU 66ZW tQKsaFj"
    //   - merge result (reference):      "U 66ZW tQKsaFjwfbq"
    //
    // We don't know the exact baseline, so try a few likely shapes.
    run_case("Case 1a: baseline = empty", "", "U 66ZW tQKsaFj", "wfbq");
    run_case(
        "Case 1b: baseline = some original 4-char content",
        "abcd",
        "U 66ZW tQKsaFj",
        "wfbq",
    );
    run_case(
        "Case 1c: baseline shares prefix with primary",
        "U 66ZW",
        "U 66ZW tQKsaFj",
        "wfbq",
    );
    run_case(
        "Case 1d: baseline shares prefix with peer",
        "wfbq",
        "U 66ZW tQKsaFj",
        "wfbq",
    );

    // Case 2: Symmetric "both sides append" — the canonical RGA case.
    run_case(
        "Case 2: both sides append distinct text",
        "shared",
        "shared+primary",
        "shared+peer",
    );
}
