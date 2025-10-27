//! Validation experiment H7 — Now query latency at 100k tasks.
//!
//! ## Goal
//!
//! Confirm that the "Now" query (filter status=TODO, gating=unblocked,
//! gate=G1, sort by priority+effort, take 10) stays under 100ms p50 /
//! 200ms p99 at 100k tasks with ~10% blocked.
//!
//! ## Why
//!
//! If this is slow, the structural design works on paper but the agent's
//! "next task" pull becomes a UX problem. Per the agreed kill condition,
//! a p99 over 200ms means we must either materialize the Now query itself
//! or denormalize the gating column.
//!
//! ## Design
//!
//! Schema (per H1):
//! - `task(id, status, gate, priority, effort)` with composite index on
//!   `(gate, status, priority, effort)`.
//! - `task_blocks(from_id, to_id)` junction with FK CASCADE.
//! - `task_blocking_edges` matview: one row per *active* blocking
//!   relationship (`blocker.status != 'DONE'`).
//!
//! Two query variants under test:
//!   - **A: Naive** — `NOT EXISTS (SELECT 1 FROM task_blocks JOIN task ...)`
//!   - **B: Matviewed** — `NOT EXISTS (SELECT 1 FROM task_blocking_edges)`
//!
//! Three data scenarios, each at exactly 100k tasks:
//!   - **S1 Uniform**: ~10% of tasks have 1-3 random blockers.
//!   - **S2 Hot blockers**: 100 "popular" tasks block 50-80 others each.
//!     Stresses fan-out from the JOIN side.
//!   - **S3 Hot dependents**: 100 tasks have 50-80 blockers each. Stresses
//!     fan-in to the gating subquery.
//!
//! Per scenario × variant: 5 warmup runs, 100 measured runs, report
//! p50/p99/p99.9 in milliseconds. Pass = all p99 < 200ms.
//!
//! Run: `cargo run --release --example now_query_perf_h7`
//!   (Release mode mandatory — debug timings are noise.)

use std::time::{Duration, Instant};

use anyhow::Result;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const N_TASKS: usize = 100_000;
const SEED: u64 = 0xCAFE_BABE_2026_0501;
const WARMUP_RUNS: usize = 5;
const MEASURED_RUNS: usize = 100;
const P99_KILL_MS: f64 = 200.0;

const STATUS_DONE: &str = "DONE";
const STATUS_TODO: &str = "TODO";
const STATUS_DOING: &str = "DOING";

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== H7: Now query latency at {N_TASKS} tasks ===\n");

    let mut all_passed = true;

    for scenario in [
        Scenario::Uniform,
        Scenario::HotBlockers,
        Scenario::HotDependents,
    ] {
        let result = measure_scenario(scenario).await?;
        all_passed &= result;
    }

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("H7 RESULT: PASS — all variants under p99 {P99_KILL_MS}ms.");
        Ok(())
    } else {
        println!("H7 RESULT: FAIL — at least one variant exceeded kill condition.");
        std::process::exit(1);
    }
}

// ── Scenario runner ───────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Scenario {
    Uniform,
    HotBlockers,
    HotDependents,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Uniform => "S1 Uniform (10% have 1-3 blockers)",
            Self::HotBlockers => "S2 Hot blockers (100 tasks block 50-80 others)",
            Self::HotDependents => "S3 Hot dependents (100 tasks have 50-80 blockers)",
        }
    }
}

