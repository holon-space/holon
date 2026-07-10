-- The `clock` relation: time as data (ADR 0024 P5).
--
-- `date('now')` is non-deterministic, so Turso rejects it as a matview source
-- (BugFunnel F4). A materialized `today` *value* here is deterministic and
-- CDC-observable, so a temporal guard becomes a plain join that re-fires on
-- day-rollover. The row is a boot-reseeded cache of the OS clock owned by the
-- `ClockScheduler` — never authoritative, an evaluator detail.
--
-- One row per grain (only `day` today). `epoch_day` gives temporal guards a
-- monotone integer to compare without parsing dates in SQL.
CREATE TABLE IF NOT EXISTS clock (
    grain TEXT PRIMARY KEY NOT NULL,
    today TEXT NOT NULL,
    epoch_day INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
