//! Reproducer for Turso IVM bug: "Index points to non-existent table row"
//!
//! This is secondary corruption from two related Turso IVM bugs:
//! 1. JoinOperator BTree cursor corruption (see HANDOFF_TURSO_IVM_JOIN_PANIC.md)
//! 2. Dirty pages pager bug (see examples/turso_ivm_dirty_pages_repro.rs)
//!
//! Both cause BTree index inconsistency during IVM cascades through chained matviews.
//!
//! The PBT (general_e2e_pbt_sql_only) catches this non-deterministically via stored
//! regression seeds. This standalone test attempts to reproduce the exact conditions
//! but the bug is timing-dependent and may not trigger every run.
//!
//! PBT regression seed that triggered it:
//!   9f86ffcce358a60a925bb894e527755cf4e75441696844b1a07a17928a99b0ea
//!
//! Conditions that trigger the bug:
//! - Multiple org files loaded at startup → bulk block inserts
//! - Todoist fake enabled → concurrent DDL during startup
//! - GQL query source in index.org → triggers graph matview creation
//! - set_field on render block → UPDATE cascades through IVM

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::{QueryLanguage, Value};
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

fn org_headings(n: usize, prefix: &str) -> String {
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!(
            "* Heading {prefix}{i}\n:PROPERTIES:\n:ID: {prefix}-{i}\n:END:\n"
        ));
    }
    s
}

/// Reproduction of the PBT-shrunk failure sequence.
///
/// The original PBT sequence:
/// 1. Create directories + JjGitInit
/// 2. Write index.org with GQL query + render source
/// 3. Write 3 additional org files (5+3+3 headings = 11 blocks + sources + defaults = ~28 total)
/// 4. Start app with todoist fake (concurrent DDL)
/// 5. set_field on render source block → "Index points to non-existent table row"
///
/// Note: This test passes most runs. The bug is timing-dependent — when it triggers,
/// the Turso pager's internal read sub-operations (used by IVM to compute deltas)
/// find dirty pages from the ongoing write transaction, corrupting the BTree index.
#[test]
fn turso_ivm_index_points_to_nonexistent_row() {
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_todoist_fake()
            .with_org_file(
                "index.org",
                concat!(
                    "* L9I \n",
                    ":PROPERTIES:\n",
                    ":ID: 1cq4-7g4-\n",
                    ":END:\n",
                    "#+BEGIN_SRC holon_gql :id 1cq4-7g4-::src::0\n",
                    "MATCH (n) RETURN n\n",
                    "#+END_SRC\n",
                    "#+BEGIN_SRC render :id 1cq4-7g4-::render::0\n",
                    "list item_template:(row (text content:\"node\"))\n",
                    "#+END_SRC\n",
                ),
            )
            .with_org_file("doc_a.org", &org_headings(5, "a"))
            .with_org_file("doc_b.org", &org_headings(3, "b"))
            .with_org_file("doc_c.org", &org_headings(3, "c"))
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        // Verify blocks loaded
        let rows = env
            .query("SELECT count(*) as cnt FROM block", QueryLanguage::HolonSql)
            .await
            .expect("count query should work");
        let count = rows[0].get("cnt").and_then(|v| v.as_i64()).unwrap_or(0);
        eprintln!("[reproducer] Block count after startup: {count}");
        assert!(
            count >= 10,
            "Should have loaded at least 10 blocks, got {count}"
        );

        // This triggers the IVM cascade: UPDATE on block table → IVM recomputes
        // blocks_with_paths (recursive CTE), events_view_block, watch_view_* matviews
        let mut params = HashMap::new();
        params.insert(
            "id".to_string(),
            Value::String("block:1cq4-7g4-::render::0".to_string()),
        );
        params.insert("field".to_string(), Value::String("content".to_string()));
        params.insert(
            "value".to_string(),
            Value::String("list(#{item_template: row(text(col(\"content\")))})".to_string()),
        );

        let result = env.execute_operation("block", "set_field", params).await;

        // When the bug triggers, this fails with:
        //   "Internal error: Index points to non-existent table row"
        // This is secondary corruption from the JoinOperator BTree cursor bug
        // (HANDOFF_TURSO_IVM_JOIN_PANIC.md) or the dirty pages pager bug.
        assert!(
            result.is_ok(),
            "set_field crashed with IVM index error: {:?}",
            result.err()
        );
    });
}

