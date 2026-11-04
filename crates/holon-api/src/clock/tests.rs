use proptest::prelude::*;

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
        fn now(&self) -> Mono {
            SystemClock.now()
        }
        fn sleep_until(&self, deadline: Mono) -> futures::future::BoxFuture<'static, ()> {
            SystemClock.sleep_until(deadline)
        }
        fn sleep_blocking(&self, d: Duration) {
            SystemClock.sleep_blocking(d)
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
    assert_eq!(Grain::Day.sample(&utc).unwrap().label, "2026-08-31");

    let ahead = FixedZoneClock(millis, 14 * 3600);
    assert_eq!(CalendarDate::from_clock(&ahead).ymd(), "2026-09-01");
    assert_eq!(Grain::Day.sample(&ahead).unwrap().label, "2026-09-01");

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
    let s = Grain::Day.sample(&clock).unwrap();
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
    let hour = Grain::Hour.sample(&clock).unwrap();
    let minute = Grain::Minute.sample(&clock).unwrap();
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
    assert_eq!(Grain::Hour.sample(&clock).unwrap().tick, hour.tick + 1);
    clock.set(
        (base + chrono::Duration::minutes(1))
            .and_utc()
            .timestamp_millis(),
    );
    assert_eq!(Grain::Minute.sample(&clock).unwrap().tick, minute.tick + 1);
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

proptest! {
    #[test]
    fn mono_arithmetic_round_trips(origin_ms in 0u64..1 << 40, d_ms in 0u64..1 << 40) {
        let t = Mono::from_origin(Duration::from_millis(origin_ms));
        let d = Duration::from_millis(d_ms);
        prop_assert_eq!((t + d) - t, d);
        prop_assert!(t + d >= t);
    }

    #[test]
    fn a_late_tick_skips_to_the_next_grid_point_after_now(
        deadline_ms in 0u64..1 << 30,
        late_ms in 0u64..1 << 30,
        period_ms in 1u64..1 << 20,
    ) {
        let deadline = Mono::from_origin(Duration::from_millis(deadline_ms));
        let now = deadline + Duration::from_millis(late_ms);
        let period = Duration::from_millis(period_ms);
        let next = next_tick_skipping(deadline, now, period);
        prop_assert!(next > now);
        prop_assert!(next - now <= period);
        prop_assert_eq!((next - deadline).as_millis() % period.as_millis(), 0);
    }
}

#[test]
#[should_panic(expected = "Mono subtraction went negative")]
fn an_earlier_minus_a_later_mono_panics() {
    let t = Mono::from_origin(Duration::from_millis(5));
    let _ = t - (t + Duration::from_millis(1));
}

/// Example test: it pins the real-timer wiring of `timeout`, which has no
/// input space beyond "the future finishes first or the deadline does".
#[tokio::test]
async fn timeout_returns_the_value_or_elapsed() {
    let clock = SystemClock;
    assert_eq!(
        clock.timeout(Duration::from_secs(5), async { 7 }).await,
        Ok(7)
    );
    assert_eq!(
        clock
            .timeout(Duration::from_millis(10), futures::future::pending::<()>())
            .await,
        Err(Elapsed)
    );
}