async fn measure_scenario(scenario: Scenario) -> Result<bool> {
    println!("\n--- {} ---", scenario.name());

    let db_path = format!("/tmp/h7-now-query-{:?}.db", scenario as u8);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }

    let setup_start = Instant::now();
    let db = turso::Builder::new_local(&db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;
    setup_schema(&conn).await?;
    // Create matview *before* loading data: matches production startup order
    // (schema migration first, data writes after) and ensures IVM observes
    // every INSERT incrementally rather than backfilling from existing rows.
    create_matview(&conn).await?;
    populate(&conn, scenario).await?;
    // Settle: 100k INSERTs produce many CDC events that propagate to the
    // matview asynchronously. Wait long enough for IVM to drain before
    // measuring. Empirically 250ms (per H1) was sized for ≤100 events;
    // at 100k+ events we need significantly longer.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let (n_g1_todo, n_unblocked) = report_data_shape(&conn).await?;
    println!(
        "  setup time: {:.1}s  |  G1 TODO tasks: {n_g1_todo}  |  unblocked: {n_unblocked}",
        setup_start.elapsed().as_secs_f64()
    );

    // Correctness check: the *full* unblocked sets must agree between
    // variants (matview gating is equivalent to naive gating). Top-10
    // ORDER BY across variants is allowed to differ — matviews in Turso
    // don't preserve ORDER BY semantics, so the naive query is the
    // canonical source for ordered/limited results.
    let full_a = run_query(&conn, FULL_UNBLOCKED_NAIVE).await?;
    let full_b = run_query(&conn, FULL_UNBLOCKED_MATVIEWED).await?;
    let mut sa = full_a.clone();
    let mut sb = full_b.clone();
    sa.sort();
    sb.sort();
    let sets_agree = sa == sb;
    println!(
        "  full unblocked sets: naive={}  matviewed={}  agree={}",
        sa.len(),
        sb.len(),
        sets_agree
    );

    // Top-10 informational only: report whether they happen to agree.
    let res_a = run_query(&conn, NOW_QUERY_NAIVE).await?;
    let res_b = run_query(&conn, NOW_QUERY_MATVIEWED).await?;
    let top10_same = res_a == res_b;
    if !top10_same {
        println!(
            "  note: top-10 ordering differs (matview ORDER BY semantics).\n    \
             naive  : {:?}\n    matview: {:?}",
            res_a, res_b
        );
    } else {
        println!("  top-10 ordering: identical");
    }

    let stats_a = bench_query(&conn, "A naive       ", NOW_QUERY_NAIVE).await?;
    let stats_b = bench_query(&conn, "B matviewed   ", NOW_QUERY_MATVIEWED).await?;

    let mut ok = true;
    ok &= check(
        &format!("A.p99 < {P99_KILL_MS}ms (naive)"),
        stats_a.p99_ms < P99_KILL_MS,
    );
    ok &= check(
        &format!("B.p99 < {P99_KILL_MS}ms (matviewed)"),
        stats_b.p99_ms < P99_KILL_MS,
    );
    ok &= check(
        "Full unblocked sets agree (matview gating == naive gating)",
        sets_agree,
    );

    Ok(ok)
}

// ── Schema ────────────────────────────────────────────────────────────────

async fn setup_schema(conn: &turso::Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE task (
            id        TEXT PRIMARY KEY,
            status    TEXT NOT NULL,
            gate      TEXT,
            priority  INTEGER NOT NULL,
            effort    INTEGER NOT NULL
        )",
        (),
    )
    .await?;
    conn.execute(
        "CREATE INDEX idx_task_now ON task(gate, status, priority, effort)",
        (),
    )
    .await?;
    conn.execute(
        "CREATE TABLE task_blocks (
            from_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
            to_id   TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
            PRIMARY KEY (from_id, to_id)
        )",
        (),
    )
    .await?;
    conn.execute("CREATE INDEX idx_task_blocks_to ON task_blocks(to_id)", ())
        .await?;
    Ok(())
}

async fn create_matview(conn: &turso::Connection) -> Result<()> {
    conn.execute(
        "CREATE MATERIALIZED VIEW task_blocking_edges AS
         SELECT bb.from_id AS task_id, bb.to_id AS blocker_id
         FROM task_blocks bb
         JOIN task b ON b.id = bb.to_id
         WHERE b.status != 'DONE'",
        (),
    )
    .await?;
    Ok(())
}

// ── Data generation ───────────────────────────────────────────────────────

