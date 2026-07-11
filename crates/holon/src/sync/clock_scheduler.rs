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
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use holon_api::clock::Clock;
use holon_api::clock::Grain;
use holon_api::streaming::ActorAbortGuard;

use crate::storage::turso::DbHandle;

/// Reader-refcount per fine grain (C6 write-amplification gate). `Day` is
/// always-on (not counted): it is the temporal-guard / journal backbone, coarse
/// enough that its at-most-one-write-per-day cost is unconditional. `Hour` and
/// `Minute` tick only while at least one net holds a subscription, so a vault
/// with no minute-resolution net never emits minute-rollover CDC.
#[derive(Default)]
struct GrainSubscriptions {
    counts: Mutex<HashMap<Grain, usize>>,
}

impl GrainSubscriptions {
    /// Increment `grain`'s refcount; returns the new count.
    fn incr(&self, grain: Grain) -> usize {
        let mut counts = self.counts.lock().expect("grain subscription lock");
        let c = counts.entry(grain).or_insert(0);
        *c += 1;
        *c
    }

    /// Decrement `grain`'s refcount; returns the new count. Panics on underflow
    /// — that would mean a [`GrainSubscription`] was dropped twice.
    fn decr(&self, grain: Grain) -> usize {
        let mut counts = self.counts.lock().expect("grain subscription lock");
        let c = counts
            .get_mut(&grain)
            .expect("decrement of an unheld grain subscription");
        *c = c
            .checked_sub(1)
            .expect("grain subscription refcount underflow");
        *c
    }

    /// Grains the scheduler must reconcile this tick: `Day` (always) plus every
    /// fine grain with a live reader.
    fn active(&self) -> Vec<Grain> {
        let counts = self.counts.lock().expect("grain subscription lock");
        let mut grains = vec![Grain::Day];
        for (&grain, &c) in counts.iter() {
            if grain != Grain::Day && c > 0 {
                grains.push(grain);
            }
        }
        grains
    }
}

/// Keeps the scheduler's ticking task alive and hands out fine-grain
/// subscriptions. Dropping it aborts the task (mirrors
/// `AdviceReconcilerHandle`).
pub struct ClockSchedulerHandle {
    _abort: ActorAbortGuard,
    subs: Arc<GrainSubscriptions>,
    db_handle: DbHandle,
    clock: Arc<dyn Clock>,
}

/// A live reader of a fine clock grain. While one exists the scheduler ticks
/// that grain; dropping the last one lets it fall idle (the row keeps its last
/// value — no reader observes it, and a re-subscribe reconciles it forward).
/// Handed out by [`ClockSchedulerHandle::subscribe`]; this is the desugaring
/// seat for `every(<interval>)` guards (each holds the subscription for its
/// grain).
#[must_use = "dropping the subscription immediately lets its grain fall idle"]
pub struct GrainSubscription {
    grain: Grain,
    subs: Arc<GrainSubscriptions>,
}

impl Drop for GrainSubscription {
    fn drop(&mut self) {
        self.subs.decr(self.grain);
    }
}

impl ClockSchedulerHandle {
    /// Register interest in a fine `grain`. On the first subscriber the grain's
    /// row is created and reconciled to the current instant (so a guard reading
    /// it sees a live value immediately); thereafter the ticking task keeps
    /// it fresh. Subscribing [`Grain::Day`] is legal but redundant (day is
    /// always-on).
    pub async fn subscribe(&self, grain: Grain) -> Result<GrainSubscription> {
        let first = self.subs.incr(grain) == 1;
        if first && grain != Grain::Day {
            ensure_grain_row(&self.db_handle, self.clock.as_ref(), grain)
                .await
                .with_context(|| format!("seeding clock row for grain {}", grain.as_str()))?;
        }
        Ok(GrainSubscription {
            grain,
            subs: self.subs.clone(),
        })
    }
}

/// Outcome of one reconcile pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockTick {
    /// The injected clock's local day already matched the stored row.
    Unchanged,
    /// The row advanced (forward or backward) to a new day.
    Advanced { today: String, epoch_day: i64 },
}

/// Reconcile the `day` grain (the always-on path). Thin wrapper over
/// [`reconcile_grain`] kept for the external drivers that advance the injected
/// clock and re-fire the journal rule.
pub async fn reconcile_clock(db_handle: &DbHandle, clock: &dyn Clock) -> Result<ClockTick> {
    reconcile_grain(db_handle, clock, Grain::Day).await
}

