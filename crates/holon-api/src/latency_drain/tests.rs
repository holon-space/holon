use super::*;
use crate::latency_slo::ClockOrigin;
use crate::latency_slo::QuietBatches;
use crate::latency_slo::Superseded;

const N: usize = DRAIN_WRITES;
const GAP_US: u64 = 50_000;

/// SplitMix64: a fixed seed gives a fixed trace.
fn split_mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// How one feed of the simulated pipeline applies rows. Times in µs.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// One row per pass; a pass takes a time drawn per pass in `lo..=hi`.
    Serial { lo: u64, hi: u64 },
    /// Every committed row per pass; a pass takes a base drawn per pass in
    /// `lo..=hi` plus `per_row` for each row it carries.
    Batching { lo: u64, hi: u64, per_row: u64 },
    /// One row per pass: 1ms for the first `fast_rows`, `per_row` after.
    FastThen { fast_rows: usize, per_row: u64 },
    /// One row per 5ms pass for the first `rows`, then no pass ever again.
    Hangs { rows: usize },
}

/// A clock setup left pending on drive target 7.
#[derive(Clone, Copy, Debug)]
enum Leftover {
    /// Its row commits at this instant (µs).
    CommitsAt(u64),
    /// It never delivers.
    Never,
}

#[derive(Clone, Debug)]
struct Pipeline {
    /// One shape per feed. The feed that applies a row first closes its
    /// clock.
    feeds: Vec<Shape>,
    /// Commit latency per write, drawn in `0..=commit_max`.
    commit_max: u64,
    /// An untracked pass of `quiet.0..=quiet.1` before a real pass, with
    /// probability `quiet.2` percent.
    quiet: (u64, u64, u64),
    /// The first door call's cost; every later one costs 0.5ms.
    cold_door: u64,
    leftover: Option<Leftover>,
}

fn fast_serial() -> Pipeline {
    Pipeline {
        feeds: vec![Shape::Serial {
            lo: 5_000,
            hi: 5_000,
        }],
        commit_max: 0,
        quiet: (0, 0, 0),
        cold_door: 500,
        leftover: None,
    }
}

const WARM_UP: &str = "block:drain-warm-up";

/// Target `N` is the warm-up's.
fn target(k: usize) -> String {
    if k == N {
        WARM_UP.to_string()
    } else {
        format!("block:drain-{k}")
    }
}

fn drive() -> Drive {
    Drive::new(
        DrainTest::gate(),
        WARM_UP.to_string(),
        (0..N).map(target).collect(),
    )
}

struct Row {
    target: usize,
    commit: u64,
    applied: Vec<bool>,
}

struct Feed {
    shape: Shape,
    busy_until: Option<u64>,
    received: u64,
    rows: Vec<usize>,
    was_quiet: bool,
    applied: usize,
}

