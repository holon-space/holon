//! Replay a .sql trace file against Turso with CDC callback support.
//!
//! This replayer understands the annotated .sql format produced by
//! `scripts/extract-sql-trace.py` and faithfully reproduces the production
//! environment by registering `set_change_callback` when directed.
//!
//! Directives (special comments parsed by the replayer):
//!   `-- !SET_CHANGE_CALLBACK <timestamp>`  Register CDC change callback
//!   `-- Wait <N>ms`                        Sleep between statements (with --replay-timing)
//!   `-- [tag] <timestamp>`                 Statement metadata (informational)
//!
//! After each DML statement, the replayer checks all materialized views for
//! consistency by comparing matview contents against raw SQL re-evaluation.
//!
//! Usage:
//!   cargo run --example turso_sql_replay -- /tmp/replay.sql
//!   cargo run --example turso_sql_replay -- /tmp/replay.sql --replay-timing
//!   cargo run --example turso_sql_replay -- /tmp/replay.sql --check-after-each

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use turso_core::types::RelationChangeEvent;

#[derive(Debug)]
enum Directive {
    SetChangeCallback,
    Wait(u64),
    Sql { tag: String, sql: String },
}

fn parse_replay_file(path: &str) -> anyhow::Result<Vec<Directive>> {
    let content = std::fs::read_to_string(path)?;
    let mut directives = Vec::new();
    let mut current_sql: Option<(String, String)> = None; // (tag, accumulated_sql)

    for line in content.lines() {
        // CDC callback directive
        if line.starts_with("-- !SET_CHANGE_CALLBACK") {
            if let Some((tag, sql)) = current_sql.take() {
                directives.push(Directive::Sql { tag, sql });
            }
            directives.push(Directive::SetChangeCallback);
            continue;
        }

        // Wait directive
        if let Some(rest) = line.strip_prefix("-- Wait ") {
            if let Some((tag, sql)) = current_sql.take() {
                directives.push(Directive::Sql { tag, sql });
            }
            if let Some(ms_str) = rest.strip_suffix("ms") {
                if let Ok(ms) = ms_str.parse::<u64>() {
                    directives.push(Directive::Wait(ms));
                }
            }
            continue;
        }

        // Tag comment (starts a new statement)
        if line.starts_with("-- [") {
            if let Some((tag, sql)) = current_sql.take() {
                directives.push(Directive::Sql { tag, sql });
            }
            let tag = line
                .trim_start_matches("-- [")
                .split(']')
                .next()
                .unwrap_or("unknown")
                .to_string();
            current_sql = Some((tag, String::new()));
            continue;
        }

        // Other comments (header, extracted-from, etc.) — skip
        if line.starts_with("--") || line.is_empty() {
            continue;
        }

        // SQL content line
        if let Some((_, ref mut sql)) = current_sql {
            let line_content = line.strip_suffix(';').unwrap_or(line);
            if !sql.is_empty() {
                sql.push('\n');
            }
            sql.push_str(line_content);

            // If line ends with `;`, this statement is complete
            if line.ends_with(';') {
                let (tag, sql) = current_sql.take().unwrap();
                directives.push(Directive::Sql { tag, sql });
            }
        }
    }

    // Flush any remaining SQL
    if let Some((tag, sql)) = current_sql.take() {
        if !sql.trim().is_empty() {
            directives.push(Directive::Sql { tag, sql });
        }
    }

    Ok(directives)
}

/// Collect all materialized view names from the directives we've executed so far
fn extract_matview_names(executed_sqls: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for sql in executed_sqls {
        let trimmed = sql.trim();
        let upper = trimmed.to_uppercase();
        // Only match statements that START with CREATE/DROP — not ones containing
        // these keywords inside string literals (e.g., INSERT ... VALUES ('...'))
        if upper.starts_with("CREATE MATERIALIZED VIEW") {
            let after_view = &trimmed["CREATE MATERIALIZED VIEW".len()..];
            let rest = after_view.trim_start();
            let rest = if rest.to_uppercase().starts_with("IF NOT EXISTS") {
                rest["IF NOT EXISTS".len()..].trim_start()
            } else {
                rest
            };
            if let Some(name) = rest.split_whitespace().next() {
                names.push(name.to_string());
            }
        } else if upper.starts_with("DROP VIEW") || upper.starts_with("DROP MATERIALIZED VIEW") {
            let after = if upper.starts_with("DROP MATERIALIZED VIEW") {
                &trimmed["DROP MATERIALIZED VIEW".len()..]
            } else {
                &trimmed["DROP VIEW".len()..]
            };
            let rest = after.trim_start();
            let rest = if rest.to_uppercase().starts_with("IF EXISTS") {
                rest["IF EXISTS".len()..].trim_start()
            } else {
                rest
            };
            if let Some(name) = rest.split_whitespace().next() {
                let name = name.trim_end_matches(';');
                names.retain(|n| n != name);
            }
        }
    }
    names
}