/// Compute the injected clock's local value at `grain` and, if it differs from
/// the stored `clock` row, write it back via a direct projection `UPDATE`.
/// Returns whether it wrote. Fails loud if the grain's row is missing — for
/// `Day` that means the schema seed did not run; for a fine grain it means
/// [`ensure_grain_row`] (via `subscribe`) did not run first.
pub async fn reconcile_grain(
    db_handle: &DbHandle,
    clock: &dyn Clock,
    grain: Grain,
) -> Result<ClockTick> {
    let sample = grain.sample(clock);
    let grain_str = grain.as_str();

    let rows = db_handle
        .query_positional(
            "SELECT epoch_day FROM clock WHERE grain = ?",
            vec![turso::Value::Text(grain_str.to_string())],
        )
        .await
        .with_context(|| format!("reading the clock row for grain '{grain_str}'"))?;
    let stored_tick = rows
        .first()
        .ok_or_else(|| {
            anyhow!("clock row for grain '{grain_str}' is missing — not seeded/subscribed")
        })?
        .get("epoch_day")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("clock.epoch_day is not an integer"))?;

    if stored_tick == sample.tick {
        return Ok(ClockTick::Unchanged);
    }

    let updated_at = chrono::DateTime::from_timestamp_millis(clock.now_millis())
        .ok_or_else(|| anyhow!("clock now_millis out of DateTime range"))?
        .to_rfc3339();

    db_handle
        .execute(
            "UPDATE clock SET today = ?, epoch_day = ?, updated_at = ? WHERE grain = ?",
            vec![
                turso::Value::Text(sample.label.clone()),
                turso::Value::Integer(sample.tick),
                turso::Value::Text(updated_at),
                turso::Value::Text(grain_str.to_string()),
            ],
        )
        .await
        .with_context(|| format!("writing the advanced clock row for grain '{grain_str}'"))?;

    Ok(ClockTick::Advanced {
        today: sample.label,
        epoch_day: sample.tick,
    })
}

/// Create a fine grain's `clock` row at the current instant if absent, then
/// reconcile it forward. Idempotent (`INSERT OR IGNORE`), so a re-subscribe
/// after idle finds the stale row and advances it via the following reconcile.
async fn ensure_grain_row(db_handle: &DbHandle, clock: &dyn Clock, grain: Grain) -> Result<()> {
    let sample = grain.sample(clock);
    let updated_at = chrono::DateTime::from_timestamp_millis(clock.now_millis())
        .ok_or_else(|| anyhow!("clock now_millis out of DateTime range"))?
        .to_rfc3339();
    db_handle
        .execute(
            "INSERT OR IGNORE INTO clock (grain, today, epoch_day, updated_at) VALUES (?, ?, ?, ?)",
            vec![
                turso::Value::Text(grain.as_str().to_string()),
                turso::Value::Text(sample.label),
                turso::Value::Integer(sample.tick),
                turso::Value::Text(updated_at),
            ],
        )
        .await
        .with_context(|| format!("inserting the clock row for grain '{}'", grain.as_str()))?;
    reconcile_grain(db_handle, clock, grain).await?;
    Ok(())
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

    let subs = Arc::new(GrainSubscriptions::default());

    let mut aborts = ActorAbortGuard::new();
    let task = {
        let db_handle = db_handle.clone();
        let clock = clock.clone();
        let subs = subs.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // The immediate first tick is redundant with the boot seed above; skip it.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                // Reconcile only the grains with a live reader (Day always). A
                // fine grain with no subscriber is never touched — the C6
                // write-amplification gate.
                for grain in subs.active() {
                    match reconcile_grain(&db_handle, clock.as_ref(), grain).await {
                        Ok(ClockTick::Advanced { today, epoch_day }) => {
                            tracing::info!(
                                grain = grain.as_str(),
                                %today,
                                epoch_day,
                                "[ClockScheduler] grain advanced"
                            );
                        }
                        Ok(ClockTick::Unchanged) => {}
                        Err(e) => {
                            tracing::error!(
                                grain = grain.as_str(),
                                error = %format!("{e:#}"),
                                "[ClockScheduler] reconcile failed"
                            );
                        }
                    }
                }
            }
        })
    };
    aborts.push(task.abort_handle());

    Ok(ClockSchedulerHandle {
        _abort: aborts,
        subs,
        db_handle,
        clock,
    })
}

