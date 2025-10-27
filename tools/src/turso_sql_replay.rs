//! Turso SQL trace extractor, replayer, and minimizer.
//!
//! Combines log extraction (formerly `scripts/extract-sql-trace.py`) with
//! replay/minimize (formerly `crates/holon/examples/turso_sql_replay.rs`).
//!
//! # Subcommands
//!
//!   `extract`   Parse a HOLON_TRACE_SQL=1 log and produce a .sql replay file
//!   `replay`    Replay a .sql trace against Turso with CDC + matview checks
//!   `minimize`  Reduce a .sql trace to a minimal crash reproducer
//!   `run`       Extract from log then replay in one shot

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Context;
use chrono::NaiveDateTime;
use clap::{Parser, Subcommand};
use regex::Regex;
use turso_core::types::RelationChangeEvent;

// ─── CLI ────────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "turso-sql-replay",
    about = "Extract, replay, and minimize Turso SQL traces"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract SQL statements from a HOLON_TRACE_SQL=1 log file
    Extract(ExtractArgs),
    /// Replay a .sql trace against Turso with CDC and matview consistency checks
    Replay(ReplayArgs),
    /// Minimize a .sql trace to a minimal crash reproducer
    Minimize(MinimizeArgs),
    /// Extract from log then replay in one shot
    Run {
        #[command(flatten)]
        extract: ExtractArgs,
        #[command(flatten)]
        replay: ReplayArgs,
    },
}

#[derive(clap::Args, Clone)]
struct ExtractArgs {
    /// Path to the SQL trace log file
    logfile: String,
    /// Only include statements mentioning these tables (comma-separated)
    #[arg(long)]
    include: Option<String>,
    /// Exclude statements mentioning these tables (comma-separated)
    #[arg(long)]
    exclude: Option<String>,
    /// Only include statements after this ISO timestamp
    #[arg(long)]
    after: Option<String>,
    /// Only include statements before this ISO timestamp
    #[arg(long)]
    before: Option<String>,
    /// Stop reading the log at this line number
    #[arg(long)]
    stop_at: Option<usize>,
    /// Stop reading when a line matches this regex
    #[arg(long)]
    stop_pattern: Option<String>,
    /// Output file (stdout if omitted)
    #[arg(short, long)]
    output: Option<String>,
    /// Deduplicate DDL (keep actor_ddl, skip execute_ddl)
    #[arg(long, default_value_t = true)]
    dedup_ddl: bool,
}

#[derive(clap::Args, Clone)]
struct ReplayArgs {
    /// Path to the .sql replay file (omit when used with `run`)
    #[arg(default_value = "")]
    replay_file: String,
    /// Sleep between statements according to recorded timing
    #[arg(long)]
    replay_timing: bool,
    /// Check matview consistency after each DML statement
    #[arg(long)]
    check_after_each: bool,
    /// Start consistency checks from this statement number
    #[arg(long, default_value_t = 0)]
    check_from: usize,
}

#[derive(clap::Args)]
struct MinimizeArgs {
    /// Path to the .sql replay file
    replay_file: String,
    /// Pattern to match in crash output (auto-detected if omitted)
    #[arg(long)]
    crash_pattern: Option<String>,
    /// Output file for minimized trace
    #[arg(short, long)]
    output: Option<String>,
}

// ─── Directive (shared between extract, replay, minimize) ───────────────────

#[derive(Debug, Clone)]
enum Directive {
    SetChangeCallback,
    Wait(u64),
    Sql { tag: String, sql: String },
}

// ─── Extraction (ported from extract-sql-trace.py) ──────────────────────────

fn parse_timestamp(ts_str: &str) -> anyhow::Result<NaiveDateTime> {
    // Truncate sub-microsecond precision (>6 fractional digits)
    let truncated = if let Some(dot_idx) = ts_str.rfind('.') {
        let frac = &ts_str[dot_idx + 1..];
        if frac.len() > 6 {
            &ts_str[..dot_idx + 7]
        } else {
            ts_str
        }
    } else {
        ts_str
    };
    NaiveDateTime::parse_from_str(truncated, "%Y-%m-%dT%H:%M:%S%.f")
        .with_context(|| format!("Failed to parse timestamp: {ts_str}"))
}