/// Drive the gate's [`Drive`] against a simulated pipeline and a
/// correlator that closes the newest clock on a delivered row's target
/// dispatched before the pass took its batch, superseding older ones.
fn run(pipeline: &Pipeline, seed: u64) -> DrainVerdict {
    let mut rng = seed;
    let mut draw = |lo: u64, hi: u64| lo + split_mix(&mut rng) % (hi - lo + 1);
    let base = Instant::now();
    let at = |us: u64| base + Duration::from_micros(us);
    let mut drive = drive();
    let mut rows: Vec<Row> = Vec::new();
    let mut clocks: Vec<Vec<u64>> = vec![Vec::new(); N + 1];
    let mut feeds: Vec<Feed> = pipeline
        .feeds
        .iter()
        .map(|&shape| Feed {
            shape,
            busy_until: None,
            received: 0,
            rows: Vec::new(),
            was_quiet: false,
            applied: 0,
        })
        .collect();
    let feed_count = feeds.len();
    match pipeline.leftover {
        Some(Leftover::CommitsAt(commit)) => {
            clocks[7].push(0);
            rows.push(Row {
                target: 7,
                commit,
                applied: vec![false; feed_count],
            });
        }
        Some(Leftover::Never) => clocks[7].push(0),
        None => {}
    }
    let mut samples: Vec<E2eSample> = Vec::new();
    let mut now = 1_000u64;
    let mut driver_at = now;
    let mut door_calls = 0;
    loop {
        for (f, feed) in feeds.iter_mut().enumerate() {
            if feed.busy_until != Some(now) {
                continue;
            }
            feed.busy_until = None;
            for r in std::mem::take(&mut feed.rows) {
                rows[r].applied[f] = true;
                feed.applied += 1;
                let open = &mut clocks[rows[r].target];
                let Some(newest) = open.iter().copied().filter(|&t| t <= feed.received).max()
                else {
                    continue;
                };
                let superseded: Vec<u64> = open
                    .iter()
                    .filter(|&&t| t < newest)
                    .map(|t| (now - t) / 1000)
                    .collect();
                open.retain(|&t| t > newest);
                samples.push(E2eSample {
                    action: "set_field".to_string(),
                    target: target(rows[r].target),
                    origin: ClockOrigin::Ui,
                    ms: (now - newest) / 1000,
                    in_flight: 1,
                    backlog: 0,
                    contended: false,
                    delivered_at: at(now),
                    delivery_batch: 0,
                    source: "block".to_string(),
                    feed: f as u64,
                    superseded: Superseded::new(superseded),
                    quiet: QuietBatches::default(),
                });
            }
        }
        for (f, feed) in feeds.iter_mut().enumerate() {
            if feed.busy_until.is_some() {
                continue;
            }
            let committed: Vec<usize> = (0..rows.len())
                .filter(|&r| !rows[r].applied[f] && rows[r].commit <= now)
                .collect();
            if committed.is_empty()
                || matches!(feed.shape, Shape::Hangs { rows } if feed.applied >= rows)
            {
                continue;
            }
            feed.received = now;
            if !feed.was_quiet && draw(0, 99) < pipeline.quiet.2 {
                feed.was_quiet = true;
                feed.busy_until = Some(now + draw(pipeline.quiet.0, pipeline.quiet.1));
                continue;
            }
            feed.was_quiet = false;
            let (took, batch) = match feed.shape {
                Shape::Serial { lo, hi } => (draw(lo, hi), vec![committed[0]]),
                Shape::Batching { lo, hi, per_row } => {
                    (draw(lo, hi) + per_row * committed.len() as u64, committed)
                }
                Shape::FastThen { fast_rows, per_row } => (
                    if feed.applied < fast_rows {
                        1_000
                    } else {
                        per_row
                    },
                    vec![committed[0]],
                ),
                Shape::Hangs { .. } => (5_000, vec![committed[0]]),
            };
            feed.busy_until = Some(now + took.max(1));
            feed.rows = batch;
        }
        let mut driver_wake = None;
        if now >= driver_at {
            let open_targets = || {
                (0..=N)
                    .filter(|&t| !clocks[t].is_empty())
                    .map(target)
                    .collect()
            };
            match drive.step(at(now), &samples, open_targets) {
                Step::Dispatch { target: name } => {
                    drive.dispatched(at(now));
                    let t = if name == WARM_UP {
                        N
                    } else {
                        name.strip_prefix("block:drain-")
                            .expect("a drive target")
                            .parse::<usize>()
                            .expect("a drive target index")
                    };
                    clocks[t].push(now);
                    rows.push(Row {
                        target: t,
                        commit: now + draw(0, pipeline.commit_max),
                        applied: vec![false; feed_count],
                    });
                    door_calls += 1;
                    driver_at = now
                        + if door_calls == 1 {
                            pipeline.cold_door
                        } else {
                            500
                        };
                }
                Step::Wait { until } => {
                    let until = (until - base).as_micros() as u64;
                    driver_wake = Some(until.max(now + 1));
                }
                Step::Done => {
                    let expiry = crate::latency_e2e::EXPIRY.as_micros() as u64;
                    let expired = (0..N)
                        .find(|&t| clocks[t].iter().any(|&c| now - c >= expiry))
                        .map(target)
                        .or_else(|| {
                            samples
                                .iter()
                                .find(|s| s.ms * 1000 >= expiry)
                                .map(|s| s.target.clone())
                        });
                    assert!(
                        expired.is_none(),
                        "the correlator expired the drive clock on {expired:?} before the \
                             drive ended: the run judges nothing"
                    );
                    return drive.verdict();
                }
            }
        }
        let idle_commit = rows
            .iter()
            .filter(|r| r.commit > now)
            .map(|r| r.commit)
            .min();
        now = feeds
            .iter()
            .filter_map(|f| f.busy_until)
            .chain(driver_wake.or((driver_at > now).then_some(driver_at)))
            .chain(idle_commit)
            .min()
            .expect("the drive always has a next event");
    }
}