/// Stress test: runs startup + mutation pattern in a loop to maximize race window.
///
/// Set TURSO_IVM_STRESS_ROUNDS=N to control iterations (default: 5).
/// Expect most rounds to pass — the bug requires specific pager timing.
#[test]
fn turso_ivm_index_bug_stress() {
    let rounds: usize = std::env::var("TURSO_IVM_STRESS_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let mut ivm_errors = 0;
    let mut pager_panics = 0;

    for round in 0..rounds {
        eprintln!("[stress round {}/{}]", round + 1, rounds);
        let rt = runtime();
        let result = std::panic::catch_unwind(|| {
            rt.block_on(async {
                let env = TestEnvironmentBuilder::new()
                    .without_loro()
                    .with_todoist_fake()
                    .with_org_file(
                        "index.org",
                        &format!(
                            concat!(
                                "* Query {round}\n",
                                ":PROPERTIES:\n",
                                ":ID: q-{round}\n",
                                ":END:\n",
                                "#+BEGIN_SRC holon_gql :id q-{round}::src::0\n",
                                "MATCH (n) RETURN n\n",
                                "#+END_SRC\n",
                                "#+BEGIN_SRC render :id q-{round}::render::0\n",
                                "list item_template:(row (text content:\"node\"))\n",
                                "#+END_SRC\n",
                            ),
                            round = round
                        ),
                    )
                    .with_org_file("a.org", &org_headings(8, &format!("a{round}")))
                    .with_org_file("b.org", &org_headings(6, &format!("b{round}")))
                    .with_org_file("c.org", &org_headings(4, &format!("c{round}")))
                    .with_org_file("d.org", &org_headings(4, &format!("d{round}")))
                    .build(rt.clone())
                    .await
                    .expect("build");

                // Rapid mutations to maximize IVM cascade pressure
                for i in 0..5 {
                    let mut params = HashMap::new();
                    params.insert(
                        "id".to_string(),
                        Value::String(format!("block:q-{round}::render::0")),
                    );
                    params.insert("field".to_string(), Value::String("content".to_string()));
                    params.insert(
                        "value".to_string(),
                        Value::String(format!(
                            "list(#{{item_template: row(text(col(\"content-{i}\")))}})"
                        )),
                    );
                    env.execute_operation("block", "set_field", params)
                        .await
                        .map_err(|e| format!("round {round} mutation {i}: {e}"))?;
                }
                Ok::<_, String>(())
            })
        });

        match result {
            Ok(Ok(())) => eprintln!("  [round {}] OK", round + 1),
            Ok(Err(e)) => {
                eprintln!("  [round {}] ERROR: {}", round + 1, e);
                if e.contains("non-existent") || e.contains("Index points") {
                    ivm_errors += 1;
                }
            }
            Err(panic) => {
                let msg = panic
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                eprintln!("  [round {}] PANIC: {}", round + 1, msg);
                if msg.contains("Freelist") || msg.contains("dirty pages") {
                    pager_panics += 1;
                }
                if msg.contains("non-existent") || msg.contains("Index points") {
                    ivm_errors += 1;
                }
            }
        }
    }

    eprintln!(
        "\n[stress summary] {rounds} rounds: {ivm_errors} IVM index errors, {pager_panics} pager panics"
    );

    if ivm_errors > 0 || pager_panics > 0 {
        eprintln!("Turso IVM bug reproduced! See HANDOFF_TURSO_IVM_JOIN_PANIC.md");
    }
}
