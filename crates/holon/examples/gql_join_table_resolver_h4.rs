//! Validation experiment H4 — JoinTableEdgeResolver fits our edge-descriptor shape.
//!
//! ## Background
//!
//! Holon plans to expose multi-valued edge fields (e.g. `:BLOCKED-BY:`) as
//! GQL-traversable edges backed by a junction table. Upstream `gql-transform`
//! ships a `JoinTableEdgeResolver` whose constructor takes exactly the shape
//! we want: `(join_table, source_column, target_column)`. But Holon's current
//! `GraphSchemaRegistry` only wires `ForeignKeyEdgeResolver` (single-valued
//! reference). H4 confirms three things before we commit:
//!
//! 1. The resolver can be registered against a relational schema (not EAV)
//!    using only the public API.
//! 2. The emitted SQL for 1-hop and 2-hop traversals stays in flat-JOIN
//!    territory (no `WITH RECURSIVE`) — keeping it out of the IVM bug class.
//! 3. Variable-length traversal does use `WITH RECURSIVE`, but the recursive
//!    step joins our junction table, *not* the EAV `edges` table.
//!
//! ## What "PASS" means
//!
//! For each of four GQL queries (forward 1-hop, forward 2-hop, variable
//! 1..3, reverse 1-hop) we:
//!   - parse + transform via the public entry points used by `BackendEngine`,
//!   - assert structural invariants on the emitted SQL,
//!   - execute the SQL against a small populated DB and assert result rows.
//!
//! Run: `cargo run --example gql_join_table_resolver_h4`

use std::collections::HashMap;

use anyhow::Result;
use gql_parser::{QueryOrUnion, parse};
use gql_transform::resolver::{
    ColumnMapping, EavEdgeResolver, EavNodeResolver, EdgeDef, GraphSchema, JoinTableEdgeResolver,
    MappedNodeResolver, NodeResolver,
};
use gql_transform::transform;

// ── GQL queries under test ────────────────────────────────────────────────

/// 1-hop forward: blockers of A.
const Q1: &str = "MATCH (a:task)-[:BLOCKED_BY]->(b:task) WHERE a.id = 'A' RETURN b.id";

/// 2-hop chain: transitive blockers reachable in exactly 2 hops from A.
const Q2: &str =
    "MATCH (a:task)-[:BLOCKED_BY]->(b:task)-[:BLOCKED_BY]->(c:task) WHERE a.id = 'A' RETURN c.id";

/// Variable-length 1..3: all transitive blockers up to depth 3.
const Q3: &str = "MATCH (a:task)-[:BLOCKED_BY*1..3]->(b:task) WHERE a.id = 'A' RETURN b.id";

/// Reverse direction: what does C unblock? (1-hop)
const Q4: &str = "MATCH (a:task)<-[:BLOCKED_BY]-(b:task) WHERE a.id = 'C' RETURN b.id";

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== H4: JoinTableEdgeResolver fits our edge-descriptor shape ===\n");

    let schema = build_schema();
    let mut all_passed = true;

    let q1_sql = compile(Q1, &schema)?;
    print_sql("Q1 (1-hop forward)", &q1_sql);
    all_passed &= check_q1(&q1_sql);

    let q2_sql = compile(Q2, &schema)?;
    print_sql("Q2 (2-hop chain)", &q2_sql);
    all_passed &= check_q2(&q2_sql);

    let q3_sql = compile(Q3, &schema)?;
    print_sql("Q3 (variable length 1..3)", &q3_sql);
    all_passed &= check_q3(&q3_sql);

    let q4_sql = compile(Q4, &schema)?;
    print_sql("Q4 (reverse 1-hop)", &q4_sql);
    all_passed &= check_q4(&q4_sql);

    println!("\n--- Executing SQL against populated DB ---");
    all_passed &= execute_against_db(&q1_sql, &q2_sql, &q3_sql, &q4_sql).await?;

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("H4 RESULT: PASS — resolver fits the descriptor shape.");
        Ok(())
    } else {
        println!("H4 RESULT: FAIL — see per-check output above.");
        std::process::exit(1);
    }
}

// ── Schema construction ──────────────────────────────────────────────────

fn build_schema() -> GraphSchema {
    let mut nodes: HashMap<String, Box<dyn NodeResolver>> = HashMap::new();
    let columns = vec![
        ColumnMapping {
            property_name: "id".into(),
            column_name: "id".into(),
        },
        ColumnMapping {
            property_name: "status".into(),
            column_name: "status".into(),
        },
    ];
    nodes.insert(
        "task".into(),
        Box::new(MappedNodeResolver {
            table_name: "task".into(),
            id_col: "id".into(),
            label: "task".into(),
            columns,
        }),
    );

    let mut edges = HashMap::new();
    edges.insert(
        "BLOCKED_BY".into(),
        EdgeDef {
            source_label: Some("task".into()),
            target_label: Some("task".into()),
            resolver: Box::new(JoinTableEdgeResolver {
                join_table: "task_blocks".into(),
                source_column: "from_id".into(),
                target_column: "to_id".into(),
            }),
        },
    );

    GraphSchema {
        nodes,
        edges,
        default_node_resolver: Box::new(EavNodeResolver),
        default_edge_resolver: Box::new(EavEdgeResolver),
        raw_return: true,
    }
}

fn compile(gql: &str, schema: &GraphSchema) -> Result<String> {
    let parsed = parse(gql).map_err(|e| anyhow::anyhow!("GQL parse error: {}", e.message))?;
    let query = match parsed {
        QueryOrUnion::Query(q) => q,
        QueryOrUnion::Union(_) => anyhow::bail!("UNION not used in this experiment"),
    };
    transform(&query, schema).map_err(|e| anyhow::anyhow!("GQL transform error: {:?}", e))
}

