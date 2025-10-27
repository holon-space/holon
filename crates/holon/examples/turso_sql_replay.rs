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
//!   cargo run --example turso_sql_replay -- /tmp/replay.sql --minimize

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use turso_core::types::RelationChangeEvent;

// ─── Minimizer ───────────────────────────────────────────────────────────────

/// Try replaying a subset of directives; returns true if it panics/crashes.
async fn try_replay(directives: &[Directive]) -> bool {
    let db_path = "/tmp/turso-sql-replay-minimize.db";
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    let db = match turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await
    {
        Ok(db) => db,
        Err(_) => return false,
    };
    let conn = match db.connect() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let cdc_count = Arc::new(AtomicUsize::new(0));
    let mut cdc_registered = false;

    for directive in directives {
        match directive {
            Directive::SetChangeCallback => {
                if cdc_registered {
                    continue;
                }
                let cdc_count_clone = cdc_count.clone();
                if conn
                    .set_change_callback(move |_event: &RelationChangeEvent| {
                        cdc_count_clone.fetch_add(1, Ordering::SeqCst);
                    })
                    .is_err()
                {
                    return false;
                }
                cdc_registered = true;
            }
            Directive::Wait(_) => {}
            Directive::Sql { sql, .. } => {
                let upper = sql.trim().to_uppercase();
                let is_query = upper.starts_with("SELECT ") || upper.starts_with("WITH ");

                let result = if is_query {
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

                if result.is_err() {
                    return false;
                }
            }
        }
    }

    false // completed without panic
}

/// Test if a set of directives triggers the SPECIFIC crash we're minimizing for.
/// Only checks for the exact panic pattern — SQL errors (e.g. "no such table")
/// are irrelevant since the replayer continues past them.
fn crashes_with_subprocess(directives: &[Directive], crash_pattern: &str) -> bool {
    let tmp_path = "/tmp/turso-minimize-candidate.sql";
    write_directives(tmp_path, directives).unwrap();

    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .arg(tmp_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            stdout.contains(crash_pattern) || stderr.contains(crash_pattern)
        }
        Err(_) => false,
    }
}

/// Classify whether a directive is structural (DDL/schema) or data (DML/queries).
/// Structural directives are never removed during minimization.
fn is_structural(d: &Directive) -> bool {
    match d {
        Directive::SetChangeCallback => true,
        Directive::Wait(_) => false,
        Directive::Sql { sql, .. } => {
            let upper = sql.trim().to_uppercase();
            upper.starts_with("CREATE ")
                || upper.starts_with("DROP ")
                || upper.starts_with("ALTER ")
        }
    }
}

/// Run the full replay and extract the panic message to use as the crash pattern.
fn detect_crash_pattern(directives: &[Directive]) -> String {
    let tmp_path = "/tmp/turso-minimize-detect.sql";
    write_directives(tmp_path, directives).unwrap();

    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .arg(tmp_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("Failed to run subprocess for crash detection");

    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);

    // Extract the panic message itself (the line AFTER "panicked at ...").
    // Rust panics look like:
    //   thread 'main' (12345) panicked at /path/to/file.rs:123:45:
    //   [PageStack::current] current_page=-1 is negative! ...
    // The thread ID changes per run, so we use only the message line.
    for (i, line) in combined.lines().enumerate() {
        if line.contains("panicked at") {
            if let Some(next_line) = combined.lines().nth(i + 1) {
                let pattern = next_line.trim().to_string();
                if !pattern.is_empty() {
                    println!("  Detected crash pattern: {pattern}");
                    return pattern;
                }
            }
        }
    }

    // Fallback: use a known pattern
    if combined.contains("current_page=-1") {
        return "current_page=-1 is negative".to_string();
    }

    panic!("Could not detect crash pattern from full replay! Output:\n{combined}");
}

