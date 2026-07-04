-- The `clock` relation: time as data (ADR 0024 P5).
--
-- `date('now')` is non-deterministic, so Turso rejects it as a matview source
-- (BugFunnel F4). A materialized `today` *value* here is deterministic and
-- CDC-observable, so a temporal guard becomes a plain join that re-fires on
-- day-rollover. The row is a boot-reseeded cache of the OS clock owned by the
-- `ClockScheduler` — never authoritative, an evaluator detail.
--
-- One row per grain (C6: `day` always-on; `hour`/`minute` materialized only
-- while a net subscribes — see ClockSchedulerHandle::subscribe). The column names
-- are day-flavoured for back-compat with the guard compiler (pattern.rs reads
-- `clock.today`), but their meaning is per-grain:
--   * `today`     — the grain LABEL: `YYYY-MM-DD` (day), `YYYY-MM-DDThh` (hour),
--                   `YYYY-MM-DDThh:mm` (minute).
--   * `epoch_day` — the grain TICK: a monotone integer count of grain-units since
--                   the Unix epoch (epoch-days / epoch-hours / epoch-minutes),
--                   compared in SQL without parsing dates. `every(N units)`
--                   desugars to `epoch_day % N = 0` over the grain's row.
CREATE TABLE IF NOT EXISTS clock (
    grain TEXT PRIMARY KEY NOT NULL,
    today TEXT NOT NULL,
    epoch_day INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