/// Every feed retires at least `f` writes/s with a latency term within
/// `s`, background passes included.
fn healthy(draw: &mut impl FnMut(u64, u64) -> u64) -> Pipeline {
    let shape = |draw: &mut dyn FnMut(u64, u64) -> u64| match draw(0, 1) {
        // At most 85ms a pass plus one 15ms quiet pass: 100ms a row.
        0 => {
            let hi = draw(1_000, 85_000);
            Shape::Serial {
                lo: draw(1_000, hi),
                hi,
            }
        }
        _ => {
            let hi = draw(1_000, 150_000);
            Shape::Batching {
                lo: draw(1_000, hi),
                hi,
                per_row: draw(0, 25_000),
            }
        }
    };
    let feeds = (0..draw(1, 2)).map(|_| shape(&mut *draw)).collect();
    Pipeline {
        feeds,
        commit_max: draw(0, 100_000),
        quiet: (1_000, 15_000, draw(0, 50)),
        cold_door: draw(500, 300_000),
        leftover: None,
    }
}

/// Every feed spends at least `per_row` on each row.
/// Every feed spends at least `per_row` on each row, except a fast
/// prefix of at most `fast_rows` rows.
fn slow(draw: &mut impl FnMut(u64, u64) -> u64, per_row: (u64, u64), fast_rows: u64) -> Pipeline {
    let feeds = (0..draw(1, 2))
        .map(|_| {
            let cost = draw(per_row.0, per_row.1);
            match draw(0, 2) {
                0 => Shape::Serial {
                    lo: cost,
                    hi: cost + draw(0, 100_000),
                },
                1 => Shape::FastThen {
                    fast_rows: draw(0, fast_rows) as usize,
                    per_row: cost,
                },
                _ => {
                    let hi = draw(0, 300_000);
                    Shape::Batching {
                        lo: draw(0, hi),
                        hi,
                        per_row: cost,
                    }
                }
            }
        })
        .collect();
    Pipeline {
        feeds,
        commit_max: draw(0, 300_000),
        quiet: (1_000, 15_000, draw(0, 100)),
        cold_door: draw(500, 300_000),
        leftover: None,
    }
}

/// A healthy pipeline never fails the drive, and one whose feeds are
/// slower than 9.9 writes/s always does.
#[test]
fn the_drain_gate_passes_every_healthy_pipeline_and_fails_every_slow_one() {
    let mut wrong: std::collections::BTreeMap<&str, (usize, Option<String>)> =
        std::collections::BTreeMap::new();
    const PIPELINES: u64 = 1_500;
    for seed in 0..PIPELINES {
        let mut rng = seed;
        let mut draw = |lo: u64, hi: u64| lo + split_mix(&mut rng) % (hi - lo + 1);
        let (pipeline, must_fail) = match seed % 3 {
            0 => (healthy(&mut draw), false),
            // 2× to 4× too slow, even after 300 fast rows.
            1 => (slow(&mut draw, (200_000, 400_000), 300), true),
            // 5 to 9.9 writes/s from the first row.
            _ => (slow(&mut draw, (101_000, 200_000), 0), true),
        };
        let verdict = run(&pipeline, draw(0, u64::MAX - 1));
        let family = match (must_fail, &verdict) {
            (true, v) if v.is_fail() => continue,
            (false, DrainVerdict::Pass { .. }) => continue,
            (true, DrainVerdict::Pass { .. }) => "slow pipeline passed",
            (true, _) => "slow pipeline without a verdict",
            (false, v) if v.is_fail() => "healthy pipeline failed",
            (false, _) => "healthy pipeline without a verdict",
        };
        let (count, first) = wrong.entry(family).or_insert((0, None));
        *count += 1;
        first.get_or_insert(format!("seed {seed}: {pipeline:?} -> {verdict:?}"));
    }
    assert!(wrong.is_empty(), "of {PIPELINES} pipelines: {wrong:#?}");
}

#[test]
fn a_cold_first_dispatch_is_not_invalid() {
    for cold_door in [51_000, 151_000, 1_000_000] {
        let verdict = run(
            &Pipeline {
                cold_door,
                ..fast_serial()
            },
            1,
        );
        assert!(
            matches!(verdict, DrainVerdict::Pass { .. }),
            "cold first door call {cold_door}us: {verdict:?}"
        );
    }
}