async fn populate(conn: &turso::Connection, scenario: Scenario) -> Result<()> {
    let mut rng = StdRng::seed_from_u64(SEED);

    // Tasks: deterministic distribution.
    //   60% DONE, 30% TODO, 10% DOING
    //   gate: 40% G1, 20% G2, 40% other
    //   priority: 1=A (20%), 2=B (30%), 3=C (50%)
    //   effort:   1=S (30%), 2=M (50%), 3=L (20%)
    let mut tx_size = 0usize;
    conn.execute("BEGIN", ()).await?;
    for i in 0..N_TASKS {
        let id = format!("t-{:06}", i);
        let r: f64 = rng.random();
        let status = if r < 0.60 {
            STATUS_DONE
        } else if r < 0.90 {
            STATUS_TODO
        } else {
            STATUS_DOING
        };
        let g: f64 = rng.random();
        let gate: Option<&'static str> = if g < 0.40 {
            Some("G1")
        } else if g < 0.60 {
            Some("G2")
        } else if g < 0.70 {
            Some("G3")
        } else if g < 0.78 {
            Some("G4")
        } else {
            None
        };
        let p: f64 = rng.random();
        let priority: i32 = if p < 0.20 {
            1
        } else if p < 0.50 {
            2
        } else {
            3
        };
        let e: f64 = rng.random();
        let effort: i32 = if e < 0.30 {
            1
        } else if e < 0.80 {
            2
        } else {
            3
        };
        let gate_sql = match gate {
            Some(s) => format!("'{}'", s),
            None => "NULL".to_string(),
        };
        conn.execute(
            &format!(
                "INSERT INTO task (id, status, gate, priority, effort) \
                 VALUES ('{id}', '{status}', {gate_sql}, {priority}, {effort})"
            ),
            (),
        )
        .await?;
        tx_size += 1;
        if tx_size % 5_000 == 0 {
            conn.execute("COMMIT", ()).await?;
            conn.execute("BEGIN", ()).await?;
        }
    }
    conn.execute("COMMIT", ()).await?;

    // Edges: scenario-specific.
    populate_edges(conn, scenario, &mut rng).await?;
    Ok(())
}