fn print_sql(label: &str, sql: &str) {
    println!("\n--- {label} emitted SQL ---");
    for line in sql.lines() {
        println!("  {}", line);
    }
}

fn check(label: &str, ok: bool) -> bool {
    let mark = if ok { "PASS" } else { "FAIL" };
    println!("  [{mark}] {label}");
    ok
}

// ── Per-query structural invariants ──────────────────────────────────────

fn check_q1(sql: &str) -> bool {
    let mut ok = true;
    ok &= check(
        "Q1: no WITH RECURSIVE for 1-hop",
        !sql.to_uppercase().contains("WITH RECURSIVE"),
    );
    ok &= check("Q1: joins task_blocks", sql.contains("task_blocks"));
    ok &= check("Q1: references task table", sql.contains("task"));
    ok &= check(
        "Q1: does not reference EAV edges/nodes tables",
        !contains_word(sql, "edges") && !contains_word(sql, "nodes"),
    );
    ok
}

fn check_q2(sql: &str) -> bool {
    let mut ok = true;
    ok &= check(
        "Q2: no WITH RECURSIVE for 2-hop",
        !sql.to_uppercase().contains("WITH RECURSIVE"),
    );
    ok &= check(
        "Q2: joins task_blocks at least twice",
        sql.matches("task_blocks").count() >= 2,
    );
    ok &= check(
        "Q2: does not reference EAV edges/nodes tables",
        !contains_word(sql, "edges") && !contains_word(sql, "nodes"),
    );
    ok
}

fn check_q3(sql: &str) -> bool {
    let mut ok = true;
    ok &= check(
        "Q3: uses WITH RECURSIVE (expected for variable length)",
        sql.to_uppercase().contains("WITH RECURSIVE"),
    );
    ok &= check(
        "Q3: recursive step joins task_blocks (not EAV edges)",
        sql.contains("task_blocks") && !contains_word(sql, "edges"),
    );
    ok
}

fn check_q4(sql: &str) -> bool {
    let mut ok = true;
    ok &= check(
        "Q4: no WITH RECURSIVE for reverse 1-hop",
        !sql.to_uppercase().contains("WITH RECURSIVE"),
    );
    ok &= check("Q4: joins task_blocks", sql.contains("task_blocks"));
    ok &= check(
        "Q4: does not reference EAV edges/nodes tables",
        !contains_word(sql, "edges") && !contains_word(sql, "nodes"),
    );
    ok
}

/// Whole-word check (so `task_blocks` doesn't trigger the `edges` exclusion etc.).
fn contains_word(haystack: &str, word: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|tok| tok == word)
}

// ── Execution test ───────────────────────────────────────────────────────

async fn execute_against_db(q1: &str, q2: &str, q3: &str, q4: &str) -> Result<bool> {
    let db_path = "/tmp/h4-gql-join-table.db";
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }
    let db = turso::Builder::new_local(db_path).build().await?;
    let conn = db.connect()?;

    // Schema: task + task_blocks junction (FK CASCADE so cleanup is clean).
    conn.execute(
        "CREATE TABLE task (id TEXT PRIMARY KEY, status TEXT NOT NULL)",
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

    // Graph: A blocked_by B, B blocked_by C, A blocked_by D.
    //   A -> B -> C
    //   A -> D
    // Reverse from C's perspective: C blocks B (i.e. B blocked_by C).
    //   So "what does C unblock" should yield {B}.
    for id in ["A", "B", "C", "D"] {
        conn.execute(
            &format!("INSERT INTO task (id, status) VALUES ('{id}', 'TODO')"),
            (),
        )
        .await?;
    }
    for (from, to) in [("A", "B"), ("B", "C"), ("A", "D")] {
        conn.execute(
            &format!("INSERT INTO task_blocks (from_id, to_id) VALUES ('{from}', '{to}')"),
            (),
        )
        .await?;
    }

    let mut ok = true;
    ok &= check_query(
        "Q1 returns {B, D} (1-hop blockers of A)",
        q1,
        &["B", "D"],
        &conn,
    )
    .await?;
    ok &= check_query("Q2 returns {C} (2-hop blockers of A)", q2, &["C"], &conn).await?;
    ok &= check_query(
        "Q3 returns {B, C, D} (transitive blockers of A, depth 1..3)",
        q3,
        &["B", "C", "D"],
        &conn,
    )
    .await?;
    ok &= check_query("Q4 returns {B} (what C unblocks)", q4, &["B"], &conn).await?;

    Ok(ok)
}

async fn check_query(
    label: &str,
    sql: &str,
    expected_ids: &[&str],
    conn: &turso::Connection,
) -> Result<bool> {
    let mut rows = conn.query(sql, ()).await?;
    let mut got: Vec<String> = Vec::new();
    while let Some(row) = rows.next().await? {
        // Result is JSON-shaped per `raw_return: true`. Extract the id whether
        // the row is a scalar string, an integer, or a JSON object containing it.
        let v = row.get_value(0)?;
        let s = match v {
            turso::Value::Text(s) => s,
            turso::Value::Integer(i) => i.to_string(),
            other => format!("{other:?}"),
        };
        got.push(s);
    }
    got.sort();
    let mut want: Vec<String> = expected_ids.iter().map(|s| s.to_string()).collect();
    want.sort();

    let matches = got == want;
    if !matches {
        println!("  [FAIL] {label}\n    expected: {want:?}\n    got:      {got:?}");
    } else {
        println!("  [PASS] {label} -> {got:?}");
    }
    Ok(matches)
}