#[test]
fn a_pipeline_fast_for_sixty_rows_then_at_five_per_sec_fails() {
    let verdict = run(
        &Pipeline {
            feeds: vec![Shape::FastThen {
                fast_rows: 60,
                per_row: 200_000,
            }],
            ..fast_serial()
        },
        1,
    );
    assert!(
        matches!(verdict, DrainVerdict::WindowHeld { .. }),
        "{verdict:?}"
    );
}

/// `rows` counts the warm-up's row: 1 hangs on the first drive write, 601
/// never hangs.
#[test]
fn a_pipeline_that_stops_delivering_fails() {
    for rows in [1, 100, 300, 520, 540, 580, 599, 600] {
        let verdict = run(
            &Pipeline {
                feeds: vec![Shape::Hangs { rows }],
                ..fast_serial()
            },
            1,
        );
        assert!(verdict.is_fail(), "hang after {rows} rows: {verdict:?}");
    }
}

#[test]
fn a_setup_clock_still_in_flight_on_a_drive_target_is_waited_out() {
    let verdict = run(
        &Pipeline {
            leftover: Some(Leftover::CommitsAt(300_000)),
            ..fast_serial()
        },
        1,
    );
    assert!(matches!(verdict, DrainVerdict::Pass { .. }), "{verdict:?}");
}

#[test]
#[should_panic(expected = "setup left an interaction in flight")]
fn a_setup_clock_that_never_closes_refuses_the_drive() {
    run(
        &Pipeline {
            leftover: Some(Leftover::Never),
            ..fast_serial()
        },
        1,
    );
}

/// Offer instants for a pipeline that delivers write `k` at `deliver(k)`
/// ms regardless of when it is offered: the gate's schedule, throttled by
/// the window. Returned as `(dispatched, delivered)` in ms.
fn throttled(deliver: impl Fn(u64) -> u64) -> (Vec<u64>, Vec<Option<u64>>) {
    let w = DRAIN_WINDOW as u64;
    let dispatched: Vec<u64> = (0..N as u64)
        .map(|k| (k * GAP_US / 1000).max(k.checked_sub(w).map_or(0, &deliver)))
        .collect();
    (
        dispatched,
        (0..N as u64).map(|k| Some(deliver(k))).collect(),
    )
}

fn judged(dispatch_ms: &[u64], delivered_ms: &[Option<u64>], until_ms: u64) -> DrainVerdict {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    DrainTest::gate().judge(
        &dispatch_ms.iter().map(|&d| at(d)).collect::<Vec<_>>(),
        &delivered_ms.iter().map(|d| d.map(at)).collect::<Vec<_>>(),
        at(until_ms),
    )
}

/// A serial pipeline at exactly the floor, 100ms a row, whose first row
/// lands after the whole 400ms allowance: it completes at exactly
/// `N/f + s`.
#[test]
fn a_pipeline_at_the_floor_with_the_full_latency_allowance_passes() {
    let (dispatched, delivered) = throttled(|k| 500 + 100 * k);
    assert_eq!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::Pass {
            completion: Duration::from_millis(60_400),
            limit: Duration::from_millis(60_400),
        }
    );
}

/// The lower edge of the band that sits below the floor yet passes,
/// `N/L ≈ 9.934/s`: 9.930/s fails and 9.940/s passes.
#[test]
fn the_passing_band_below_the_floor_is_9_934_to_10_per_sec() {
    let (dispatched, delivered) = throttled(|k| (k + 1) * 1007 / 10);
    assert!(matches!(
        judged(&dispatched, &delivered, 60_420),
        DrainVerdict::Late {
            completion: Some(_),
            delivered: N,
            ..
        }
    ));
    let (dispatched, delivered) = throttled(|k| (k + 1) * 1006 / 10);
    assert!(matches!(
        judged(&dispatched, &delivered, 60_360),
        DrainVerdict::Pass { .. }
    ));
}

/// A 5/s dip at the end of a fast drive: 202 writes complete within `L`,
/// 203 do not, and a dip long enough to hold the window full fails there.
#[test]
fn a_dip_below_the_floor_passes_only_while_the_completion_meets_the_limit() {
    let tail = |len: u64| {
        let fast = N as u64 - len;
        move |k: u64| {
            if k < fast {
                50 * k + 20
            } else {
                50 * (fast - 1) + 20 + 200 * (k - fast + 1)
            }
        }
    };
    let (dispatched, delivered) = throttled(tail(202));
    assert!(matches!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::Pass { .. }
    ));
    let (dispatched, delivered) = throttled(tail(203));
    assert!(matches!(
        judged(&dispatched, &delivered, 60_420),
        DrainVerdict::Late {
            completion: Some(_),
            delivered: N,
            ..
        }
    ));
    let (dispatched, delivered) = throttled(tail(300));
    assert_eq!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::WindowHeld {
            write: 569,
            pending: DRAIN_WINDOW,
        }
    );
}

