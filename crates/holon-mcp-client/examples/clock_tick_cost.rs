//! What one `clock` tick costs on a copy of a real database, with the shipped
//! sidecar views that read the clock.
//!
//! Usage (RELEASE, on a COPY — the run writes to the file):
//!   cargo run --release -p holon-mcp-client --example clock_tick_cost -- <db
//! copy> [sessions] [events]
//!
//! Prints every view whose SQL reads `clock`, then for the `minute` and `hour`
//! grains the latency of the tick write (IVM maintenance runs inside it) and
//! which relations sent CDC for it.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use holon_mcp_client::integration_config::IntegrationFileConfig;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

const TICKS: usize = 30;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("usage: clock_tick_cost <db copy> [sessions] [events]"))?;
    let sessions: usize = args.get(2).map_or(Ok(200), |s| s.parse())?;
    let events: usize = args.get(3).map_or(Ok(500), |s| s.parse())?;

    let db = TursoBackend::open_database(path)?;
    let (cdc_tx, mut cdc_rx) = broadcast::channel(1 << 16);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx)?;

    for file in ["claude-history.yaml", "gcal.yaml"] {
        let yaml = std::fs::read_to_string(format!(
            "{}/../../assets/integrations/{file}",
            env!("CARGO_MANIFEST_DIR")
        ))?;
        let config: IntegrationFileConfig = serde_yaml::from_str(&yaml)?;
        let prefix = config.entity_prefix.unwrap_or_default();
        for view in config.views {
            reconcile_named_view(&handle, &format!("{prefix}{}", view.name), &view.sql).await?;
        }
    }
    seed(&handle, sessions, events).await?;

    let reading = handle
        .query(
            "SELECT name FROM sqlite_schema WHERE type = 'view' AND sql LIKE '%clock%' ORDER BY name",
            HashMap::new(),
        )
        .await?;
    let all = handle
        .query(
            "SELECT count(*) AS n FROM sqlite_schema WHERE type = 'view'",
            HashMap::new(),
        )
        .await?;
    println!("views in the database: {:?}", all[0].get("n"));
    println!("views whose SQL reads clock:");
    for row in &reading {
        println!("  {:?}", row.get("name"));
    }

    for grain in ["minute", "hour"] {
        let mut millis = Vec::with_capacity(TICKS);
        let mut touched: BTreeMap<String, usize> = BTreeMap::new();
        for i in 0..TICKS {
            while cdc_rx.try_recv().is_ok() {}
            let at = format!("2026-09-29T{:02}:{:02}:00+00:00", 12 + i / 60, i % 60);
            let started = Instant::now();
            handle
                .execute(
                    "UPDATE clock SET epoch_day = epoch_day + 1, updated_at = ? WHERE grain = ?",
                    vec![
                        turso::Value::Text(at),
                        turso::Value::Text(grain.to_string()),
                    ],
                )
                .await?;
            millis.push(started.elapsed().as_secs_f64() * 1000.0);
            tokio::time::sleep(Duration::from_millis(50)).await;
            while let Ok(batch) = cdc_rx.try_recv() {
                for change in batch.inner.items {
                    *touched.entry(change.relation_name).or_default() += 1;
                }
            }
        }
        millis.sort_by(f64::total_cmp);
        println!(
            "{grain} tick over {TICKS} ticks: p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
            millis[TICKS / 2],
            millis[TICKS * 95 / 100],
            millis[TICKS - 1]
        );
        println!("  CDC rows per relation over all ticks: {touched:?}");
    }
    Ok(())
}

/// `sessions` sessions whose assistant spoke in the last 20 minutes of the
/// tick range, and `events` calendar events over the next two weeks.
async fn seed(handle: &DbHandle, sessions: usize, events: usize) -> anyhow::Result<()> {
    for grain in ["minute", "hour"] {
        handle
            .execute(
                "INSERT OR IGNORE INTO clock (grain, today, epoch_day, updated_at) VALUES (?, \
                 'seed', 0, '2026-09-29T12:00:00+00:00')",
                vec![turso::Value::Text(grain.to_string())],
            )
            .await?;
    }
    for s in 0..sessions {
        for (role, minute) in [("user", s % 20), ("assistant", s % 20 + 1)] {
            handle
                .execute(
                    "INSERT INTO cc_message (uuid, session_id, role, timestamp) VALUES (?, ?, ?, ?)",
                    vec![
                        turso::Value::Text(format!("tick-cost-{s}-{role}")),
                        turso::Value::Text(format!("tick-cost-{s}")),
                        turso::Value::Text(role.to_string()),
                        turso::Value::Text(format!("2026-09-29T12:{minute:02}:00.000Z")),
                    ],
                )
                .await?;
        }
    }
    for e in 0..events {
        handle
            .execute(
                "INSERT INTO gcal_event (id, summary, start, end_time, all_day, status) VALUES \
                 (?, 'tick cost', ?, '', 0, 'confirmed')",
                vec![
                    turso::Value::Text(format!("tick-cost-{e}")),
                    turso::Value::Text(format!(
                        "2026-{:02}-{:02}T{:02}:00:00",
                        if e % 28 < 2 { 9 } else { 10 },
                        if e % 28 < 2 { 29 + e % 28 } else { e % 28 - 1 },
                        e % 24
                    )),
                ],
            )
            .await?;
    }
    Ok(())
}
