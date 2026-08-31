use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use anyhow::Context;

/// Wall-clock authority. Passed as a typed value so timestamps are
/// deterministic in tests instead of reaching for the ambient system clock.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Milliseconds since the Unix epoch.
    fn now_millis(&self) -> i64;

    /// Seconds to add to UTC for the user's local wall clock at `millis`.
    /// Carried by the clock rather than read from `chrono::Local` at the use
    /// site because wasm32 targets ship no tz database: there `chrono::Local`
    /// silently *is* UTC, which mints the day page on the wrong calendar date
    /// for every user whose zone disagrees with UTC. Takes the instant so a
    /// caller that already sampled the clock cannot read a second, later one.
    fn utc_offset_seconds_at(&self, millis: i64) -> i32;
}

fn local_offset(clock: &dyn Clock, millis: i64) -> chrono::FixedOffset {
    let secs = clock.utc_offset_seconds_at(millis);
    chrono::FixedOffset::east_opt(secs)
        .unwrap_or_else(|| panic!("clock reported an out-of-range utc offset of {secs}s"))
}

/// The host's local offset at `millis`, per-instant so a DST boundary between
/// two readings is honoured. Returns 0 on wasm32, where there is no tz data —
/// a wasm build must inject an offset instead of relying on this.
pub fn host_utc_offset_seconds(millis: i64) -> i32 {
    use chrono::Offset as _;
    let dt = chrono::DateTime::from_timestamp_millis(millis)
        .expect("clock now_millis out of DateTime range");
    dt.with_timezone(&chrono::Local)
        .offset()
        .fix()
        .local_minus_utc()
}

/// DI seam for the wall-clock authority the `ClockScheduler` ticks on.
/// Production boot registers nothing and the engine factory falls back to
/// [`SystemClock`]; a test wiring registers this newtype holding a controllable
/// [`TestClock`] so the keystone `AdvanceDay` transition advances time through
/// the real scheduler path (never a raw `clock`-relation write). A wrapper
/// newtype (not a bare `Arc<dyn Clock>`) so fluxdi keys on one stable type both
/// sides name.
#[derive(Clone)]
pub struct InjectedClock(pub Arc<dyn Clock>);

impl std::fmt::Debug for InjectedClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("InjectedClock").field(&self.0).finish()
    }
}

/// Production clock: reads the real system time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_millis(&self) -> i64 {
        // Preserves the exact pre-existing call used across block constructors.
        chrono::Utc::now().timestamp_millis()
    }

    fn utc_offset_seconds_at(&self, millis: i64) -> i32 {
        host_utc_offset_seconds(millis)
    }
}

/// Deterministic clock for tests — instant AND zone. Cheap to clone; clones
/// share the same instant.
///
/// The offset defaults to [`TestClock::DEFAULT_UTC_OFFSET_SECONDS`], a fixed
/// zone that is deliberately NOT UTC: a date assertion written against a
/// UTC-equivalent clock passes whether or not the code under test honours the
/// zone at all, which is how the browser day-page bug survived a green suite.
#[derive(Debug, Clone)]
pub struct TestClock {
    millis: Arc<AtomicI64>,
    utc_offset_seconds: i32,
}

impl TestClock {
    /// +14:00 (Pacific/Kiritimati) — the furthest zone ahead of UTC, so a
    /// calendar date computed in UTC differs from the local one for the widest
    /// part of the day.
    pub const DEFAULT_UTC_OFFSET_SECONDS: i32 = 14 * 3600;

    pub fn new(start_millis: i64) -> Self {
        Self::with_utc_offset(start_millis, Self::DEFAULT_UTC_OFFSET_SECONDS)
    }

    pub fn with_utc_offset(start_millis: i64, utc_offset_seconds: i32) -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(start_millis)),
            utc_offset_seconds,
        }
    }
    pub fn set(&self, millis: i64) {
        self.millis.store(millis, Ordering::SeqCst);
    }
    /// Advance by `delta_millis` and return the new value.
    pub fn advance(&self, delta_millis: i64) -> i64 {
        self.millis.fetch_add(delta_millis, Ordering::SeqCst) + delta_millis
    }
}

impl Clock for TestClock {
    fn now_millis(&self) -> i64 {
        self.millis.load(Ordering::SeqCst)
    }

