//! Validation experiment H3 — LoroMap-as-set convergence under concurrent
//! add/remove across two peers.
//!
//! ## Goal
//!
//! Confirm that a `LoroMap<slug, true>` used to encode a set converges
//! correctly under common concurrent edit patterns. Set semantics on the
//! Loro side rest on this.
//!
//! ## Scenarios
//!
//! Each scenario starts with two fresh peers (A, B), applies a different
//! operation on each, syncs in both directions via export/import, then
//! asserts the resulting sets match the expected set on both peers.
//!
//! - **S1** — both peers insert the SAME key → expected: {k}
//! - **S2** — peers insert DIFFERENT keys → expected: {ka, kb}
//! - **S3** — A adds X; B removes Y (Y was in shared seed) → expected: {X}
//! - **S4** — both peers delete the SAME key (in shared seed) → expected: {}
//! - **S5** (bonus, LWW) — A adds key K; B removes K (K in shared seed)
//!   → expected: deterministic last-write-wins; both peers agree
//!
//! ## What "PASS" means
//!
//! For S1-S4, both peers must converge to the expected set exactly. For
//! S5, both peers must agree on the same outcome (whatever it is).
//!
//! Run: `cargo run --example loro_map_as_set_h3`

use std::collections::BTreeSet;

use anyhow::Result;
use loro::{ExportMode, LoroDoc, LoroValue};

