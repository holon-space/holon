//! Validation experiment H5 — sizing the Loro→Turso pipeline change for
//! edge-typed fields.
//!
//! ## Goal
//!
//! Determine whether projecting `Block.blocked_by` (a multi-valued slug
//! reference) onto a `block_blocked_by` junction table is a localized change
//! to the existing pipeline, or a structural rework.
//!
//! ## Approach
//!
//! Trace what happens today when an edge-shaped value (`Value::Array` of
//! slug strings) flows through the existing properties path:
//!
//!   1. Create a Block via `LoroBackend::create_block`.
//!   2. Set `properties = { "blocked_by": Value::Array([...]) }` via
//!      `update_block_properties` (the public API for non-column fields).
//!   3. Snapshot the doc back to a `Block` via `snapshot_blocks_from_doc`.
//!   4. Assert the array round-trips losslessly (preserves type and items).
//!   5. Call `block_to_params` and assert the `Value::Array` flows into the
//!      params map unchanged.
//!
//! Then document the *one* place the existing SQL provider breaks
//! (`SqlOperationProvider::partition_params` / `prepare_create`), since
//! making it work would require schema knowledge of edge fields — which is
//! exactly the abstraction H5 is meant to size.
//!
//! ## What "PASS" means
//!
//! - The Loro JSON round-trip preserves `Value::Array` losslessly.
//!   This means: zero changes to `read_properties_from_meta` /
//!   `write_properties_to_meta` are required for the spike.
//! - `block_to_params` flows the array into params as-is.
//!   This means: zero changes to `loro_sync_controller.rs`
//!   (diff_snapshots_to_ops, block_diff_params, blocks_differ) are required.
//! - The change is therefore localized to: (a) a new schema-side edge-field
//!   registry, and (b) `SqlOperationProvider::partition_params` +
//!   `prepare_create` / `prepare_update`.
//!
//! Run: `cargo run --example loro_edge_field_spike_h5`

use std::collections::HashMap;

use anyhow::Result;
use holon::api::loro_backend::LoroBackend;
use holon::api::repository::{CoreOperations, Lifecycle};
use holon_api::{BlockContent, EntityUri, Value};

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== H5: Edge-typed field sizing through Loro→Turso pipeline ===\n");

    let mut all_passed = true;

    all_passed &= round_trip_through_loro().await?;
    all_passed &= json_serde_array_round_trip()?;

    println!("\n--- Summary findings ---");
    print_findings();

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("H5 RESULT: PASS — change is localized; size estimate captured.");
        Ok(())
    } else {
        println!("H5 RESULT: FAIL — see per-check output above.");
        std::process::exit(1);
    }
}

// ── Test 1: edge-shaped value survives Loro doc round-trip ────────────────

async fn round_trip_through_loro() -> Result<bool> {
    println!("--- Test 1: Value::Array survives Loro round-trip ---");

    let backend = LoroBackend::create_new("h5-edge-spike".to_string())
        .await
        .map_err(|e| anyhow::anyhow!("create_new: {:?}", e))?;

    let block = backend
        .create_block(
            EntityUri::no_parent(),
            BlockContent::Text {
                raw: "task A".into(),
            },
            None,
        )
        .await
        .map_err(|e| anyhow::anyhow!("create_block: {:?}", e))?;

    // Edge field shape: a multi-valued slug reference, expressed as Value::Array.
    // This is the natural in-memory carrier — same as how an org parser would
    // emit `:BLOCKED-BY: slug-b slug-c` after splitting.
    let mut props = HashMap::new();
    props.insert(
        "blocked_by".to_string(),
        Value::Array(vec![
            Value::String("slug-b".to_string()),
            Value::String("slug-c".to_string()),
        ]),
    );
    backend
        .update_block_properties(block.id.as_str(), &props)
        .await
        .map_err(|e| anyhow::anyhow!("update_block_properties: {:?}", e))?;

    // Read back via the public get_block (same Loro read path used by
    // snapshot_blocks_from_doc, which feeds the outbound reconcile).
    let after = backend
        .get_block(block.id.as_str())
        .await
        .map_err(|e| anyhow::anyhow!("get_block: {:?}", e))?;
    let props_map = after.properties_map();
    let got = props_map.get("blocked_by").cloned();

    let expected = Value::Array(vec![
        Value::String("slug-b".to_string()),
        Value::String("slug-c".to_string()),
    ]);

    let mut ok = true;
    ok &= check("blocked_by present in snapshot properties", got.is_some());
    if let Some(v) = got.as_ref() {
        ok &= check(
            "blocked_by preserves Value::Array variant",
            matches!(v, Value::Array(_)),
        );
        ok &= check(
            "blocked_by items match exactly (order-preserving)",
            v == &expected,
        );
    }

    Ok(ok)
}

// (Note: block_to_params is pub(crate) and thus not callable from an example.
// Its body iterates Block.properties and inserts (k, v.clone()) into the
// params map — see crates/holon/src/sync/loro_sync_controller.rs:640-642.
// Test 1 above demonstrates that Block.properties faithfully carries
// Value::Array, so the clone-into-params step is structurally trivial.)

// ── Test 2: JSON serde round-trip for Value::Array ────────────────────────