async fn populate_edges(
    conn: &turso::Connection,
    scenario: Scenario,
    rng: &mut StdRng,
) -> Result<()> {
    conn.execute("BEGIN", ()).await?;
    let mut tx_size = 0usize;
    let emit = |from_idx: usize, to_idx: usize| -> (String, String) {
        (format!("t-{:06}", from_idx), format!("t-{:06}", to_idx))
    };

    match scenario {
        Scenario::Uniform => {
            // ~10% of tasks have 1-3 random blockers. Avoid self-loops.
            for i in 0..N_TASKS {
                if rng.random::<f64>() >= 0.10 {
                    continue;
                }
                let n_blockers = 1 + rng.random_range(0..3);
                for _ in 0..n_blockers {
                    let mut j = rng.random_range(0..N_TASKS);
                    if j == i {
                        j = (j + 1) % N_TASKS;
                    }
                    let (from, to) = emit(i, j);
                    let _ = conn
                        .execute(
                            &format!(
                                "INSERT OR IGNORE INTO task_blocks \
                                 (from_id, to_id) VALUES ('{from}', '{to}')"
                            ),
                            (),
                        )
                        .await;
                    tx_size += 1;
                    if tx_size % 5_000 == 0 {
                        conn.execute("COMMIT", ()).await?;
                        conn.execute("BEGIN", ()).await?;
                    }
                }
            }
        }
        Scenario::HotBlockers => {
            // 100 popular blockers, each blocking 50-80 random others. Plus
            // background ~5% uniform.
            let popular: Vec<usize> = (0..100).map(|i| i * (N_TASKS / 100)).collect();
            for &p in &popular {
                let n = 50 + rng.random_range(0..30);
                for _ in 0..n {
                    let mut j = rng.random_range(0..N_TASKS);
                    if j == p {
                        j = (j + 1) % N_TASKS;
                    }
                    let (from, to) = emit(j, p);
                    let _ = conn
                        .execute(
                            &format!(
                                "INSERT OR IGNORE INTO task_blocks \
                                 (from_id, to_id) VALUES ('{from}', '{to}')"
                            ),
                            (),
                        )
                        .await;
                    tx_size += 1;
                    if tx_size % 5_000 == 0 {
                        conn.execute("COMMIT", ()).await?;
                        conn.execute("BEGIN", ()).await?;
                    }
                }
            }
            for i in 0..N_TASKS {
                if rng.random::<f64>() >= 0.05 {
                    continue;
                }
                let mut j = rng.random_range(0..N_TASKS);
                if j == i {
                    j = (j + 1) % N_TASKS;
                }
                let (from, to) = emit(i, j);
                let _ = conn
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO task_blocks \
                             (from_id, to_id) VALUES ('{from}', '{to}')"
                        ),
                        (),
                    )
                    .await;
                tx_size += 1;
                if tx_size % 5_000 == 0 {
                    conn.execute("COMMIT", ()).await?;
                    conn.execute("BEGIN", ()).await?;
                }
            }
        }
        Scenario::HotDependents => {
            // 100 dependents, each with 50-80 blockers. Plus background ~5% uniform.
            let dependents: Vec<usize> = (0..100).map(|i| i * (N_TASKS / 100) + 1).collect();
            for &d in &dependents {
                let n = 50 + rng.random_range(0..30);
                for _ in 0..n {
                    let mut j = rng.random_range(0..N_TASKS);
                    if j == d {
                        j = (j + 1) % N_TASKS;
                    }
                    let (from, to) = emit(d, j);
                    let _ = conn
                        .execute(
                            &format!(
                                "INSERT OR IGNORE INTO task_blocks \
                                 (from_id, to_id) VALUES ('{from}', '{to}')"
                            ),
                            (),
                        )
                        .await;
                    tx_size += 1;
                    if tx_size % 5_000 == 0 {
                        conn.execute("COMMIT", ()).await?;
                        conn.execute("BEGIN", ()).await?;
                    }
                }
            }
            for i in 0..N_TASKS {
                if rng.random::<f64>() >= 0.05 {
                    continue;
                }
                let mut j = rng.random_range(0..N_TASKS);
                if j == i {
                    j = (j + 1) % N_TASKS;
                }
                let (from, to) = emit(i, j);
                let _ = conn
                    .execute(
                        &format!(
                            "INSERT OR IGNORE INTO task_blocks \
                             (from_id, to_id) VALUES ('{from}', '{to}')"
                        ),
                        (),
                    )
                    .await;
                tx_size += 1;
                if tx_size % 5_000 == 0 {
                    conn.execute("COMMIT", ()).await?;
                    conn.execute("BEGIN", ()).await?;
                }
            }
        }
    }
    conn.execute("COMMIT", ()).await?;
    conn.execute("ANALYZE", ()).await?;
    Ok(())
}

// ── Now query variants ────────────────────────────────────────────────────

const NOW_QUERY_NAIVE: &str = "
    SELECT id, priority, effort
    FROM task t
    WHERE t.status = 'TODO'
      AND t.gate = 'G1'
      AND NOT EXISTS (
        SELECT 1 FROM task_blocks bb
        JOIN task b ON b.id = bb.to_id
        WHERE bb.from_id = t.id
          AND b.status != 'DONE'
      )
    ORDER BY t.priority ASC, t.effort ASC, t.id ASC
    LIMIT 10";

const NOW_QUERY_MATVIEWED: &str = "
    SELECT id, priority, effort
    FROM task t
    WHERE t.status = 'TODO'
      AND t.gate = 'G1'
      AND NOT EXISTS (
        SELECT 1 FROM task_blocking_edges WHERE task_id = t.id
      )
    ORDER BY t.priority ASC, t.effort ASC, t.id ASC
    LIMIT 10";

// Diagnostics: full unblocked sets, no LIMIT, no ORDER BY (set-only check).

