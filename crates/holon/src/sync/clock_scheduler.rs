//! The clock scheduler: time-as-data (ADR 0024 P5).
//!
//! `date('now')` is non-deterministic and Turso rejects it as a matview source
//! (BugFunnel F4). Instead the `clock` relation carries a materialized `today`
//! *value*; a temporal guard is then a plain join that re-fires on day-rollover
//! because the `UPDATE` emits CDC.
//!
//! This actor owns a [`DbHandle`] and an injected [`Clock`]. It never reads the
//! OS clock directly — the keystone `AdvanceDay` transition advances a *fake*
//! `Clock` and lets the real prod path propagate the new day, so there is no
//! clock race. On boot it reconciles once synchronously (replacing the schema's
//! placeholder row with the real local date) before the ticking task starts, so
//! by the time any temporal-guard matview is created the row already holds the
//! real day.
//!
//! The write is a **direct projection write** (A3), not a block intent, issued
//! on **any** change — forward *or* backward (DST fall-back / timezone travel
//! west) — because deterministic effect IDs (WP2) make the re-fire converge.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use holon_api::clock::CalendarDate;
use holon_api::clock::Clock;
use holon_api::clock::Grain;
use holon_api::streaming::ActorAbortGuard;

use crate::storage::turso::DbHandle;

/// Keeps the scheduler's ticking task alive. Dropping it aborts the task
/// (mirrors `AdviceReconcilerHandle`).
pub struct ClockSchedulerHandle {
    _abort: ActorAbortGuard,
}

/// Outcome of one reconcile pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockTick {
    /// The injected clock's local day already matched the stored row.
    Unchanged,
    /// The row advanced (forward or backward) to a new day.
    Advanced { today: String, epoch_day: i64 },
}

/// Compute the injected clock's local day and, if it differs from the stored
/// `clock` row, write it back via a direct projection `UPDATE`. Returns whether
/// it wrote. Fails loud if the seeded `day` row is missing.
pub async fn reconcile_clock(db_handle: &DbHandle, clock: &dyn Clock) -> Result<ClockTick> {
    let cal = CalendarDate::from_clock(clock);
    let today = cal.ymd();
    let epoch_day = cal.epoch_day();

    let grain = Grain::Day.as_str();
    let rows = db_handle
        .query(
            &format!("SELECT epoch_day FROM clock WHERE grain = '{grain}'"),
            HashMap::new(),
        )
        .await
        .context("reading the clock day row")?;
    let stored_epoch = rows
        .first()
        .ok_or_else(|| {
            anyhow!("clock row for grain '{grain}' is missing — schema seed did not run")
        })?
        .get("epoch_day")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("clock.epoch_day is not an integer"))?;

    if stored_epoch == epoch_day {
        return Ok(ClockTick::Unchanged);
    }

    let updated_at = chrono::DateTime::from_timestamp_millis(clock.now_millis())
        .ok_or_else(|| anyhow!("clock now_millis out of DateTime range"))?
        .to_rfc3339();

    db_handle
        .execute(
            "UPDATE clock SET today = ?, epoch_day = ?, updated_at = ? WHERE grain = 'day'",
            vec![
                turso::Value::Text(today.clone()),
                turso::Value::Integer(epoch_day),
                turso::Value::Text(updated_at),
            ],
        )
        .await
        .context("writing the advanced clock day row")?;

    Ok(ClockTick::Advanced { today, epoch_day })
}

/// Spawn the clock scheduler. Reconciles once synchronously (boot seed) so the
/// `clock` row holds the real local date before returning, then ticks on
/// `interval` to catch day-rollover. Returns a handle that must be held for the
/// scheduler to keep running.
pub async fn spawn_clock_scheduler(
    db_handle: DbHandle,
    clock: Arc<dyn Clock>,
    interval: Duration,
) -> Result<ClockSchedulerHandle> {
    // Boot seed — must succeed so the boot guard finds a live, real day row.
    let first = reconcile_clock(&db_handle, clock.as_ref())
        .await
        .context("clock scheduler boot reconcile")?;
    tracing::info!(?first, "[ClockScheduler] boot reconcile complete");

    let mut aborts = ActorAbortGuard::new();
    let task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // The immediate first tick is redundant with the boot seed above; skip it.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match reconcile_clock(&db_handle, clock.as_ref()).await {
                Ok(ClockTick::Advanced { today, epoch_day }) => {
                    tracing::info!(%today, epoch_day, "[ClockScheduler] day advanced");
                }
                Ok(ClockTick::Unchanged) => {}
                Err(e) => {
                    tracing::error!(error = %format!("{e:#}"), "[ClockScheduler] reconcile failed");
                }
            }
        }
    });
    aborts.push(task.abort_handle());

    Ok(ClockSchedulerHandle { _abort: aborts })
}

