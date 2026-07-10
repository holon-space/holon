use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use anyhow::Context;

/// Wall-clock authority. Passed as a typed value so timestamps are
/// deterministic in tests instead of reaching for the ambient system clock.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Milliseconds since the Unix epoch.
    fn now_millis(&self) -> i64;
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
}

/// Deterministic clock for tests. Cheap to clone; clones share the same
/// instant.
#[derive(Debug, Clone)]
pub struct TestClock {
    millis: Arc<AtomicI64>,
}

impl TestClock {
    pub fn new(start_millis: i64) -> Self {
        Self {
            millis: Arc::new(AtomicI64::new(start_millis)),
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
}

/// Temporal granularity of a `clock` relation row. Only `Day` is materialized
/// today; finer grains stay reserved so the `clock` schema and the scheduler
/// grow without a stringly `grain` column (parse-don't-validate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grain {
    Day,
    // Hour,   // reserved
    // Minute, // reserved
}

impl Grain {
    /// The `grain` primary-key value stored in the `clock` relation.
    pub fn as_str(self) -> &'static str {
        match self {
            Grain::Day => "day",
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
        let dt = chrono::DateTime::from_timestamp_millis(clock.now_millis())
            .expect("clock now_millis out of DateTime range");
        Self(dt.with_timezone(&chrono::Local).date_naive())
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
        // Noon UTC lands on the same civil date in every timezone from UTC-12 to
        // UTC+12, so this stays timezone-robust regardless of the test host's TZ.
        let millis = chrono::NaiveDate::from_ymd_opt(2026, 7, 10)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let clock = TestClock::new(millis);
        let d = CalendarDate::from_clock(&clock);
        assert_eq!(d.ymd(), "2026-07-10");
        assert_eq!(
            d.epoch_day(),
            CalendarDate::parse("2026-07-10").unwrap().epoch_day()
        );
    }

    #[test]
    fn grain_day_stringifies() {
        assert_eq!(Grain::Day.as_str(), "day");
    }
}