#[cfg(test)]
mod tests {
    use holon_api::clock::CalendarDate;
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

    // --- C6: fine grains + recurrence -----------------------------------------

    fn hour_rows_query(handle: &DbHandle) -> impl std::future::Future<Output = usize> + '_ {
        async move {
            handle
                .query_positional(
                    "SELECT epoch_day FROM clock WHERE grain = ?",
                    vec![turso::Value::Text("hour".to_string())],
                )
                .await
                .unwrap()
                .len()
        }
    }

    /// The write-amplification gate, at the logic level: `active()` never
    /// returns an unsubscribed fine grain, and always returns `Day`.
    #[test]
    fn subscriptions_gate_active_grains() {
        let subs = GrainSubscriptions::default();
        assert_eq!(subs.active(), vec![Grain::Day], "day is always-on");

        assert_eq!(subs.incr(Grain::Hour), 1);
        assert_eq!(subs.incr(Grain::Hour), 2, "refcounted");
        let active = subs.active();
        assert!(active.contains(&Grain::Day) && active.contains(&Grain::Hour));

        assert_eq!(subs.decr(Grain::Hour), 1);
        assert!(subs.active().contains(&Grain::Hour), "still one reader");
        assert_eq!(subs.decr(Grain::Hour), 0);
        assert_eq!(subs.active(), vec![Grain::Day], "last reader gone -> idle");
    }

    /// End-to-end write gate: a fine grain has no `clock` row until subscribed;
    /// subscribing materializes it at the current instant.
    #[tokio::test]
    async fn subscribe_materializes_fine_grain_row_absent_otherwise() {
        let handle = booted_clock_db().await;
        let clock = TestClock::new(noon_utc_millis(2026, 7, 11));
        let scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            // Long interval: the ticking task must not fire inside the test window,
            // so every observed write comes from subscribe/reconcile, not timing.
            Duration::from_secs(3600),
        )
        .await
        .unwrap();

        assert_eq!(
            hour_rows_query(&handle).await,
            0,
            "an unsubscribed hour grain must never be materialized"
        );

        let sub = scheduler.subscribe(Grain::Hour).await.unwrap();
        assert_eq!(
            hour_rows_query(&handle).await,
            1,
            "subscribe creates the row"
        );
        let stored = handle
            .query_positional(
                "SELECT epoch_day FROM clock WHERE grain = ?",
                vec![turso::Value::Text("hour".to_string())],
            )
            .await
            .unwrap();
        assert_eq!(
            stored[0].get("epoch_day").unwrap().as_i64().unwrap(),
            Grain::Hour.sample(&clock).tick,
            "the row holds the current hour tick"
        );

        drop(sub);
        drop(scheduler);
    }

    /// `every(2 hours)` desugared to a matview toggles present/absent at each
    /// 2-hour boundary — the reactive-guard firing behaviour, driven by
    /// deterministic manual reconciles (no timing).
    #[tokio::test]
    async fn every_two_hours_read_arc_toggles_on_even_ticks() {
        use holon_api::clock::Recurrence;

        let handle = booted_clock_db().await;
        let clock = TestClock::new(noon_utc_millis(2026, 7, 11));
        let scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        let _sub = scheduler.subscribe(Grain::Hour).await.unwrap();

        let arc = Recurrence::parse("every 2 hours").unwrap().desugar();
        assert_eq!(arc.grain, Grain::Hour);
        handle
            .execute_ddl(&format!(
                "CREATE MATERIALIZED VIEW every_2h AS {}",
                arc.read_arc_sql
            ))
            .await
            .unwrap();

        // Walk four consecutive hours; the matview has a row exactly on even ticks.
        for h in 0..4i64 {
            clock.set(noon_utc_millis(2026, 7, 11) + h * 3_600_000);
            reconcile_grain(&handle, &clock, Grain::Hour).await.unwrap();
            let tick = Grain::Hour.sample(&clock).tick;
            let present = handle
                .query("SELECT tick FROM every_2h", HashMap::new())
                .await
                .unwrap()
                .len();
            let expected = if tick % 2 == 0 { 1 } else { 0 };
            assert_eq!(
                present,
                expected,
                "hour tick {tick} (parity {}) -> {expected} matview rows",
                tick % 2
            );
        }

        drop(scheduler);
    }
}
