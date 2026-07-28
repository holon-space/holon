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
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Context;
use chrono::NaiveDateTime;
use clap::Parser;
use clap::Subcommand;
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
    /// Replay a .sql trace against Turso with CDC and matview consistency
    /// checks
    Replay(ReplayArgs),
    /// Minimize a .sql trace to a minimal crash reproducer
    Minimize(MinimizeArgs),
    /// Split a trace into prefix + suffix around the first divergence, then
    /// verify the split reproduces the SAME failure mechanism (see the
    /// mechanism-preservation trap on `verify_mechanism_preservation`).
    Split(SplitArgs),
    /// Replay a trace N times in N fresh processes and report the red-rate.
    /// Per-process-seeded IVM bugs (std HashMap RandomState iteration order in
    /// DBSP consolidate) are deterministic within a process but vary across
    /// processes, so in-process repetition samples one seed; this sweep samples
    /// the distribution and makes A/B trigger removal cheap.
    Sweep(SweepArgs),
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
    /// Run the lightweight CHK oracle after every COMMIT: for each active
    /// matview, compare its stored row count (`SELECT count(*) FROM <view>`)
    /// against a fresh recompute of the view's defining SELECT. This is the
    /// recursive-CTE-safe oracle — the recompute runs the defining SELECT
    /// directly (never wrapped in a subquery, which fails with "Recursive CTEs
    /// are not yet supported") and counts the rows it yields. The first commit
    /// after which MV != recompute is reported as the first divergence point.
    #[arg(long)]
    check_after_commit: bool,
    /// With `--check-after-commit`, when the cheap row-count CHK agrees, also
    /// run the precise multiset compare (full EXCEPT diff via a shadow matview)
    /// so same-count-but-different-rows drift is caught too. Heavier — it
    /// creates and drops a shadow matview per view per commit.
    #[arg(long)]
    chk_precise: bool,
    /// Start consistency checks from this statement number
    #[arg(long, default_value_t = 0)]
    check_from: usize,
    /// With `--check-after-each`, keep running past the first inconsistency
    /// instead of bailing. Useful for finding *every* place a matview goes
    /// stale, not just the earliest detection point.
    #[arg(long)]
    no_break_on_inconsistency: bool,
    /// Track an `id` across one or more tables/matviews after every DML.
    /// Format: `<table>=<id>` or just `<id>` (defaults to `block_raw,block`).
    /// Repeat for multiple ids (e.g. `--track-id block:abc --track-id
    /// block:def`). On every change in presence the tool prints
    /// `[stmt_idx] track block_raw=✓ block=✗`. Use this to pinpoint the
    /// exact statement that drops a row from a chained matview.
    #[arg(long = "track-id", value_name = "[TABLE=]ID")]
    track_id: Vec<String>,
    /// Stop the replay after this many SQL statements have been processed.
    /// Used by the minimizer to cap individual candidates that aren't
    /// reproducing the crash, so they don't grind through the full trace.
    #[arg(long, value_name = "N")]
    max_stmts: Option<usize>,
    /// Stop the replay after this many wall-clock seconds. Used by the
    /// minimizer as a safety budget per candidate. The replay still runs
    /// the final consistency check on what it processed so the verdict
    /// reflects partial progress.
    #[arg(long, value_name = "SECS")]
    max_secs: Option<u64>,
    /// Replay against a copy of an existing Turso database file instead
    /// of opening a fresh one at `/tmp/turso-sql-replay.db`. The given
    /// file (plus `-wal` / `-shm` siblings) is copied to a scratch path
    /// so the original is never mutated. Use this to reproduce bugs
    /// that only occur with pre-existing on-disk state.
    #[arg(long, value_name = "PATH")]
    existing_db: Option<String>,
    /// Spawn a background reader task that repeatedly queries the given
    /// matview(s) (comma-separated) on a separate connection while the
    /// main loop is writing. Reproduces production's reactive watchers
    /// holding cursors open during DBSP consolidation — required to
    /// surface bugs that fire on async post-commit consolidation
    /// (e.g. `json_group_array multiset went negative`).
    #[arg(long, value_name = "TABLE[,TABLE...]")]
    background_read: Option<String>,
    /// Interval between background reader queries (default 5 ms).
    #[arg(long, value_name = "MS", default_value_t = 5)]
    background_read_interval_ms: u64,
    /// After every `actor_tx_commit`, sleep this many milliseconds and
    /// issue one read against each background-read target. Gives the
    /// async DBSP consolidation queued by the commit a chance to run
    /// before the next BEGIN, exposing post-commit panics. 0 disables.
    #[arg(long, value_name = "MS", default_value_t = 50)]
    post_commit_poll_ms: u64,
    /// Drive the replay through Holon's `TursoBackend` actor instead of a
    /// raw `turso::Connection`. Matches production's CDC fan-out, DDL
    /// dependency ordering, and tokio actor model — needed to surface
    /// panics that only fire under the actor's `process_actor_command`
    /// path (e.g. async post-commit `aggregate_operator` invariants).
    #[arg(long)]
    via_holon_actor: bool,
    /// Path to the scratch database this replay opens. Defaults to
    /// `/tmp/turso-sql-replay.db`. Overriding it lets independent replays
    /// (e.g. the parallel `sweep` runs, or a prefix/suffix split) each use a
    /// private on-disk file so they don't clobber each other.
    #[arg(long, value_name = "PATH")]
    db_path: Option<String>,
}

#[derive(clap::Args)]
struct SplitArgs {
    /// Path to the .sql replay file
    replay_file: String,
    /// 1-based index of the transaction to isolate as the suffix (the failing
    /// tx). When omitted, the tool auto-detects it by replaying with
    /// `--check-after-commit` and reading the first-divergence commit.
    #[arg(long, value_name = "N")]
    at: Option<usize>,
    /// Output basename; produces `<base>.prefix.sql` and `<base>.suffix.sql`.
    /// Defaults to the replay file with its `.sql` suffix stripped.
    #[arg(short, long)]
    output_prefix: Option<String>,
    /// Skip the mechanism-preservation verification (not recommended — a
    /// fresh-process suffix replay exercises the state-restore path, a
    /// DIFFERENT mechanism than the in-process divergence).
    #[arg(long)]
    skip_verify: bool,
}

#[derive(clap::Args)]
struct SweepArgs {
    /// Path to the .sql replay file
    replay_file: String,
    /// Number of fresh-process runs to sample.
    #[arg(short = 'n', long, default_value_t = 40)]
    runs: usize,
    /// Second .sql file to A/B against (e.g. the same trace with the suspected
    /// trigger feature removed). Reported as a second red-rate.
    #[arg(long, value_name = "PATH")]
    compare: Option<String>,
    /// Number of runs to execute concurrently (each in its own process with a
    /// private `--db-path`).
    #[arg(long, default_value_t = 4)]
    jobs: usize,
    /// Directory to keep artifacts (stdout + database) of red runs. Green runs
    /// are cleaned up. Defaults to `/tmp/turso-sweep`.
    #[arg(long, value_name = "DIR")]
    keep_dir: Option<String>,
    /// Extra argument to pass through to each `replay` child (repeatable).
    /// Defaults to `--check-after-commit` when none are given, so matview drift
    /// counts as red (panics are always red via the panic hook regardless).
    #[arg(long = "replay-arg", value_name = "ARG")]
    replay_arg: Vec<String>,
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
    Sql {
        tag: String,
        sql: String,
    },
    /// Self-checking assertion. Parsed from lines like
    /// `-- ?ASSERT ROW-EXISTS block "block:abc"` or
    /// `-- ?ASSERT ROW-COUNT block 17`.
    /// Failure exits non-zero; success is silent unless `verbose=true`.
    Assert(AssertKind),
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)] // Row* prefix mirrors the assertion DSL used in trace files
enum AssertKind {
    RowExists { table: String, id: String },
    RowAbsent { table: String, id: String },
    RowCount { table: String, count: i64 },
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
    // String values are Rust-Debug-escaped in the log (`\"` for quotes), so
    // the capture must allow escape pairs — mirroring inline_positional_params.
    let named_re = Regex::new(
        r#"(\w+)=(?:String\("((?:[^"\\]|\\.)*)"\)|Integer\((-?\d+)\)|Real\(([\d.eE+-]+)\)|Null)"#,
    )
    .unwrap();

