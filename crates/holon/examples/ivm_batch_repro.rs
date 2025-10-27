//! Reproduction attempts for IVM recursive CTE + external JOIN bug.
//!
//! Tests multiple scenarios with varying batch sizes, tree depths, and
//! pre-existing matviews to find what triggers the lossy IVM behavior.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use turso_core::types::RelationChangeEvent;

async fn count(conn: &turso::Connection, sql: &str) -> i64 {
    let mut rows = conn.query(sql, ()).await.unwrap();
    if let Some(row) = rows.next().await.unwrap() {
        row.get::<i64>(0).unwrap()
    } else {
        0
    }
}

const WATCH_VIEW_SQL: &str = "WITH RECURSIVE _vl2 AS (
        SELECT b.id AS node_id, b.id AS source_id, 0 AS depth,
               CAST(b.id AS TEXT) AS visited
        FROM block AS b
        UNION ALL
        SELECT child.id, _vl2.source_id, _vl2.depth + 1,
               _vl2.visited || ',' || CAST(child.id AS TEXT)
        FROM _vl2
        JOIN block child ON child.parent_id = _vl2.node_id
        WHERE _vl2.depth < 20
          AND ',' || _vl2.visited || ',' NOT LIKE '%,' || CAST(child.id AS TEXT) || ',%'
    )
    SELECT b2.*
    FROM focus_roots AS fr
    JOIN block AS b1 ON b1.id = fr.root_id
    JOIN _vl2 ON _vl2.source_id = b1.id
    JOIN block AS b2 ON b2.id = _vl2.node_id
    WHERE fr.region = 'main' AND b2.content_type <> 'source'";

/// Turso doesn't support recursive CTEs in direct SELECT — only in matviews.
/// Create a temporary matview to get the "fresh" count.
async fn fresh_count(conn: &turso::Connection, check_id: &mut u64) -> i64 {
    let name = format!("_check_{}", *check_id);
    *check_id += 1;
    let _ = conn
        .execute(&format!("DROP VIEW IF EXISTS {name}"), ())
        .await;
    conn.execute(
        &format!("CREATE MATERIALIZED VIEW {name} AS {WATCH_VIEW_SQL}"),
        (),
    )
    .await
    .unwrap();
    let c = count(conn, &format!("SELECT count(*) FROM {name}")).await;
    let _ = conn
        .execute(&format!("DROP VIEW IF EXISTS {name}"), ())
        .await;
    c
}

struct TestConfig {
    label: &'static str,
    use_transaction: bool,
    num_initial_blocks: usize,
    batch_size: usize,
    num_batches: usize,
    tree_depth: usize,
    create_other_matviews: bool,
    create_matview_after_n_batches: usize,
}