/// Get the CREATE statement for a matview by querying sqlite_master
async fn get_matview_sql(conn: &turso::Connection, name: &str) -> anyhow::Result<Option<String>> {
    let sql = format!("SELECT sql FROM sqlite_master WHERE type='view' AND name='{name}'");
    let mut rows = conn.query(&sql, ()).await?;
    if let Some(row) = rows.next().await? {
        let sql: String = row.get(0)?;
        // Strip the CREATE MATERIALIZED VIEW ... AS prefix to get the SELECT
        let upper = sql.to_uppercase();
        if let Some(pos) = upper.find(" AS ") {
            Ok(Some(sql[pos + 4..].to_string()))
        } else {
            Ok(Some(sql))
        }
    } else {
        Ok(None)
    }
}

struct ConsistencyResult {
    inconsistencies: Vec<String>,
    has_data_mismatch: bool,
}

/// Check all matviews for consistency: compare matview rows to raw SQL re-evaluation
async fn check_matview_consistency(
    conn: &turso::Connection,
    matview_names: &[String],
    step_label: &str,
) -> anyhow::Result<ConsistencyResult> {
    let mut inconsistencies = Vec::new();
    let mut has_data_mismatch = false;

    for name in matview_names {
        let select_sql = match get_matview_sql(conn, name).await? {
            Some(sql) => sql,
            None => continue,
        };

        // Count rows in matview
        let mv_count = match count_rows(conn, &format!("SELECT count(*) FROM {name}")).await {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("[{step_label}] ERROR querying matview {name}: {e}");
                println!("!!! {msg}");
                inconsistencies.push(msg);
                continue;
            }
        };

        // Try raw SQL re-evaluation first; if it fails (e.g. recursive CTEs
        // aren't supported for SELECT), create a fresh matview copy instead.
        let raw_count = match count_rows(conn, &format!("SELECT count(*) FROM ({select_sql})"))
            .await
        {
            Ok(c) => c,
            Err(_) => {
                // Fallback: create a temporary matview from the same SQL and compare
                let tmp_name = format!("_check_{name}");
                let create_sql =
                    format!("CREATE MATERIALIZED VIEW IF NOT EXISTS {tmp_name} AS {select_sql}");
                match conn.execute(&create_sql, ()).await {
                    Ok(_) => {
                        let c = count_rows(conn, &format!("SELECT count(*) FROM {tmp_name}"))
                            .await
                            .unwrap_or(-1);
                        let _ = conn
                            .execute(&format!("DROP VIEW IF EXISTS {tmp_name}"), ())
                            .await;
                        c
                    }
                    Err(e) => {
                        let msg =
                            format!("[{step_label}] ERROR re-evaluating {name} (both paths): {e}");
                        println!("!!! {msg}");
                        inconsistencies.push(msg);
                        continue;
                    }
                }
            }
        };

        if mv_count != raw_count {
            let msg = format!(
                "[{step_label}] INCONSISTENCY in {name}: matview={mv_count} rows, raw={raw_count} rows"
            );
            println!("!!! {msg}");
            inconsistencies.push(msg);
            has_data_mismatch = true;

            // Show sample rows from matview (best-effort)
            let _ = print_query(
                conn,
                &format!("{name} (matview)"),
                &format!("SELECT * FROM {name}"),
            )
            .await;
        }
    }

    Ok(ConsistencyResult {
        inconsistencies,
        has_data_mismatch,
    })
}

async fn count_rows(conn: &turso::Connection, sql: &str) -> anyhow::Result<i64> {
    let mut rows = conn.query(sql, ()).await?;
    if let Some(row) = rows.next().await? {
        Ok(row.get(0)?)
    } else {
        Ok(0)
    }
}

