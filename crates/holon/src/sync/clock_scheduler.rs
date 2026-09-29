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

/// Hands out fine-grain subscriptions. The ticking task belongs to the
/// session's `SessionShutdown`, which is what stops it — this handle no longer
/// owns its lifetime.
pub struct ClockSchedulerHandle {
    subs: Arc<GrainSubscriptions>,
    db_handle: DbHandle,
    clock: Arc<dyn Clock>,
}

impl ClockSchedulerHandle {
    /// Seeds a grain's row for a watch that reads it; `clock_reader` then
    /// keeps it ticking while the watch's view lives.
    pub fn grain_seeder(&self) -> Arc<dyn holon_turso::matview_manager::ClockGrainSeeder> {
        Arc::new(GrainSeeder {
            db_handle: self.db_handle.clone(),
            clock: self.clock.clone(),
        })
    }
}

struct GrainSeeder {
    db_handle: DbHandle,
    clock: Arc<dyn Clock>,
}

#[async_trait::async_trait]
impl holon_turso::matview_manager::ClockGrainSeeder for GrainSeeder {
    async fn seed(&self, grain: Grain) -> Result<()> {
        ensure_grain_row(&self.db_handle, self.clock.as_ref(), grain).await
    }
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
/// reconcile it forward. Idempotent, so a re-subscribe after idle finds the
/// stale row and advances it via the following reconcile.
async fn ensure_grain_row(db_handle: &DbHandle, clock: &dyn Clock, grain: Grain) -> Result<()> {
    seed_grain_row(db_handle, clock, grain).await?;
    reconcile_grain(db_handle, clock, grain).await?;
    Ok(())
}

/// Create a grain's `clock` row at the current instant if absent.
async fn seed_grain_row(db_handle: &DbHandle, clock: &dyn Clock, grain: Grain) -> Result<()> {
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
    shutdown: &holon_api::lifecycle::SessionShutdown,
) -> Result<ClockSchedulerHandle> {
    // Boot seed — must succeed so the boot guard finds a live, real day row.
    let first = reconcile_clock(&db_handle, clock.as_ref())
        .await
        .context("clock scheduler boot reconcile")?;
    tracing::info!(?first, "[ClockScheduler] boot reconcile complete");

    let subs = Arc::new(GrainSubscriptions::default());
    let handle = ClockSchedulerHandle {
        subs: subs.clone(),
        db_handle: db_handle.clone(),
        clock: clock.clone(),
    };

    // A view that joins a fine grain must have rows from the first read on,
    // whether or not anything subscribes the grain.
    for grain in stored_reader_grains(&db_handle).await? {
        ensure_grain_row(&db_handle, clock.as_ref(), grain)
            .await
            .with_context(|| format!("seeding the clock row a stored view reads ({grain:?})"))?;
    }

    // Registered with the session, not merely guarded by the handle: the handle
    // lives on the `BackendEngine`, which is dropped AFTER the storage actor
    // closes, so abort-on-drop is too late to keep the ticker off a dead store.
    let cancelled = shutdown.cancelled();
    {
        let db_handle = db_handle.clone();
        let clock = clock.clone();
        let subs = subs.clone();
        shutdown.spawn("clock-scheduler", async move {
            tokio::pin!(cancelled);
            let mut ticker = tokio::time::interval(interval);
            // The immediate first tick is redundant with the boot seed above; skip it.
            ticker.tick().await;
            loop {
                // `biased`: a tick that fires alongside the shutdown must not
                // start a reconcile against a store that is closing.
                tokio::select! {
                    biased;
                    () = &mut cancelled => return,
                    _ = ticker.tick() => {}
                }
                // Reconcile only the grains something reads: Day always, a
                // subscribed grain, and a grain a stored view reads (read again
                // each tick, so a dropped view stops its grain). A fine grain
                // nothing reads is never touched — the C6 write-amplification
                // gate.
                let mut grains = subs.active();
                match stored_reader_grains(&db_handle).await {
                    Ok(stored) => {
                        for grain in stored {
                            if grains.contains(&grain) {
                                continue;
                            }
                            // A sidecar view records its grain at connect, before
                            // anything seeds that grain's row.
                            if let Err(e) = seed_grain_row(&db_handle, clock.as_ref(), grain).await
                            {
                                tracing::error!(
                                    grain = grain.as_str(),
                                    error = %format!("{e:#}"),
                                    "[ClockScheduler] seeding a grain a stored view reads failed"
                                );
                            }
                            grains.push(grain);
                        }
                    }
                    Err(e) => tracing::error!(
                        error = %format!("{e:#}"),
                        "[ClockScheduler] reading the grains stored views read failed; only \
                         subscribed grains tick this round"
                    ),
                }
                for grain in grains {
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
        });
    }

    Ok(handle)
}

/// The fine grains that stored views read, as `clock_reader` records them.
async fn stored_reader_grains(db_handle: &DbHandle) -> Result<Vec<Grain>> {
    let rows = db_handle
        .query("SELECT DISTINCT grain FROM clock_reader", HashMap::new())
        .await
        .context("reading the grains stored views read (clock_reader)")?;
    rows.iter()
        .map(|row| match row.get("grain") {
            Some(holon_api::Value::String(raw)) => Grain::parse(raw),
            other => Err(anyhow!("clock_reader.grain is {other:?}, not text")),
        })
        .filter(|grain| !matches!(grain, Ok(Grain::Day)))
        .collect()
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

    /// Millis for noon UTC on the given date. The callers pair it with a
    /// zero-offset clock, so the civil date they assert is this date; a clock
    /// carrying a real offset would land these on a neighbouring day.
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
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 10), 0);

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
                if rc.relation_name == "clock_mirror"
                    && let Change::Updated { data, .. } = &rc.change
                {
                    assert_eq!(data.get("today").unwrap().as_string(), Some("2026-07-11"));
                    updates += 1;
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
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 10), 0);
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
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 10), 0);
        reconcile_clock(&handle, &clock).await.unwrap();

        // Same day, a few hours later: no day change, no write.
        clock.set(noon_utc_millis(2026, 7, 10) + 3 * 3_600_000);
        let tick = reconcile_clock(&handle, &clock).await.unwrap();
        assert_eq!(tick, ClockTick::Unchanged);
    }

    /// The scheduler's per-tick pair for a stored grain no subscriber holds
    /// (`seed_grain_row` then `reconcile_grain`) must write nothing when the
    /// grain's sample has not changed — the same no-write contract
    /// `no_change_tick_writes_nothing` pins for `day`, extended to a fine
    /// grain a stored view reads.
    #[tokio::test]
    async fn no_change_tick_writes_nothing_for_a_stored_fine_grain_either() {
        let handle = booted_clock_db().await;
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 10), 0);
        ensure_grain_row(&handle, &clock, Grain::Hour)
            .await
            .unwrap();

        handle
            .execute_ddl(
                "CREATE MATERIALIZED VIEW clock_hour_mirror AS SELECT grain, today, epoch_day, \
                 updated_at FROM clock WHERE grain = 'hour'",
            )
            .await
            .unwrap();
        let mut cdc_rx = handle.subscribe_cdc("clock_hour_mirror").await.unwrap();

        // Exactly the pair the scheduler runs each tick for a stored grain
        // nothing subscribes, with no time change.
        seed_grain_row(&handle, &clock, Grain::Hour).await.unwrap();
        let tick = reconcile_grain(&handle, &clock, Grain::Hour).await.unwrap();
        assert_eq!(tick, ClockTick::Unchanged);

        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut events = 0usize;
        while let Ok(batch) = cdc_rx.try_recv() {
            events += batch.inner.items.len();
        }
        assert_eq!(
            events, 0,
            "a stored fine grain's unchanged tick must write nothing"
        );
    }

    // --- C6: fine grains + recurrence -----------------------------------------

    async fn hour_rows_query(handle: &DbHandle) -> usize {
        handle
            .query_positional(
                "SELECT epoch_day FROM clock WHERE grain = ?",
                vec![turso::Value::Text("hour".to_string())],
            )
            .await
            .unwrap()
            .len()
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

    /// A stored view that joins a fine grain keeps working in a session where
    /// nothing subscribes that grain (its integration did not connect): the
    /// scheduler itself ticks every grain `clock_reader` records.
    #[tokio::test]
    async fn a_grain_a_stored_view_reads_ticks_with_no_subscriber() {
        let handle = booted_clock_db().await;
        for ddl in [
            "CREATE TABLE msg (id TEXT PRIMARY KEY, ts TEXT)",
            "CREATE MATERIALIZED VIEW msg_clock AS SELECT id, ts, 'minute' AS clock_grain FROM msg",
            "CREATE MATERIALIZED VIEW msg_recent AS SELECT m.id, iif(m.ts > \
             strftime('%Y-%m-%dT%H:%M:%S', c.updated_at, '-10 minutes'), 'recent', 'old') AS \
             state FROM msg_clock m JOIN clock c ON c.grain = m.clock_grain",
        ] {
            handle.execute_ddl(ddl).await.unwrap();
        }
        handle
            .execute(
                "INSERT INTO msg (id, ts) VALUES ('m1', '2026-07-11T11:55:00')",
                vec![],
            )
            .await
            .unwrap();
        handle
            .execute(
                "INSERT INTO clock_reader (view, grain) VALUES ('msg_recent', 'minute')",
                vec![],
            )
            .await
            .unwrap();

        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 11), 0);
        let _scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            Duration::from_millis(20),
            &holon_api::lifecycle::SessionShutdown::new(),
        )
        .await
        .unwrap();

        let state = || async {
            handle
                .query("SELECT state FROM msg_recent", HashMap::new())
                .await
                .unwrap()
                .iter()
                .map(|r| format!("{:?}", r.get("state")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            state().await,
            vec![format!(
                "{:?}",
                Some(holon_api::Value::String("recent".into()))
            )],
            "a view that joins a grain a stored view reads must hold its rows with no subscriber"
        );

        clock.advance(11 * 60_000);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let now = state().await;
            if now
                == vec![format!(
                    "{:?}",
                    Some(holon_api::Value::String("old".into()))
                )]
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the minute grain did not tick with no subscriber; the view still holds {now:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A user watch over a fine grain has rows at once and keeps ticking with
    /// no subscriber, and stops ticking when its view goes.
    #[tokio::test]
    async fn a_watch_over_a_fine_grain_has_rows_and_ticks_with_no_subscriber() {
        let handle = booted_clock_db().await;
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 11), 0);
        let scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            Duration::from_millis(20),
            &holon_api::lifecycle::SessionShutdown::new(),
        )
        .await
        .unwrap();
        let manager =
            crate::sync::MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
        manager.set_clock_seeder(scheduler.grain_seeder());

        let watch = manager
            .watch("SELECT grain, epoch_day FROM clock WHERE grain = 'minute'")
            .await
            .expect("a watch that names its grain");
        assert_eq!(
            watch.initial_rows.len(),
            1,
            "a watch over the minute grain must have its row from the first read"
        );
        let tick = || async {
            handle
                .query(
                    &format!("SELECT epoch_day FROM {}", watch.view_name),
                    HashMap::new(),
                )
                .await
                .unwrap()[0]
                .get("epoch_day")
                .and_then(|v| v.as_i64())
                .unwrap()
        };
        let first = tick().await;
        clock.advance(3 * 60_000);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tick().await == first {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the minute grain a watch reads did not tick with no subscriber"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_watch_that_reads_clock_without_naming_a_grain_is_refused() {
        let handle = booted_clock_db().await;
        handle
            .execute_ddl("CREATE TABLE wants (id TEXT PRIMARY KEY, g TEXT)")
            .await
            .unwrap();
        let manager =
            crate::sync::MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
        let err = manager
            .watch("SELECT w.id, c.updated_at FROM wants w JOIN clock c ON c.grain = w.g")
            .await
            .err()
            .expect("a watch that cannot say which grain it reads must be refused");
        assert!(
            format!("{err:#}").contains("names no grain"),
            "the refusal must say why: {err:#}"
        );
        assert!(
            err.downcast_ref::<holon_api::QueryRefused>().is_some(),
            "the refusal must be final, so no watcher retries it: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_watch_that_only_mentions_clock_is_not_refused() {
        let handle = booted_clock_db().await;
        handle
            .execute_ddl("CREATE TABLE wants (id TEXT PRIMARY KEY, g TEXT)")
            .await
            .unwrap();
        let manager =
            crate::sync::MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
        for sql in [
            "SELECT id FROM wants WHERE g LIKE '%clock%'",
            "SELECT id, g FROM wants WHERE g = 'clock'",
            "SELECT id FROM wants WHERE g LIKE '%Clock%'",
            "SELECT id FROM wants WHERE g = 'CLOCK' OR g = 'minute'",
            "SELECT id AS clock FROM wants",
            "SELECT \"clock\".id FROM wants AS \"clock\" WHERE \"clock\".g = 'day'",
        ] {
            manager
                .watch(sql)
                .await
                .unwrap_or_else(|e| panic!("a query that reads no clock row was refused: {e:#}"));
        }
    }

    /// A view naming `clock` in another case must resolve on the real create
    /// path (`MatviewManager::watch`), not only in the SQL-parse unit test:
    /// the dependency it waits on and the resource `clock`'s schema module
    /// marks available must be the SAME Resource regardless of spelling.
    #[tokio::test]
    async fn a_watch_that_reads_clock_in_another_case_still_works() {
        let handle = booted_clock_db().await;
        let manager =
            crate::sync::MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
        // Names only the `day` grain — no fine grain, so no clock seeder is
        // needed and the only thing under test is whether the DDL dependency
        // resolves.
        for sql in [
            "SELECT grain, epoch_day FROM CLOCK WHERE grain = 'day'",
            "SELECT grain, epoch_day FROM \"Clock\" WHERE grain = 'day'",
        ] {
            tokio::time::timeout(Duration::from_secs(5), manager.watch(sql))
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "{sql}: creating the view hung waiting for its `clock` dependency — the \
                         case-differing table name did not resolve to the same Resource"
                    )
                })
                .unwrap_or_else(|e| panic!("{sql}: {e:#}"));
        }
    }

    /// A watch reads the grains its query names, however it spells the
    /// relation, and records every one of them.
    #[tokio::test]
    async fn a_watch_over_a_clock_join_records_each_grain_it_names() {
        let handle = booted_clock_db().await;
        handle
            .execute_ddl("CREATE TABLE wants (id TEXT PRIMARY KEY, g TEXT)")
            .await
            .unwrap();
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 11), 0);
        let scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            Duration::from_secs(3600),
            &holon_api::lifecycle::SessionShutdown::new(),
        )
        .await
        .unwrap();
        let manager =
            crate::sync::MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
        manager.set_clock_seeder(scheduler.grain_seeder());

        let recorded = |view: String| {
            let handle = handle.clone();
            async move {
                handle
                    .query(
                        &format!(
                            "SELECT grain FROM clock_reader WHERE view = '{view}' ORDER BY grain"
                        ),
                        HashMap::new(),
                    )
                    .await
                    .unwrap()
                    .iter()
                    .map(|r| format!("{:?}", r.get("grain")))
                    .collect::<Vec<_>>()
            }
        };
        let text = |g: &str| format!("{:?}", Some(holon_api::Value::String(g.into())));

        let joined = manager
            .watch("SELECT w.id, c.today FROM wants w JOIN clock c ON c.grain = w.g WHERE c.grain = 'minute'")
            .await
            .expect("a join on the minute grain");
        assert_eq!(recorded(joined.view_name).await, vec![text("minute")]);

        let both = manager
            .watch("SELECT grain, today FROM clock WHERE grain IN ('hour', 'minute')")
            .await
            .expect("a watch over two grains");
        assert_eq!(
            recorded(both.view_name).await,
            vec![text("hour"), text("minute")],
            "a view that reads two grains must keep both recorded, or the next session ticks one"
        );
    }

    /// A grain recorded after the scheduler started (a sidecar view created
    /// at connect) has no `clock` row yet; the next tick makes it.
    #[tokio::test]
    async fn a_grain_recorded_after_spawn_gets_its_row_at_the_next_tick() {
        let handle = booted_clock_db().await;
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 11), 0);
        let _scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            Duration::from_millis(20),
            &holon_api::lifecycle::SessionShutdown::new(),
        )
        .await
        .unwrap();
        handle
            .execute(
                "INSERT INTO clock_reader (view, grain) VALUES ('later_view', 'hour')",
                vec![],
            )
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while hour_rows_query(&handle).await == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "a recorded grain with no clock row was never seeded by the tick"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// End-to-end write gate: a fine grain has no `clock` row until subscribed;
    /// subscribing materializes it at the current instant.
    #[tokio::test]
    async fn subscribe_materializes_fine_grain_row_absent_otherwise() {
        let handle = booted_clock_db().await;
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 11), 0);
        let scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            // Long interval: the ticking task must not fire inside the test window,
            // so every observed write comes from subscribe/reconcile, not timing.
            Duration::from_secs(3600),
            &holon_api::lifecycle::SessionShutdown::new(),
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
        let clock = TestClock::with_utc_offset(noon_utc_millis(2026, 7, 11), 0);
        let scheduler = spawn_clock_scheduler(
            handle.clone(),
            Arc::new(clock.clone()) as Arc<dyn Clock>,
            Duration::from_secs(3600),
            &holon_api::lifecycle::SessionShutdown::new(),
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