fn escape_sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn inline_named_params(sql: &str, params_str: &str) -> String {
    let named_re =
        Regex::new(r#"(\w+)=(?:String\("([^"]*)"\)|Integer\((\d+)\)|Real\(([\d.]+)\)|Null)"#)
            .unwrap();

    let mut params = std::collections::HashMap::new();
    for cap in named_re.captures_iter(params_str) {
        let key = cap.get(1).unwrap().as_str();
        let value = if let Some(m) = cap.get(2) {
            escape_sql_string(m.as_str())
        } else if let Some(m) = cap.get(3) {
            m.as_str().to_string()
        } else if let Some(m) = cap.get(4) {
            m.as_str().to_string()
        } else {
            "NULL".to_string()
        };
        params.insert(key.to_string(), value);
    }

    // Handle standalone Null tokens not captured by the combined regex
    let null_re = Regex::new(r"(\w+)=Null").unwrap();
    for cap in null_re.captures_iter(params_str) {
        let key = cap.get(1).unwrap().as_str().to_string();
        params.entry(key).or_insert_with(|| "NULL".to_string());
    }

    let mut result = sql.to_string();
    for (key, value) in &params {
        result = result.replace(&format!("${key}"), value);
    }
    result
}

fn inline_positional_params(sql: &str, params_str: &str) -> String {
    let pos_re = Regex::new(
        r#"(?:Text\("((?:[^"\\]|\\.)*)"\)|Integer\((-?\d+)\)|Real\(([\d.eE+-]+)\)|Null)"#,
    )
    .unwrap();

    // Walk through params string collecting values in order, handling standalone Null
    let mut values: Vec<(String, usize)> = Vec::new();
    let mut pos = 0;
    while pos < params_str.len() {
        let remaining = &params_str[pos..];

        // Check for standalone Null before next regex match
        if let Some(null_offset) = remaining.find("Null") {
            let null_abs = pos + null_offset;
            let m = pos_re.find(remaining);

            if m.is_none() || null_offset < m.unwrap().start() {
                // Verify it's standalone (not part of a longer token)
                let before_ok =
                    null_abs == 0 || b" ,[".contains(&params_str.as_bytes()[null_abs - 1]);
                let after_ok = null_abs + 4 >= params_str.len()
                    || b" ,]".contains(&params_str.as_bytes()[null_abs + 4]);
                if before_ok && after_ok {
                    values.push(("NULL".to_string(), null_abs));
                    pos = null_abs + 4;
                    continue;
                }
            }
        }

        if let Some(cap) = pos_re.captures(remaining) {
            let full_match = cap.get(0).unwrap();
            let abs_start = pos + full_match.start();
            let value = if let Some(m) = cap.get(1) {
                escape_sql_string(&m.as_str().replace("\\\"", "\""))
            } else if let Some(m) = cap.get(2) {
                m.as_str().to_string()
            } else if let Some(m) = cap.get(3) {
                m.as_str().to_string()
            } else {
                "NULL".to_string()
            };
            values.push((value, abs_start));
            pos = pos + full_match.end();
        } else {
            break;
        }
    }

    // Sort by position
    values.sort_by_key(|(_, p)| *p);

    // Replace ? placeholders left-to-right
    let mut result = String::new();
    let mut val_idx = 0;
    for ch in sql.chars() {
        if ch == '?' && val_idx < values.len() {
            result.push_str(&values[val_idx].0);
            val_idx += 1;
        } else {
            result.push(ch);
        }
    }
    result
}

fn should_include(sql: &str, include_tables: &[String], exclude_tables: &[String]) -> bool {
    // Strip single-quoted strings to avoid matching table names in param values
    let stripped = Regex::new(r"'[^']*'").unwrap().replace_all(sql, "''");
    let lower = stripped.to_lowercase();
    if !include_tables.is_empty() {
        return include_tables
            .iter()
            .any(|t| lower.contains(&t.to_lowercase()));
    }
    if !exclude_tables.is_empty() {
        return !exclude_tables
            .iter()
            .any(|t| lower.contains(&t.to_lowercase()));
    }
    true
}

struct ExtractedTrace {
    statements: Vec<(NaiveDateTime, String, Option<String>)>, // (timestamp, tag, sql_or_none)
    source_path: String,
}

fn extract_from_log(args: &ExtractArgs) -> anyhow::Result<ExtractedTrace> {
    let ansi_re = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let trace_re = Regex::new(
        r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z\s+(?:TRACE|DEBUG|INFO)\s+holon::storage::turso:\s+\[TursoBackend\]\s+([\w]+):\s+(.*)",
    ).unwrap();
    let any_log_line_re =
        Regex::new(r#"^(?:\d{4}-\d{2}-\d{2}T|flutter:|The relevant|\[|thread ')"#).unwrap();

    let include_tables: Vec<String> = args
        .include
        .as_deref()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default();
    let exclude_tables: Vec<String> = args
        .exclude
        .as_deref()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default();
    let after_ts = args.after.as_deref().map(parse_timestamp).transpose()?;
    let before_ts = args.before.as_deref().map(parse_timestamp).transpose()?;
    let stop_pattern = args
        .stop_pattern
        .as_deref()
        .map(Regex::new)
        .transpose()
        .context("Invalid --stop-pattern regex")?;

    let content = std::fs::read_to_string(&args.logfile)
        .with_context(|| format!("Failed to read log file: {}", args.logfile))?;

    let mut lines: Vec<String> = Vec::new();
    for (line_num, raw_line) in content.lines().enumerate() {
        let clean = ansi_re.replace_all(raw_line, "").to_string();
        if let Some(stop) = args.stop_at {
            if line_num + 1 >= stop {
                break;
            }
        }
        if let Some(ref pat) = stop_pattern {
            if pat.is_match(&clean) {
                break;
            }
        }
        lines.push(clean);
    }

    let actor_tags: std::collections::HashSet<&str> = [
        "actor_ddl",
        "actor_exec",
        "actor_query",
        "actor_tx_begin",
        "actor_tx_commit",
        "execute_sql",
        "execute_via_actor",
        "transaction_stmt",
    ]
    .into_iter()
    .collect();
    let directive_tags: std::collections::HashSet<&str> =
        ["set_change_callback"].into_iter().collect();
    let skip_tags: std::collections::HashSet<&str> = if args.dedup_ddl {
        ["execute_ddl"].into_iter().collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut statements: Vec<(NaiveDateTime, String, Option<String>)> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = &lines[i];
        let Some(cap) = trace_re.captures(line) else {
            i += 1;
            continue;
        };

        let timestamp_str = cap.get(1).unwrap().as_str();
        let tag = cap.get(2).unwrap().as_str();
        let mut sql_and_params = cap.get(3).unwrap().as_str().to_string();

        i += 1;

        if skip_tags.contains(tag) {
            while i < lines.len() && !any_log_line_re.is_match(&lines[i]) {
                i += 1;
            }
            continue;
        }

        if directive_tags.contains(tag) {
            let ts = parse_timestamp(timestamp_str)?;
            if after_ts.is_some_and(|a| ts < a) {
                continue;
            }
            if before_ts.is_some_and(|b| ts > b) {
                continue;
            }
            statements.push((ts, tag.to_string(), None));
            continue;
        }

        if !actor_tags.contains(tag) {
            continue;
        }

        // Collect continuation lines (multiline DDL)
        while i < lines.len() && !any_log_line_re.is_match(&lines[i]) {
            sql_and_params.push('\n');
            sql_and_params.push_str(lines[i].trim_end());
            i += 1;
        }

        let ts = parse_timestamp(timestamp_str)?;
        if after_ts.is_some_and(|a| ts < a) {
            continue;
        }
        if before_ts.is_some_and(|b| ts > b) {
            continue;
        }

        // Split SQL from params
        let sql = if let Some(idx) = sql_and_params.find(" -- params: ") {
            let sql_part = sql_and_params[..idx].trim().to_string();
            let params_str = &sql_and_params[idx + " -- params: ".len()..];

            if params_str.starts_with('[') {
                inline_positional_params(&sql_part, params_str)
            } else {
                inline_named_params(&sql_part, params_str)
            }
        } else {
            sql_and_params.trim().to_string()
        };

        // Collapse internal blank lines
        let sql: String = sql
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        if !should_include(&sql, &include_tables, &exclude_tables) {
            continue;
        }

        statements.push((ts, tag.to_string(), Some(sql)));
    }

    Ok(ExtractedTrace {
        statements,
        source_path: args.logfile.clone(),
    })
}

fn write_extracted_trace(trace: &ExtractedTrace, writer: &mut dyn Write) -> anyhow::Result<()> {
    writeln!(writer, "-- Extracted from: {}", trace.source_path)?;
    writeln!(writer, "-- Statements: {}", trace.statements.len())?;
    if !trace.statements.is_empty() {
        writeln!(
            writer,
            "-- Time range: {}Z .. {}Z",
            trace
                .statements
                .first()
                .unwrap()
                .0
                .format("%Y-%m-%dT%H:%M:%S%.6f"),
            trace
                .statements
                .last()
                .unwrap()
                .0
                .format("%Y-%m-%dT%H:%M:%S%.6f"),
        )?;
    }
    writeln!(writer)?;

    let mut prev_ts: Option<NaiveDateTime> = None;
    for (ts, tag, sql) in &trace.statements {
        if let Some(prev) = prev_ts {
            let delta_ms = (*ts - prev).num_milliseconds();
            if delta_ms >= 1 {
                writeln!(writer, "-- Wait {delta_ms}ms")?;
            }
        }
        if sql.is_none() {
            // Directive tag
            let directive_name = tag.to_uppercase();
            writeln!(
                writer,
                "-- !{directive_name} {}Z",
                ts.format("%Y-%m-%dT%H:%M:%S%.6f")
            )?;
        } else {
            writeln!(writer, "-- [{tag}] {}Z", ts.format("%Y-%m-%dT%H:%M:%S%.6f"))?;
            writeln!(writer, "{};", sql.as_deref().unwrap())?;
        }
        writeln!(writer)?;
        prev_ts = Some(*ts);
    }
    Ok(())
}

fn cmd_extract(args: &ExtractArgs) -> anyhow::Result<Option<String>> {
    let trace = extract_from_log(args)?;

    match &args.output {
        Some(path) => {
            let mut f = std::fs::File::create(path)
                .with_context(|| format!("Failed to create output file: {path}"))?;
            write_extracted_trace(&trace, &mut f)?;
            eprintln!("Extracted {} statements to {path}", trace.statements.len());
            Ok(Some(path.clone()))
        }
        None => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            write_extracted_trace(&trace, &mut handle)?;
            Ok(None)
        }
    }
}

// ─── Replay ─────────────────────────────────────────────────────────────────────

fn parse_replay_file(path: &str) -> anyhow::Result<Vec<Directive>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read replay file: {path}"))?;
    let mut directives = Vec::new();
    let mut current_sql: Option<(String, String)> = None; // (tag, accumulated_sql)

    for line in content.lines() {
        if line.starts_with("-- !SET_CHANGE_CALLBACK") {
            if let Some((tag, sql)) = current_sql.take() {
                directives.push(Directive::Sql { tag, sql });
            }
            directives.push(Directive::SetChangeCallback);
            continue;
        }

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

        if line.starts_with("--") || line.is_empty() {
            continue;
        }

        if let Some((_, ref mut sql)) = current_sql {
            let line_content = line.strip_suffix(';').unwrap_or(line);
            if !sql.is_empty() {
                sql.push('\n');
            }
            sql.push_str(line_content);

            if line.ends_with(';') {
                let (tag, sql) = current_sql.take().unwrap();
                directives.push(Directive::Sql { tag, sql });
            }
        }
    }

    if let Some((tag, sql)) = current_sql.take() {
        if !sql.trim().is_empty() {
            directives.push(Directive::Sql { tag, sql });
        }
    }

    Ok(directives)
}

