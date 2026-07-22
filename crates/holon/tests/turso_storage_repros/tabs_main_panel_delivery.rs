//! Vault-scale DELIVERY guard for the reconciled cursor-filtered main panel.
//!
//! Background: the fresh-seed main panel used to run an UNGUARDED recursive CTE
//! (no depth cap, no visited-path guard, plus a `LEFT JOIN block_tags`
//! page-stop inside the recursive term). At ~66-file vault scale that shape
//! never delivered — the Turso IVM matview creation hung 60-90s and the watch
//! expired. Main replaced it with the compiled-GQL anchored-varlen form (depth
//! cap + visited guard). Tabs re-introduces a hand-written CTE for the main
//! panel so the recursion seed can be scoped to the ACTIVE tab
//! (`navigation_cursor` equi-join) — GQL's seed-anchor optimization only scopes
//! to `focus_roots`, so a GQL form would over-materialize every open tab's
//! subtree.
//!
//! This test pins that the reconciled hand-written CTE keeps the guarded shape:
//! it MUST create the matview and deliver first rows within a couple of seconds
//! over a synthetic ~70-page vault (~1000 blocks), and it MUST contain exactly
//! the active tab's subtree — never the other open tabs.
//!
//! Run with:
//!   cargo test -p holon --features test-helpers --test turso_storage_repros \
//!     tabs_main_panel_delivery -- --nocapture

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use holon::storage::turso::TursoBackend;
use tempfile::TempDir;

fn ids(rows: &[holon_api::StorageEntity], col: &str) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| r.get(col).and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

/// The reconciled main-panel src::0 body, inlined as a matview (ORDER BY
/// omitted — Turso IVM rejects column-reference ORDER BY in matview DDL and the
/// production path strips it via `strip_order_by`). Kept byte-faithful to the
/// guarded CTE shipped in `assets/default/index.org`.
const MAIN_PANEL_CTE: &str = "CREATE MATERIALIZED VIEW main_panel AS \
     WITH RECURSIVE focus_descendants AS ( \
       SELECT b.id AS node_id, b.id AS source_id, 0 AS depth, CAST(b.id AS TEXT) AS visited \
       FROM block b \
       JOIN focus_roots fr ON b.id = fr.root_id \
       UNION ALL \
       SELECT child.id, focus_descendants.source_id, focus_descendants.depth + 1, \
              focus_descendants.visited || ',' || CAST(child.id AS TEXT) \
       FROM focus_descendants \
       JOIN block child ON child.parent_id = focus_descendants.node_id \
       WHERE focus_descendants.depth < 20 \
         AND ',' || focus_descendants.visited || ',' NOT LIKE '%,' || CAST(child.id AS TEXT) || ',%' \
     ) \
     SELECT d.* \
     FROM focus_roots fr \
     JOIN block root ON root.id = fr.root_id \
     JOIN focus_descendants ON focus_descendants.source_id = root.id \
     JOIN block d ON d.id = focus_descendants.node_id \
     JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id \
     WHERE fr.region = 'main'";