    fn utc_offset_seconds_at(&self, _: i64) -> i32 {
        self.utc_offset_seconds
    }
}

/// Temporal granularity of a `clock` relation row (ADR 0024 P5; C6). `Day` is
/// always materialized (always-on, one write/day). `Hour`/`Minute` are finer
/// grains materialized only while at least one net reads them
/// (subscription-counted — see `ClockSchedulerHandle::subscribe`), so a net
/// that never asks for minute resolution never pays for minute-rollover writes.
///
/// A parse-don't-validate enum, never a stringly `grain` column: the only way
/// to name a grain is a variant, and [`Grain::sample`] is the single place that
/// maps a wall-clock instant to the `(epoch_day, today)` the relation stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grain {
    Day,
    Hour,
    Minute,
}

/// The two `clock`-relation column values for one grain at one instant: the
/// monotone integer tick (stored in the `epoch_day` column — a misnomer kept
/// for day back-compat; for finer grains it holds epoch-hours / epoch-minutes)
/// and the human label (stored in the `today` column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrainSample {
    /// Monotone integer count of grain-units since the Unix epoch, in local
    /// wall-clock time. Goes backwards on a backwards clock change (DST /
    /// travel west), which is exactly what lets temporal guards
    /// re-converge.
    pub tick: i64,
    /// Human label of the truncated instant (`YYYY-MM-DD` for day,
    /// `YYYY-MM-DDThh` for hour, `YYYY-MM-DDThh:mm` for minute).
    pub label: String,
}

impl Grain {
    /// The `grain` primary-key value stored in the `clock` relation.
    pub fn as_str(self) -> &'static str {
        match self {
            Grain::Day => "day",
            Grain::Hour => "hour",
            Grain::Minute => "minute",
        }
    }

    /// Whole seconds in one grain-unit — the divisor turning epoch-seconds into
    /// a monotone per-grain tick.
    fn unit_seconds(self) -> i64 {
        match self {
            Grain::Day => 86_400,
            Grain::Hour => 3_600,
            Grain::Minute => 60,
        }
    }

    /// `chrono` format string for this grain's label (truncates finer fields).
    fn label_fmt(self) -> &'static str {
        match self {
            Grain::Day => "%Y-%m-%d",
            Grain::Hour => "%Y-%m-%dT%H",
            Grain::Minute => "%Y-%m-%dT%H:%M",
        }
    }

    /// The `(tick, label)` pair for this grain at the injected clock's *local*
    /// instant. For [`Grain::Day`] `tick` equals [`CalendarDate::epoch_day`]
    /// and `label` equals [`CalendarDate::ymd`], so the day path is
    /// byte-for-byte the pre-C6 behaviour.
    pub fn sample(self, clock: &dyn Clock) -> GrainSample {
        let millis = clock.now_millis();
        let dt = chrono::DateTime::from_timestamp_millis(millis)
            .expect("clock now_millis out of DateTime range");
        let naive = dt.with_timezone(&local_offset(clock, millis)).naive_local();
        // Treat the local wall-clock reading as if UTC to get a monotone integer
        // that ticks at each *local* grain boundary. `div_euclid` floors toward
        // negative infinity so pre-epoch instants still land in the right bucket.
        let secs = naive.and_utc().timestamp();
        GrainSample {
            tick: secs.div_euclid(self.unit_seconds()),
            label: naive.format(self.label_fmt()).to_string(),
        }
    }
}

/// A parsed `every(<interval>)` recurrence: `count` units of `grain` (ADR 0024
/// P5, time-as-data). It carries no engine polling — [`Recurrence::desugar`]
/// turns it into a read arc on the `clock` relation, so a periodic transition
/// is an ordinary reactive guard that re-fires when the grain's row updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recurrence {
    /// How many grain-units per period (`every 2 hours` -> 2). Always `>= 1`.
    pub count: u32,
    /// The clock grain the recurrence reads.
    pub grain: Grain,
}