async fn run_scenario(cfg: &TestConfig) -> bool {
    let mut check_id: u64 = 0;
    let db_path = format!("/tmp/ivm-repro-{}.db", cfg.label.replace(' ', "-"));
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    let db = turso::Builder::new_local(&db_path)
        .experimental_materialized_views(true)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();

    let cdc_count = Arc::new(AtomicUsize::new(0));
    let cdc_clone = cdc_count.clone();
    conn.set_change_callback(move |_: &RelationChangeEvent| {
        cdc_clone.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();

    conn.execute("CREATE TABLE block (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text', properties TEXT DEFAULT '{}')", ()).await.unwrap();
    conn.execute(
        "CREATE TABLE focus_roots (region TEXT PRIMARY KEY, root_id TEXT NOT NULL)",
        (),
    )
    .await
    .unwrap();

    conn.execute(
        "INSERT INTO focus_roots (region, root_id) VALUES ('main', 'block:root')",
        (),
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO block VALUES ('block:root', 'doc:1', 'Root', 'text', '{}')",
        (),
    )
    .await
    .unwrap();

    if cfg.create_other_matviews {
        conn.execute(
            "CREATE MATERIALIZED VIEW block_with_path AS
            WITH RECURSIVE paths AS (
                SELECT id, parent_id, content, content_type, properties,
                       '/' || id AS path, id AS root_id
                FROM block WHERE parent_id LIKE 'doc:%'
                UNION ALL
                SELECT b.id, b.parent_id, b.content, b.content_type, b.properties,
                       p.path || '/' || b.id, p.root_id
                FROM block b INNER JOIN paths p ON b.parent_id = p.id
            )
            SELECT * FROM paths",
            (),
        )
        .await
        .unwrap();

        conn.execute(
            "CREATE MATERIALIZED VIEW structural_view AS
            SELECT id, content, content_type, parent_id FROM block
            WHERE id = 'block:root' OR parent_id = 'block:root'",
            (),
        )
        .await
        .unwrap();
    }

    let mut all_inserts: Vec<Vec<String>> = Vec::new();
    let mut block_id = 0usize;

    // Initial flat blocks
    let mut current_batch = Vec::new();
    for i in 0..cfg.num_initial_blocks {
        current_batch.push(format!(
            "INSERT INTO block VALUES ('block:init-{i}', 'block:root', 'Init {i}', 'text', '{{}}')"
        ));
        if current_batch.len() >= cfg.batch_size {
            all_inserts.push(std::mem::take(&mut current_batch));
        }
    }
    if !current_batch.is_empty() {
        all_inserts.push(std::mem::take(&mut current_batch));
    }

    fn gen_tree(
        parent: &str,
        depth: usize,
        max_depth: usize,
        id_counter: &mut usize,
        batch: &mut Vec<String>,
        children_per_node: usize,
    ) {
        if depth >= max_depth {
            return;
        }
        for _ in 0..children_per_node {
            let id = format!("block:node-{}", *id_counter);
            *id_counter += 1;
            batch.push(format!(
                "INSERT INTO block VALUES ('{id}', '{parent}', 'Node {}', 'text', '{{}}')",
                *id_counter
            ));
            gen_tree(
                &id,
                depth + 1,
                max_depth,
                id_counter,
                batch,
                children_per_node,
            );
        }
    }

    for batch_idx in 0..cfg.num_batches {
        let mut batch = Vec::new();
        let parent = format!("block:batch-root-{batch_idx}");
        batch.push(format!(
            "INSERT INTO block VALUES ('{parent}', 'block:root', 'Batch Root {batch_idx}', 'text', '{{}}')"
        ));
        gen_tree(&parent, 0, cfg.tree_depth, &mut block_id, &mut batch, 3);

        while batch.len() < cfg.batch_size {
            let id = format!("block:flat-{batch_idx}-{}", block_id);
            block_id += 1;
            batch.push(format!(
                "INSERT INTO block VALUES ('{id}', '{parent}', 'Flat {block_id}', 'text', '{{}}')"
            ));
        }
        all_inserts.push(batch);
    }

    let mut matview_created = false;
    let mut bug_found = false;

    for (batch_idx, batch) in all_inserts.iter().enumerate() {
        if !matview_created && batch_idx >= cfg.create_matview_after_n_batches {
            conn.execute(
                &format!("CREATE MATERIALIZED VIEW watch_view AS {WATCH_VIEW_SQL}"),
                (),
            )
            .await
            .unwrap();
            matview_created = true;

            let mv = count(&conn, "SELECT count(*) FROM watch_view").await;
            let direct = fresh_count(&conn, &mut check_id).await;
            if mv != direct {
                println!("  [{}] BUG at creation! mv={mv} direct={direct}", cfg.label);
                bug_found = true;
            }
        }

        if cfg.use_transaction {
            conn.execute("BEGIN TRANSACTION", ()).await.unwrap();
            for sql in batch {
                conn.execute(sql, ()).await.unwrap();
            }
            conn.execute("COMMIT", ()).await.unwrap();
        } else {
            for sql in batch {
                conn.execute(sql, ()).await.unwrap();
            }
        }

        if matview_created {
            let mv = count(&conn, "SELECT count(*) FROM watch_view").await;
            let direct = fresh_count(&conn, &mut check_id).await;
            if mv != direct {
                println!(
                    "  [{}] BUG after batch {batch_idx}! mv={mv} direct={direct} delta={}",
                    cfg.label,
                    direct - mv
                );
                bug_found = true;
            }
        }
    }

    if !matview_created {
        conn.execute(
            &format!("CREATE MATERIALIZED VIEW watch_view AS {WATCH_VIEW_SQL}"),
            (),
        )
        .await
        .unwrap();
    }

    let base = count(&conn, "SELECT count(*) FROM block").await;
    let mv = count(&conn, "SELECT count(*) FROM watch_view").await;
    let direct = fresh_count(&conn, &mut check_id).await;

    if mv != direct {
        println!(
            "[{}] FAIL: base={base} mv={mv} direct={direct} missing={}",
            cfg.label,
            direct - mv
        );
        bug_found = true;
    } else {
        println!("[{}] OK: base={base} mv={mv} direct={direct}", cfg.label);
    }

    bug_found
}

#[tokio::main]
async fn main() {
    println!("=== IVM Batch Transaction Reproduction Test (Extended) ===\n");

    let scenarios = vec![
        TestConfig {
            label: "auto-commit-simple",
            use_transaction: false,
            num_initial_blocks: 0,
            batch_size: 50,
            num_batches: 5,
            tree_depth: 4,
            create_other_matviews: false,
            create_matview_after_n_batches: 0,
        },
        TestConfig {
            label: "batch-simple",
            use_transaction: true,
            num_initial_blocks: 0,
            batch_size: 50,
            num_batches: 5,
            tree_depth: 4,
            create_other_matviews: false,
            create_matview_after_n_batches: 0,
        },
        TestConfig {
            label: "batch-with-matviews",
            use_transaction: true,
            num_initial_blocks: 0,
            batch_size: 50,
            num_batches: 5,
            tree_depth: 4,
            create_other_matviews: true,
            create_matview_after_n_batches: 0,
        },
        TestConfig {
            label: "batch-mid-insert",
            use_transaction: true,
            num_initial_blocks: 0,
            batch_size: 100,
            num_batches: 5,
            tree_depth: 4,
            create_other_matviews: true,
            create_matview_after_n_batches: 2,
        },
        TestConfig {
            label: "batch-large-200",
            use_transaction: true,
            num_initial_blocks: 0,
            batch_size: 200,
            num_batches: 3,
            tree_depth: 5,
            create_other_matviews: true,
            create_matview_after_n_batches: 1,
        },
        TestConfig {
            label: "batch-initial-data",
            use_transaction: true,
            num_initial_blocks: 100,
            batch_size: 100,
            num_batches: 3,
            tree_depth: 4,
            create_other_matviews: true,
            create_matview_after_n_batches: 1,
        },
        TestConfig {
            label: "many-small-batches",
            use_transaction: true,
            num_initial_blocks: 50,
            batch_size: 10,
            num_batches: 20,
            tree_depth: 3,
            create_other_matviews: true,
            create_matview_after_n_batches: 5,
        },
    ];

    let mut any_bug = false;
    for cfg in &scenarios {
        if run_scenario(cfg).await {
            any_bug = true;
        }
    }

    println!(
        "\n=== {} ===",
        if any_bug {
            "BUGS FOUND"
        } else {
            "ALL PASSED — bug not reproduced"
        }
    );
}