/// A pipeline at the floor whose last write is offered at 54.4s and still
/// undelivered at the 60.4s limit.
#[test]
fn an_undelivered_write_is_late() {
    let (dispatched, mut delivered) = throttled(|k| 500 + 100 * k);
    delivered[N - 1] = None;
    assert_eq!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::Late {
            completion: None,
            delivered: N - 1,
            limit: Duration::from_millis(60_400),
        }
    );
}

/// Write 3, offered at 150ms, delivered 1ms inside and at the 20s patience.
#[test]
fn one_slow_write_passes_until_it_waits_the_patience() {
    let (dispatched, mut delivered) = throttled(|k| 50 * k + 20);
    delivered[3] = Some(150 + 19_999);
    assert!(matches!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::Pass { .. }
    ));
    delivered[3] = Some(150 + 20_000);
    assert_eq!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::Undelivered {
            write: 3,
            after: DRAIN_PATIENCE,
        }
    );
}

/// Write 3 offered at 301ms, 1ms behind `t0 + 3/f`, with the window empty:
/// the Late proof's premise is void and no Fail proof exists.
#[test]
fn a_driver_behind_the_floor_schedule_with_room_is_invalid() {
    let (mut dispatched, _) = throttled(|k| 50 * k + 20);
    for d in dispatched.iter_mut().skip(3) {
        *d += 151;
    }
    let delivered: Vec<Option<u64>> = dispatched.iter().map(|d| Some(d + 20)).collect();
    assert_eq!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::Invalid {
            write: 3,
            late_by: Duration::from_millis(1),
        }
    );
}

/// Sixty writes offered on schedule and none delivered by write 60's floor
/// deadline at 6s: the drive stops there.
#[test]
fn a_window_held_full_at_a_floor_deadline_fails() {
    let dispatched: Vec<u64> = (0..DRAIN_WINDOW as u64).map(|k| k * 50).collect();
    let delivered = vec![None; DRAIN_WINDOW];
    assert_eq!(
        judged(&dispatched, &delivered, 6_000),
        DrainVerdict::WindowHeld {
            write: DRAIN_WINDOW,
            pending: DRAIN_WINDOW,
        }
    );
}

/// A pipeline at 20/s whose first window lands at once after a cold start:
/// at 6.00s it frees write 60 on its floor deadline, at 6.05s it does not.
#[test]
fn the_first_window_may_take_six_seconds() {
    let cold = |first: u64| move |k: u64| first.max(50 * k + 20);
    let (dispatched, delivered) = throttled(cold(6_000));
    assert!(matches!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::Pass { .. }
    ));
    let (dispatched, delivered) = throttled(cold(6_050));
    assert_eq!(
        judged(&dispatched, &delivered, 60_400),
        DrainVerdict::WindowHeld {
            write: DRAIN_WINDOW,
            pending: DRAIN_WINDOW,
        }
    );
}

#[test]
#[should_panic(expected = "closed twice")]
fn a_write_closed_twice_is_refused() {
    let mut drive = drive();
    let base = Instant::now();
    let at = |ms: u64| base + Duration::from_millis(ms);
    let sample = |k: usize, delivered: u64, ms: u64| E2eSample {
        action: "set_field".to_string(),
        target: target(k),
        origin: ClockOrigin::Ui,
        ms,
        in_flight: 1,
        backlog: 0,
        contended: false,
        delivered_at: at(delivered),
        delivery_batch: 0,
        source: "block".to_string(),
        feed: 0,
        superseded: Superseded::default(),
        quiet: QuietBatches::default(),
    };
    assert!(matches!(
        drive.step(at(0), &[], Vec::new),
        Step::Dispatch { .. }
    ));
    drive.dispatched(at(0));
    let mut samples = vec![sample(N, 10, 10)];
    assert!(matches!(
        drive.step(at(10), &samples, Vec::new),
        Step::Dispatch { .. }
    ));
    drive.dispatched(at(10));
    samples.push(sample(0, 20, 10));
    samples.push(sample(0, 30, 20));
    drive.step(at(30), &samples, Vec::new);
}