/// The read-arc desugaring of a [`Recurrence`]: the grain to subscribe plus a
/// SQL `SELECT` over the `clock` relation that yields exactly one row on the
/// periods where the recurrence fires (and none otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecurrenceReadArc {
    /// The grain whose ticking the recurrence depends on — the caller must hold
    /// a subscription for it (fine grains do not tick otherwise).
    pub grain: Grain,
    /// A `SELECT epoch_day AS tick, today AS label FROM clock WHERE ...`
    /// yielding a row only on firing periods. Joined into a transition
    /// guard by the compiler.
    pub read_arc_sql: String,
}

impl Recurrence {
    /// Parse an interval like `1d` / `2h` / `30m` / `day` / `2 hours` /
    /// `hourly` / `1w` into a [`Recurrence`]. Fails loud on anything else —
    /// including recurrences coarser than a day (`year` / `month`), which need
    /// an anniversary predicate the day grain cannot express and are out of
    /// C6 scope.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let raw = s.trim();
        let raw = raw.strip_prefix("every").map(str::trim).unwrap_or(raw);
        // Split an optional leading integer from the unit word.
        let (count, unit) = match raw.find(|c: char| c.is_alphabetic()) {
            Some(0) => (1u32, raw),
            Some(idx) => {
                let count: u32 = raw[..idx].trim().parse().with_context(|| {
                    format!("recurrence count in {s:?} is not a positive integer")
                })?;
                (count, raw[idx..].trim())
            }
            None => anyhow::bail!("recurrence {s:?} has no time unit (expected day/hour/minute)"),
        };
        if count == 0 {
            anyhow::bail!("recurrence {s:?} has a zero count");
        }
        let (grain, multiplier) = match unit.to_ascii_lowercase().as_str() {
            "d" | "day" | "days" | "daily" => (Grain::Day, 1),
            "w" | "week" | "weeks" | "weekly" => (Grain::Day, 7),
            "h" | "hour" | "hours" | "hourly" => (Grain::Hour, 1),
            "m" | "min" | "mins" | "minute" | "minutes" => (Grain::Minute, 1),
            "y" | "year" | "years" | "yearly" | "annually" | "month" | "months" | "monthly" => {
                anyhow::bail!(
                    "recurrence {s:?} is coarser than a day; year/month need an anniversary \
                     predicate the clock grain cannot express (C6 supports minute/hour/day/week)"
                )
            }
            other => anyhow::bail!("unknown recurrence unit {other:?} in {s:?}"),
        };
        Ok(Recurrence {
            count: count * multiplier,
            grain,
        })
    }

    /// Desugar to a read arc on the `clock` relation. The `epoch_day % count =
    /// 0` predicate gates multi-unit periods (`every 2 hours` -> even
    /// epoch-hours); `count == 1` fires on every boundary.
    pub fn desugar(self) -> RecurrenceReadArc {
        RecurrenceReadArc {
            grain: self.grain,
            read_arc_sql: format!(
                "SELECT epoch_day AS tick, today AS label FROM clock WHERE grain = '{}' AND \
                 epoch_day % {} = 0",
                self.grain.as_str(),
                self.count,
            ),
        }
    }
}

/// A calendar date proven to be a real `YYYY-MM-DD`. The only constructors are
/// parsers ([`CalendarDate::parse`], [`CalendarDate::from_clock`]), so callers
/// never carry an unvalidated date `String` (parse-don't-validate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalendarDate(chrono::NaiveDate);

impl CalendarDate {
    /// Parse a `YYYY-MM-DD` string, failing loud on any other shape.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let date = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .with_context(|| format!("CalendarDate expects YYYY-MM-DD, got {s:?}"))?;
        Ok(Self(date))
    }

    /// The *local* calendar date at the instant the injected clock reports.
    /// Replaces the SQL `date('now','localtime')` the journal trigger used to
    /// run, sourced from the deterministic injected [`Clock`] instead.
    pub fn from_clock(clock: &dyn Clock) -> Self {
        let millis = clock.now_millis();
        let dt = chrono::DateTime::from_timestamp_millis(millis)
            .expect("clock now_millis out of DateTime range");
        Self(dt.with_timezone(&local_offset(clock, millis)).date_naive())
    }

    /// Canonical `YYYY-MM-DD` rendering — the `today` column value.
    pub fn ymd(self) -> String {
        self.0.format("%Y-%m-%d").to_string()
    }

    /// The calendar date `days` days after this one (negative goes backwards).
    /// Used by the reference model to advance its `today` in lockstep with a
    /// clock the SUT moves by whole days (ADR 0024 §6).
    pub fn add_days(self, days: i64) -> Self {
        Self(
            self.0
                .checked_add_signed(chrono::Duration::days(days))
                .expect("CalendarDate::add_days out of range"),
        )
    }

    /// Days since the Unix epoch (1970-01-01): a monotone integer temporal
    /// guards compare in SQL without parsing dates. Goes backwards on a
    /// backwards day change, which is exactly what lets guards re-converge.
    pub fn epoch_day(self) -> i64 {
        self.0
            .signed_duration_since(
                chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("1970-01-01 is a valid date"),
            )
            .num_days()
    }
}