#[cfg(test)]
mod tests {
    use holon_api::clock::TestClock;
    use holon_api::streaming::Change;
    use holon_turso::schema_module::SchemaModule;
    use holon_turso::schema_modules::CoreSchemaModule;

    use super::*;
    use crate::storage::turso::TursoBackend;

    /// millis for local-noon on the given date — timezone-robust (noon UTC
    /// lands on the same civil date from UTC-12 to UTC+12).
    fn noon_utc_millis(y: i32, m: u32, d: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    async fn booted_clock_db() -> DbHandle {
        let (_backend, handle) = TursoBackend::new_in_memory().await.unwrap();
        CoreSchemaModule.ensure_schema(&handle).await.unwrap();
        std::mem::forget(_backend);
        handle
    }

    #[tokio::test]
    async fn day_forward_advance_writes_once_and_emits_one_cdc_update() {
        let handle = booted_clock_db().await;
        let clock = TestClock::new(noon_utc_millis(2026, 7, 10));

        // Boot seed: placeholder(1970) -> 2026-07-10.
        let first = reconcile_clock(&handle, &clock).await.unwrap();
        assert!(matches!(first, ClockTick::Advanced { .. }));

        // Watch the day value through a mirror matview (base tables never emit).
        handle
            .execute_ddl(
                "CREATE MATERIALIZED VIEW clock_mirror AS SELECT grain, today, epoch_day FROM \
                 clock",
            )
            .await
            .unwrap();
        let mut cdc_rx = handle.subscribe_cdc("clock_mirror").await.unwrap();

        // Advance the fake clock exactly one day.
        clock.set(noon_utc_millis(2026, 7, 11));
        let tick = reconcile_clock(&handle, &clock).await.unwrap();
        assert_eq!(
            tick,
            ClockTick::Advanced {
                today: "2026-07-11".into(),
                epoch_day: CalendarDate::parse("2026-07-11").unwrap().epoch_day(),
            }
        );

        // Row advanced.
        let rows = handle
            .query(
                "SELECT today FROM clock WHERE grain = 'day'",
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            rows[0].get("today").unwrap().as_string(),
            Some("2026-07-11")
        );

        // Exactly one CDC Updated fired for the advance.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut updates = 0usize;
        while let Ok(batch) = cdc_rx.try_recv() {
            for rc in batch.inner.items {
                if rc.relation_name == "clock_mirror" {
                    if let Change::Updated { data, .. } = &rc.change {
                        assert_eq!(data.get("today").unwrap().as_string(), Some("2026-07-11"));
                        updates += 1;
                    }
                }
            }
        }
        assert_eq!(
            updates, 1,
            "a day advance must emit exactly one CDC Updated"
        );
    }

    #[tokio::test]
    async fn backwards_day_change_still_writes() {
        let handle = booted_clock_db().await;
        let clock = TestClock::new(noon_utc_millis(2026, 7, 10));
        reconcile_clock(&handle, &clock).await.unwrap();

        // DST fall-back / travel west: the local day moves *earlier*.
        clock.set(noon_utc_millis(2026, 7, 9));
        let tick = reconcile_clock(&handle, &clock).await.unwrap();
        assert_eq!(
            tick,
            ClockTick::Advanced {
                today: "2026-07-09".into(),
                epoch_day: CalendarDate::parse("2026-07-09").unwrap().epoch_day(),
            },
            "the scheduler must write on ANY change, including backwards"
        );
        let rows = handle
            .query(
                "SELECT today FROM clock WHERE grain = 'day'",
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            rows[0].get("today").unwrap().as_string(),
            Some("2026-07-09")
        );
    }

    #[tokio::test]
    async fn no_change_tick_writes_nothing() {
        let handle = booted_clock_db().await;
        let clock = TestClock::new(noon_utc_millis(2026, 7, 10));
        reconcile_clock(&handle, &clock).await.unwrap();

        // Same day, a few hours later: no day change, no write.
        clock.set(noon_utc_millis(2026, 7, 10) + 3 * 3_600_000);
        let tick = reconcile_clock(&handle, &clock).await.unwrap();
        assert_eq!(tick, ClockTick::Unchanged);
    }
}