    let mut params = std::collections::HashMap::new();
    for cap in named_re.captures_iter(params_str) {
        let key = cap.get(1).unwrap().as_str();
        let value = if let Some(m) = cap.get(2) {
            escape_sql_string(&m.as_str().replace("\\\"", "\""))
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

    // Walk through params string collecting values in order, handling standalone
    // Null
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
            pos += full_match.end();
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
        r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z\s+(?:TRACE|DEBUG|INFO)\s+(?:holon::storage::turso|holon_turso::turso):\s+\[TursoBackend\]\s+([\w]+):\s+(.*)",
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
        if let Some(stop) = args.stop_at
            && line_num + 1 >= stop
        {
            break;
        }
        if let Some(ref pat) = stop_pattern
            && pat.is_match(&clean)
        {
            break;
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

// ─── Replay
// ─────────────────────────────────────────────────────────────────────

/// True iff `sql` ends inside an unclosed single-quoted SQL string
/// literal. Used by the line-based parser so that `;` and `--` on lines
/// of multi-line literal *content* are not treated as statement
/// terminators or comments. SQLite's escape for a literal apostrophe is
/// `''` (a doubled single quote), so we toggle state on every `'`: an
/// even count of consecutive `'` returns to the same state, an odd count
/// flips it. Backslash is not an escape character in SQL string
/// literals, so we don't honor `\'`.
fn sql_has_open_literal(sql: &str) -> bool {
    let mut in_literal = false;
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            in_literal = !in_literal;
        }
        i += 1;
    }
    in_literal
}

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
            if let Some(ms_str) = rest.strip_suffix("ms")
                && let Ok(ms) = ms_str.parse::<u64>()
            {
                directives.push(Directive::Wait(ms));
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("-- ?ASSERT ") {
            if let Some((tag, sql)) = current_sql.take() {
                directives.push(Directive::Sql { tag, sql });
            }
            // Forms:
            //   ROW-EXISTS <table> '<id>'   (single-quoted id; quoted to allow spaces)
            //   ROW-ABSENT <table> '<id>'
            //   ROW-COUNT  <table> <n>
            // Permissive whitespace; quotes optional for ids without spaces.
            let kind = parse_assert_directive(rest)
                .with_context(|| format!("Failed to parse `-- ?ASSERT {rest}`"))?;
            directives.push(Directive::Assert(kind));
            continue;
        }

        // Inside a multi-line single-quoted SQL literal, `--` and `;` are
        // just content. We only treat them as directives/terminators when
        // we're not currently inside an open literal.
        let in_literal = current_sql
            .as_ref()
            .map(|(_, sql)| sql_has_open_literal(sql))
            .unwrap_or(false);

        if !in_literal && line.starts_with("-- [") {
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

        // Indented `--` comment lines are still comments — without the
        // trim a comment line ending in `;` (e.g. inside a CREATE TABLE
        // body) would be appended to the SQL and its `;` would terminate
        // the statement mid-body ("incomplete input").
        if !in_literal && (line.trim_start().starts_with("--") || line.trim().is_empty()) {
            continue;
        }

        if let Some((_, ref mut sql)) = current_sql {
            // Append the line first, then decide whether the appended `;`
            // terminates the statement — by inspecting the literal state
            // *after* appending, which is the only reliable way to know.
            if !sql.is_empty() {
                sql.push('\n');
            }
            sql.push_str(line);
            if line.ends_with(';') && !sql_has_open_literal(sql) {
                // Strip the terminating semicolon.
                if let Some(stripped) = sql.strip_suffix(';') {
                    let stripped = stripped.to_string();
                    let tag = current_sql.as_ref().unwrap().0.clone();
                    current_sql = None;
                    directives.push(Directive::Sql { tag, sql: stripped });
                }
            }
        }
    }

    if let Some((tag, sql)) = current_sql.take()
        && !sql.trim().is_empty()
    {
        directives.push(Directive::Sql { tag, sql });
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

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Byte offset of the next whole-word, case-insensitive `keyword` at
/// parenthesis depth 0, scanning from `start`. Matches inside single-quoted
/// string literals and inside any parenthesised subexpression (depth > 0) are
/// skipped. Used to locate the outer query's clauses when preparing a
/// recursive-CTE view for the CHK oracle: the CTE bodies live inside `(...)`
/// at depth > 0, and window `OVER (ORDER BY …)` clauses likewise, so the outer
/// SELECT / FROM / ORDER BY / LIMIT are the depth-0 hits.
fn find_kw_depth0(sql: &str, keyword: &str, start: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let kw = keyword.as_bytes();
    let mut depth: i32 = 0;
    let mut in_lit = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if in_lit {
            if c == b'\'' {
                in_lit = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' => in_lit = true,
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {
                if depth == 0
                    && i + kw.len() <= bytes.len()
                    && bytes[i..i + kw.len()].eq_ignore_ascii_case(kw)
                    && (i == 0 || !is_ident_byte(bytes[i - 1]))
                    && (i + kw.len() == bytes.len() || !is_ident_byte(bytes[i + kw.len()]))
                {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Strip a trailing top-level `ORDER BY …` and/or `LIMIT …` from a SELECT so it
/// can be replayed purely to count its rows. Removing `LIMIT` is mandatory —
/// it would otherwise cap the recompute count; `ORDER BY` is dropped for speed.
/// Clauses inside parens (subqueries, window `OVER (...)`) are at depth > 0 and
/// left untouched.
fn strip_trailing_order_limit(sql: &str) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim_end();
    let mut cut = trimmed.len();
    if let Some(p) = find_kw_depth0(trimmed, "LIMIT", 0) {
        cut = cut.min(p);
    }
    let mut search = 0;
    while let Some(p) = find_kw_depth0(trimmed, "ORDER", search) {
        let rest = trimmed[p + "ORDER".len()..].trim_start();
        if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case("BY") {
            cut = cut.min(p);
            break;
        }
        search = p + "ORDER".len();
    }
    trimmed[..cut].trim_end().to_string()
}

/// Recursive-CTE-safe row count of a view's defining SELECT. The SELECT is run
/// directly (never wrapped in `SELECT count(*) FROM (<select>)`, which fails
/// with "Recursive CTEs are not yet supported"); its rows are drained and
/// counted client-side. `ORDER BY` / `LIMIT` tails are stripped so the count
/// reflects the full result set regardless of the view's own presentation
/// clauses.
async fn chk_recompute_count(conn: &turso::Connection, select_sql: &str) -> anyhow::Result<i64> {
    let query = strip_trailing_order_limit(select_sql);
    let mut rows = conn.query(&query, ()).await?;
    let mut n = 0i64;
    while rows.next().await?.is_some() {
        n += 1;
    }
    Ok(n)
}

struct ChkOutcome {
    mismatch: bool,
    first_divergence: Option<String>,
    lines: Vec<String>,
}

/// The CHK oracle: after a COMMIT, compare each matview's stored row count
/// against a fresh recompute of its defining SELECT. Emits one
/// `CHK|commit<K>|<view>|MV=<n>|RC=<m>` line per view (a machine-parseable
/// first-line oracle); a row-count mismatch flags divergence. With `precise`,
/// agreeing counts are further checked for same-count-different-rows drift via
/// the full multiset compare.
async fn chk_after_commit(
    conn: &turso::Connection,
    matview_names: &[String],
    commit_idx: usize,
    precise: bool,
) -> ChkOutcome {
    let mut outcome = ChkOutcome {
        mismatch: false,
        first_divergence: None,
        lines: Vec::new(),
    };
    for name in matview_names {
        let mv = match count_rows(conn, &format!("SELECT count(*) FROM {name}")).await {
            Ok(n) => n,
            Err(e) => {
                outcome
                    .lines
                    .push(format!("CHK|commit{commit_idx}|{name}|ERROR MV-count: {e}"));
                continue;
            }
        };
        let select_sql = match get_matview_sql(conn, name).await {
            Ok(Some(sql)) => sql,
            Ok(None) => continue,
            Err(e) => {
                outcome
                    .lines
                    .push(format!("CHK|commit{commit_idx}|{name}|ERROR view-sql: {e}"));
                continue;
            }
        };
        let rc = match chk_recompute_count(conn, &select_sql).await {
            Ok(n) => n,
            Err(e) => {
                outcome.lines.push(format!(
                    "CHK|commit{commit_idx}|{name}|ERROR recompute: {e}"
                ));
                continue;
            }
        };
        let line = format!("CHK|commit{commit_idx}|{name}|MV={mv}|RC={rc}");
        if mv != rc {
            let full = format!("{line}  <<< MISMATCH");
            println!("!!! {full}");
            outcome.lines.push(full);
            outcome.mismatch = true;
            if outcome.first_divergence.is_none() {
                outcome.first_divergence =
                    Some(format!("commit#{commit_idx} view={name} MV={mv} RC={rc}"));
            }
        } else {
            println!("{line}");
            outcome.lines.push(line);
            if precise {
                if let Ok(res) = check_matview_consistency(
                    conn,
                    std::slice::from_ref(name),
                    &format!("commit{commit_idx}"),
                )
                .await
                    && res.has_data_mismatch
                {
                    outcome.mismatch = true;
                    if outcome.first_divergence.is_none() {
                        outcome.first_divergence =
                            Some(format!("commit#{commit_idx} view={name} multiset-drift"));
                    }
                    outcome.lines.extend(res.inconsistencies);
                }
            }
        }
    }
    outcome
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

        // Build a fresh re-evaluation matview alongside the live one. We
        // can't compare against `(<select_sql>)` directly: many holon
        // matviews chain off other matviews, and `(SELECT … FROM block …)`
        // forces the planner to re-plan the inner query without the IVM
        // engine's bookkeeping, which can fail or pick a different code
        // path. A second materialized view replays through the same
        // pipeline, so any divergence is incremental-state drift, not a
        // planner artifact.
        let tmp_name = format!("_diff_check_{name}");
        let create_sql =
            format!("CREATE MATERIALIZED VIEW IF NOT EXISTS {tmp_name} AS {select_sql}");
        if let Err(e) = conn.execute(&create_sql, ()).await {
            let msg = format!("[{step_label}] ERROR re-evaluating {name} (CREATE): {e}");
            println!("!!! {msg}");
            inconsistencies.push(msg);
            continue;
        }

        let mv_only = count_rows(
            conn,
            &format!("SELECT count(*) FROM (SELECT * FROM {name} EXCEPT SELECT * FROM {tmp_name})"),
        )
        .await;
        let src_only = count_rows(
            conn,
            &format!("SELECT count(*) FROM (SELECT * FROM {tmp_name} EXCEPT SELECT * FROM {name})"),
        )
        .await;

        match (mv_only, src_only) {
            (Ok(0), Ok(0)) => {
                // Consistent — no row-level drift.
            }
            (Ok(extra), Ok(missing)) if extra == 0 && missing == 0 => {
                // (Defensive — already covered above, kept for clarity.)
            }
            (Ok(extra), Ok(missing)) => {
                let mv_count = count_rows(conn, &format!("SELECT count(*) FROM {name}"))
                    .await
                    .unwrap_or(-1);
                let src_count = count_rows(conn, &format!("SELECT count(*) FROM {tmp_name}"))
                    .await
                    .unwrap_or(-1);
                let msg = format!(
                    "[{step_label}] INCONSISTENCY in {name}: matview={mv_count}, \
                     fresh={src_count}, extra={extra} missing={missing} rows"
                );
                println!("!!! {msg}");
                inconsistencies.push(msg);
                has_data_mismatch = true;

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
            }
            (Err(e), _) | (_, Err(e)) => {
                let msg = format!("[{step_label}] ERROR diffing {name}: {e}");
                println!("!!! {msg}");
                inconsistencies.push(msg);
            }
        }

        let _ = conn
            .execute(&format!("DROP VIEW IF EXISTS {tmp_name}"), ())
            .await;
    }

    Ok(ConsistencyResult {
        inconsistencies,
        has_data_mismatch,
    })
}

/// Tracker spec parsed from `--track-id <[TABLE=]ID>` flags.
#[derive(Debug, Clone)]
struct IdTracker {
    id: String,
    tables: Vec<String>,
}

fn parse_track_id_args(raw: &[String]) -> Vec<IdTracker> {
    raw.iter()
        .map(|spec| match spec.split_once('=') {
            Some((tables, id)) => IdTracker {
                id: id.to_string(),
                tables: tables.split(',').map(|s| s.trim().to_string()).collect(),
            },
            None => IdTracker {
                id: spec.clone(),
                tables: vec!["block_raw".into(), "block".into()],
            },
        })
        .collect()
}

/// Probe each tracker's tables for `id` presence; print a one-line
/// status whenever the presence vector changes from the previous probe.
/// Intentionally lightweight (one indexed lookup per table) so it's
/// cheap enough to run after every DML in a long replay.
async fn probe_trackers(
    conn: &turso::Connection,
    trackers: &[IdTracker],
    prev: &mut [Option<Vec<bool>>],
    stmt_idx: usize,
    sql_preview: &str,
) {
    for (tracker, prev_slot) in trackers.iter().zip(prev.iter_mut()) {
        let mut current = Vec::with_capacity(tracker.tables.len());
        for table in &tracker.tables {
            let sql = format!("SELECT 1 FROM {table} WHERE id = '{}'", tracker.id);
            let hit = match conn.query(&sql, ()).await {
                Ok(mut rows) => rows.next().await.map(|r| r.is_some()).unwrap_or(false),
                Err(_) => false,
            };
            current.push(hit);
        }
        let changed = match prev_slot {
            None => true,
            Some(prev) => prev != &current,
        };
        if changed {
            let parts: Vec<String> = tracker
                .tables
                .iter()
                .zip(current.iter())
                .map(|(t, hit)| format!("{t}={}", if *hit { "✓" } else { "✗" }))
                .collect();
            println!(
                "[{stmt_idx}] track id={} {}  (after: {sql_preview})",
                tracker.id,
                parts.join(" "),
            );
            *prev_slot = Some(current);
        }
    }
}

fn parse_assert_directive(rest: &str) -> anyhow::Result<AssertKind> {
    let rest = rest.trim();
    let (verb, tail) = rest
        .split_once(char::is_whitespace)
        .ok_or_else(|| anyhow::anyhow!("expected `<VERB> <args>`, got `{rest}`"))?;
    let tail = tail.trim();
    let (table, value) = tail
        .split_once(char::is_whitespace)
        .ok_or_else(|| anyhow::anyhow!("expected `<table> <value>`, got `{tail}`"))?;
    let table = table.trim().to_string();
    let value = value.trim();

    let strip_quotes = |s: &str| -> String {
        s.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(s)
            .to_string()
    };

    match verb.to_ascii_uppercase().as_str() {
        "ROW-EXISTS" => Ok(AssertKind::RowExists {
            table,
            id: strip_quotes(value),
        }),
        "ROW-ABSENT" => Ok(AssertKind::RowAbsent {
            table,
            id: strip_quotes(value),
        }),
        "ROW-COUNT" => {
            let count = value
                .parse::<i64>()
                .with_context(|| format!("ROW-COUNT expects integer, got `{value}`"))?;
            Ok(AssertKind::RowCount { table, count })
        }
        other => Err(anyhow::anyhow!(
            "unknown assertion verb `{other}` (expected ROW-EXISTS / ROW-ABSENT / ROW-COUNT)"
        )),
    }
}

fn describe_assertion(kind: &AssertKind) -> String {
    match kind {
        AssertKind::RowExists { table, id } => format!("ROW-EXISTS {table} '{id}'"),
        AssertKind::RowAbsent { table, id } => format!("ROW-ABSENT {table} '{id}'"),
        AssertKind::RowCount { table, count } => format!("ROW-COUNT {table} {count}"),
    }
}

async fn run_assertion(conn: &turso::Connection, kind: &AssertKind) -> anyhow::Result<bool> {
    match kind {
        AssertKind::RowExists { table, id } => {
            let sql = format!("SELECT 1 FROM {table} WHERE id = '{id}'");
            let mut rows = conn.query(&sql, ()).await?;
            Ok(rows.next().await?.is_some())
        }
        AssertKind::RowAbsent { table, id } => {
            let sql = format!("SELECT 1 FROM {table} WHERE id = '{id}'");
            let mut rows = conn.query(&sql, ()).await?;
            Ok(rows.next().await?.is_none())
        }
        AssertKind::RowCount { table, count } => {
            let actual = count_rows(conn, &format!("SELECT count(*) FROM {table}")).await?;
            Ok(actual == *count)
        }
    }
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
    let col_count = rows.column_count();
    let mut row_count = 0;
    while let Some(row) = rows.next().await? {
        let mut parts = Vec::with_capacity(col_count);
        for i in 0..col_count {
            parts.push(match row.get_value(i) {
                Ok(turso::Value::Null) => "NULL".to_string(),
                Ok(turso::Value::Integer(n)) => n.to_string(),
                Ok(turso::Value::Real(f)) => f.to_string(),
                Ok(turso::Value::Text(s)) => s,
                Ok(turso::Value::Blob(b)) => format!("<blob {} B>", b.len()),
                Err(e) => format!("<err: {e}>"),
            });
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

/// Install a panic hook that captures payload + location + thread into
/// the returned `Arc<Mutex<Vec<String>>>`. The hook chains to the default
/// hook so backtraces still print. Used by both replay paths so panics
/// caught inside Turso's IVM or Holon's actor `catch_unwind` boundary
/// are still surfaced in the final verdict.
fn install_panic_hook() -> Arc<Mutex<Vec<String>>> {
    let panic_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let panic_log_hook = panic_log.clone();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let thread = std::thread::current()
            .name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("{:?}", std::thread::current().id()));
        let entry = format!("[panic] thread={thread} at {location}: {payload}");
        eprintln!("{entry}");
        panic_log_hook.lock().unwrap().push(entry);
        default_hook(info);
    }));
    panic_log
}

/// Replay the trace through Holon's `TursoBackend` actor. Matches
/// production's CDC callback (`process_cdc_event` → coalesce → broadcast)
/// and DDL dependency ordering. The actor wraps each command in
/// `catch_unwind`, but our panic hook fires *before* the unwind is
/// caught, so panics on tokio worker threads are recorded in
/// `panic_log` even when the actor "continues".
async fn cmd_replay_via_actor(args: &ReplayArgs, replay_file: &str) -> anyhow::Result<()> {
    use holon::storage::turso::TursoBackend;

    println!("=== Turso SQL Replay via Holon TursoBackend actor ===\n");
    println!("  File: {replay_file}");

    let panic_log = install_panic_hook();

    let directives = parse_replay_file(replay_file)?;
    let sql_count = directives
        .iter()
        .filter(|d| matches!(d, Directive::Sql { .. }))
        .count();
    println!("  SQL statements: {sql_count}");

    let background_read_targets: Vec<String> = args
        .background_read
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !background_read_targets.is_empty() {
        println!(
            "  Background read: {:?} every {}ms",
            background_read_targets, args.background_read_interval_ms
        );
    }
    if args.post_commit_poll_ms > 0 {
        println!("  Post-commit poll: {}ms", args.post_commit_poll_ms);
    }
    println!();

    let db_path = "/tmp/turso-sql-replay-actor.db";
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    // Construct the actor using production's open_database + new path so
    // we get the same DatabaseOpts (with_views = true), the same internal
    // turso_config (async_io: false), and the same actor mpsc + CDC
    // broadcast wiring as a live Holon process.
    let db = TursoBackend::open_database(db_path)
        .map_err(|e| anyhow::anyhow!("open_database failed: {e}"))?;
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx)
        .map_err(|e| anyhow::anyhow!("TursoBackend::new failed: {e}"))?;

    // Subscribe to the CDC stream and drain it on a background task. This
    // mirrors what production downstream consumers do — a live receiver
    // prevents the broadcast channel from filling and keeps the actor
    // hot. Without a subscriber, send() on a full channel drops messages
    // silently and the actor's processing pattern differs from production.
    let cdc_drain = handle.cdc_broadcast().subscribe();
    let cdc_count = Arc::new(AtomicUsize::new(0));
    let cdc_count_clone = cdc_count.clone();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_cdc = stop_flag.clone();
    let cdc_handle = tokio::spawn(async move {
        let mut rx = cdc_drain;
        while !stop_flag_cdc.load(Ordering::Relaxed) {
            match rx.recv().await {
                Ok(_batch) => {
                    cdc_count_clone.fetch_add(1, Ordering::Relaxed);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    // Background readers — separate connections via additional handles
    // would require more plumbing; we route reads through the same
    // DbHandle which queues them through the actor (still useful: a
    // queued query exercises the same command-dispatch path that
    // production reactive watchers do).
    let handle_for_readers = handle.clone();
    let mut reader_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    for target in &background_read_targets {
        let h = handle_for_readers.clone();
        let target = target.clone();
        let stop = stop_flag.clone();
        let interval = args.background_read_interval_ms;
        reader_handles.push(tokio::spawn(async move {
            let scan_sql = format!("SELECT * FROM {target} LIMIT 50");
            while !stop.load(Ordering::Relaxed) {
                let _ = h.query(&scan_sql, std::collections::HashMap::new()).await;
                if interval > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
                }
            }
        }));
    }

    // Drive the replay. Statements between actor_tx_begin and
    // actor_tx_commit are buffered and shipped as one `handle.transaction`
    // call — that's the path production takes and the path
    // `process_actor_command` (where the catch_unwind lives) wraps.
    let mut stmt_idx = 0usize;
    let mut tx_buffer: Option<Vec<(String, Vec<turso::Value>)>> = None;
    let mut ddl_errors = 0usize;
    let mut tx_errors = 0usize;

    for directive in &directives {
        match directive {
            Directive::SetChangeCallback => {
                // No-op: the actor already wired CDC during TursoBackend::new.
            }
            Directive::Wait(ms) => {
                if args.replay_timing {
                    tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
                }
            }
            Directive::Assert(_) => {
                // Assertions are not meaningful in actor mode (queries go
                // through the actor's serialized path) — skip.
            }
            Directive::Sql { tag, sql } => {
                stmt_idx += 1;
                if tag == "actor_tx_begin" {
                    if stmt_idx.is_multiple_of(500) || stmt_idx < 5 {
                        println!(
                            "[{stmt_idx}/{sql_count}] BEGIN  (CDC: {})",
                            cdc_count.load(Ordering::Relaxed)
                        );
                    }
                    tx_buffer = Some(Vec::new());
                    continue;
                }
                if tag == "actor_tx_commit" {
                    if let Some(stmts) = tx_buffer.take() {
                        let len = stmts.len();
                        println!(
                            "[{stmt_idx}/{sql_count}] COMMIT ({len} stmts)  (CDC: {})",
                            cdc_count.load(Ordering::Relaxed)
                        );
                        if let Err(e) = handle.transaction(stmts.clone()).await {
                            tx_errors += 1;
                            eprintln!("!!! [{stmt_idx}] transaction failed: {e}");
                            for (i, (s, _)) in stmts.iter().enumerate() {
                                if let Err(per) = handle.execute(s, Vec::new()).await {
                                    let preview: String = s.chars().take(250).collect();
                                    eprintln!("    [{stmt_idx}.{i}] {per}\n    SQL: {preview}");
                                    break;
                                }
                            }
                        }
                        if args.post_commit_poll_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                args.post_commit_poll_ms,
                            ))
                            .await;
                            for target in &background_read_targets {
                                let sql = format!("SELECT COUNT(*) FROM {target}");
                                let _ = handle.query(&sql, std::collections::HashMap::new()).await;
                            }
                        }
                    }
                    continue;
                }

                // Inside a transaction — buffer the statement.
                if let Some(buf) = tx_buffer.as_mut() {
                    buf.push((sql.clone(), Vec::new()));
                    continue;
                }

                // Outside any transaction: classify and route.
                let upper = sql.trim_start().to_uppercase();
                let is_ddl = upper.starts_with("CREATE ")
                    || upper.starts_with("DROP ")
                    || upper.starts_with("ALTER ");
                if is_ddl {
                    if let Err(e) = handle.execute_ddl(sql).await {
                        ddl_errors += 1;
                        eprintln!("!!! [{stmt_idx}] DDL failed: {e}");
                    }
                } else if upper.starts_with("SELECT ") || upper.starts_with("WITH ") {
                    let _ = handle.query(sql, std::collections::HashMap::new()).await;
                } else {
                    let _ = handle.execute(sql, Vec::new()).await;
                }
            }
        }
    }

    // Give async post-commit consolidation one more chance to fire before
    // we tear down.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    stop_flag.store(true, Ordering::Relaxed);
    for h in reader_handles {
        let _ = h.await;
    }
    drop(cdc_handle);

    let captured_panics = panic_log.lock().unwrap().clone();
    let has_panics = !captured_panics.is_empty();

    println!("\n=== SUMMARY (via-holon-actor) ===");
    println!("  Statements processed: {stmt_idx}");
    println!(
        "  CDC batches observed: {}",
        cdc_count.load(Ordering::Relaxed)
    );
    println!("  DDL errors: {ddl_errors}");
    println!("  Transaction errors: {tx_errors}");
    println!("  Panics captured: {}", captured_panics.len());

    if has_panics {
        println!("\n  VERDICT: PANIC CAPTURED (IVM bug reproduced via actor)");
        for p in &captured_panics {
            println!("    - {p}");
        }
        std::process::exit(1);
    }
    println!("\n  VERDICT: No panics captured.");
    Ok(())
}

/// Coarse classification of a SQL statement, used to decide whether a
/// statement needs a matview-consistency check (DML), should be run as a
/// query that drains its rows (Query), or is DDL/other (Other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StmtKind {
    Dml,
    Query,
    Other,
}

fn statement_kind(sql: &str) -> StmtKind {
    // Match on the first token, not a "KEYWORD " prefix — multi-line SQL can
    // have a newline right after the keyword (`SELECT\n    b.id, ...`), which
    // a trailing-space prefix check misclassifies as Other → execute path →
    // "unexpected row during execution".
    let first = sql.split_whitespace().next().unwrap_or("").to_uppercase();
    match first.as_str() {
        "INSERT" | "UPDATE" | "DELETE" | "REPLACE" => StmtKind::Dml,
        "SELECT" | "WITH" => StmtKind::Query,
        _ => StmtKind::Other,
    }
}

/// The terminal verdict of a replay, derived from whether data mismatched,
/// a panic was captured, and whether any (non-mismatch) inconsistencies were
/// recorded. Pure so it can be unit-tested without running a replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayVerdict {
    /// A matview/assertion data mismatch was observed — the bug reproduced.
    BugReproduced,
    /// No data mismatch, but a panic fired (caught by the panic hook).
    PanicCaptured,
    /// Nothing went wrong.
    Clean,
    /// No data mismatch and no panic, but some query errors were logged.
    QueryErrorsOnly,
}

fn replay_verdict(
    has_data_mismatch: bool,
    has_panics: bool,
    has_inconsistencies: bool,
) -> ReplayVerdict {
    if has_data_mismatch {
        ReplayVerdict::BugReproduced
    } else if has_panics {
        ReplayVerdict::PanicCaptured
    } else if !has_inconsistencies {
        ReplayVerdict::Clean
    } else {
        ReplayVerdict::QueryErrorsOnly
    }
}

/// Mutable state accumulated across one replay loop. Owned by `cmd_replay`
/// and threaded by `&mut` into the per-directive handlers so the Sql and
/// Assert arms can record executed SQL, inconsistencies, and tracker state.
struct ReplayProgress {
    executed_sqls: Vec<String>,
    all_inconsistencies: Vec<String>,
    has_data_mismatch: bool,
    stmt_idx: usize,
    /// COMMIT counter, used to label CHK-oracle divergences by transaction.
    commit_idx: usize,
    /// First `commit#K view=… MV=… RC=…` at which the CHK oracle diverged.
    first_divergence: Option<String>,
    tracker_prev: Vec<Option<Vec<bool>>>,
}

impl ReplayProgress {
    fn new(tracker_count: usize) -> Self {
        Self {
            executed_sqls: Vec::new(),
            all_inconsistencies: Vec::new(),
            has_data_mismatch: false,
            stmt_idx: 0,
            commit_idx: 0,
            first_divergence: None,
            tracker_prev: vec![None; tracker_count],
        }
    }
}

/// Whether the replay loop should advance to the next directive or stop.
enum LoopControl {
    Continue,
    Break,
}

/// Execute one `Directive::Sql`. Handles transaction-boundary tags
/// (`actor_tx_begin` / `actor_tx_commit`) specially, runs queries vs.
/// statements, probes id trackers, and—when `--check-after-each`—diffs
/// matviews after each DML. Returns `Break` only when an inconsistency was
/// found and the caller asked to stop on the first one.
async fn replay_sql_directive(
    conn: &turso::Connection,
    args: &ReplayArgs,
    progress: &mut ReplayProgress,
    cdc_count: &Arc<AtomicUsize>,
    background_read_targets: &[String],
    trackers: &[IdTracker],
    tag: &str,
    sql: &str,
    sql_count: usize,
) -> anyhow::Result<LoopControl> {
    progress.stmt_idx += 1;
    let stmt_idx = progress.stmt_idx;

    // Handle transaction boundaries faithfully
    if tag == "actor_tx_begin" {
        let sql_preview: String = sql.chars().take(100).collect();
        println!(
            "[{stmt_idx}/{sql_count}] [{tag}] {sql_preview}  (CDC: {})",
            cdc_count.load(Ordering::SeqCst)
        );
        match conn.execute("BEGIN TRANSACTION", ()).await {
            Ok(_) => {
                progress.executed_sqls.push("BEGIN TRANSACTION".to_string());
            }
            Err(e) => {
                println!("!!! [{stmt_idx}] BEGIN TRANSACTION failed: {e}");
            }
        }
        return Ok(LoopControl::Continue);
    }
    if tag == "actor_tx_commit" {
        println!(
            "[{stmt_idx}/{sql_count}] [{tag}] COMMIT  (CDC: {})",
            cdc_count.load(Ordering::SeqCst)
        );
        match conn.execute("COMMIT", ()).await {
            Ok(_) => {
                progress.executed_sqls.push("COMMIT".to_string());
            }
            Err(e) => {
                println!("!!! [{stmt_idx}] COMMIT failed: {e}");
            }
        }
        // Post-commit settle window: give the async DBSP consolidation queued
        // by this commit a chance to run before the next BEGIN, and issue one
        // read against each background-read target so a cursor is open during
        // the consolidation. This is what makes post-commit panics (e.g.
        // multiset-went-negative) visible in the replay's panic hook.
        if args.post_commit_poll_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(args.post_commit_poll_ms)).await;
            for target in background_read_targets {
                let sql = format!("SELECT COUNT(*) FROM {target}");
                if let Ok(mut rows) = conn.query(&sql, ()).await {
                    while let Ok(Some(_)) = rows.next().await {}
                }
            }
        }

        if args.check_after_commit {
            progress.commit_idx += 1;
            let matview_names = extract_matview_names(&progress.executed_sqls);
            if !matview_names.is_empty() {
                let outcome =
                    chk_after_commit(conn, &matview_names, progress.commit_idx, args.chk_precise)
                        .await;
                if outcome.mismatch {
                    let was_first = !progress.has_data_mismatch;
                    progress.has_data_mismatch = true;
                    progress.all_inconsistencies.extend(
                        outcome
                            .lines
                            .iter()
                            .filter(|l| l.contains("MISMATCH") || l.contains("drift"))
                            .cloned(),
                    );
                    if progress.first_divergence.is_none() {
                        progress.first_divergence = outcome.first_divergence.clone();
                    }
                    if was_first {
                        println!(
                            "\n=== FIRST DIVERGENCE (CHK oracle) after {} ===",
                            progress.first_divergence.as_deref().unwrap_or("?")
                        );
                    }
                    if !args.no_break_on_inconsistency {
                        return Ok(LoopControl::Break);
                    }
                }
            }
        }
        return Ok(LoopControl::Continue);
    }

    let kind = statement_kind(sql);
    let is_dml = kind == StmtKind::Dml;
    let is_query = kind == StmtKind::Query;

    if stmt_idx % 500 == 0 || stmt_idx >= args.check_from.saturating_sub(10) {
        let sql_preview: String = sql.chars().take(100).collect();
        println!(
            "[{stmt_idx}/{sql_count}] [{tag}] {sql_preview}  (CDC: {})",
            cdc_count.load(Ordering::SeqCst)
        );
    }

    // DML with RETURNING yields rows; `conn.execute` rejects them with
    // "unexpected row during execution", so drain via the query path.
    let has_returning = is_dml && sql.to_uppercase().contains("RETURNING");
    let result = if is_query || has_returning {
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
            progress.executed_sqls.push(sql.to_string());
        }
        Err(e) => {
            let sql_preview: String = sql.chars().take(120).collect();
            println!("!!! [{stmt_idx}] ERROR in [{tag}]: {e}\n    SQL: {sql_preview}...");
        }
    }

    if !trackers.is_empty() && is_dml {
        let preview: String = sql.chars().take(60).collect();
        probe_trackers(
            conn,
            trackers,
            &mut progress.tracker_prev,
            stmt_idx,
            &preview,
        )
        .await;
    }

    if args.check_after_each && is_dml && stmt_idx >= args.check_from {
        let matview_names = extract_matview_names(&progress.executed_sqls);
        if !matview_names.is_empty() {
            let label = format!("stmt#{stmt_idx}");
            let result = check_matview_consistency(conn, &matview_names, &label).await?;
            if result.has_data_mismatch {
                let was_first = !progress.has_data_mismatch;
                progress.has_data_mismatch = true;
                progress.all_inconsistencies.extend(result.inconsistencies);
                if was_first {
                    println!("\n=== BUG REPRODUCED at statement {stmt_idx}! ===");
                    let sql_preview: String = sql.chars().take(200).collect();
                    println!("    SQL: {sql_preview}");
                    println!();
                }
                if !args.no_break_on_inconsistency {
                    return Ok(LoopControl::Break);
                }
            }
        }
    }

    Ok(LoopControl::Continue)
}

/// Run one `Directive::Assert`, recording failures and errors into `progress`.
async fn replay_assert_directive(
    conn: &turso::Connection,
    progress: &mut ReplayProgress,
    kind: &AssertKind,
) {
    match run_assertion(conn, kind).await {
        Ok(true) => {
            println!("[assert] OK: {}", describe_assertion(kind));
        }
        Ok(false) => {
            let msg = format!("FAIL: {}", describe_assertion(kind));
            println!("!!! [assert] {msg}");
            progress.all_inconsistencies.push(msg);
            progress.has_data_mismatch = true;
        }
        Err(e) => {
            let msg = format!("ERROR running assertion {}: {e}", describe_assertion(kind));
            println!("!!! [assert] {msg}");
            progress.all_inconsistencies.push(msg);
        }
    }
}

async fn cmd_replay(args: &ReplayArgs, replay_file: &str) -> anyhow::Result<()> {
    if args.via_holon_actor {
        return cmd_replay_via_actor(args, replay_file).await;
    }
    println!("=== Turso SQL Replay with CDC ===\n");
    println!("  File: {replay_file}");
    println!("  Replay timing: {}", args.replay_timing);
    println!("  Check after each: {}", args.check_after_each);

    // Capture panics that fire on tokio worker threads inside Turso's IVM
    // consolidation. Without this, panics caught by Turso's internal
    // catch_unwind never surface and the replay verdict reads "no mismatch"
    // even when a `json_group_array multiset went negative` or similar
    // invariant-violation panic was raised on a worker thread.
    let panic_log = install_panic_hook();

    let directives = parse_replay_file(replay_file)?;
    let sql_count = directives
        .iter()
        .filter(|d| matches!(d, Directive::Sql { .. }))
        .count();
    let has_cdc = directives
        .iter()
        .any(|d| matches!(d, Directive::SetChangeCallback));
    println!("  SQL statements: {sql_count}");
    println!("  CDC callback: {has_cdc}");

    let background_read_targets: Vec<String> = args
        .background_read
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !background_read_targets.is_empty() {
        println!(
            "  Background read: {:?} every {}ms",
            background_read_targets, args.background_read_interval_ms
        );
    }
    if args.post_commit_poll_ms > 0 {
        println!("  Post-commit poll: {}ms", args.post_commit_poll_ms);
    }
    println!();

    let db_path = args
        .db_path
        .as_deref()
        .unwrap_or("/tmp/turso-sql-replay.db");
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    if let Some(src) = &args.existing_db {
        println!("  Seeding scratch DB from existing: {src}");
        std::fs::copy(src, db_path)
            .with_context(|| format!("Failed to copy existing DB from {src}"))?;
        for ext in ["-wal", "-shm"] {
            let src_side = format!("{src}{ext}");
            if std::path::Path::new(&src_side).exists() {
                std::fs::copy(&src_side, format!("{db_path}{ext}"))
                    .with_context(|| format!("Failed to copy {src_side}"))?;
            }
        }
    }

    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    // Spawn one background reader task per target matview. Each task holds
    // its own connection and polls in a loop until the main replay drops
    // the stop flag. The reader queries are deliberately cheap (COUNT(*) +
    // a small LIMITed SELECT) so the loop runs hot rather than spending
    // most of its time in result decoding.
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut reader_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    for target in &background_read_targets {
        let reader_conn = db.connect()?;
        let target = target.clone();
        let stop = stop_flag.clone();
        let interval = args.background_read_interval_ms;
        reader_handles.push(tokio::spawn(async move {
            let count_sql = format!("SELECT COUNT(*) FROM {target}");
            let scan_sql = format!("SELECT * FROM {target} LIMIT 50");
            while !stop.load(Ordering::Relaxed) {
                if let Ok(mut rows) = reader_conn.query(&count_sql, ()).await {
                    while let Ok(Some(_)) = rows.next().await {}
                }
                if let Ok(mut rows) = reader_conn.query(&scan_sql, ()).await {
                    while let Ok(Some(_)) = rows.next().await {}
                }
                if interval > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
                }
            }
        }));
    }