/// Extract the primary table/view name from a SQL statement (for dependency tracking).
fn extract_table_name(sql: &str) -> Option<String> {
    let upper = sql.trim().to_uppercase();
    // INSERT INTO / INSERT OR REPLACE INTO / UPDATE ... SET / DELETE FROM
    for prefix in [
        "INSERT OR REPLACE INTO ",
        "INSERT INTO ",
        "DELETE FROM ",
        "UPDATE ",
    ] {
        if upper.starts_with(prefix) {
            let rest = sql.trim()[prefix.len()..].trim_start();
            // Table name is next token (may be quoted or have parens)
            let name = rest
                .split(|c: char| c.is_whitespace() || c == '(' || c == '"')
                .next()?;
            return Some(name.to_string());
        }
    }
    None
}

/// Try removing a single directive. Returns the reduced vec if the crash still reproduces.
fn try_remove(kept: &[Directive], i: usize, crash_pattern: &str) -> Option<Vec<Directive>> {
    let mut candidate: Vec<Directive> = Vec::with_capacity(kept.len() - 1);
    candidate.extend(kept[..i].iter().cloned());
    candidate.extend(kept[i + 1..].iter().cloned());

    if crashes_with_subprocess(&candidate, crash_pattern) {
        Some(candidate)
    } else {
        None
    }
}

/// Try removing all directives at the given indices at once.
fn try_remove_batch(
    kept: &[Directive],
    indices: &[usize],
    crash_pattern: &str,
) -> Option<Vec<Directive>> {
    let skip: std::collections::HashSet<usize> = indices.iter().copied().collect();
    let candidate: Vec<Directive> = kept
        .iter()
        .enumerate()
        .filter(|(i, _)| !skip.contains(i))
        .map(|(_, d)| d.clone())
        .collect();

    if crashes_with_subprocess(&candidate, crash_pattern) {
        Some(candidate)
    } else {
        None
    }
}

fn directive_preview(d: &Directive) -> String {
    match d {
        Directive::Sql { sql, .. } => sql.chars().take(70).collect(),
        other => format!("{other:?}"),
    }
}