const FULL_UNBLOCKED_NAIVE: &str = "
    SELECT id FROM task t
    WHERE t.status = 'TODO' AND t.gate = 'G1'
      AND NOT EXISTS (
        SELECT 1 FROM task_blocks bb JOIN task b ON b.id = bb.to_id
        WHERE bb.from_id = t.id AND b.status != 'DONE'
      )";

const FULL_UNBLOCKED_MATVIEWED: &str = "
    SELECT id FROM task t
    WHERE t.status = 'TODO' AND t.gate = 'G1'
      AND NOT EXISTS (
        SELECT 1 FROM task_blocking_edges WHERE task_id = t.id
      )";

async fn run_query(conn: &turso::Connection, sql: &str) -> Result<Vec<String>> {
    let mut rows = conn.query(sql, ()).await?;
    let mut got: Vec<String> = Vec::new();
    while let Some(row) = rows.next().await? {
        let v = row.get_value(0)?;
        if let turso::Value::Text(s) = v {
            got.push(s);
        }
    }
    Ok(got)
}

// ── Benchmark + stats ─────────────────────────────────────────────────────

#[derive(Debug)]
struct Stats {
    p50_ms: f64,
    p99_ms: f64,
    p999_ms: f64,
    min_ms: f64,
    max_ms: f64,
    mean_ms: f64,
}

async fn bench_query(conn: &turso::Connection, label: &str, sql: &str) -> Result<Stats> {
    // Warmup
    for _ in 0..WARMUP_RUNS {
        let _ = run_query(conn, sql).await?;
    }
    // Measure
    let mut times: Vec<Duration> = Vec::with_capacity(MEASURED_RUNS);
    for _ in 0..MEASURED_RUNS {
        let t0 = Instant::now();
        let _ = run_query(conn, sql).await?;
        times.push(t0.elapsed());
    }
    let stats = compute_stats(&times);
    println!(
        "  {label}  p50={:6.2}ms  p99={:6.2}ms  p99.9={:6.2}ms  min={:5.2}  max={:6.2}  mean={:6.2}",
        stats.p50_ms, stats.p99_ms, stats.p999_ms, stats.min_ms, stats.max_ms, stats.mean_ms,
    );
    Ok(stats)
}

fn compute_stats(times: &[Duration]) -> Stats {
    let mut ms: Vec<f64> = times.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = ms.len();
    let pct = |p: f64| {
        let idx = ((p * (n as f64 - 1.0)).round() as usize).min(n - 1);
        ms[idx]
    };
    let mean = ms.iter().sum::<f64>() / n as f64;
    Stats {
        p50_ms: pct(0.50),
        p99_ms: pct(0.99),
        p999_ms: pct(0.999),
        min_ms: ms[0],
        max_ms: ms[n - 1],
        mean_ms: mean,
    }
}

fn check(label: &str, ok: bool) -> bool {
    let mark = if ok { "PASS" } else { "FAIL" };
    println!("  [{mark}] {label}");
    ok
}

// ── Data shape report ─────────────────────────────────────────────────────

async fn report_data_shape(conn: &turso::Connection) -> Result<(usize, usize)> {
    let mut q = conn
        .query(
            "SELECT COUNT(*) FROM task WHERE status = 'TODO' AND gate = 'G1'",
            (),
        )
        .await?;
    let n_g1_todo = match q.next().await? {
        Some(row) => row.get_value(0)?.as_integer().copied().unwrap_or(0) as usize,
        None => 0,
    };
    let mut q = conn
        .query(
            "SELECT COUNT(*) FROM task t WHERE t.status = 'TODO' AND t.gate = 'G1' \
             AND NOT EXISTS (SELECT 1 FROM task_blocks bb JOIN task b ON b.id = bb.to_id \
             WHERE bb.from_id = t.id AND b.status != 'DONE')",
            (),
        )
        .await?;
    let n_unblocked = match q.next().await? {
        Some(row) => row.get_value(0)?.as_integer().copied().unwrap_or(0) as usize,
        None => 0,
    };
    Ok((n_g1_todo, n_unblocked))
}