    let cdc_count = Arc::new(AtomicUsize::new(0));
    let mut cdc_registered = false;
    let replay_start = std::time::Instant::now();
    let mut hit_max_stmts = false;
    let mut hit_max_secs = false;

    // Per-id tracker state: (id, [table_a, table_b, ...]) → previous presence
    // vector. We only print on transitions to keep the output manageable.
    let trackers = parse_track_id_args(&args.track_id);
    if !trackers.is_empty() {
        println!("  Tracking ids: {trackers:?}\n");
    }
    let mut progress = ReplayProgress::new(trackers.len());

    for directive in &directives {
        if let Some(max) = args.max_secs
            && replay_start.elapsed().as_secs() >= max
        {
            hit_max_secs = true;
            println!(
                "\n!!! max-secs ({max}) reached at stmt {}; bailing out",
                progress.stmt_idx
            );
            break;
        }
        if let Some(max) = args.max_stmts
            && progress.stmt_idx >= max
        {
            hit_max_stmts = true;
            println!("\n!!! max-stmts ({max}) reached; bailing out");
            break;
        }
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
                    if (prev + 1).is_multiple_of(100) {
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
                let control = replay_sql_directive(
                    &conn,
                    args,
                    &mut progress,
                    &cdc_count,
                    &background_read_targets,
                    &trackers,
                    tag,
                    sql,
                    sql_count,
                )
                .await?;
                if let LoopControl::Break = control {
                    break;
                }
            }
            Directive::Assert(kind) => {
                replay_assert_directive(&conn, &mut progress, kind).await;
            }
        }
    }

    // Drop background readers before the final consistency check so they
    // aren't racing it.
    stop_flag.store(true, Ordering::Relaxed);
    for handle in reader_handles {
        let _ = handle.await;
    }

    println!("\n=== Final Consistency Check ===");
    let matview_names = extract_matview_names(&progress.executed_sqls);
    println!("Active matviews: {:?}", matview_names);

    let final_result = check_matview_consistency(&conn, &matview_names, "FINAL").await?;
    let has_data_mismatch = progress.has_data_mismatch || final_result.has_data_mismatch;
    progress
        .all_inconsistencies
        .extend(final_result.inconsistencies);
    let first_divergence = progress.first_divergence.clone();
    let all_inconsistencies = progress.all_inconsistencies;

    let captured_panics = panic_log.lock().unwrap().clone();
    let has_panics = !captured_panics.is_empty();

    println!("\n=== SUMMARY ===");
    println!("  Statements executed: {}", progress.stmt_idx);
    println!("  CDC events total: {}", cdc_count.load(Ordering::SeqCst));
    println!("  Issues found: {}", all_inconsistencies.len());
    println!("  Panics captured: {}", captured_panics.len());
    if let Some(fd) = &first_divergence {
        println!("  First divergence: {fd}");
    }
    if hit_max_stmts {
        println!("  Budget: max-stmts hit (partial replay)");
    }
    if hit_max_secs {
        println!("  Budget: max-secs hit (partial replay)");
    }

    match replay_verdict(
        has_data_mismatch,
        has_panics,
        !all_inconsistencies.is_empty(),
    ) {
        ReplayVerdict::BugReproduced => {
            println!("\n  VERDICT: IVM BUG REPRODUCED!");
            for issue in &all_inconsistencies {
                println!("    - {issue}");
            }
            if has_panics {
                println!("  Panics captured during replay:");
                for p in &captured_panics {
                    println!("    - {p}");
                }
            }
            std::process::exit(1);
        }
        ReplayVerdict::PanicCaptured => {
            println!("\n  VERDICT: PANIC CAPTURED (IVM bug reproduced via panic hook)");
            for p in &captured_panics {
                println!("    - {p}");
            }
            if !all_inconsistencies.is_empty() {
                println!("  Other issues:");
                for issue in &all_inconsistencies {
                    println!("    - {issue}");
                }
            }
            std::process::exit(1);
        }
        ReplayVerdict::Clean => {
            println!("\n  VERDICT: No IVM inconsistencies detected.");
        }
        ReplayVerdict::QueryErrorsOnly => {
            println!("\n  VERDICT: No data mismatches (query errors only).");
            for issue in &all_inconsistencies {
                println!("    - {issue}");
            }
        }
    }

    Ok(())
}