const MAP_KEY: &str = "blocked_by_set";

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== H3: LoroMap-as-set convergence under concurrent edits ===\n");

    let mut all_passed = true;

    all_passed &= scenario_s1_concurrent_add_same_key()?;
    all_passed &= scenario_s2_concurrent_add_different_keys()?;
    all_passed &= scenario_s3_unrelated_concurrent_add_remove()?;
    all_passed &= scenario_s4_concurrent_delete_same_key()?;
    all_passed &= scenario_s5_concurrent_add_remove_same_key()?;

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("H3 RESULT: PASS — LoroMap-as-set converges under concurrent edits.");
        Ok(())
    } else {
        println!("H3 RESULT: FAIL — see per-scenario output above.");
        std::process::exit(1);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn fresh_doc() -> LoroDoc {
    LoroDoc::new()
}

fn set_insert(doc: &LoroDoc, key: &str) -> Result<()> {
    let map = doc.get_map(MAP_KEY);
    map.insert(key, LoroValue::from(true))?;
    doc.commit();
    Ok(())
}

fn set_delete(doc: &LoroDoc, key: &str) -> Result<()> {
    let map = doc.get_map(MAP_KEY);
    map.delete(key)?;
    doc.commit();
    Ok(())
}

/// Sync both directions: A exports to B, B exports to A.
fn sync_bidir(a: &LoroDoc, b: &LoroDoc) -> Result<()> {
    let snap_a = a
        .export(ExportMode::all_updates())
        .map_err(anyhow::Error::msg)?;
    let snap_b = b
        .export(ExportMode::all_updates())
        .map_err(anyhow::Error::msg)?;
    a.import(&snap_b)
        .map_err(|e| anyhow::anyhow!("a.import: {:?}", e))?;
    b.import(&snap_a)
        .map_err(|e| anyhow::anyhow!("b.import: {:?}", e))?;
    Ok(())
}

fn read_set(doc: &LoroDoc) -> BTreeSet<String> {
    let map = doc.get_map(MAP_KEY);
    let mut out = BTreeSet::new();
    map.for_each(|key, _value| {
        // Set semantics: presence-only; we don't inspect the value.
        out.insert(key.to_string());
    });
    out
}

fn check_eq(label: &str, got: &BTreeSet<String>, want: &BTreeSet<String>) -> bool {
    if got == want {
        println!("  [PASS] {label}: {:?}", got);
        true
    } else {
        println!(
            "  [FAIL] {label}\n    expected: {:?}\n    got:      {:?}",
            want, got
        );
        false
    }
}

fn check(label: &str, ok: bool) -> bool {
    let mark = if ok { "PASS" } else { "FAIL" };
    println!("  [{mark}] {label}");
    ok
}

fn shared_seed(a: &LoroDoc, b: &LoroDoc, keys: &[&str]) -> Result<()> {
    for k in keys {
        set_insert(a, k)?;
    }
    sync_bidir(a, b)?;
    Ok(())
}

// ── Scenarios ─────────────────────────────────────────────────────────────

fn scenario_s1_concurrent_add_same_key() -> Result<bool> {
    println!("--- S1: both peers concurrently insert the SAME key 'k' ---");
    let a = fresh_doc();
    let b = fresh_doc();
    set_insert(&a, "k")?;
    set_insert(&b, "k")?;
    sync_bidir(&a, &b)?;

    let want: BTreeSet<String> = ["k".to_string()].into_iter().collect();
    let mut ok = true;
    ok &= check_eq("A converges to {k}", &read_set(&a), &want);
    ok &= check_eq("B converges to {k}", &read_set(&b), &want);
    Ok(ok)
}

fn scenario_s2_concurrent_add_different_keys() -> Result<bool> {
    println!("\n--- S2: peers concurrently insert DIFFERENT keys ('ka' on A, 'kb' on B) ---");
    let a = fresh_doc();
    let b = fresh_doc();
    set_insert(&a, "ka")?;
    set_insert(&b, "kb")?;
    sync_bidir(&a, &b)?;

    let want: BTreeSet<String> = ["ka".to_string(), "kb".to_string()].into_iter().collect();
    let mut ok = true;
    ok &= check_eq("A converges to {ka, kb}", &read_set(&a), &want);
    ok &= check_eq("B converges to {ka, kb}", &read_set(&b), &want);
    Ok(ok)
}

fn scenario_s3_unrelated_concurrent_add_remove() -> Result<bool> {
    println!("\n--- S3: A adds 'x'; B removes 'y' (y in shared seed) ---");
    let a = fresh_doc();
    let b = fresh_doc();
    shared_seed(&a, &b, &["y"])?;
    // Sanity: both have {y} after seed.
    let seed_state = read_set(&a);
    assert_eq!(seed_state, ["y".to_string()].into_iter().collect());

    set_insert(&a, "x")?;
    set_delete(&b, "y")?;
    sync_bidir(&a, &b)?;

    let want: BTreeSet<String> = ["x".to_string()].into_iter().collect();
    let mut ok = true;
    ok &= check_eq("A converges to {x}", &read_set(&a), &want);
    ok &= check_eq("B converges to {x}", &read_set(&b), &want);
    Ok(ok)
}

fn scenario_s4_concurrent_delete_same_key() -> Result<bool> {
    println!("\n--- S4: both peers concurrently DELETE the same key 'k' (k in seed) ---");
    let a = fresh_doc();
    let b = fresh_doc();
    shared_seed(&a, &b, &["k"])?;
    set_delete(&a, "k")?;
    set_delete(&b, "k")?;
    sync_bidir(&a, &b)?;

    let want: BTreeSet<String> = BTreeSet::new();
    let mut ok = true;
    ok &= check_eq("A converges to {} (k removed)", &read_set(&a), &want);
    ok &= check_eq("B converges to {} (k removed)", &read_set(&b), &want);
    Ok(ok)
}

fn scenario_s5_concurrent_add_remove_same_key() -> Result<bool> {
    println!("\n--- S5 (bonus): A inserts 'k' AGAIN; B deletes 'k' (k in shared seed) ---");
    println!("    Last-write-wins: outcome may be either {{k}} or {{}}, but BOTH peers");
    println!("    must agree.");
    let a = fresh_doc();
    let b = fresh_doc();
    shared_seed(&a, &b, &["k"])?;

    // Both peers act concurrently on the same key.
    set_insert(&a, "k")?; // re-asserts presence
    set_delete(&b, "k")?; // removes it
    sync_bidir(&a, &b)?;

    let sa = read_set(&a);
    let sb = read_set(&b);
    println!("    A final: {:?}", sa);
    println!("    B final: {:?}", sb);

    let mut ok = true;
    ok &= check("Both peers agree on outcome", sa == sb);
    // Either outcome is acceptable for set semantics; what matters is
    // convergence + that it's deterministic across peers.
    ok &= check(
        "Outcome is one of the two valid options ({k} or {})",
        sa == BTreeSet::new() || sa == ["k".to_string()].into_iter().collect(),
    );
    Ok(ok)
}