fn minimize(
    directives: Vec<Directive>,
    output_path: &str,
    crash_pattern: &str,
) -> anyhow::Result<()> {
    let total = directives.len();
    println!("=== Minimizer: {total} directives ===");
    println!("  Crash pattern: {crash_pattern}\n");

    // Phase 1: Binary search for minimal prefix
    println!("--- Phase 1: Find minimal prefix ---");
    let mut lo: usize = 1;
    let mut hi: usize = total;

    while lo < hi {
        let mid = (lo + hi) / 2;
        print!("  Prefix [{mid}/{total}]... ");
        if crashes_with_subprocess(&directives[..mid], &crash_pattern) {
            println!("CRASHES");
            hi = mid;
        } else {
            println!("ok");
            lo = mid + 1;
        }
    }

    let mut kept: Vec<Directive> = directives[..lo].to_vec();
    println!("Minimal prefix: {lo} directives\n");

    // Phase 2: Remove entire table groups at once.
    // Group all directives by the table they reference. Try removing all directives
    // for each table (DDL + DML together). This is the fastest way to eliminate
    // unrelated tables — one subprocess call per table instead of per statement.
    println!("--- Phase 2: Remove table groups ---");
    let mut removed = 0;
    {
        // Build table → indices mapping
        let mut table_groups: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, d) in kept.iter().enumerate() {
            if let Directive::Sql { sql, .. } = d {
                let upper = sql.trim().to_uppercase();
                // Extract table name from any statement type
                let table = extract_table_name(sql).or_else(|| {
                    // Also handle CREATE TABLE/INDEX/VIEW
                    for kw in [
                        "CREATE MATERIALIZED VIEW IF NOT EXISTS ",
                        "CREATE MATERIALIZED VIEW ",
                        "CREATE TABLE IF NOT EXISTS ",
                        "CREATE TABLE ",
                        "CREATE INDEX IF NOT EXISTS ",
                        "CREATE INDEX ",
                        "DROP VIEW IF EXISTS ",
                        "DROP VIEW ",
                        "DROP TABLE IF EXISTS ",
                        "DROP TABLE ",
                    ] {
                        if upper.starts_with(kw) {
                            let rest = sql.trim()[kw.len()..].trim_start();
                            let name = rest
                                .split(|c: char| {
                                    c.is_whitespace() || c == '(' || c == '"' || c == ';'
                                })
                                .next()
                                .filter(|s| !s.is_empty());
                            if let Some(n) = name {
                                return Some(n.to_string());
                            }
                        }
                    }
                    // SELECT queries — extract FROM table
                    if upper.starts_with("SELECT ") {
                        if let Some(from_pos) = upper.find(" FROM ") {
                            let rest = sql.trim()[from_pos + 6..].trim_start();
                            let name = rest
                                .split(|c: char| c.is_whitespace() || c == '(' || c == ',')
                                .next()
                                .filter(|s| !s.is_empty());
                            if let Some(n) = name {
                                return Some(n.to_string());
                            }
                        }
                    }
                    None
                });
                if let Some(t) = table {
                    table_groups.entry(t.to_lowercase()).or_default().push(i);
                }
            }
        }

        // Sort groups by size (largest first — biggest potential win)
        let mut groups: Vec<(String, Vec<usize>)> = table_groups.into_iter().collect();
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        println!("  Found {} table groups", groups.len());
        for (table, indices) in &groups {
            // Don't try removing groups that include the last statement (the crashing one)
            let last_idx = kept.len() - 1;
            if indices.contains(&last_idx) {
                println!(
                    "  [{table}] ({} stmts) — contains crashing stmt, skip",
                    indices.len()
                );
                continue;
            }

            print!(
                "  [{table}] ({} stmts, indices {}..{})... ",
                indices.len(),
                indices.first().unwrap(),
                indices.last().unwrap()
            );

            if let Some(candidate) = try_remove_batch(&kept, indices, &crash_pattern) {
                println!("REMOVED all {} stmts", indices.len());
                removed += indices.len();
                kept = candidate;
            } else {
                println!("needed");
            }
        }
        println!(
            "  Removed {removed} via table groups, {} remaining\n",
            kept.len()
        );
    }

    // Phase 3: ddmin-style chunk removal.
    // Try removing large chunks (halves, quarters, etc.) before individual statements.
    println!("--- Phase 3: Chunk removal (ddmin) ---");
    let phase3_start = removed;
    {
        let mut chunk_size = kept.len() / 2;
        while chunk_size >= 1 {
            let mut offset = 0;
            let mut any_removed = false;
            while offset < kept.len().saturating_sub(1) {
                let end = (offset + chunk_size).min(kept.len() - 1); // never remove last
                if offset >= end {
                    offset += chunk_size;
                    continue;
                }
                let indices: Vec<usize> = (offset..end).collect();
                let chunk_preview = if let Directive::Sql { sql, .. } = &kept[offset] {
                    let p: String = sql.chars().take(40).collect();
                    format!("{p}...")
                } else {
                    format!("{:?}", kept[offset])
                };

                print!(
                    "  chunk [{offset}..{end}) of {} (size {chunk_size}): {chunk_preview} ",
                    kept.len()
                );

                if let Some(candidate) = try_remove_batch(&kept, &indices, &crash_pattern) {
                    println!("REMOVED {} stmts", indices.len());
                    removed += indices.len();
                    kept = candidate;
                    any_removed = true;
                    // Don't advance offset — new statements slid into this position
                } else {
                    println!("needed");
                    offset += chunk_size;
                }
            }
            // ddmin: if we removed something at this chunk size, try same size again
            // otherwise halve the chunk
            if !any_removed {
                chunk_size /= 2;
            }
        }
    }
    println!(
        "  Removed {} via chunks, {} remaining\n",
        removed - phase3_start,
        kept.len()
    );

    // Phase 4: Remove individual DML statements.
    println!("--- Phase 4: Remove individual DML statements ---");
    let phase4_start = removed;
    let mut i = kept.len().saturating_sub(2); // don't remove last (crashing) stmt
    loop {
        if is_structural(&kept[i]) || matches!(kept[i], Directive::SetChangeCallback) {
            if i == 0 {
                break;
            }
            i -= 1;
            continue;
        }

        let preview = directive_preview(&kept[i]);
        print!("  [{i}/{}] remove: {preview}... ", kept.len());

        if let Some(candidate) = try_remove(&kept, i, &crash_pattern) {
            println!("REMOVED");
            kept = candidate;
            removed += 1;
        } else {
            println!("needed");
        }

        if i == 0 {
            break;
        }
        i -= 1;
    }
    println!(
        "  Removed {} DML statements, {} remaining\n",
        removed - phase4_start,
        kept.len()
    );

    // Phase 5: Remove individual DDL statements.
    println!("--- Phase 5: Remove individual DDL statements ---");
    let phase5_start = removed;
    let mut i = kept.len().saturating_sub(2);
    loop {
        if !is_structural(&kept[i]) || matches!(kept[i], Directive::SetChangeCallback) {
            if i == 0 {
                break;
            }
            i -= 1;
            continue;
        }

        let preview = directive_preview(&kept[i]);
        print!("  [{i}/{}] remove: {preview}... ", kept.len());

        // When removing DDL, also try batch-removing any remaining DML that
        // references the same table (in case Phase 2 kept them as dependencies
        // of this DDL).
        let table_name = if let Directive::Sql { sql, .. } = &kept[i] {
            extract_table_name(sql)
        } else {
            None
        };

        let batch_indices: Vec<usize> = if let Some(ref tbl) = table_name {
            let tbl_upper = tbl.to_uppercase();
            kept.iter()
                .enumerate()
                .filter(|(j, d)| {
                    *j != i
                        && if let Directive::Sql { sql, .. } = d {
                            sql.to_uppercase().contains(&tbl_upper)
                        } else {
                            false
                        }
                })
                .map(|(j, _)| j)
                .collect()
        } else {
            vec![]
        };

        let mut did_remove = false;

        // Try batch removal first (DDL + all referencing DML)
        if !batch_indices.is_empty() {
            let mut all_indices = batch_indices.clone();
            all_indices.push(i);
            if let Some(candidate) = try_remove_batch(&kept, &all_indices, &crash_pattern) {
                println!("REMOVED (+ {} dependents)", batch_indices.len());
                removed += all_indices.len();
                kept = candidate;
                did_remove = true;
            }
        }

        // Fall back to removing just the DDL
        if !did_remove {
            if let Some(candidate) = try_remove(&kept, i, &crash_pattern) {
                println!("REMOVED");
                kept = candidate;
                removed += 1;
                did_remove = true;
            } else {
                println!("needed");
            }
        }

        if i == 0 {
            break;
        }
        // After batch removal, indices shifted — be safe
        i = i.min(kept.len().saturating_sub(2));
        if !did_remove && i > 0 {
            i -= 1;
        }
    }
    println!(
        "  Removed {} DDL statements, {} remaining\n",
        removed - phase5_start,
        kept.len()
    );

    // Phase 6: Final pass — try removing ANY remaining statement (DDL or DML)
    println!("--- Phase 6: Final cleanup ---");
    let phase6_start = removed;
    let mut i = kept.len().saturating_sub(2);
    loop {
        if matches!(kept[i], Directive::SetChangeCallback) {
            if i == 0 {
                break;
            }
            i -= 1;
            continue;
        }

        let preview = directive_preview(&kept[i]);
        print!("  [{i}/{}] remove: {preview}... ", kept.len());

        if let Some(candidate) = try_remove(&kept, i, &crash_pattern) {
            println!("REMOVED");
            kept = candidate;
            removed += 1;
        } else {
            println!("needed");
        }

        if i == 0 {
            break;
        }
        i -= 1;
    }
    println!(
        "  Removed {} more statements, {} remaining\n",
        removed - phase6_start,
        kept.len()
    );

    println!(
        "=== Minimized: {} directives (removed {removed} total) ===",
        kept.len()
    );

    // Write output
    write_directives(output_path, &kept)?;
    println!("Written to: {output_path}");

    // Print the minimal reproducer
    println!("\n--- Minimal SQL reproducer ---");
    for d in &kept {
        match d {
            Directive::SetChangeCallback => println!("-- !SET_CHANGE_CALLBACK"),
            Directive::Wait(ms) => println!("-- Wait {ms}ms"),
            Directive::Sql { tag, sql } => {
                println!("-- [{tag}]");
                println!("{sql};");
                println!();
            }
        }
    }

    Ok(())
}

