//! Probe: which parent-cycle shapes spin the Turso IVM commit of a
//! `blocks_with_paths`-shaped recursive matview, and does a guard in the CTE
//! stop it. One scenario per process; the caller applies the deadline.
//!
//! Run: cargo run -p holon --example cycle_guard_ivm_probe --
//! <unguarded|depth|visited> <scenario-index>

use std::time::Instant;

const VIEW_UNGUARDED: &str = r#"CREATE MATERIALIZED VIEW blocks_with_paths AS
WITH RECURSIVE paths AS (
    SELECT id, parent_id, '/' || id AS path, id AS root_id
    FROM block WHERE parent_id LIKE 'sentinel:%'
    UNION ALL
    SELECT b.id, b.parent_id, p.path || '/' || b.id AS path, p.root_id
    FROM block b INNER JOIN paths p ON b.parent_id = p.id
)
SELECT * FROM paths"#;

const VIEW_DEPTH: &str = r#"CREATE MATERIALIZED VIEW blocks_with_paths AS
WITH RECURSIVE paths AS (
    SELECT id, parent_id, '/' || id AS path, id AS root_id, 0 AS depth
    FROM block WHERE parent_id LIKE 'sentinel:%'
    UNION ALL
    SELECT b.id, b.parent_id, p.path || '/' || b.id AS path, p.root_id, p.depth + 1 AS depth
    FROM block b INNER JOIN paths p ON b.parent_id = p.id
    WHERE p.depth < 50
)
SELECT * FROM paths"#;

const VIEW_VISITED: &str = r#"CREATE MATERIALIZED VIEW blocks_with_paths AS
WITH RECURSIVE paths AS (
    SELECT id, parent_id, '/' || id AS path, id AS root_id
    FROM block WHERE parent_id LIKE 'sentinel:%'
    UNION ALL
    SELECT b.id, b.parent_id, p.path || '/' || b.id AS path, p.root_id
    FROM block b INNER JOIN paths p ON b.parent_id = p.id
    WHERE instr(p.path || '/', '/' || b.id || '/') = 0
)
SELECT * FROM paths"#;

struct Scenario {
    name: &'static str,
    setup: &'static [&'static str],
    write: &'static [&'static str],
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "S1 attached: move A under its child B",
        setup: &[
            "INSERT INTO block VALUES ('a', 'sentinel:no_parent')",
            "INSERT INTO block VALUES ('b', 'a')",
        ],
        write: &["UPDATE block SET parent_id = 'b' WHERE id = 'a'"],
    },
    Scenario {
        name: "S2 attached: self-parent A",
        setup: &["INSERT INTO block VALUES ('a', 'sentinel:no_parent')"],
        write: &["UPDATE block SET parent_id = 'a' WHERE id = 'a'"],
    },
    Scenario {
        name: "S3 attached: root R, A under R, B under A, move A under B",
        setup: &[
            "INSERT INTO block VALUES ('r', 'sentinel:no_parent')",
            "INSERT INTO block VALUES ('a', 'r')",
            "INSERT INTO block VALUES ('b', 'a')",
        ],
        write: &["UPDATE block SET parent_id = 'b' WHERE id = 'a'"],
    },
    Scenario {
        name: "S4 detached: insert A->B, B->A fresh (one tx)",
        setup: &[],
        write: &[
            "BEGIN",
            "INSERT INTO block VALUES ('a', 'b')",
            "INSERT INTO block VALUES ('b', 'a')",
            "COMMIT",
        ],
    },
    Scenario {
        name: "S5 detached: insert A->B, then B->A (two tx)",
        setup: &["INSERT INTO block VALUES ('a', 'b')"],
        write: &["INSERT INTO block VALUES ('b', 'a')"],
    },
    Scenario {
        name: "S6 detached cycle exists, then attach A to root (breaks it)",
        setup: &[
            "INSERT INTO block VALUES ('a', 'b')",
            "INSERT INTO block VALUES ('b', 'a')",
        ],
        write: &["UPDATE block SET parent_id = 'sentinel:no_parent' WHERE id = 'a'"],
    },
    Scenario {
        name: "S7 attached: A,B under R; one tx A->B and B->A",
        setup: &[
            "INSERT INTO block VALUES ('r', 'sentinel:no_parent')",
            "INSERT INTO block VALUES ('a', 'r')",
            "INSERT INTO block VALUES ('b', 'r')",
        ],
        write: &[
            "BEGIN",
            "UPDATE block SET parent_id = 'b' WHERE id = 'a'",
            "UPDATE block SET parent_id = 'a' WHERE id = 'b'",
            "COMMIT",
        ],
    },
];

async fn run(view: &str, s: &Scenario, tag: &str) -> anyhow::Result<String> {
    let db_path = format!("/tmp/cycle-guard-probe-{tag}.db");
    for suffix in ["", "-wal", "-shm"] {
        let p = format!("{db_path}{suffix}");
        if std::path::Path::new(&p).exists() {
            std::fs::remove_file(&p)?;
        }
    }
    let db = turso::Builder::new_local(&db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;
    conn.execute(
        "CREATE TABLE block (id TEXT PRIMARY KEY, parent_id TEXT)",
        (),
    )
    .await?;
    conn.execute(view, ()).await?;
    for sql in s.setup {
        conn.execute(sql, ()).await?;
    }
    for sql in s.write {
        conn.execute(sql, ()).await?;
    }
    let mut rows = conn
        .query("SELECT id, path FROM blocks_with_paths ORDER BY path", ())
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let path: String = row.get(1)?;
        out.push(format!("{id}:{path}"));
    }
    Ok(format!("{} rows [{}]", out.len(), out.join(", ")))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let vname = args[1].as_str();
    let i: usize = args[2].parse().unwrap();
    let view = match vname {
        "unguarded" => VIEW_UNGUARDED.to_string(),
        "depth" => VIEW_DEPTH.to_string(),
        "depth1000" => VIEW_DEPTH.replace("p.depth < 50", "p.depth < 1000"),
        "visited" => VIEW_VISITED.to_string(),
        other => panic!("unknown view {other}"),
    };
    let s = &SCENARIOS[i];
    let start = Instant::now();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let r = rt.block_on(run(&view, s, &format!("{vname}-{i}")));
    match r {
        Ok(rows) => println!("[{vname}] {}: OK in {:?}: {rows}", s.name, start.elapsed()),
        Err(e) => println!(
            "[{vname}] {}: ERROR in {:?}: {e:#}",
            s.name,
            start.elapsed()
        ),
    }
}
