//! Reproducer: Turso IVM DBSP chain break when upstream matviews are DROP+CREATEd
//! while downstream matviews persist from a previous session.
//!
//! Session 1 (fresh DB): Create tables, matview chain, insert data, navigate → CDC works ✓
//! Session 2 (reopen): DROP+CREATE upstream matviews, skip downstream → CDC broken ✗
//!
//! Run with:
//!   cargo test -p holon turso_ivm_navigation_cursor_repro -- --nocapture

use std::collections::{BTreeSet, HashMap};

use super::turso::TursoBackend;
use tempfile::TempDir;

fn ids_from_rows(rows: &[HashMap<String, holon_api::Value>], col: &str) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| {
            r.get(col)
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect()
}

fn collect_cdc(
    cdc_rx: &mut tokio::sync::broadcast::Receiver<
        holon_api::streaming::WithMetadata<
            holon_api::streaming::Batch<super::turso::RowChange>,
            holon_api::streaming::BatchMetadata,
        >,
    >,
) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    while let Ok(batch) = cdc_rx.try_recv() {
        let name = batch.metadata.relation_name.clone();
        let count = batch.inner.items.len();
        if count > 0 {
            *counts.entry(name).or_default() += count;
        }
    }
    counts
}

/// DROP+CREATE of upstream matviews breaks DBSP chain to persisted downstream matviews.
#[tokio::test]
async fn dbsp_chain_break_after_upstream_drop_create() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("repro_chain_break.db");

    // ═══════════════════════════════════════════
    // Session 1: Fresh database — everything works
    // ═══════════════════════════════════════════
    {
        let db = TursoBackend::open_database(&db_path).expect("open db");
        let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
        let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

        // Tables
        handle.execute_ddl("CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT NOT NULL, block_id TEXT NOT NULL)").await.unwrap();
        handle
            .execute_ddl(
                "CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER)",
            )
            .await
            .unwrap();
        handle.execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text', parent_id TEXT DEFAULT '', properties TEXT DEFAULT '{}')").await.unwrap();
        handle
            .execute(
                "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
                vec![],
            )
            .await
            .unwrap();

        // Matview chain: current_focus → focus_roots → watch_view (recursive CTE)
        handle.execute_ddl("CREATE MATERIALIZED VIEW current_focus AS SELECT nc.region, nh.block_id FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = nh.id").await.unwrap();
        handle.execute_ddl("CREATE MATERIALIZED VIEW focus_roots AS SELECT cf.region, cf.block_id, b.id AS root_id FROM current_focus cf JOIN block b ON b.parent_id = cf.block_id UNION ALL SELECT cf.region, cf.block_id, b.id AS root_id FROM current_focus cf JOIN block b ON b.id = cf.block_id").await.unwrap();
        handle
            .execute_ddl(
                "CREATE MATERIALIZED VIEW watch_view AS
             WITH RECURSIVE _vl2 AS (
                 SELECT _v1.id AS node_id, _v1.id AS source_id, 0 AS depth,
                        CAST(_v1.id AS TEXT) AS visited
                 FROM block AS _v1
                 UNION ALL
                 SELECT _fk.id, _vl2.source_id, _vl2.depth + 1,
                        _vl2.visited || ',' || CAST(_fk.id AS TEXT)
                 FROM _vl2 JOIN block _fk ON _fk.parent_id = _vl2.node_id
                 WHERE _vl2.depth < 20
                   AND ',' || _vl2.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%'
             )
             SELECT _v3.*, json_extract(_v3.\"properties\", '$.sequence') AS \"sequence\"
             FROM focus_roots AS _v0
             JOIN block AS _v1 ON _v1.\"id\" = _v0.\"root_id\"
             JOIN _vl2 ON _vl2.source_id = _v1.id
             JOIN block AS _v3 ON _v3.id = _vl2.node_id
             WHERE _v0.\"region\" = 'main' AND _v3.\"content_type\" != 'source'
               AND _vl2.depth >= 0 AND _vl2.depth <= 20",
            )
            .await
            .unwrap();

        // Data: doc_a (3 children), doc_b (2 children)
        handle
            .execute(
                "INSERT INTO block (id, content, parent_id) VALUES ('doc_a', 'Doc A', 'root')",
                vec![],
            )
            .await
            .unwrap();
        for i in 1..=3 {
            handle.execute(&format!("INSERT INTO block (id, content, parent_id) VALUES ('a_child_{i}', 'Child', 'doc_a')"), vec![]).await.unwrap();
        }
        handle
            .execute(
                "INSERT INTO block (id, content, parent_id) VALUES ('doc_b', 'Doc B', 'root')",
                vec![],
            )
            .await
            .unwrap();
        for i in 1..=2 {
            handle.execute(&format!("INSERT INTO block (id, content, parent_id) VALUES ('b_child_{i}', 'Child', 'doc_b')"), vec![]).await.unwrap();
        }

        // Navigate to doc_a, then doc_b
        handle
            .execute(
                "INSERT INTO navigation_history (id, region, block_id) VALUES (1, 'main', 'doc_a')",
                vec![],
            )
            .await
            .unwrap();
        handle
            .execute(
                "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
                vec![],
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = collect_cdc(&mut cdc_rx);

        handle
            .execute(
                "INSERT INTO navigation_history (id, region, block_id) VALUES (2, 'main', 'doc_b')",
                vec![],
            )
            .await
            .unwrap();
        handle
            .execute(
                "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 2)",
                vec![],
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let cdc1 = collect_cdc(&mut cdc_rx);
        eprintln!("[session1] CDC: {:?}", cdc1);

        let ids = ids_from_rows(
            &handle
                .query("SELECT id FROM watch_view", HashMap::new())
                .await
                .unwrap(),
            "id",
        );
        eprintln!("[session1] watch_view: {:?}", ids);
        assert!(
            ids.contains("doc_b"),
            "Session 1: watch_view should show doc_b"
        );
        assert!(
            *cdc1.get("watch_view").unwrap_or(&0) > 0,
            "Session 1: watch_view CDC should fire"
        );
        eprintln!("[session1] PASS");
    }

    // ═══════════════════════════════════════════
    // Session 2: Reopen + DROP+CREATE upstream matviews
    // ═══════════════════════════════════════════
    eprintln!("\n[session2] Reopening...");
    {
        let db = TursoBackend::open_database(&db_path).expect("reopen db");
        let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
        let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

        // Recreate upstream matviews (production schema setup does this every start)
        handle
            .execute_ddl("DROP VIEW IF EXISTS focus_roots")
            .await
            .unwrap();
        handle
            .execute_ddl("DROP VIEW IF EXISTS current_focus")
            .await
            .unwrap();
        handle.execute_ddl("CREATE MATERIALIZED VIEW current_focus AS SELECT nc.region, nh.block_id FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = nh.id").await.unwrap();
        handle.execute_ddl("CREATE MATERIALIZED VIEW focus_roots AS SELECT cf.region, cf.block_id, b.id AS root_id FROM current_focus cf JOIN block b ON b.parent_id = cf.block_id UNION ALL SELECT cf.region, cf.block_id, b.id AS root_id FROM current_focus cf JOIN block b ON b.id = cf.block_id").await.unwrap();
        // watch_view is NOT recreated — persists from session 1

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = collect_cdc(&mut cdc_rx);

        // Navigate to doc_a
        handle
            .execute(
                "INSERT INTO navigation_history (id, region, block_id) VALUES (3, 'main', 'doc_a')",
                vec![],
            )
            .await
            .unwrap();
        handle
            .execute(
                "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 3)",
                vec![],
            )
            .await
            .unwrap();
        eprintln!("[session2] Navigated to doc_a");

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let cdc2 = collect_cdc(&mut cdc_rx);
        eprintln!("[session2] CDC: {:?}", cdc2);

        // Upstream matviews update correctly
        let focus = handle
            .query(
                "SELECT block_id FROM current_focus WHERE region = 'main'",
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            focus[0].get("block_id").unwrap().as_string().unwrap(),
            "doc_a"
        );
        let roots = ids_from_rows(
            &handle
                .query(
                    "SELECT root_id FROM focus_roots WHERE region = 'main'",
                    HashMap::new(),
                )
                .await
                .unwrap(),
            "root_id",
        );
        eprintln!("[session2] focus_roots: {:?}", roots);
        assert!(roots.contains("doc_a"));

        // watch_view should show doc_a, not stale doc_b
        let ids = ids_from_rows(
            &handle
                .query("SELECT id FROM watch_view", HashMap::new())
                .await
                .unwrap(),
            "id",
        );
        eprintln!("[session2] watch_view: {:?}", ids);

        let watch_cdc = cdc2.get("watch_view").copied().unwrap_or(0);
        assert!(
            ids.contains("doc_a") && !ids.contains("doc_b"),
            "BUG: DROP+CREATE of upstream matviews (current_focus, focus_roots) broke \
             the DBSP chain to the persisted downstream matview (watch_view).\n\
             \n\
             current_focus CDC: {}, focus_roots CDC: {}, watch_view CDC: {}\n\
             watch_view (stale): {:?}\n\
             focus_roots (correct): {:?}",
            cdc2.get("current_focus").unwrap_or(&0),
            cdc2.get("focus_roots").unwrap_or(&0),
            watch_cdc,
            ids,
            roots,
        );
        eprintln!("[session2] PASS");
    }
}