fn write_directives(path: &str, directives: &[Directive]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    writeln!(f, "-- Minimized replay ({} statements)", directives.len())?;
    writeln!(f)?;
    for d in directives {
        match d {
            Directive::SetChangeCallback => {
                writeln!(f, "-- !SET_CHANGE_CALLBACK")?;
                writeln!(f)?;
            }
            Directive::Wait(ms) => {
                writeln!(f, "-- Wait {ms}ms")?;
                writeln!(f)?;
            }
            Directive::Sql { tag, sql } => {
                writeln!(f, "-- [{tag}]")?;
                writeln!(f, "{sql};")?;
                writeln!(f)?;
            }
        }
    }
    Ok(())
}

// ─── Directive Clone ─────────────────────────────────────────────────────────

impl Clone for Directive {
    fn clone(&self) -> Self {
        match self {
            Self::SetChangeCallback => Self::SetChangeCallback,
            Self::Wait(ms) => Self::Wait(*ms),
            Self::Sql { tag, sql } => Self::Sql {
                tag: tag.clone(),
                sql: sql.clone(),
            },
        }
    }
}

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

            // Show the diff: rows in matview but NOT in fresh re-evaluation
            // Create a temp fresh matview to compare against
            let tmp_name = format!("_diff_check_{name}");
            let _ = conn
                .execute(
                    &format!("CREATE MATERIALIZED VIEW IF NOT EXISTS {tmp_name} AS {select_sql}"),
                    (),
                )
                .await;
            let _ = print_query(
                conn,
                &format!("{name}: EXTRA rows in matview (not in fresh)"),
                &format!("SELECT * FROM {name} EXCEPT SELECT * FROM {tmp_name}"),
            )
            .await;
            let _ = print_query(
                conn,
                &format!("{name}: MISSING rows (in fresh but not matview)"),
                &format!("SELECT * FROM {tmp_name} EXCEPT SELECT * FROM {name}"),
            )
            .await;
            let _ = conn
                .execute(&format!("DROP VIEW IF EXISTS {tmp_name}"), ())
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
            "Usage: {} <replay.sql> [--replay-timing] [--check-after-each] [--check-from N] [--minimize --crash-pattern PATTERN]",
            args[0]
        );
        std::process::exit(1);
    }

    let replay_file = &args[1];
    let do_minimize = args.contains(&"--minimize".to_string());
    let replay_timing = args.contains(&"--replay-timing".to_string());
    let check_after_each = args.contains(&"--check-after-each".to_string());
    let check_from: usize = args
        .iter()
        .position(|a| a == "--check-from")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let crash_pattern_arg: Option<String> = args
        .iter()
        .position(|a| a == "--crash-pattern")
        .and_then(|i| args.get(i + 1))
        .cloned();

    if do_minimize {
        let directives = parse_replay_file(replay_file)?;
        let output_path = replay_file.replace(".sql", "-minimal.sql");
        let crash_pattern = match crash_pattern_arg {
            Some(p) => p,
            None => detect_crash_pattern(&directives),
        };
        minimize(directives, &output_path, &crash_pattern)?;
        return Ok(());
    }

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