async fn print_query(conn: &turso::Connection, label: &str, sql: &str) -> anyhow::Result<()> {
    println!("  {label}:");
    let mut rows = conn.query(sql, ()).await?;
    let mut row_count = 0;
    while let Some(row) = rows.next().await? {
        let mut parts = Vec::new();
        for i in 0..20 {
            match row.get::<String>(i) {
                Ok(val) => parts.push(val),
                Err(_) => break,
            }
        }
        println!("    {}", parts.join(" | "));
        row_count += 1;
        if row_count >= 50 {
            println!("    ... (truncated)");
            break;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage: {} <replay.sql> [--replay-timing] [--check-after-each] [--check-from N]",
            args[0]
        );
        std::process::exit(1);
    }

    let replay_file = &args[1];
    let replay_timing = args.contains(&"--replay-timing".to_string());
    let check_after_each = args.contains(&"--check-after-each".to_string());
    let check_from: usize = args
        .iter()
        .position(|a| a == "--check-from")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    println!("=== Turso SQL Replay with CDC ===\n");
    println!("  File: {replay_file}");
    println!("  Replay timing: {replay_timing}");
    println!("  Check after each: {check_after_each}");

    // Parse the replay file
    let directives = parse_replay_file(replay_file)?;
    let sql_count = directives
        .iter()
        .filter(|d| matches!(d, Directive::Sql { .. }))
        .count();
    let has_cdc = directives
        .iter()
        .any(|d| matches!(d, Directive::SetChangeCallback));
    println!("  SQL statements: {sql_count}");
    println!("  CDC callback: {has_cdc}\n");

    // Create fresh database
    let db_path = "/tmp/turso-sql-replay.db";
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    let cdc_count = Arc::new(AtomicUsize::new(0));
    let mut cdc_registered = false;
    let mut executed_sqls: Vec<String> = Vec::new();
    let mut all_inconsistencies: Vec<String> = Vec::new();
    let mut has_data_mismatch = false;
    let mut stmt_idx = 0;

    for directive in &directives {
        match directive {
            Directive::SetChangeCallback => {
                if cdc_registered {
                    println!("[CDC] Callback already registered, skipping duplicate");
                    continue;
                }
                println!("[CDC] Registering set_change_callback...");
                let cdc_count_clone = cdc_count.clone();
                conn.set_change_callback(move |event: &RelationChangeEvent| {
                    let prev = cdc_count_clone.fetch_add(1, Ordering::SeqCst);
                    if (prev + 1) % 100 == 0 {
                        println!(
                            "  [CDC] event #{}: {} changes to {}",
                            prev + 1,
                            event.changes.len(),
                            event.relation_name
                        );
                    }
                })?;
                cdc_registered = true;
                println!("[CDC] Callback registered");
            }
            Directive::Wait(ms) => {
                if replay_timing {
                    tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
                }
            }
            Directive::Sql { tag, sql } => {
                stmt_idx += 1;
                let is_dml = {
                    let upper = sql.trim().to_uppercase();
                    upper.starts_with("INSERT ")
                        || upper.starts_with("UPDATE ")
                        || upper.starts_with("DELETE ")
                        || upper.starts_with("REPLACE ")
                };
                let is_query = {
                    let upper = sql.trim().to_uppercase();
                    upper.starts_with("SELECT ") || upper.starts_with("WITH ")
                };

                if stmt_idx % 500 == 0 || stmt_idx >= check_from.saturating_sub(10) {
                    let sql_preview: String = sql.chars().take(100).collect();
                    println!(
                        "[{stmt_idx}/{sql_count}] [{tag}] {sql_preview}  (CDC: {})",
                        cdc_count.load(Ordering::SeqCst)
                    );
                }

                let result = if is_query {
                    // SELECT/WITH statements must use query() and drain rows
                    match conn.query(sql, ()).await {
                        Ok(mut rows) => {
                            while let Ok(Some(_)) = rows.next().await {}
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    conn.execute(sql, ()).await.map(|_| ())
                };

                match result {
                    Ok(_) => {
                        executed_sqls.push(sql.clone());
                    }
                    Err(e) => {
                        let sql_preview: String = sql.chars().take(120).collect();
                        println!(
                            "!!! [{stmt_idx}] ERROR in [{tag}]: {e}\n    SQL: {sql_preview}..."
                        );
                    }
                }

                // Check matview consistency after DML if requested
                if check_after_each && is_dml && stmt_idx >= check_from {
                    let matview_names = extract_matview_names(&executed_sqls);
                    if !matview_names.is_empty() {
                        let label = format!("stmt#{stmt_idx}");
                        let result =
                            check_matview_consistency(&conn, &matview_names, &label).await?;
                        if result.has_data_mismatch {
                            has_data_mismatch = true;
                            all_inconsistencies.extend(result.inconsistencies);
                            println!("\n=== BUG REPRODUCED at statement {stmt_idx}! ===");
                            let sql_preview: String = sql.chars().take(200).collect();
                            println!("    SQL: {sql_preview}");
                            println!();
                            break;
                        }
                    }
                }
            }
        }
    }

    // Final consistency check
    println!("\n=== Final Consistency Check ===");
    let matview_names = extract_matview_names(&executed_sqls);
    println!("Active matviews: {:?}", matview_names);

    let final_result = check_matview_consistency(&conn, &matview_names, "FINAL").await?;
    let has_data_mismatch = has_data_mismatch || final_result.has_data_mismatch;
    all_inconsistencies.extend(final_result.inconsistencies);

    // Summary
    println!("\n=== SUMMARY ===");
    println!("  Statements executed: {stmt_idx}");
    println!("  CDC events total: {}", cdc_count.load(Ordering::SeqCst));
    println!("  Issues found: {}", all_inconsistencies.len());

    if has_data_mismatch {
        println!("\n  VERDICT: IVM BUG REPRODUCED!");
        for issue in &all_inconsistencies {
            println!("    - {issue}");
        }
        std::process::exit(1);
    } else if all_inconsistencies.is_empty() {
        println!("\n  VERDICT: No IVM inconsistencies detected.");
    } else {
        println!("\n  VERDICT: No data mismatches (query errors only).");
        for issue in &all_inconsistencies {
            println!("    - {issue}");
        }
    }

    Ok(())
}