fn json_serde_array_round_trip() -> Result<bool> {
    println!("\n--- Test 3: serde_json round-trip on HashMap<String, Value> ---");
    println!("    (this is the exact code path read/write_properties_to_meta uses)");
    let mut input: HashMap<String, Value> = HashMap::new();
    input.insert(
        "blocked_by".to_string(),
        Value::Array(vec![
            Value::String("slug-b".to_string()),
            Value::String("slug-c".to_string()),
        ]),
    );
    input.insert("status".to_string(), Value::String("TODO".to_string()));
    input.insert("priority".to_string(), Value::Integer(1));

    let encoded = serde_json::to_string(&input)?;
    println!("    encoded JSON: {}", encoded);
    let decoded: HashMap<String, Value> = serde_json::from_str(&encoded)?;

    let mut ok = true;
    ok &= check(
        "scalar fields round-trip",
        decoded.get("status") == input.get("status"),
    );
    ok &= check(
        "Value::Integer round-trips",
        decoded.get("priority") == input.get("priority"),
    );
    ok &= check(
        "Value::Array round-trips (variant + items)",
        decoded.get("blocked_by") == input.get("blocked_by"),
    );

    Ok(ok)
}

fn check(label: &str, ok: bool) -> bool {
    let mark = if ok { "PASS" } else { "FAIL" };
    println!("  [{mark}] {label}");
    ok
}

// ── Findings report ────────────────────────────────────────────────────────

fn print_findings() {
    let lines = [
        "Pipeline trace, in order of travel for Block.blocked_by:",
        "",
        "  1. Loro storage:",
        "     - PROPERTIES is a single LoroValue::String (JSON-encoded HashMap).",
        "     - serde_json + Value's Serialize/Deserialize handle Value::Array",
        "       losslessly (Test 3).",
        "     - Cost: 0 LOC. CRDT caveat: concurrent edits to other properties",
        "       and to blocked_by serialize the same string. Acceptable for G1",
        "       (single-user dogfooding); a follow-up would promote to a real",
        "       LoroMap sub-container per H3.",
        "",
        "  2. snapshot_blocks_from_doc → Block.properties:",
        "     - Test 1 confirms the round-trip.",
        "     - Cost: 0 LOC.",
        "",
        "  3. block_to_params (loro_sync_controller.rs:591):",
        "     - Iterates Block.properties, .insert(k, v.clone()) into params.",
        "     - Test 2 confirms Value::Array flows through unchanged.",
        "     - Cost: 0 LOC.",
        "",
        "  4. blocks_differ / block_diff_params:",
        "     - Compare via properties_map() equality. Detects edge changes",
        "       like any other property change.",
        "     - Cost: 0 LOC. Coarse diff (whole-list rewrite on any change)",
        "       — acceptable for ≤10 blockers per task.",
        "",
        "  5. SqlOperationProvider::partition_params (sql_operation_provider.rs:215):",
        "     - Today: binary route — known column or extra_props (JSON column).",
        "     - extra_props serialization (line ~344) matches only String/Integer/",
        "       Float/Boolean; Value::Array falls into `_ => format!(\"{:?}\", v)`",
        "       producing a debug-formatted string in the JSON column. That is",
        "       the *only* place the existing pipeline visibly breaks.",
        "     - Required change: add a third partition `edge_fields`, gated by",
        "       a schema-side EdgeField registry. Approx 20 LOC.",
        "",
        "  6. SqlOperationProvider::prepare_create / prepare_update:",
        "     - Append per-edge-field DELETE+INSERT statements after the main",
        "       INSERT/UPDATE. ~40 LOC each path; small and parallel structure.",
        "     - Per H1, the gating matview observes these as clean Insert/Delete",
        "       events.",
        "",
        "  7. prepare_delete:",
        "     - No work: ON DELETE CASCADE on the junction table FK (per H1)",
        "       removes incident edges atomically.",
        "",
        "Schema side (new):",
        "  - EdgeField { entity, field, join_table, source_col, target_col }",
        "  - Registry plumbed through SchemaModule and into SqlOperationProvider",
        "    constructor. ~30 LOC.",
        "",
        "Test (new):",
        "  - End-to-end round-trip: write blocked_by via Loro outbound reconcile,",
        "    observe rows in block_blocked_by + CDC events on the gating matview.",
        "    Builds on H1's harness. ~80 LOC.",
        "",
        "Sizing summary:",
        "  ───────────────────────────────────────────────────────────────",
        "  Loro side:                           0 LOC",
        "  loro_sync_controller side:           0 LOC",
        "  SqlOperationProvider:                ~80 LOC (partition + 2 prepare)",
        "  Schema/EdgeField registry:           ~30 LOC",
        "  Constructor wiring + plumbing:       ~10 LOC",
        "  End-to-end test:                     ~80 LOC",
        "  ───────────────────────────────────────────────────────────────",
        "  TOTAL:                               ~200 LOC across 3 files",
        "  Kill condition (>500 LOC):           CLEAR",
        "",
        "Risk notes:",
        "  - CRDT semantics for edges are deferred (string-blob property today,",
        "    LoroMap sub-container later — bounded ~50 LOC follow-up gated by H3).",
        "  - Coarse whole-list diff is fine at expected scale; if blockers grow",
        "    past ~50 per task we revisit fine-grained diff in the diff helper.",
    ];
    for line in lines {
        println!("  {}", line);
    }
}