/// Free helper for sites that have no clock to inject (free constructors,
/// `Default`). Routes through [`SystemClock`] — the single chokepoint that a
/// future change can make injectable. Prefer holding a `Clock` where a `self`
/// exists (see `SqlOperationProvider`).
pub fn now_millis() -> i64 {
    SystemClock.now_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_clock_is_deterministic() {
        let c = TestClock::new(1_000);
        assert_eq!(c.now_millis(), 1_000);
        assert_eq!(c.advance(500), 1_500);
        assert_eq!(c.now_millis(), 1_500);
        c.set(42);
        assert_eq!(c.now_millis(), 42);
    }
    #[test]
    fn system_clock_is_positive() {
        assert!(SystemClock.now_millis() > 0);
    }

    #[test]
    fn calendar_date_parses_and_rejects() {
        let d = CalendarDate::parse("2026-07-10").unwrap();
        assert_eq!(d.ymd(), "2026-07-10");
        assert!(CalendarDate::parse("2026-13-01").is_err());
        assert!(CalendarDate::parse("not-a-date").is_err());
        assert!(CalendarDate::parse("2026/07/10").is_err());
    }

    #[test]
    fn epoch_day_is_days_since_unix_epoch() {
        assert_eq!(CalendarDate::parse("1970-01-01").unwrap().epoch_day(), 0);
        assert_eq!(CalendarDate::parse("1970-01-02").unwrap().epoch_day(), 1);
        // Backwards dates yield a smaller (here negative) epoch_day.
        assert_eq!(CalendarDate::parse("1969-12-31").unwrap().epoch_day(), -1);
    }

    #[test]
    fn from_clock_matches_local_date() {
        let millis = chrono::NaiveDate::from_ymd_opt(2026, 7, 10)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        // Names its zone rather than inheriting one: the date this asserts is
        // only meaningful against a stated offset.
        let clock = TestClock::with_utc_offset(millis, 0);
        let d = CalendarDate::from_clock(&clock);
        assert_eq!(d.ymd(), "2026-07-10");
        assert_eq!(
            d.epoch_day(),
            CalendarDate::parse("2026-07-10").unwrap().epoch_day()
        );
    }

    /// A clock in a zone ahead of UTC is already on the next calendar day at
    /// 23:00 UTC. Pins the wasm day-page fix: the offset must come from the
    /// clock, since `chrono::Local` is UTC there and would report 08-31.
    #[test]
    fn calendar_date_follows_the_clocks_offset_not_the_host_zone() {
        #[derive(Debug)]
        struct FixedZoneClock(i64, i32);
        impl Clock for FixedZoneClock {
            fn now_millis(&self) -> i64 {
                self.0
            }
            fn utc_offset_seconds_at(&self, _: i64) -> i32 {
                self.1
            }
        }

        let millis = chrono::NaiveDate::from_ymd_opt(2026, 8, 31)
            .unwrap()
            .and_hms_opt(23, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();

        let utc = FixedZoneClock(millis, 0);
        assert_eq!(CalendarDate::from_clock(&utc).ymd(), "2026-08-31");
        assert_eq!(Grain::Day.sample(&utc).label, "2026-08-31");

        let ahead = FixedZoneClock(millis, 14 * 3600);
        assert_eq!(CalendarDate::from_clock(&ahead).ymd(), "2026-09-01");
        assert_eq!(Grain::Day.sample(&ahead).label, "2026-09-01");

        let behind = FixedZoneClock(millis, -8 * 3600);
        assert_eq!(CalendarDate::from_clock(&behind).ymd(), "2026-08-31");
    }

    #[test]
    fn grain_stringifies() {
        assert_eq!(Grain::Day.as_str(), "day");
        assert_eq!(Grain::Hour.as_str(), "hour");
        assert_eq!(Grain::Minute.as_str(), "minute");
    }

    #[test]
    fn day_sample_matches_calendar_date() {
        // The C6 grain sampler must reproduce the pre-C6 day values exactly.
        let millis = chrono::NaiveDate::from_ymd_opt(2026, 7, 11)
            .unwrap()
            .and_hms_opt(9, 37, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let clock = TestClock::new(millis);
        let s = Grain::Day.sample(&clock);
        let cal = CalendarDate::from_clock(&clock);
        assert_eq!(s.label, cal.ymd());
        assert_eq!(s.tick, cal.epoch_day());
    }

    #[test]
    fn hour_and_minute_samples_truncate_and_tick() {
        // 2026-07-11 09:37:42 UTC (noon-robust across TZ is not needed here: we
        // compare the sampler against itself under a fixed offset via Local).
        let base = chrono::NaiveDate::from_ymd_opt(2026, 7, 11)
            .unwrap()
            .and_hms_opt(9, 37, 42)
            .unwrap();
        let clock = TestClock::new(base.and_utc().timestamp_millis());
        let hour = Grain::Hour.sample(&clock);
        let minute = Grain::Minute.sample(&clock);
        // Labels truncate to their grain (rendered in Local; assert shape+prefix).
        assert!(hour.label.starts_with("2026-07-11T"), "hour label {hour:?}");
        assert_eq!(hour.label.len(), "2026-07-11T09".len());
        assert_eq!(minute.label.len(), "2026-07-11T09:37".len());
        // One hour later -> tick advances by exactly 1; one minute -> +1.
        clock.set(
            (base + chrono::Duration::hours(1))
                .and_utc()
                .timestamp_millis(),
        );
        assert_eq!(Grain::Hour.sample(&clock).tick, hour.tick + 1);
        clock.set(
            (base + chrono::Duration::minutes(1))
                .and_utc()
                .timestamp_millis(),
        );
        assert_eq!(Grain::Minute.sample(&clock).tick, minute.tick + 1);
    }

    #[test]
    fn recurrence_parses_all_supported_forms() {
        let cases = [
            ("1d", 1, Grain::Day),
            ("day", 1, Grain::Day),
            ("daily", 1, Grain::Day),
            ("every 3 days", 3, Grain::Day),
            ("1w", 7, Grain::Day),
            ("2 weeks", 14, Grain::Day),
            ("h", 1, Grain::Hour),
            ("2h", 2, Grain::Hour),
            ("every 2 hours", 2, Grain::Hour),
            ("30m", 30, Grain::Minute),
            ("15 minutes", 15, Grain::Minute),
        ];
        for (input, count, grain) in cases {
            let r = Recurrence::parse(input).unwrap_or_else(|e| panic!("parse {input:?}: {e:#}"));
            assert_eq!(r, Recurrence { count, grain }, "input {input:?}");
        }
    }

    #[test]
    fn recurrence_rejects_coarse_and_garbage() {
        for bad in [
            "year",
            "every month",
            "annually",
            "fortnight",
            "0 days",
            "3 blorks",
            "",
        ] {
            assert!(
                Recurrence::parse(bad).is_err(),
                "expected {bad:?} to fail loud"
            );
        }
    }

    #[test]
    fn recurrence_desugars_to_clock_read_arc() {
        let arc = Recurrence::parse("every 2 hours").unwrap().desugar();
        assert_eq!(arc.grain, Grain::Hour);
        assert!(arc.read_arc_sql.contains("grain = 'hour'"), "{arc:?}");
        assert!(arc.read_arc_sql.contains("epoch_day % 2 = 0"), "{arc:?}");
        // count == 1 still emits a modulo (always true), so the shape is uniform.
        let daily = Recurrence::parse("day").unwrap().desugar();
        assert!(
            daily.read_arc_sql.contains("epoch_day % 1 = 0"),
            "{daily:?}"
        );
        assert!(daily.read_arc_sql.contains("grain = 'day'"), "{daily:?}");
    }
}
