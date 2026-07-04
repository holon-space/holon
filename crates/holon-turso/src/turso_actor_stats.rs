//! Per-command counters for the Turso actor loop.
//!
//! Enabled by setting `HOLON_ACTOR_STATS=<interval-seconds>` (or `=1` for the
//! 5s default). Every interval, the spawned logger task drains and logs a
//! summary of which `DbCommand` variants and SQL statements the actor was
//! processing, plus how many CDC events each relation produced. This is the
//! diagnostic that originally pinned a constant ~40% CPU usage on the
//! `holon-mcp --stdio` process to a runaway IVM circuit re-firing on a
//! particular query/CDC pattern — see `devlog/2026-05-13-mcp-cpu.md`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use super::turso::DbCommand;

pub fn enabled_interval() -> Option<Duration> {
    static IV: OnceLock<Option<Duration>> = OnceLock::new();
    *IV.get_or_init(|| {
        let raw = std::env::var("HOLON_ACTOR_STATS").ok()?;
        let secs = match raw.as_str() {
            "" | "0" => return None,
            "1" => 5,
            other => other.parse::<u64>().unwrap_or_else(|e| {
                panic!("HOLON_ACTOR_STATS={other:?} is not a non-negative integer (seconds): {e}")
            }),
        };
        Some(Duration::from_secs(secs))
    })
}

pub struct ActorStats {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    by_variant: HashMap<&'static str, (u64, Duration)>,
    by_sql: HashMap<String, (u64, Duration)>,
    cdc_events: HashMap<String, (u64, u64)>,
}

impl ActorStats {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
        })
    }

    pub fn record_command(
        &self,
        variant: &'static str,
        sql_key: Option<String>,
        elapsed: Duration,
    ) {
        let mut inner = self.inner.lock().expect("ActorStats inner poisoned");
        let v = inner.by_variant.entry(variant).or_default();
        v.0 += 1;
        v.1 += elapsed;
        if let Some(key) = sql_key {
            let s = inner.by_sql.entry(key).or_default();
            s.0 += 1;
            s.1 += elapsed;
        }
    }

    pub fn record_cdc(&self, relation: &str, raw_changes: u64, coalesced: u64) {
        let mut inner = self.inner.lock().expect("ActorStats inner poisoned");
        let bucket = inner.cdc_events.entry(relation.to_string()).or_default();
        bucket.0 += raw_changes;
        bucket.1 += coalesced;
    }

    pub fn drain_and_log(&self, window: Duration) {
        let snapshot: Inner = {
            let mut inner = self.inner.lock().expect("ActorStats inner poisoned");
            std::mem::take(&mut *inner)
        };
        if snapshot.by_variant.is_empty()
            && snapshot.by_sql.is_empty()
            && snapshot.cdc_events.is_empty()
        {
            return;
        }

        let total_cmds: u64 = snapshot.by_variant.values().map(|(c, _)| *c).sum();
        let total_time: Duration = snapshot.by_variant.values().map(|(_, t)| *t).sum();
        let busy_pct = (total_time.as_secs_f64() / window.as_secs_f64()) * 100.0;

        let mut variants: Vec<_> = snapshot.by_variant.into_iter().collect();
        variants.sort_by_key(|(_, (_, t))| std::cmp::Reverse(*t));
        let by_var_str = variants
            .iter()
            .take(8)
            .map(|(v, (c, t))| format!("{v}={c}({}ms)", t.as_millis()))
            .collect::<Vec<_>>()
            .join(" ");

        tracing::info!(
            target: "holon::storage::turso::actor_stats",
            window_secs = window.as_secs_f64(),
            cmds = total_cmds,
            busy_pct = busy_pct,
            "[actor-stats] cmds={total_cmds} busy={busy_pct:.1}% top: {by_var_str}"
        );

        let mut sqls: Vec<_> = snapshot.by_sql.into_iter().collect();
        sqls.sort_by_key(|(_, (_, t))| std::cmp::Reverse(*t));
        for (sql, (count, time)) in sqls.into_iter().take(5) {
            tracing::info!(
                target: "holon::storage::turso::actor_stats",
                count = count,
                total_ms = time.as_secs_f64() * 1000.0,
                "[actor-stats sql] {count}x {:.1}ms total: {sql}",
                time.as_secs_f64() * 1000.0,
            );
        }

        if !snapshot.cdc_events.is_empty() {
            let mut cdc: Vec<_> = snapshot.cdc_events.into_iter().collect();
            cdc.sort_by_key(|(_, (raw, _))| std::cmp::Reverse(*raw));
            let cdc_str = cdc
                .iter()
                .take(8)
                .map(|(rel, (raw, coalesced))| format!("{rel}={raw}->{coalesced}"))
                .collect::<Vec<_>>()
                .join(" ");
            tracing::info!(
                target: "holon::storage::turso::actor_stats",
                "[actor-stats cdc] raw->coalesced per relation: {cdc_str}"
            );
        }
    }
}

pub fn cmd_fingerprint(cmd: &DbCommand) -> (&'static str, Option<&str>) {
    match cmd {
        DbCommand::Query { sql, .. } => ("Query", Some(sql.as_str())),
        DbCommand::QueryPositional { sql, .. } => ("QueryPositional", Some(sql.as_str())),
        DbCommand::Execute { sql, .. } => ("Execute", Some(sql.as_str())),
        DbCommand::ExecuteDdl { sql, .. } => ("ExecuteDdl", Some(sql.as_str())),
        DbCommand::ExecuteDdlWithDeps { sql, .. } => ("ExecuteDdlWithDeps", Some(sql.as_str())),
        DbCommand::ExecuteDdlAuto { sql, .. } => ("ExecuteDdlAuto", Some(sql.as_str())),
        DbCommand::MarkAvailable { .. } => ("MarkAvailable", None),
        DbCommand::ResourceExists { .. } => ("ResourceExists", None),
        DbCommand::Transaction { .. } => ("Transaction", None),
        DbCommand::SubscribeCdc { .. } => ("SubscribeCdc", None),
        DbCommand::TransitionToReady { .. } => ("TransitionToReady", None),
        DbCommand::GetPhase { .. } => ("GetPhase", None),
        DbCommand::RegisterForeignTable { .. } => ("RegisterForeignTable", None),
        DbCommand::AcquireViewLease { select_sql, .. } => {
            ("AcquireViewLease", Some(select_sql.as_str()))
        }
        DbCommand::ReleaseViewLease { .. } => ("ReleaseViewLease", None),
        DbCommand::EnsurePinnedView { select_sql, .. } => {
            ("EnsurePinnedView", Some(select_sql.as_str()))
        }
        DbCommand::ResetWatchViews { .. } => ("ResetWatchViews", None),
        DbCommand::Shutdown { .. } => ("Shutdown", None),
    }
}

pub fn fingerprint_sql(sql: &str) -> String {
    let mut buf = String::with_capacity(sql.len().min(120));
    let mut last_was_space = false;
    for c in sql.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                buf.push(' ');
                last_was_space = true;
            }
        } else {
            buf.push(c);
            last_was_space = false;
        }
    }
    let trimmed = buf.trim();
    let truncated: String = trimmed.chars().take(100).collect();
    if truncated.chars().count() < trimmed.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

pub fn spawn_logger(stats: Arc<ActorStats>, interval: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tick.tick().await;
            stats.drain_and_log(interval);
        }
    });
}