// ─── Minimizer
// ──────────────────────────────────────────────────────────────────

/// Subprocess wall-clock budget for a single minimization candidate. Hit
/// regularly when the candidate doesn't reproduce — without it ddmin would
/// wait for a 3985-stmt replay to finish naturally. With it the worst case
/// per candidate is bounded, and the stream-and-kill path below makes the
/// good case (candidate does reproduce) finish ~as fast as the crash
/// pattern first appears in stdout.
const MINIMIZE_CANDIDATE_MAX_SECS: u64 = 120;

fn crashes_with_subprocess(
    directives: &[Directive],
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> bool {
    let tmp_path = "/tmp/turso-minimize-candidate.sql";
    write_directives(tmp_path, directives).unwrap();

    // The caller decides whether per-DML matview diffing is necessary based
    // on a probe of the full trace (see `probe_needs_per_stmt_check`):
    //   - panic-style patterns / FINAL-detectable matview drift -> false
    //     (subprocess runs plain replay; pattern is caught at FINAL or by panic)
    //   - intermittent mid-trace drift that self-heals -> true (subprocess runs
    //     --check-after-each --no-break-on-inconsistency, slow)
    //
    // For matview bugs that persist to the end of the trace this is a 100×+
    // speedup per candidate; a 5554-stmt minimization that was 1h/candidate
    // becomes ~5-8s/candidate. We deliberately omit --check-from: the
    // minimizer mutates positions by removing statements, so a fixed offset
    // would skip checks that have shifted earlier.

    let exe = std::env::current_exe().unwrap();
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("replay").arg(tmp_path);
    if needs_per_stmt_check {
        cmd.arg("--check-after-each")
            .arg("--no-break-on-inconsistency");
    }
    cmd.arg("--max-secs")
        .arg(MINIMIZE_CANDIDATE_MAX_SECS.to_string());

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let stdout = child.stdout.take().expect("stdout was set to piped");
    let stderr = child.stderr.take().expect("stderr was set to piped");
    let needle = crash_pattern.to_string();
    let (tx, rx) = std::sync::mpsc::channel::<bool>();

    fn spawn_scanner<R: std::io::Read + Send + 'static>(
        reader: R,
        needle: String,
        tx: std::sync::mpsc::Sender<bool>,
    ) {
        std::thread::spawn(move || {
            use std::io::BufRead;
            let buf = std::io::BufReader::new(reader);
            for line in buf.lines().map_while(Result::ok) {
                if line.contains(&needle) {
                    let _ = tx.send(true);
                    return;
                }
            }
            let _ = tx.send(false);
        });
    }
    spawn_scanner(stdout, needle.clone(), tx.clone());
    spawn_scanner(stderr, needle, tx.clone());
    // Drop our own sender so `recv` returns Disconnected once both scanners exit.
    drop(tx);

    // Hard ceiling slightly past the subprocess's own --max-secs, so we
    // always see the subprocess's bail-out output before reaping.
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(MINIMIZE_CANDIDATE_MAX_SECS + 10);
    let mut found = false;
    let mut scanners_done = 0;
    while scanners_done < 2 {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining.min(std::time::Duration::from_millis(500))) {
            Ok(true) => {
                found = true;
                break;
            }
            Ok(false) => {
                scanners_done += 1;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // If the child has already exited, both scanners should drain
                // and report shortly; loop back and recv again.
                if let Ok(Some(_)) = child.try_wait() {
                    // give the scanners one more poll cycle before bailing
                    continue;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    found
}

fn is_structural(d: &Directive) -> bool {
    match d {
        Directive::SetChangeCallback => true,
        Directive::Assert(_) => true,
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
        .arg("--check-after-each")
        .arg("--no-break-on-inconsistency")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("Failed to run subprocess for crash detection");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Prefer matview-inconsistency markers — these are why we're minimizing in
    // the first place when chasing IVM bugs. Pick the first INCONSISTENCY line
    // and use the table name as the pattern so the minimizer locks onto the
    // matview that drifted.
    for line in combined.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("INCONSISTENCY in ")
            && let Some(name) = rest.split(':').next()
        {
            let pattern = format!("INCONSISTENCY in {name}");
            println!("  Detected crash pattern: {pattern}");
            return pattern;
        }
    }

    for (i, line) in combined.lines().enumerate() {
        if line.contains("panicked at")
            && let Some(next_line) = combined.lines().nth(i + 1)
        {
            let pattern = next_line.trim().to_string();
            if !pattern.is_empty() {
                println!("  Detected crash pattern: {pattern}");
                return pattern;
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

fn try_remove(
    kept: &[Directive],
    i: usize,
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> Option<Vec<Directive>> {
    let mut candidate: Vec<Directive> = Vec::with_capacity(kept.len() - 1);
    candidate.extend(kept[..i].iter().cloned());
    candidate.extend(kept[i + 1..].iter().cloned());

    if crashes_with_subprocess(&candidate, crash_pattern, needs_per_stmt_check) {
        Some(candidate)
    } else {
        None
    }
}

fn try_remove_batch(
    kept: &[Directive],
    indices: &[usize],
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> Option<Vec<Directive>> {
    let skip: std::collections::HashSet<usize> = indices.iter().copied().collect();
    let candidate: Vec<Directive> = kept
        .iter()
        .enumerate()
        .filter(|(i, _)| !skip.contains(i))
        .map(|(_, d)| d.clone())
        .collect();

    if crashes_with_subprocess(&candidate, crash_pattern, needs_per_stmt_check) {
        Some(candidate)
    } else {
        None
    }
}

/// Decide whether the minimizer's per-candidate subprocess needs
/// `--check-after-each` (per-DML matview diff). Probes by running the full
/// directives once *without* the per-DML check; if the crash pattern still
/// appears (panic surfaces in stderr, or `[FINAL] INCONSISTENCY` lands in
/// stdout), the FINAL check is sufficient and every later candidate can skip
/// the O(N²) per-DML diff. This is what unblocks ~5500-stmt minimizations
/// that were taking ~1h/candidate with the per-DML diff on.
fn probe_needs_per_stmt_check(directives: &[Directive], crash_pattern: &str) -> bool {
    let panic_like = crash_pattern.contains("panicked")
        || crash_pattern.contains("multiset went negative")
        || crash_pattern.contains("invalid state")
        || crash_pattern.contains("Uninitialized");
    if panic_like {
        return false;
    }

    println!(
        "--- Probing: does FINAL-only replay catch '{crash_pattern}'? (skipping \
         --check-after-each) ---"
    );
    let fast_works = crashes_with_subprocess(directives, crash_pattern, false);
    if fast_works {
        println!("  Yes — using fast subprocess for all candidates.\n");
        false
    } else {
        println!(
            "  No — falling back to --check-after-each (slow). Pattern only surfaces mid-replay.\n"
        );
        true
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
            Directive::Assert(kind) => {
                writeln!(f, "-- ?ASSERT {}", describe_assertion(kind))?;
                writeln!(f)?;
            }
        }
    }
    Ok(())
}

/// A minimizer phase: take the current directive set, try to shrink it while
/// preserving the crash, and return the shrunk set plus how many it removed.
type MinimizePhase = fn(Vec<Directive>, &str, bool) -> (Vec<Directive>, usize);

/// Phase 1: binary-search the shortest directive prefix that still crashes.
fn minimize_find_prefix(
    directives: &[Directive],
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> Vec<Directive> {
    println!("--- Phase 1: Find minimal prefix ---");
    let total = directives.len();
    let mut lo: usize = 1;
    let mut hi: usize = total;
    while lo < hi {
        let mid = (lo + hi) / 2;
        print!("  Prefix [{mid}/{total}]... ");
        if crashes_with_subprocess(&directives[..mid], crash_pattern, needs_per_stmt_check) {
            println!("CRASHES");
            hi = mid;
        } else {
            println!("ok");
            lo = mid + 1;
        }
    }
    let kept = directives[..lo].to_vec();
    println!("Minimal prefix: {lo} directives\n");
    kept
}

/// Best-effort table/view name for grouping a statement in Phase 2 — covers
/// DML (via `extract_table_name`) plus DDL and `SELECT ... FROM` forms.
fn group_table_name(sql: &str) -> Option<String> {
    if let Some(name) = extract_table_name(sql) {
        return Some(name);
    }
    let upper = sql.trim().to_uppercase();
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
                .split(|c: char| c.is_whitespace() || c == '(' || c == '"' || c == ';')
                .next()
                .filter(|s| !s.is_empty());
            if let Some(n) = name {
                return Some(n.to_string());
            }
        }
    }
    if upper.starts_with("SELECT ")
        && let Some(from_pos) = upper.find(" FROM ")
    {
        let rest = sql.trim()[from_pos + 6..].trim_start();
        let name = rest
            .split(|c: char| c.is_whitespace() || c == '(' || c == ',')
            .next()
            .filter(|s| !s.is_empty());
        if let Some(n) = name {
            return Some(n.to_string());
        }
    }
    None
}

/// Phase 2: drop every statement touching one table/view in a single batch,
/// largest groups first.
fn minimize_table_groups(
    mut kept: Vec<Directive>,
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> (Vec<Directive>, usize) {
    println!("--- Phase 2: Remove table groups ---");
    let mut removed = 0;
    let mut table_groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, d) in kept.iter().enumerate() {
        if let Directive::Sql { sql, .. } = d
            && let Some(t) = group_table_name(sql)
        {
            table_groups.entry(t.to_lowercase()).or_default().push(i);
        }
    }

    let mut groups: Vec<(String, Vec<usize>)> = table_groups.into_iter().collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.1.len()));

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

        if let Some(candidate) =
            try_remove_batch(&kept, indices, crash_pattern, needs_per_stmt_check)
        {
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
    (kept, removed)
}

/// Phase 3: ddmin-style chunk removal, halving the chunk size when a pass
/// removes nothing.
fn minimize_chunks(
    mut kept: Vec<Directive>,
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> (Vec<Directive>, usize) {
    println!("--- Phase 3: Chunk removal (ddmin) ---");
    let mut removed = 0;
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

            if let Some(candidate) =
                try_remove_batch(&kept, &indices, crash_pattern, needs_per_stmt_check)
            {
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
    println!("  Removed {removed} via chunks, {} remaining\n", kept.len());
    (kept, removed)
}

/// Phase 4: remove individual non-structural (DML) statements, walking from
/// the end toward the start.
fn minimize_individual_dml(
    mut kept: Vec<Directive>,
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> (Vec<Directive>, usize) {
    println!("--- Phase 4: Remove individual DML statements ---");
    let mut removed = 0;
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

        if let Some(candidate) = try_remove(&kept, i, crash_pattern, needs_per_stmt_check) {
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
        "  Removed {removed} DML statements, {} remaining\n",
        kept.len()
    );
    (kept, removed)
}

/// Phase 5: remove individual structural (DDL) statements, attempting to drop
/// each together with the statements that reference its table first.
fn minimize_individual_ddl(
    mut kept: Vec<Directive>,
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> (Vec<Directive>, usize) {
    println!("--- Phase 5: Remove individual DDL statements ---");
    let mut removed = 0;
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
            if let Some(candidate) =
                try_remove_batch(&kept, &all_indices, crash_pattern, needs_per_stmt_check)
            {
                println!("REMOVED (+ {} dependents)", batch_indices.len());
                removed += all_indices.len();
                kept = candidate;
                did_remove = true;
            }
        }

        if !did_remove {
            if let Some(candidate) = try_remove(&kept, i, crash_pattern, needs_per_stmt_check) {
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
        "  Removed {removed} DDL statements, {} remaining\n",
        kept.len()
    );
    (kept, removed)
}

/// Phase 6: final pass removing any remaining directive except the CDC
/// callback registration.
fn minimize_final_cleanup(
    mut kept: Vec<Directive>,
    crash_pattern: &str,
    needs_per_stmt_check: bool,
) -> (Vec<Directive>, usize) {
    println!("--- Phase 6: Final cleanup ---");
    let mut removed = 0;
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

        if let Some(candidate) = try_remove(&kept, i, crash_pattern, needs_per_stmt_check) {
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
        "  Removed {removed} more statements, {} remaining\n",
        kept.len()
    );
    (kept, removed)
}

/// Print the minimized reproducer as replay-file directives on stdout.
fn print_minimal_reproducer(kept: &[Directive]) {
    println!("\n--- Minimal SQL reproducer ---");
    for d in kept {
        match d {
            Directive::SetChangeCallback => println!("-- !SET_CHANGE_CALLBACK"),
            Directive::Wait(ms) => println!("-- Wait {ms}ms"),
            Directive::Sql { tag, sql } => {
                println!("-- [{tag}]");
                println!("{sql};");
                println!();
            }
            Directive::Assert(kind) => {
                println!("-- ?ASSERT {}", describe_assertion(kind));
                println!();
            }
        }
    }
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

    println!("=== Minimizer: {} directives ===", directives.len());
    println!("  Crash pattern: {crash_pattern}\n");

    let needs = probe_needs_per_stmt_check(&directives, &crash_pattern);

    let mut kept = minimize_find_prefix(&directives, &crash_pattern, needs);
    let mut removed = 0;
    let phases: [MinimizePhase; 5] = [
        minimize_table_groups,
        minimize_chunks,
        minimize_individual_dml,
        minimize_individual_ddl,
        minimize_final_cleanup,
    ];
    for phase in phases {
        let (next, n) = phase(kept, &crash_pattern, needs);
        kept = next;
        removed += n;
    }

    println!(
        "=== Minimized: {} directives (removed {removed} total) ===",
        kept.len()
    );

    write_directives(&output_path, &kept)?;
    println!("Written to: {output_path}");

    print_minimal_reproducer(&kept);

    Ok(())
}

// ─── Split & Sweep (fresh-process reproduction tooling) ─────────────────────

/// Outcome of one `replay` child process. `red` is the coarse verdict —
/// `cmd_replay` exits non-zero on a data mismatch or captured panic, so a
/// failed exit status is the reproduction signal. `signature` normalizes the
/// *kind* of failure across processes so single-process and split-process runs
/// can be compared even though their commit indices differ.
struct ChildRun {
    red: bool,
    signature: Option<String>,
    output: String,
}

/// Shell out to `<self> replay <extra_args…>` and classify the result.
///
/// tursodb CLI gotchas, encoded here for anyone who extends this to drive the
/// `tursodb` binary directly instead of re-execing this tool: dot-commands are
/// NOT parsed inside a `.read` file, so pass `.read <file>` as the positional
/// SQL argument; use `-m list` for machine-parseable output; matviews need
/// `--experimental-views`; and a copied DB file of a live holon.db is readable
/// by tursodb but NOT by sqlite3 (it rejects the materialized-view DDL). We
/// deliberately re-exec *this* binary instead: per-process HashMap RandomState
/// seeding (the DBSP consolidate iteration order that makes IVM bugs
/// per-process-seeded) lives in the linked `turso_core`, so a fresh child of
/// this same binary samples a fresh seed and carries the panic hook + CHK
/// oracle for free.
fn run_replay_child(extra_args: &[String]) -> ChildRun {
    let exe = std::env::current_exe().expect("current_exe");
    let out = std::process::Command::new(exe)
        .arg("replay")
        .args(extra_args)
        .output()
        .expect("spawn replay child");
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let red = !out.status.success();
    let signature = parse_divergence_signature(&output);
    ChildRun {
        red,
        signature,
        output,
    }
}

/// Normalize a replay child's output to a process-independent failure
/// signature. The commit index is intentionally dropped: the same divergence
/// lands at commit #K in the full trace but commit #1 in a split suffix, so the
/// signature keys on the failure *kind* and the affected view/payload instead.
fn parse_divergence_signature(output: &str) -> Option<String> {
    for line in output.lines() {
        if line.contains("<<< MISMATCH")
            && let Some(view) = line.split('|').nth(2)
        {
            return Some(format!("chk-mismatch:{}", view.trim()));
        }
    }
    for line in output.lines() {
        if let Some(idx) = line.find("INCONSISTENCY in ") {
            let rest = &line[idx + "INCONSISTENCY in ".len()..];
            if let Some(name) = rest.split(':').next() {
                return Some(format!("inconsistency:{}", name.trim()));
            }
        }
    }
    for line in output.lines() {
        if (line.contains("[panic]") || line.contains("panicked at"))
            && let Some(at) = line.find(" at ")
        {
            let tail = &line[at + 4..];
            if let Some(cp) = tail.find(": ") {
                let payload: String = tail[cp + 2..].chars().take(50).collect();
                return Some(format!("panic:{}", payload.trim()));
            }
        }
    }
    None
}

/// First `commit#K` mentioned in a replay child's output (the CHK oracle's
/// first-divergence report). Used by `split --at auto`.
fn parse_first_divergence_commit(output: &str) -> Option<usize> {
    let re = Regex::new(r"commit#(\d+)").unwrap();
    re.captures(output)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<usize>().ok())
}

/// Byte-index bounds `(begin_idx, commit_idx)` of the `k`-th (1-based)
/// transaction in `directives`, delimited by `actor_tx_begin` /
/// `actor_tx_commit`.
fn nth_transaction_bounds(directives: &[Directive], k: usize) -> Option<(usize, usize)> {
    let mut last_begin: Option<usize> = None;
    let mut commit_count = 0usize;
    for (i, d) in directives.iter().enumerate() {
        if let Directive::Sql { tag, .. } = d {
            if tag == "actor_tx_begin" {
                last_begin = Some(i);
            } else if tag == "actor_tx_commit" {
                commit_count += 1;
                if commit_count == k {
                    return Some((last_begin.unwrap_or(i), i));
                }
            }
        }
    }
    None
}

fn cmd_split(args: &SplitArgs) -> anyhow::Result<()> {
    let directives = parse_replay_file(&args.replay_file)?;

    let commit_k = match args.at {
        Some(k) => k,
        None => {
            println!("--- Auto-detecting first divergence (replay --check-after-commit) ---");
            let run = run_replay_child(&[args.replay_file.clone(), "--check-after-commit".into()]);
            let k = parse_first_divergence_commit(&run.output).context(
                "could not auto-detect first divergence from --check-after-commit output; \
                 pass --at <N>",
            )?;
            println!("  First divergence at transaction #{k}\n");
            k
        }
    };

    let (begin_idx, commit_idx) = nth_transaction_bounds(&directives, commit_k)
        .with_context(|| format!("could not locate transaction #{commit_k} bounds"))?;

    let base = args.output_prefix.clone().unwrap_or_else(|| {
        args.replay_file
            .strip_suffix(".sql")
            .unwrap_or(&args.replay_file)
            .to_string()
    });
    let prefix_path = format!("{base}.prefix.sql");
    let suffix_path = format!("{base}.suffix.sql");

    let prefix: Vec<Directive> = directives[..begin_idx].to_vec();
    let suffix: Vec<Directive> = directives[begin_idx..=commit_idx].to_vec();
    write_directives(&prefix_path, &prefix)?;
    write_directives(&suffix_path, &suffix)?;
    println!(
        "  Wrote {prefix_path} ({} directives) + {suffix_path} ({} directives)",
        prefix.len(),
        suffix.len()
    );

    if args.skip_verify {
        println!("  (mechanism-preservation verification skipped)");
    } else {
        verify_mechanism_preservation(&args.replay_file, &prefix_path, &suffix_path)?;
    }
    Ok(())
}

/// Verify that replaying the suffix against a saved prefix DB in a FRESH
/// process reproduces the SAME failure as the in-process full-trace replay.
///
/// THE MECHANISM-PRESERVATION TRAP (cost real time today): a fresh-process
/// replay of the suffix forces the matview to be RELOADED FROM DISK, which
/// exercises the state-RESTORE code path — a DIFFERENT mechanism than the
/// in-process divergence that arises from live incremental DBSP updates. A
/// split-mode red is therefore NOT proof that you reproduced the original bug;
/// it can silently substitute one bug for another. Treat split-mode red as a
/// SEPARATE finding until its red/green outcome AND signature match the
/// single-process run.
fn verify_mechanism_preservation(
    original: &str,
    prefix_path: &str,
    suffix_path: &str,
) -> anyhow::Result<()> {
    println!("\n--- Verifying mechanism preservation ---");

    let single = run_replay_child(&[original.into(), "--check-after-commit".into()]);
    println!(
        "  single-process (in-process divergence): red={} signature={:?}",
        single.red, single.signature
    );

    let prefix_db = "/tmp/turso-split-prefix.db";
    let saved_db = "/tmp/turso-split-saved.db";
    let suffix_db = "/tmp/turso-split-suffix.db";
    for base in [prefix_db, saved_db, suffix_db] {
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{base}{ext}"));
        }
    }

    // Build prefix state, then snapshot it so the suffix runs against a
    // pristine copy (replay mutates its --db-path in place).
    let _prefix_run = run_replay_child(&[prefix_path.into(), "--db-path".into(), prefix_db.into()]);
    for ext in ["", "-wal", "-shm"] {
        let src = format!("{prefix_db}{ext}");
        if std::path::Path::new(&src).exists() {
            std::fs::copy(&src, format!("{saved_db}{ext}"))
                .with_context(|| format!("copy {src}"))?;
        }
    }

    let split = run_replay_child(&[
        suffix_path.into(),
        "--existing-db".into(),
        saved_db.into(),
        "--db-path".into(),
        suffix_db.into(),
        "--check-after-commit".into(),
    ]);
    println!(
        "  split-process (state-restore from disk):  red={} signature={:?}",
        split.red, split.signature
    );

    if single.red != split.red || single.signature != split.signature {
        println!("\n  !!! WARNING: split-mode and single-process-mode DISAGREE.");
        println!("      A fresh-process replay of the suffix reloads the matview from disk");
        println!("      (state-restore path), which is NOT the same mechanism as the");
        println!("      in-process divergence. Treat the split-mode result as a SEPARATE");
        println!("      finding until proven identical.");
    } else {
        println!(
            "\n  OK: split-mode matches single-process (red={}, signature={:?}).",
            split.red, split.signature
        );
    }
    Ok(())
}

/// Aggregated result of one sweep leg.
struct SweepRate {
    red: usize,
    total: usize,
    sigs: std::collections::HashMap<String, usize>,
}

fn print_sig_histogram(rate: &SweepRate) {
    let mut sigs: Vec<(&String, &usize)> = rate.sigs.iter().collect();
    sigs.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (sig, n) in sigs {
        println!("      {n:>4}x  {sig}");
    }
}

fn sweep_one(
    file: &str,
    label: &str,
    runs: usize,
    jobs: usize,
    keep_dir: &str,
    extra: &[String],
) -> anyhow::Result<SweepRate> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(bool, Option<String>)>> = Mutex::new(Vec::new());
    let jobs = jobs.clamp(1, runs.max(1));

    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= runs {
                        break;
                    }
                    let db = format!("{keep_dir}/sweep-{label}-{i}.db");
                    let mut child_args =
                        vec![file.to_string(), "--db-path".to_string(), db.clone()];
                    child_args.extend(extra.iter().cloned());
                    let run = run_replay_child(&child_args);
                    if run.red {
                        let _ = std::fs::write(
                            format!("{keep_dir}/sweep-{label}-{i}.out"),
                            &run.output,
                        );
                    } else {
                        for ext in ["", "-wal", "-shm"] {
                            let _ = std::fs::remove_file(format!("{db}{ext}"));
                        }
                    }
                    println!(
                        "  [{label} {}/{runs}] {}  {}",
                        i + 1,
                        if run.red { "RED  " } else { "green" },
                        run.signature.as_deref().unwrap_or("")
                    );
                    results.lock().unwrap().push((run.red, run.signature));
                }
            });
        }
    });

    let results = results.into_inner().unwrap();
    let mut rate = SweepRate {
        red: 0,
        total: results.len(),
        sigs: std::collections::HashMap::new(),
    };
    for (red, sig) in results {
        if red {
            rate.red += 1;
            let key = sig.unwrap_or_else(|| "red-no-signature".to_string());
            *rate.sigs.entry(key).or_insert(0) += 1;
        }
    }
    Ok(rate)
}

fn cmd_sweep(args: &SweepArgs) -> anyhow::Result<()> {
    let keep_dir = args
        .keep_dir
        .clone()
        .unwrap_or_else(|| "/tmp/turso-sweep".to_string());
    std::fs::create_dir_all(&keep_dir)?;
    let extra: Vec<String> = if args.replay_arg.is_empty() {
        vec!["--check-after-commit".to_string()]
    } else {
        args.replay_arg.clone()
    };

    println!("=== Sweep: {} run(s) x {} job(s) ===", args.runs, args.jobs);
    println!("  Replay args: {extra:?}");
    println!("  Artifacts (red runs kept): {keep_dir}\n");

    let rate_a = sweep_one(
        &args.replay_file,
        "A",
        args.runs,
        args.jobs,
        &keep_dir,
        &extra,
    )?;
    let mut rate_b = None;
    if let Some(other) = &args.compare {
        println!();
        rate_b = Some(sweep_one(
            other, "B", args.runs, args.jobs, &keep_dir, &extra,
        )?);
    }

    let pct = |r: &SweepRate| 100.0 * r.red as f64 / r.total.max(1) as f64;
    println!("\n=== SWEEP RESULT ===");
    println!(
        "  A {}: {}/{} red ({:.0}%)",
        args.replay_file,
        rate_a.red,
        rate_a.total,
        pct(&rate_a)
    );
    print_sig_histogram(&rate_a);
    if let (Some(other), Some(rate_b)) = (&args.compare, &rate_b) {
        println!(
            "  B {}: {}/{} red ({:.0}%)",
            other,
            rate_b.red,
            rate_b.total,
            pct(rate_b)
        );
        print_sig_histogram(rate_b);
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
        Command::Split(args) => {
            cmd_split(&args)?;
        }
        Command::Sweep(args) => {
            cmd_sweep(&args)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_kind_classifies_dml_query_other() {
        assert_eq!(statement_kind("INSERT INTO t VALUES (1)"), StmtKind::Dml);
        assert_eq!(statement_kind("  update t set x = 1"), StmtKind::Dml);
        assert_eq!(statement_kind("DELETE FROM t"), StmtKind::Dml);
        assert_eq!(statement_kind("REPLACE INTO t VALUES (1)"), StmtKind::Dml);
        assert_eq!(statement_kind("SELECT * FROM t"), StmtKind::Query);
        assert_eq!(
            statement_kind("WITH cte AS (SELECT 1) SELECT * FROM cte"),
            StmtKind::Query
        );
        assert_eq!(statement_kind("CREATE TABLE t (id TEXT)"), StmtKind::Other);
        assert_eq!(statement_kind(""), StmtKind::Other);
        // A bare identifier prefix must not be mistaken for a keyword.
        assert_eq!(statement_kind("INSERTED something"), StmtKind::Other);
    }

    #[test]
    fn replay_verdict_priority_order() {
        // data mismatch dominates everything.
        assert_eq!(
            replay_verdict(true, true, true),
            ReplayVerdict::BugReproduced
        );
        // panic without mismatch.
        assert_eq!(
            replay_verdict(false, true, true),
            ReplayVerdict::PanicCaptured
        );
        // nothing wrong.
        assert_eq!(replay_verdict(false, false, false), ReplayVerdict::Clean);
        // only non-mismatch inconsistencies (e.g. query errors).
        assert_eq!(
            replay_verdict(false, false, true),
            ReplayVerdict::QueryErrorsOnly
        );
    }

    #[test]
    fn escape_sql_string_doubles_quotes() {
        assert_eq!(escape_sql_string("abc"), "'abc'");
        assert_eq!(escape_sql_string("O'Brien"), "'O''Brien'");
        assert_eq!(escape_sql_string(""), "''");
    }

    #[test]
    fn sql_has_open_literal_counts_quotes() {
        assert!(!sql_has_open_literal("SELECT 'abc'"));
        assert!(sql_has_open_literal("SELECT 'abc"));
        // SQLite escapes an apostrophe as '' — an even number of quotes.
        assert!(!sql_has_open_literal("INSERT INTO t VALUES ('a''b')"));
        assert!(!sql_has_open_literal("no quotes here"));
    }

    #[test]
    fn extract_matview_names_create_and_drop() {
        let names =
            extract_matview_names(&["CREATE MATERIALIZED VIEW foo AS SELECT 1".to_string()]);
        assert_eq!(names, vec!["foo"]);

        let names = extract_matview_names(&["CREATE MATERIALIZED VIEW IF NOT EXISTS bar AS \
                                             SELECT 1"
            .to_string()]);
        assert_eq!(names, vec!["bar"]);

        // DROP removes a previously created matview from the active set.
        let names = extract_matview_names(&[
            "CREATE MATERIALIZED VIEW foo AS SELECT 1".to_string(),
            "DROP VIEW foo".to_string(),
        ]);
        assert!(names.is_empty());

        let names = extract_matview_names(&[
            "CREATE MATERIALIZED VIEW foo AS SELECT 1".to_string(),
            "DROP MATERIALIZED VIEW IF EXISTS foo;".to_string(),
        ]);
        assert!(names.is_empty());
    }

    #[test]
    fn parse_track_id_args_with_and_without_tables() {
        let trackers = parse_track_id_args(&["abc".to_string()]);
        assert_eq!(trackers.len(), 1);
        assert_eq!(trackers[0].id, "abc");
        assert_eq!(trackers[0].tables, vec!["block_raw", "block"]);

        let trackers = parse_track_id_args(&["block_raw,block=xyz".to_string()]);
        assert_eq!(trackers[0].id, "xyz");
        assert_eq!(trackers[0].tables, vec!["block_raw", "block"]);
    }

    #[test]
    fn parse_assert_directive_variants_and_errors() {
        match parse_assert_directive("ROW-EXISTS block 'block:abc'").unwrap() {
            AssertKind::RowExists { table, id } => {
                assert_eq!(table, "block");
                assert_eq!(id, "block:abc");
            }
            other => panic!("expected RowExists, got {other:?}"),
        }
        match parse_assert_directive("row-absent block xyz").unwrap() {
            AssertKind::RowAbsent { table, id } => {
                assert_eq!(table, "block");
                assert_eq!(id, "xyz");
            }
            other => panic!("expected RowAbsent, got {other:?}"),
        }
        match parse_assert_directive("ROW-COUNT block 17").unwrap() {
            AssertKind::RowCount { table, count } => {
                assert_eq!(table, "block");
                assert_eq!(count, 17);
            }
            other => panic!("expected RowCount, got {other:?}"),
        }
        assert!(parse_assert_directive("ROW-COUNT block notanumber").is_err());
        assert!(parse_assert_directive("BOGUS table value").is_err());
        assert!(parse_assert_directive("ROW-EXISTS onlyonearg").is_err());
    }

    #[test]
    fn describe_assertion_round_trips_through_parse() {
        for kind in [
            AssertKind::RowExists {
                table: "block".into(),
                id: "abc".into(),
            },
            AssertKind::RowAbsent {
                table: "block".into(),
                id: "abc".into(),
            },
            AssertKind::RowCount {
                table: "block".into(),
                count: 5,
            },
        ] {
            let described = describe_assertion(&kind);
            let reparsed = parse_assert_directive(&described).unwrap();
            assert_eq!(describe_assertion(&reparsed), described);
        }
    }

    #[test]
    fn is_structural_classifies_directives() {
        assert!(is_structural(&Directive::SetChangeCallback));
        assert!(is_structural(&Directive::Assert(AssertKind::RowCount {
            table: "t".into(),
            count: 0,
        })));
        assert!(!is_structural(&Directive::Wait(5)));
        assert!(is_structural(&Directive::Sql {
            tag: "t".into(),
            sql: "CREATE TABLE t (id TEXT)".into(),
        }));
        assert!(!is_structural(&Directive::Sql {
            tag: "t".into(),
            sql: "INSERT INTO t VALUES (1)".into(),
        }));
    }

    #[test]
    fn group_table_name_covers_dml_ddl_and_select() {
        assert_eq!(
            group_table_name("INSERT INTO block VALUES (1)").as_deref(),
            Some("block")
        );
        assert_eq!(
            group_table_name("CREATE TABLE IF NOT EXISTS foo (id TEXT)").as_deref(),
            Some("foo")
        );
        assert_eq!(
            group_table_name("CREATE MATERIALIZED VIEW bar AS SELECT 1").as_deref(),
            Some("bar")
        );
        assert_eq!(
            group_table_name("CREATE INDEX idx_foo ON foo(a, b)").as_deref(),
            Some("idx_foo")
        );
        assert_eq!(group_table_name("DROP TABLE foo").as_deref(), Some("foo"));
        assert_eq!(
            group_table_name("SELECT * FROM block WHERE id = 'x'").as_deref(),
            Some("block")
        );
        assert_eq!(group_table_name("PRAGMA foreign_keys = ON"), None);
    }

    #[test]
    fn extract_table_name_for_dml() {
        assert_eq!(
            extract_table_name("INSERT INTO block (id) VALUES ('x')").as_deref(),
            Some("block")
        );
        assert_eq!(
            extract_table_name("INSERT OR REPLACE INTO block VALUES ('x')").as_deref(),
            Some("block")
        );
        assert_eq!(
            extract_table_name("DELETE FROM block WHERE id = 'x'").as_deref(),
            Some("block")
        );
        assert_eq!(
            extract_table_name("UPDATE block SET x = 1").as_deref(),
            Some("block")
        );
        assert_eq!(extract_table_name("SELECT * FROM block"), None);
    }

    #[test]
    fn should_include_filters_by_table_ignoring_string_literals() {
        let block = vec!["block".to_string()];
        let other = vec!["other".to_string()];
        let empty: Vec<String> = vec![];

        assert!(should_include("SELECT * FROM block", &block, &empty));
        assert!(!should_include("SELECT * FROM block", &other, &empty));
        assert!(!should_include("SELECT * FROM block", &empty, &block));
        assert!(should_include("SELECT * FROM block", &empty, &other));
        assert!(should_include("SELECT * FROM block", &empty, &empty));

        // A table name appearing only inside a quoted string must not match.
        assert!(!should_include(
            "INSERT INTO foo VALUES ('block')",
            &block,
            &empty
        ));
    }

    #[test]
    fn inline_named_params_substitutes() {
        let out = inline_named_params("SELECT $id, $name", r#"id=Integer(5) name=String("bob")"#);
        assert_eq!(out, "SELECT 5, 'bob'");
        assert_eq!(inline_named_params("SELECT $x", "x=Null"), "SELECT NULL");
    }

    #[test]
    fn inline_positional_params_substitutes_in_order() {
        let out = inline_positional_params(
            "INSERT INTO t VALUES (?, ?, ?)",
            r#"[Text("a"), Integer(5), Null]"#,
        );
        assert_eq!(out, "INSERT INTO t VALUES ('a', 5, NULL)");
    }

    #[test]
    fn parse_timestamp_truncates_submicros_and_rejects_garbage() {
        assert!(parse_timestamp("2026-03-19T17:16:01.730043").is_ok());
        // >6 fractional digits are truncated to microseconds, still valid.
        assert!(parse_timestamp("2026-03-19T17:16:01.730043999").is_ok());
        assert!(parse_timestamp("not-a-timestamp").is_err());
    }

    #[test]
    fn directive_preview_renders_sql_and_others() {
        assert_eq!(
            directive_preview(&Directive::Sql {
                tag: "t".into(),
                sql: "SELECT 1".into(),
            }),
            "SELECT 1"
        );
        assert_eq!(directive_preview(&Directive::Wait(5)), "Wait(5)");
    }

    #[test]
    fn write_then_parse_round_trips_directives() {
        let original = vec![
            Directive::SetChangeCallback,
            Directive::Wait(5),
            Directive::Sql {
                tag: "actor_exec".into(),
                sql: "INSERT INTO t VALUES (1)".into(),
            },
            Directive::Assert(AssertKind::RowCount {
                table: "t".into(),
                count: 1,
            }),
        ];
        let tmp = tempfile::NamedTempFile::with_suffix(".sql").unwrap();
        let path = tmp.path().to_str().unwrap();
        write_directives(path, &original).unwrap();

        let parsed = parse_replay_file(path).unwrap();
        let fmt = |ds: &[Directive]| ds.iter().map(|d| format!("{d:?}")).collect::<Vec<_>>();
        assert_eq!(fmt(&parsed), fmt(&original));
    }

    #[test]
    fn find_kw_depth0_skips_subqueries_and_literals() {
        // Outer SELECT is the first depth-0 hit; the subquery's SELECT is at
        // depth 1 and must be skipped.
        let sql = "SELECT a FROM (SELECT b FROM t) x";
        assert_eq!(find_kw_depth0(sql, "SELECT", 0), Some(0));
        assert_eq!(find_kw_depth0(sql, "FROM", "SELECT".len()), Some(9));
        // A recursive CTE: the outer SELECT follows the parenthesised CTE body.
        let cte = "WITH RECURSIVE r AS (SELECT 1 UNION SELECT n+1 FROM r) SELECT d.* FROM r d";
        let sel = find_kw_depth0(cte, "SELECT", 0).unwrap();
        assert_eq!(&cte[sel..sel + 6], "SELECT");
        assert!(cte[sel..].starts_with("SELECT d.*"));
        // Keyword inside a string literal is ignored.
        assert_eq!(find_kw_depth0("SELECT 'FROM' AS x", "FROM", 6), None);
        // Whole-word only: FROMAGE must not match FROM.
        assert_eq!(find_kw_depth0("SELECT fromage", "FROM", 0), None);
    }

    #[test]
    fn strip_trailing_order_limit_removes_tails_keeps_windows() {
        assert_eq!(
            strip_trailing_order_limit("SELECT a FROM t ORDER BY a LIMIT 10"),
            "SELECT a FROM t"
        );
        assert_eq!(
            strip_trailing_order_limit("SELECT a FROM t LIMIT 5"),
            "SELECT a FROM t"
        );
        assert_eq!(
            strip_trailing_order_limit("SELECT a FROM t;"),
            "SELECT a FROM t"
        );
        // A window ORDER BY is inside parens (depth > 0) and must survive.
        let win = "SELECT row_number() OVER (ORDER BY a) FROM t";
        assert_eq!(strip_trailing_order_limit(win), win);
    }

    #[test]
    fn parse_divergence_signature_kinds() {
        assert_eq!(
            parse_divergence_signature("!!! CHK|commit7|focus_roots|MV=10|RC=9  <<< MISMATCH"),
            Some("chk-mismatch:focus_roots".to_string())
        );
        assert_eq!(
            parse_divergence_signature("[stmt#3] INCONSISTENCY in block: matview=5, fresh=4"),
            Some("inconsistency:block".to_string())
        );
        assert_eq!(
            parse_divergence_signature(
                "[panic] thread=main at src/x.rs:12:5: multiset went negative"
            ),
            Some("panic:multiset went negative".to_string())
        );
        assert_eq!(parse_divergence_signature("all good here"), None);
    }

    #[test]
    fn parse_first_divergence_commit_reads_number() {
        assert_eq!(
            parse_first_divergence_commit("First divergence: commit#42 view=x MV=1 RC=0"),
            Some(42)
        );
        assert_eq!(parse_first_divergence_commit("no divergence"), None);
    }

    #[test]
    fn nth_transaction_bounds_finds_kth_tx() {
        let d = vec![
            Directive::Sql {
                tag: "actor_ddl".into(),
                sql: "CREATE TABLE t (id TEXT)".into(),
            },
            Directive::Sql {
                tag: "actor_tx_begin".into(),
                sql: "BEGIN".into(),
            },
            Directive::Sql {
                tag: "actor_exec".into(),
                sql: "INSERT INTO t VALUES ('a')".into(),
            },
            Directive::Sql {
                tag: "actor_tx_commit".into(),
                sql: "COMMIT".into(),
            },
            Directive::Sql {
                tag: "actor_tx_begin".into(),
                sql: "BEGIN".into(),
            },
            Directive::Sql {
                tag: "actor_exec".into(),
                sql: "INSERT INTO t VALUES ('b')".into(),
            },
            Directive::Sql {
                tag: "actor_tx_commit".into(),
                sql: "COMMIT".into(),
            },
        ];
        assert_eq!(nth_transaction_bounds(&d, 1), Some((1, 3)));
        assert_eq!(nth_transaction_bounds(&d, 2), Some((4, 6)));
        assert_eq!(nth_transaction_bounds(&d, 3), None);
    }
}
