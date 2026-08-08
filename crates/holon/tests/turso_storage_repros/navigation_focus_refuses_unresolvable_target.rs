//! `navigation.focus` must REFUSE a target that names no block.
//!
//! The escape it pins (BugFunnel 2026-08-08, task #17): clicking an external
//! link in read mode dispatched `navigation.focus{block_id:"https://example.com"}`
//! — a URL as an entity id. The op accepted it, wrote a focus root nothing
//! joins to, and the main panel rendered EMPTY with zero ERROR lines. The
//! refusal is the deeper fix: it protects every caller, not just link clicks.
//!
//! Drives the REAL `NavigationProvider` through
//! `OperationProvider::execute_operation` so the `include_str!` precondition
//! SQL runs exactly as production does.
//!
//! Run with:
//!   cargo test -p holon --features test-helpers --test turso_storage_repros \
//!     navigation_focus_refuses -- --nocapture

use std::collections::HashMap;
use std::sync::Arc;

use holon::navigation::NavigationProvider;
use holon::storage::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::Region;
use holon_api::Value;
use holon_core::OperationProvider;
use tempfile::TempDir;

fn param(pairs: &[(&str, Value)]) -> holon_api::StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

/// The narrow schema `focus` touches: its two tables plus the base block table
/// its precondition reads.
async fn setup(handle: &DbHandle) {
    handle
        .execute_ddl(
            "CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT \
             NOT NULL, block_id TEXT, timestamp TEXT DEFAULT (datetime('now')), closed_at TEXT \
             NULL)",
        )
        .await
        .unwrap();
    handle
        .execute_ddl("CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER)")
        .await
        .unwrap();
    handle
        .execute_ddl("CREATE TABLE block_raw (id TEXT PRIMARY KEY, content TEXT DEFAULT '')")
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO block_raw (id, content) VALUES ('block:real-page', 'a real page')",
            vec![],
        )
        .await
        .unwrap();
}

async fn focus(provider: &NavigationProvider, block_id: &str) -> Result<(), String> {
    provider
        .execute_operation(
            &EntityName::new("navigation"),
            "focus",
            param(&[
                ("region", Value::from(Region::Main)),
                ("block_id", Value::String(block_id.to_string())),
            ]),
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn open_focus_roots(handle: &DbHandle) -> Vec<String> {
    handle
        .query(
            "SELECT block_id FROM navigation_history WHERE closed_at IS NULL AND block_id IS NOT \
             NULL",
            HashMap::new(),
        )
        .await
        .unwrap()
        .iter()
        .filter_map(|r| {
            r.get("block_id")
                .and_then(|v| v.as_string())
                .map(String::from)
        })
        .collect()
}

/// Every unresolvable target shape is refused with a message that names it, and
/// leaves the region's existing focus untouched — a refusal writes nothing.
#[tokio::test]
async fn focus_refuses_a_target_that_names_no_block() {
    let temp = TempDir::new().unwrap();
    let db = TursoBackend::open_database(&temp.path().join("nav_refuse.db")).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    setup(&handle).await;
    let provider = NavigationProvider::new(handle.clone());

    // Control: a real block focuses fine.
    focus(&provider, "block:real-page")
        .await
        .expect("focusing an existing block must succeed");
    assert_eq!(
        open_focus_roots(&handle).await,
        vec!["block:real-page".to_string()]
    );

    // The three shapes Martin's six-kind matrix blanked the panel with.
    for target in ["https://example.com", "cc-session:0f3a", "tag:rust"] {
        let err = focus(&provider, target).await.expect_err(&format!(
            "focusing '{target}' must be refused, not accepted"
        ));
        assert!(
            err.contains(target) && err.contains("not 'block'"),
            "the refusal must name the offending target and its scheme; got: {err}"
        );
    }

    assert_eq!(
        open_focus_roots(&handle).await,
        vec!["block:real-page".to_string()],
        "a refused focus must write nothing — the region keeps its prior focus root, so the \
         panel keeps rendering instead of going blank"
    );

    // NOT refused, and deliberately so: a well-formed `block:` URI for a block
    // that does not exist still blanks the panel. The existence half of this
    // precondition is ESCALATED, not implemented — boot's own
    // `seed_default_layout` focuses `block:journals` before any such row is
    // visible, so refusing on non-existence fails 13 windowed tests at their
    // first frame. This assertion pins the hole so it cannot be mistaken for
    // covered ground, and it FLIPS (expect → expect_err) the day existence is
    // enforced.
    focus(&provider, "block:00000000-0000-4000-8000-000000000000")
        .await
        .expect("KNOWN HOLE: a nonexistent block: target is still accepted (existence unchecked)");
}

/// `go_home` focuses NOTHING (`block_id` absent). The precondition must not
/// mistake "no target" for "unresolvable target".
#[tokio::test]
async fn focus_with_no_block_id_is_still_accepted() {
    let temp = TempDir::new().unwrap();
    let db = TursoBackend::open_database(&temp.path().join("nav_home.db")).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    setup(&handle).await;
    let provider = NavigationProvider::new(handle.clone());

    provider
        .execute_operation(
            &EntityName::new("navigation"),
            "focus",
            param(&[("region", Value::from(Region::Main))]),
        )
        .await
        .expect("focus with no block_id (go-home) must be accepted");
    assert!(
        open_focus_roots(&handle).await.is_empty(),
        "a go-home focus contributes no focus root"
    );
}