#[tokio::test]
async fn cursor_filtered_main_panel_delivers_at_vault_scale() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("delivery.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(4096);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

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
        .execute_ddl(
            "CREATE TABLE block (id TEXT PRIMARY KEY, parent_id TEXT DEFAULT '', sort_key TEXT NOT \
             NULL DEFAULT 'A0', content TEXT NOT NULL DEFAULT '')",
        )
        .await
        .unwrap();
    // Real `focus_roots` matview (multi-tab capable) — main_panel reads FROM it,
    // exactly as production (matview-on-matview chain, mirrors
    // tabs_cursor_filtered_panel + matview_focus_roots.sql).
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW focus_roots AS SELECT region, block_id AS root_id, timestamp \
             AS added_ts, id AS history_id FROM navigation_history WHERE closed_at IS NULL AND \
             block_id IS NOT NULL",
        )
        .await
        .unwrap();

    // Synthetic ~70-page vault: 70 pages (doc_0..doc_69); each a small tree of
    // 15 blocks. The active page (doc_0) additionally carries a DEEP chain of 30
    // to exercise the depth cap (< 20) and the visited-path guard.
    let mut rows: Vec<String> = Vec::new();
    let mut expected_active: BTreeSet<String> = BTreeSet::new();
    for p in 0..70u32 {
        let root = format!("doc_{p}");
        rows.push(format!("('{root}', '', 'A0', 'Page {p}')"));
        for c in 0..14u32 {
            let parent = if c < 4 {
                root.clone()
            } else {
                format!("doc_{p}_c{}", c % 4)
            };
            let id = format!("doc_{p}_c{c}");
            rows.push(format!("('{id}', '{parent}', 'A{c}', 'blk {p}.{c}')"));
        }
    }
    let mut prev = "doc_0".to_string();
    for d in 0..30u32 {
        let id = format!("doc_0_deep{d}");
        rows.push(format!("('{id}', '{prev}', 'Z{d}', 'deep {d}')"));
        prev = id;
    }
    expected_active.insert("doc_0".to_string());
    for c in 0..14u32 {
        expected_active.insert(format!("doc_0_c{c}"));
    }
    // Deep chain: deep{d} sits at _depth = d+1; the recursive step admits a
    // child while the PARENT's _depth < 20, so deep0..deep19 survive (deep19 at
    // _depth 20), deep20.. are truncated.
    for d in 0..20u32 {
        expected_active.insert(format!("doc_0_deep{d}"));
    }

    for chunk in rows.chunks(200) {
        let sql = format!(
            "INSERT INTO block (id, parent_id, sort_key, content) VALUES {}",
            chunk.join(", ")
        );
        handle.execute(&sql, vec![]).await.unwrap();
    }

    // Open FIVE tabs (doc_0..doc_4) — all live in focus_roots simultaneously.
    for (hid, p) in (1..=5u32).zip(0..5u32) {
        handle
            .execute(
                &format!(
                    "INSERT INTO navigation_history (id, region, block_id) VALUES ({hid}, 'main', \
                     'doc_{p}')"
                ),
                vec![],
            )
            .await
            .unwrap();
    }
    // Active tab = doc_0 (history_id 1).
    handle
        .execute(
            "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
            vec![],
        )
        .await
        .unwrap();

    // DELIVERY: create the matview + read first rows, timed.
    let t0 = Instant::now();
    handle
        .execute_ddl(MAIN_PANEL_CTE)
        .await
        .expect("main_panel matview must CREATE (guarded CTE — no 60-90s IVM hang)");
    let panel = handle
        .query("SELECT id FROM main_panel", HashMap::new())
        .await
        .expect("main_panel must deliver rows");
    let elapsed = t0.elapsed();
    println!(
        "[delivery] create+first-read = {:?} ({} rows)",
        elapsed,
        panel.len()
    );

    assert!(
        elapsed < Duration::from_secs(5),
        "cursor-filtered main panel must deliver within a couple of seconds at ~70-page vault \
         scale; took {elapsed:?} (unguarded landmine form hung 60-90s)"
    );

    let got = ids(&panel, "id");
    assert_eq!(
        got, expected_active,
        "panel must contain exactly doc_0's depth-capped subtree — not the other open tabs \
         (over-materialization) and not beyond depth 20 (cap/visited guard)"
    );
    assert!(
        !got.iter().any(|id| id.starts_with("doc_1")
            || id.starts_with("doc_2")
            || id.starts_with("doc_3")
            || id.starts_with("doc_4")),
        "cursor filter must exclude every non-active open tab's subtree"
    );
    assert!(
        !got.contains("doc_0_deep20") && !got.contains("doc_0_deep29"),
        "depth cap (< 20) must truncate the deep chain"
    );
}