fn extract_matview_names(executed_sqls: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for sql in executed_sqls {
        let trimmed = sql.trim();
        let upper = trimmed.to_uppercase();
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

async fn get_matview_sql(conn: &turso::Connection, name: &str) -> anyhow::Result<Option<String>> {
    let sql = format!("SELECT sql FROM sqlite_master WHERE type='view' AND name='{name}'");
    let mut rows = conn.query(&sql, ()).await?;
    if let Some(row) = rows.next().await? {
        let sql: String = row.get(0)?;
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

        let mv_count = match count_rows(conn, &format!("SELECT count(*) FROM {name}")).await {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("[{step_label}] ERROR querying matview {name}: {e}");
                println!("!!! {msg}");
                inconsistencies.push(msg);
                continue;
            }
        };

        let raw_count = match count_rows(conn, &format!("SELECT count(*) FROM ({select_sql})"))
            .await
        {
            Ok(c) => c,
            Err(_) => {
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

async fn cmd_replay(args: &ReplayArgs, replay_file: &str) -> anyhow::Result<()> {
    println!("=== Turso SQL Replay with CDC ===\n");
    println!("  File: {replay_file}");
    println!("  Replay timing: {}", args.replay_timing);
    println!("  Check after each: {}", args.check_after_each);

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
                if args.replay_timing {
                    tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
                }
            }
            Directive::Sql { tag, sql } => {
                stmt_idx += 1;

                // Handle transaction boundaries faithfully
                if tag == "actor_tx_begin" {
                    let sql_preview: String = sql.chars().take(100).collect();
                    println!(
                        "[{stmt_idx}/{sql_count}] [{tag}] {sql_preview}  (CDC: {})",
                        cdc_count.load(Ordering::SeqCst)
                    );
                    match conn.execute("BEGIN TRANSACTION", ()).await {
                        Ok(_) => {
                            executed_sqls.push("BEGIN TRANSACTION".to_string());
                        }
                        Err(e) => {
                            println!("!!! [{stmt_idx}] BEGIN TRANSACTION failed: {e}");
                        }
                    }
                    continue;
                }
                if tag == "actor_tx_commit" {
                    println!(
                        "[{stmt_idx}/{sql_count}] [{tag}] COMMIT  (CDC: {})",
                        cdc_count.load(Ordering::SeqCst)
                    );
                    match conn.execute("COMMIT", ()).await {
                        Ok(_) => {
                            executed_sqls.push("COMMIT".to_string());
                        }
                        Err(e) => {
                            println!("!!! [{stmt_idx}] COMMIT failed: {e}");
                        }
                    }
                    continue;
                }

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

                if stmt_idx % 500 == 0 || stmt_idx >= args.check_from.saturating_sub(10) {
                    let sql_preview: String = sql.chars().take(100).collect();
                    println!(
                        "[{stmt_idx}/{sql_count}] [{tag}] {sql_preview}  (CDC: {})",
                        cdc_count.load(Ordering::SeqCst)
                    );
                }

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

                if args.check_after_each && is_dml && stmt_idx >= args.check_from {
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

    println!("\n=== Final Consistency Check ===");
    let matview_names = extract_matview_names(&executed_sqls);
    println!("Active matviews: {:?}", matview_names);

    let final_result = check_matview_consistency(&conn, &matview_names, "FINAL").await?;
    let has_data_mismatch = has_data_mismatch || final_result.has_data_mismatch;
    all_inconsistencies.extend(final_result.inconsistencies);

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

// ─── Minimizer ──────────────────────────────────────────────────────────────────

fn crashes_with_subprocess(directives: &[Directive], crash_pattern: &str) -> bool {
    let tmp_path = "/tmp/turso-minimize-candidate.sql";
    write_directives(tmp_path, directives).unwrap();

    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .arg("replay")
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

fn detect_crash_pattern(directives: &[Directive]) -> String {
    let tmp_path = "/tmp/turso-minimize-detect.sql";
    write_directives(tmp_path, directives).unwrap();

    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .arg("replay")
        .arg(tmp_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("Failed to run subprocess for crash detection");

    let combined = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);

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

    if combined.contains("current_page=-1") {
        return "current_page=-1 is negative".to_string();
    }

    panic!("Could not detect crash pattern from full replay! Output:\n{combined}");
}

fn extract_table_name(sql: &str) -> Option<String> {
    let upper = sql.trim().to_uppercase();
    for prefix in [
        "INSERT OR REPLACE INTO ",
        "INSERT INTO ",
        "DELETE FROM ",
        "UPDATE ",
    ] {
        if upper.starts_with(prefix) {
            let rest = sql.trim()[prefix.len()..].trim_start();
            let name = rest
                .split(|c: char| c.is_whitespace() || c == '(' || c == '"')
                .next()?;
            return Some(name.to_string());
        }
    }
    None
}

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

fn write_directives(path: &str, directives: &[Directive]) -> anyhow::Result<()> {
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

fn cmd_minimize(args: &MinimizeArgs) -> anyhow::Result<()> {
    let directives = parse_replay_file(&args.replay_file)?;
    let output_path = args
        .output
        .clone()
        .unwrap_or_else(|| args.replay_file.replace(".sql", "-minimal.sql"));
    let crash_pattern = match &args.crash_pattern {
        Some(p) => p.clone(),
        None => detect_crash_pattern(&directives),
    };

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

    // Phase 2: Remove entire table groups at once
    println!("--- Phase 2: Remove table groups ---");
    let mut removed = 0;
    {
        let mut table_groups: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for (i, d) in kept.iter().enumerate() {
            if let Directive::Sql { sql, .. } = d {
                let upper = sql.trim().to_uppercase();
                let table = extract_table_name(sql).or_else(|| {
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

        let mut groups: Vec<(String, Vec<usize>)> = table_groups.into_iter().collect();
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        println!("  Found {} table groups", groups.len());
        for (table, indices) in &groups {
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

    // Phase 3: ddmin-style chunk removal
    println!("--- Phase 3: Chunk removal (ddmin) ---");
    let phase3_start = removed;
    {
        let mut chunk_size = kept.len() / 2;
        while chunk_size >= 1 {
            let mut offset = 0;
            let mut any_removed = false;
            while offset < kept.len().saturating_sub(1) {
                let end = (offset + chunk_size).min(kept.len() - 1);
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
                } else {
                    println!("needed");
                    offset += chunk_size;
                }
            }
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

    // Phase 4: Remove individual DML statements
    println!("--- Phase 4: Remove individual DML statements ---");
    let phase4_start = removed;
    let mut i = kept.len().saturating_sub(2);
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

    // Phase 5: Remove individual DDL statements
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

    // Phase 6: Final cleanup
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

    write_directives(&output_path, &kept)?;
    println!("Written to: {output_path}");

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

// ─── Main ───────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Extract(args) => {
            cmd_extract(&args)?;
        }
        Command::Replay(args) => {
            anyhow::ensure!(!args.replay_file.is_empty(), "replay_file is required");
            cmd_replay(&args, &args.replay_file).await?;
        }
        Command::Minimize(args) => {
            cmd_minimize(&args)?;
        }
        Command::Run {
            extract,
            mut replay,
        } => {
            // Extract to a temp file, then replay it
            let tmp = tempfile::NamedTempFile::with_suffix(".sql")?;
            let tmp_path = tmp.path().to_str().unwrap().to_string();

            let extract_with_output = ExtractArgs {
                output: Some(tmp_path.clone()),
                ..extract
            };
            cmd_extract(&extract_with_output)?;

            replay.replay_file = tmp_path;
            cmd_replay(&replay, &replay.replay_file).await?;
        }
    }

    Ok(())
}
