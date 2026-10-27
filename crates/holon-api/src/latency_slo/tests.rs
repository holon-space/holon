use super::*;

/// A sample closed by its own delivery batch.
fn sample(ms: u64, in_flight: usize, backlog: usize, at: Instant) -> E2eSample {
    static NEXT_BATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let batch = NEXT_BATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    batched(ms, in_flight, backlog, at, batch)
}

fn batched(ms: u64, in_flight: usize, backlog: usize, at: Instant, batch: u64) -> E2eSample {
    E2eSample {
        action: "set_field".to_string(),
        target: "block:x".to_string(),
        origin: ClockOrigin::Ui,
        ms,
        in_flight,
        backlog,
        contended: false,
        delivered_at: at,
        delivery_batch: batch,
        source: "block".to_string(),
        feed: 0,
        superseded: Superseded::default(),
        quiet: QuietBatches::default(),
    }
}

/// A paced sample that shared the pipeline with the other origin.
fn contended(ms: u64, at: Instant) -> E2eSample {
    E2eSample {
        contended: true,
        ..sample(ms, 1, 0, at)
    }
}

/// A window of paced samples: each dispatched alone, so all of them are
/// service time and the p95 is the pipeline's own cost.
fn paced(ms: &[u64]) -> SloWindow {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for (i, &m) in ms.iter().enumerate() {
        w.record(sample(m, 1, 0, t0 + Duration::from_millis(50 * i as u64)));
    }
    w
}

/// The bug this module fixes: a queue ramp under saturation. Every sample
/// after the first was dispatched behind others, and `ms` grows with the
/// queue even though the pipeline retires one write every 10ms.
fn burst(n: usize) -> SloWindow {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..n {
        w.record(sample(
            10 + 10 * i as u64,
            i + 1,
            n - i - 1,
            t0 + Duration::from_millis(10 * i as u64),
        ));
    }
    w
}

#[test]
fn service_rung_scores_no_sample_from_a_saturated_burst() {
    let w = burst(60);
    // Every sample ramps past the SLO, and not one of them was alone for
    // its whole life: the head was overtaken, the rest queued behind. The
    // rung reports no verdict rather than reporting the ramp.
    assert_eq!(w.service_sample_count(), 0);
    assert!(matches!(
        w.service_verdict(),
        RungVerdict::Unjudged { n: 0, .. }
    ));
}

/// The burst HEAD is dispatched into an empty queue but overtaken by
/// everything behind it, so its `ms` carries contention. `backlog` is what
/// excludes it; without that half the rung would score a 2150ms sample from
/// a pipeline doing 62ms of work per write.
#[test]
fn the_head_of_a_burst_is_not_a_service_sample() {
    let t0 = Instant::now();
    let head = sample(2150, 1, 39, t0);
    assert!(!head.is_service_time());
    let alone = sample(26, 1, 0, t0);
    assert!(alone.is_service_time());
}

#[test]
fn service_rung_passes_on_a_fast_paced_pipeline() {
    let w = paced(&[11; 32]);
    assert_eq!(w.service_p95_ms(), Some(11));
    assert!(matches!(w.service_verdict(), RungVerdict::Pass { .. }));
}

#[test]
fn service_rung_fails_on_a_slow_paced_pipeline() {
    let w = paced(&[250; 32]);
    assert!(w.service_verdict().is_fail());
}

/// The old measurement's teeth, restated: gating raw `ms` would fail the
/// burst window, which is a healthy pipeline driven fast.
#[test]
fn raw_ms_p95_would_condemn_the_same_healthy_pipeline_the_service_rung_clears() {
    let w = burst(60);
    let mut all: Vec<u64> = w.samples().iter().map(|s| s.ms).collect();
    all.sort_unstable();
    let raw_p95 = all[((all.len() as f64 * 0.95).ceil() as usize) - 1];
    assert!(
        raw_p95 > SERVICE_TIME_SLO_MS,
        "the burst window's raw p95 is {raw_p95}ms — the measurement D50.a replaces"
    );
    // Same events, judged the new way: no service verdict is even offered.
    assert_eq!(w.service_sample_count(), 0);
    // Same events, same pipeline: 10ms per write is 100/s, far above floor.
    assert!(matches!(
        w.drain_estimate(),
        DrainEstimate::AtOrAbove { .. }
    ));
}

/// A write of source `block`, dispatched and landed at the given offsets
/// from `t0` in microseconds, closed by delivery batch `batch`.
fn write(t0: Instant, dispatched_us: u64, landed_us: u64, batch: u64) -> E2eSample {
    batched(
        (landed_us - dispatched_us) / 1000,
        2,
        0,
        t0 + Duration::from_micros(landed_us),
        batch,
    )
}

/// A serial pipeline of `pass_ms` per write, fed so that write `k` is
/// dispatched `lead_ms` before write `k-1` lands. Write 0 is dispatched at
/// `t0 + start_ms`.
fn serial(
    w: &mut SloWindow,
    t0: Instant,
    start_ms: u64,
    writes: u64,
    pass_ms: u64,
    lead_ms: u64,
    first_batch: u64,
) {
    for k in 0..writes {
        let landed = start_ms + pass_ms * (k + 1);
        let dispatched = if k == 0 {
            start_ms
        } else {
            landed - pass_ms - lead_ms
        };
        w.record(write(t0, dispatched * 1000, landed * 1000, first_batch + k));
    }
}

/// The estimate's rate, or `None` when nothing was judged.
fn rate(w: &SloWindow) -> Option<f64> {
    match w.drain_estimate() {
        DrainEstimate::AtOrAbove { rate, .. } | DrainEstimate::Below { rate, .. } => Some(rate),
        DrainEstimate::Unjudged { .. } => None,
    }
}

/// Saturated passes and deliveries across every judged busy period.
fn judged(w: &SloWindow) -> (usize, usize) {
    match w.drain_estimate() {
        DrainEstimate::AtOrAbove {
            passes, deliveries, ..
        }
        | DrainEstimate::Below {
            passes, deliveries, ..
        } => (passes, deliveries),
        DrainEstimate::Unjudged { .. } => (0, 0),
    }
}

fn assert_unjudged(w: &SloWindow, longest: usize) {
    assert_eq!(
        w.drain_estimate(),
        DrainEstimate::Unjudged {
            longest_busy_period: longest,
            needed: MIN_SATURATED_PASSES
        },
        "{}",
        w.report()
    );
}

fn assert_below_at(w: &SloWindow, expected: f64, passes: usize) {
    let DrainEstimate::Below {
        rate,
        passes: judged,
        ..
    } = w.drain_estimate()
    else {
        panic!("expected a drain estimate below the floor: {}", w.report());
    };
    assert_eq!(judged, passes, "{}", w.report());
    assert!(
        (rate - expected).abs() < 0.01,
        "expected {expected}/s, got {rate}/s"
    );
}

#[test]
fn throughput_rung_ignores_idle_gaps() {
    // Paced samples: each write lands before the next is dispatched.
    let w = paced(&[11; 60]);
    assert_unjudged(&w, 0);
}

/// Batched deliveries retire together. Averaging the per-delivery gaps
/// counted the zeros between them as infinitely fast work and reported
/// 99,156 writes/s for a pipeline sleeping 300ms per batch; landing gaps
/// report what actually happened.
#[test]
fn batched_deliveries_do_not_inflate_the_drain_rate() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    // 60 writes dispatched at t0, retired in six passes of 10, 300ms
    // apart: passes 2-4 each leave work behind, 30 deliveries over 900ms.
    for pass in 0..6u64 {
        for i in 0..10u64 {
            w.record(write(t0, 0, 300_000 * (pass + 1) + i, pass));
        }
    }
    assert_eq!(judged(&w), (3, 30));
    let rate = rate(&w).expect("a judged busy period");
    assert!((rate - 30.0 / 0.9).abs() < 0.01, "got {rate}");
}

/// One pass says nothing about a drain rate: its start is not observable,
/// and the time before it holds the first write's own service time.
#[test]
fn a_burst_retired_in_one_pass_is_unjudged() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..64u64 {
        w.record(write(t0, 20 * i, 100_000 + i, 7));
    }
    assert_unjudged(&w, 0);
}

/// Probe P1. A write alone for 300ms, joined by a second one 1ms before
/// the pass lands, is a slow SERVICE, not a slow drain.
#[test]
fn a_lone_slow_write_joined_late_is_not_a_slow_drain() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for round in 0..3u64 {
        let start = 1_000_000 * round;
        w.record(write(t0, start, start + 300_000, round));
        w.record(write(t0, start + 299_000, start + 300_005, round));
    }
    assert_unjudged(&w, 0);
}

/// Probe M1's fixture. C waits behind X's pass and lands in the next one:
/// one saturated pass is not sustained load.
#[test]
fn a_write_queued_across_one_landing_is_not_sustained_load() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    w.record(write(t0, 50_000, 100_000, 1));
    w.record(write(t0, 10_000, 200_000, 2));
    assert_unjudged(&w, 1);
}

/// Probe M2's fixture. A write whose dispatch cannot be placed before the
/// previous landing, even within its millisecond, did not keep the queue
/// non-empty.
#[test]
fn a_pass_after_an_empty_queue_is_not_saturated() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    w.record(write(t0, 90_000, 100_000, 1));
    for _ in 0..2 {
        w.record(write(t0, 100_300, 150_500, 2));
    }
    assert_unjudged(&w, 0);
}

/// Probe M4's fixture: one pass, one of whose writes reports `ms` 0.
#[test]
fn a_sub_millisecond_write_is_one_pass() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    w.record(write(t0, 0, 5_000, 1));
    w.record(write(t0, 4_900, 5_003, 1));
    assert_unjudged(&w, 0);
}

/// X and Y are dispatched together, X lands after 5ms, and Y takes another
/// 300ms on its own. A two-write burst is a service event.
#[test]
fn a_write_left_alone_behind_a_retired_one_is_not_a_slow_drain() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for round in 0..3u64 {
        let start = 1_000_000 * round;
        w.record(write(t0, start, start + 5_000, 2 * round));
        w.record(write(t0, start + 1_000, start + 305_000, 2 * round + 1));
    }
    assert_unjudged(&w, 1);
}

/// Probe P2. A clock that never delivers holds no busy period open: the
/// paced writes after it each fly alone, whatever backlog the correlator
/// reports.
#[test]
fn an_undelivered_clock_does_not_make_idle_gaps_saturated() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..40u64 {
        w.record(sample(20, 2, 1, t0 + Duration::from_secs(i)));
    }
    assert_unjudged(&w, 0);
}

/// Probe Q1. A serial 300ms pipeline fed at exactly its own rate: write
/// `k` arrives 1ms before pass `k-1` lands. The queue never holds a second
/// write, so nothing shows the capacity is short: no verdict.
#[test]
fn a_serial_pipeline_fed_at_its_own_rate_is_unjudged() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    serial(&mut w, t0, 0, 40, 300, 1, 0);
    assert_unjudged(&w, 39);
}

/// Probe Q2. Arrivals every 299ms against a 300ms serial pipeline: the
/// queue grows by one write every 299 passes. Once it holds a third write,
/// every pass leaves work behind for the pass after next, and the rate is
/// the pipeline's own.
#[test]
fn an_overloaded_serial_pipeline_is_a_slow_drain_once_its_queue_grows() {
    let record = |n: u64| {
        let mut w = SloWindow::new(
            ClockOrigin::Ui,
            n as usize,
            SERVICE_TIME_SLO_MS,
            THROUGHPUT_FLOOR_WRITES_PER_SEC,
        );
        let t0 = Instant::now();
        for k in 0..n {
            w.record(write(t0, 299_000 * k, 300_000 * (k + 1), k));
        }
        w
    };
    for n in [40u64, 150, 400] {
        assert_unjudged(&record(n), n as usize - 1);
    }
    assert_below_at(&record(700), 1000.0 / 300.0, 100);
}

/// Probe Q3. Two writes together through a serial 300ms pipeline, three
/// times with idle gaps: a two-write burst, so no verdict.
#[test]
fn a_two_write_burst_through_a_slow_pipeline_is_unjudged() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for round in 0..3u64 {
        let start = 10_000_000 * round;
        w.record(write(t0, start, start + 300_000, 2 * round));
        w.record(write(t0, start, start + 600_000, 2 * round + 1));
    }
    assert_unjudged(&w, 1);
}

/// Probe Q4. A slow head alone for 300ms and a cheap write that joins 1ms
/// after it and lands 1ms after it: the slowness is the head's service.
#[test]
fn a_slow_head_with_a_cheap_follower_is_unjudged() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for round in 0..3u64 {
        let start = 10_000_000 * round;
        w.record(write(t0, start, start + 300_000, 2 * round));
        w.record(write(t0, start + 1_000, start + 301_000, 2 * round + 1));
    }
    assert_unjudged(&w, 1);
}

/// Probe "leftover at the landing instant": write `k` is dispatched as
/// pass `k-2` lands and retired by pass `k`. It is also the trace of an
/// unbounded 300ms pipeline whose writes commit 1ms after dispatch, too
/// late for pass `k-1`, so the one pass it waited proves no backlog.
#[test]
fn a_write_dispatched_as_a_pass_lands_is_unjudged() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for k in 0..6u64 {
        let dispatched = 300_000 * k.saturating_sub(1);
        w.record(write(t0, dispatched, 300_000 * (k + 1), k));
    }
    assert_unjudged(&w, 5);
}

/// [`MIN_SATURATED_PASSES`] is the line between a burst and sustained
/// load: two passes that leave work behind are not judged, three are.
#[test]
fn two_saturated_passes_are_not_a_busy_period_and_three_are() {
    let t0 = Instant::now();
    let mut w = SloWindow::default();
    for k in 0..5u64 {
        w.record(write(t0, 0, 300_000 * (k + 1), k));
    }
    assert_unjudged(&w, 4);
    w.record(write(t0, 0, 1_800_000, 5));
    assert_below_at(&w, 1000.0 / 300.0, 3);
}

/// An idle gap ends a busy period. Two short ones either side of it are
/// not one long one, and two judged ones do not charge the gap.
#[test]
fn an_idle_gap_splits_busy_periods() {
    let t0 = Instant::now();
    let burst = |w: &mut SloWindow, start_ms: u64, writes: u64, first_batch: u64| {
        for k in 0..writes {
            let landed = start_ms + 300 * (k + 1);
            w.record(write(t0, start_ms * 1000, landed * 1000, first_batch + k));
        }
    };
    let mut w = SloWindow::default();
    burst(&mut w, 0, 5, 0);
    burst(&mut w, 10_000, 5, 10);
    assert_unjudged(&w, 4);

    let mut w = SloWindow::default();
    burst(&mut w, 0, 6, 0);
    burst(&mut w, 10_000, 6, 10);
    assert_below_at(&w, 1000.0 / 300.0, 6);
}

/// Passes are sequenced per source: a subscriber applies its own batches in
/// order, and two sources run side by side. The slowest judged source
/// decides the verdict.
#[test]
fn sources_are_sequenced_and_judged_separately() {
    let t0 = Instant::now();
    let mut w = SloWindow::default();
    for k in 0..7u64 {
        w.record(write(t0, 0, 300_000 * (k + 1), k));
    }
    for k in 0..6u64 {
        w.record(E2eSample {
            source: "focus_roots".to_string(),
            feed: 1,
            ..write(t0, 150_000, 160_000 + 10_000 * k, 100 + k)
        });
    }
    assert_below_at(&w, 1000.0 / 300.0, 7);
    assert!(
        w.report().contains("slowest source block"),
        "{}",
        w.report()
    );
}

/// Two subscribers of one source are two feeds side by side. The second
/// applies each of the first's batches 1ms later and closes nothing until
/// its last one. Sequenced as one feed, its quiet passes would halve every
/// gap and make the probe above look two passes behind.
#[test]
fn two_feeds_of_one_source_are_sequenced_apart() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for k in 0..6u64 {
        let dispatched = 300_000 * k.saturating_sub(1);
        w.record(write(t0, dispatched, 300_000 * (k + 1), k));
    }
    let closed_at = 2_000_000;
    w.record(E2eSample {
        feed: 1,
        quiet: QuietBatches::new(
            (0..6)
                .map(|k| closed_at - 300_000 * (k + 1) - 1_000)
                .collect(),
        ),
        ..write(t0, 1_900_000, closed_at, 6)
    });
    assert_unjudged(&w, 5);
}

/// One keystroke every `gap_ms` to ONE block through a serial subscriber
/// of `pass_ms` per pass. A pass starts when the previous one lands, or at
/// the next keystroke if none is queued, and shows every keystroke
/// dispatched before it started. The correlator closes only the newest
/// clock on the block and retires the older ones with it.
fn typed_on_one_block(gap_ms: u64, pass_ms: u64, keys: u64) -> SloWindow {
    let mut w = SloWindow::new(
        ClockOrigin::Ui,
        keys as usize,
        SERVICE_TIME_SLO_MS,
        THROUGHPUT_FLOOR_WRITES_PER_SEC,
    );
    let t0 = Instant::now();
    let mut next = 0u64;
    let mut start = 0u64;
    let mut batch = 0u64;
    while next < keys {
        start = start.max(gap_ms * next);
        let taken = (next..keys).take_while(|k| gap_ms * k <= start).count() as u64;
        let newest = next + taken - 1;
        let landed = start + pass_ms;
        w.record(E2eSample {
            superseded: Superseded::new((next..newest).map(|k| landed - gap_ms * k).collect()),
            ..write(t0, gap_ms * newest * 1000, landed * 1000, batch)
        });
        next += taken;
        start = landed;
        batch += 1;
    }
    w
}

fn assert_arrival_limited(gap: u64, pass: u64) {
    let w = typed_on_one_block(gap, pass, 400);
    assert!(
        matches!(w.drain_estimate(), DrainEstimate::Unjudged { .. }),
        "gap {gap}ms pass {pass}ms: {}",
        w.report()
    );
}

/// Probe T1 and its variants: typing at 10/s, 20/s and 12.5/s, at or
/// above the floor, into passes longer than the gap. The rate is the
/// typing rate, retired in full: a pass. Counting one delivery per pass
/// once scored these 6.7/s, 8.3/s and 9.1/s.
#[test]
fn typing_on_one_block_above_the_floor_is_retired_in_full() {
    for (gap, pass, offered) in [(100, 150, 10.0), (50, 120, 20.0), (80, 110, 12.5)] {
        let w = typed_on_one_block(gap, pass, 400);
        let rate = rate(&w).expect("a judged busy period");
        assert!(
            (rate - offered).abs() < offered * 0.01,
            "gap {gap}ms pass {pass}ms: offered {offered}/s, got {rate}/s: {}",
            w.report()
        );
        assert!(
            matches!(w.drain_estimate(), DrainEstimate::AtOrAbove { .. }),
            "{}",
            w.report()
        );
    }
}

/// A subscriber that applies everything queued in one pass, at `base_ms`
/// plus `per_write_ms` per write, fed one write every `gap_ms` to its own
/// block without end. Records the first `passes` passes.
fn batcher(gap_ms: u64, base_ms: u64, per_write_ms: u64, passes: u64) -> SloWindow {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    let mut next = 0u64;
    let mut start = 0u64;
    for batch in 0..passes {
        start = start.max(gap_ms * next);
        let taken = (start / gap_ms) + 1 - next;
        let landed = start + base_ms + per_write_ms * taken;
        for k in next..next + taken {
            w.record(write(t0, gap_ms * k * 1000, landed * 1000, batch));
        }
        next += taken;
        start = landed;
    }
    w
}

/// A batching pipeline whose per-write cost outruns arrivals at the floor:
/// 20 writes/s against 120ms per write. Every pass takes everything queued,
/// so no pass leaves work behind, yet the queue grows. The arrivals reach
/// the floor, so the rate, ~1/120ms = 8.3/s, is the capacity: a slow drain.
#[test]
fn an_overloaded_batching_pipeline_is_a_slow_drain() {
    let w = batcher(50, 0, 120, 7);
    let rate = rate(&w).expect("a judged busy period");
    assert!(
        (rate - 1000.0 / 120.0).abs() < 0.05,
        "got {rate}/s: {}",
        w.report()
    );
    assert!(w.drain_estimate().is_below(), "{}", w.report());
}

/// The arrival-limited residuals: typing slower than the floor into passes
/// longer than the gap. Judging such passes failed a user typing 8 keys/s
/// into 150ms passes.
#[test]
fn typing_slower_than_the_floor_is_not_a_slow_drain() {
    for (gap, pass) in [(120, 150), (150, 180), (190, 199)] {
        assert_arrival_limited(gap, pass);
    }
}

/// Five keystrokes on one block, one every 80ms (12.5/s), through 150ms
/// passes that each take everything queued: the capacity is unbounded, so
/// no verdict may fail. Measuring the arrivals over one pass less than the
/// rate once failed it at 8.9/s.
#[test]
fn a_short_typing_burst_through_an_unbounded_pipeline_does_not_fail() {
    let w = typed_on_one_block(80, 150, 5);
    assert!(!w.drain_estimate().is_below(), "{}", w.report());
}

/// Three blocks, flat 140ms passes that take everything committed, 20ms
/// commit latency: the capacity is unbounded. A pass can close a clock
/// whose row it does not carry yet and supersede an older one with it.
/// Counting that older write at its winner's dispatch moved it into the
/// arrival window and failed the run at 9.5/s.
#[test]
fn a_superseded_write_counts_at_its_own_dispatch() {
    let t0 = Instant::now();
    let mut w = SloWindow::default();
    for (dispatched, landed, batch, superseded) in [
        (217, 377, 0, None),
        (311, 517, 1, None),
        (525, 670, 2, Some(450)),
        (650, 810, 3, None),
        (861, 1021, 4, None),
        (931, 1161, 5, None),
        (1073, 1301, 6, Some(1002)),
        (1238, 1441, 7, None),
    ] {
        w.record(E2eSample {
            superseded: Superseded::new(superseded.map(|d| landed - d).into_iter().collect()),
            ..write(t0, dispatched * 1000, landed * 1000, batch)
        });
    }
    assert!(!w.drain_estimate().is_below(), "{}", w.report());
}

/// A pipeline of unbounded capacity: every pass takes `pass_ms` whatever it
/// carries. It starts when the previous one lands, or once the next write
/// is committed if none is queued, and carries every write committed by
/// its start, `commit_ms` after dispatch. Per target it carries, it closes
/// the newest clock dispatched by its start and supersedes the older ones,
/// as `close_received` does. A pass that closes nothing is reported by the
/// next one that does, as `log_quiet` does. `writes` are `(dispatch ms,
/// target)`, sorted.
fn unbounded_pipeline(writes: &[(u64, u8)], pass_ms: u64, commit_ms: u64) -> SloWindow {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    let (mut carried, mut opened) = (0, 0);
    let mut pending: Vec<(u64, u8)> = Vec::new();
    let mut quiet: Vec<u64> = Vec::new();
    let mut start = 0;
    let mut batch = 0;
    while carried < writes.len() {
        start = start.max(writes[carried].0 + commit_ms);
        let mut targets: Vec<u8> = Vec::new();
        while carried < writes.len() && writes[carried].0 + commit_ms <= start {
            targets.push(writes[carried].1);
            carried += 1;
        }
        targets.sort_unstable();
        targets.dedup();
        while opened < writes.len() && writes[opened].0 <= start {
            pending.push(writes[opened]);
            opened += 1;
        }
        let landed = start + pass_ms;
        let quiet_ages = QuietBatches::new(quiet.iter().map(|q| (landed - q) * 1000).collect());
        let mut closed_any = false;
        for target in targets {
            let mut on_target: Vec<u64> = pending
                .iter()
                .filter(|p| p.1 == target)
                .map(|p| p.0)
                .collect();
            pending.retain(|p| p.1 != target);
            on_target.sort_unstable();
            let Some(newest) = on_target.pop() else {
                continue;
            };
            closed_any = true;
            w.record(E2eSample {
                target: format!("block:{target}"),
                superseded: Superseded::new(on_target.iter().map(|d| landed - d).collect()),
                quiet: quiet_ages.clone(),
                ..write(t0, newest * 1000, landed * 1000, batch)
            });
        }
        if closed_any {
            quiet.clear();
        } else {
            quiet.push(landed);
        }
        batch += 1;
        start = landed;
    }
    w
}

/// A write dispatched one millisecond after the busy period's first one
/// is an arrival: here it is what lifts the arrivals to 12.1/s.
#[test]
fn a_write_one_millisecond_after_the_first_is_an_arrival() {
    let w = unbounded_pipeline(&[(20, 3), (21, 3), (121, 1), (221, 1), (241, 1)], 110, 0);
    assert_eq!(
        w.drain_estimate(),
        DrainEstimate::AtOrAbove {
            rate: 4.0 / 0.33,
            passes: 3,
            deliveries: judged(&w).1,
        },
        "{}",
        w.report()
    );
}

/// A write dispatched in the instant the last pass starts is an arrival:
/// here the two at 520ms lift the arrivals to 14.3/s.
#[test]
fn a_write_dispatched_as_the_last_pass_starts_is_an_arrival() {
    let w = unbounded_pipeline(
        &[
            (100, 2),
            (200, 0),
            (280, 0),
            (380, 0),
            (480, 0),
            (520, 0),
            (520, 0),
        ],
        140,
        0,
    );
    assert_eq!(
        w.drain_estimate(),
        DrainEstimate::AtOrAbove {
            rate: 6.0 / 0.42,
            passes: 3,
            deliveries: judged(&w).1,
        },
        "{}",
        w.report()
    );
}

/// A write dispatched before the busy period's first one, superseded by
/// a winner inside it, did not arrive during the period. Counted at its
/// winner's dispatch it would lift the arrivals from 9.7/s to 12.9/s and
/// fail a 6.9/s rate that is only the arrivals'.
#[test]
fn a_write_older_than_the_busy_period_is_not_its_arrival() {
    let t0 = Instant::now();
    let mut w = SloWindow::default();
    for (dispatched, landed, batch) in [(10, 20, 0), (15, 150, 1), (140, 320, 2)] {
        w.record(write(t0, dispatched * 1000, landed * 1000, batch));
    }
    w.record(E2eSample {
        superseded: Superseded::new(vec![600]),
        ..write(t0, 300_000, 600_000, 3)
    });
    assert_unjudged(&w, 3);
}

/// Only the busy period's own passes retire its arrivals. Pass 0 here
/// retired writes dispatched after its first one, 30ms commit latency
/// apart; counting them as arrivals failed an unbounded pipeline at
/// 7.5/s.
#[test]
fn writes_retired_before_the_busy_period_are_not_its_arrivals() {
    let w = unbounded_pipeline(
        &[(5, 0), (10, 0), (30, 0), (130, 2), (210, 0), (310, 1)],
        100,
        30,
    );
    assert!(!w.drain_estimate().is_below(), "{}", w.report());
}

/// Unbounded 202ms passes, 125ms commit latency. The pass landing at
/// 1012ms carries the writes of 522 and 607 on block 1, whose clock the
/// pass before closed early, so it closes nothing. Without it the last
/// gap spans two passes and the busy period fails at 9.9/s.
#[test]
fn a_pass_that_closes_nothing_is_still_a_pass() {
    let w = unbounded_pipeline(
        &[
            (79, 0),
            (122, 1),
            (232, 1),
            (330, 1),
            (367, 0),
            (417, 1),
            (522, 1),
            (607, 1),
            (705, 0),
        ],
        202,
        125,
    );
    assert_unjudged(&w, 4);
}

/// A write commits 25ms after its dispatch, while its feed applies five
/// 5ms batches of untracked rows. Those passes retired nothing and only
/// waited on the commit: judged as passes that left the write behind they
/// fail the drain at 0/s.
#[test]
fn quiet_passes_waiting_on_a_commit_are_not_a_slow_drain() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    w.record(write(t0, 0, 100_000, 0));
    w.record(E2eSample {
        quiet: QuietBatches::new((1..=5).map(|k| 30_000 - 5_000 * k).collect()),
        ..write(t0, 100_000, 130_000, 1)
    });
    assert_unjudged(&w, 6);
}

/// A pass that closes one clock and supersedes another retired both. The
/// drain rate counts retired interactions, not closures.
#[test]
fn a_saturated_pass_counts_every_interaction_it_retired() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for k in 0..6u64 {
        w.record(E2eSample {
            superseded: Superseded::new(vec![300 * (k + 1)]),
            ..write(t0, 0, 300_000 * (k + 1), k)
        });
    }
    assert_eq!(judged(&w).1, 6);
    assert_below_at(&w, 6.0 / 0.9, 3);
}

/// **The false-red the third estimator shipped.** A healthy session — 40
/// writes of 10ms, one per minute — plus a single delivery that happened
/// to leave a backlog. Idle gaps must contribute nothing.
#[test]
fn an_idle_session_with_one_queued_delivery_is_not_a_slow_drain() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..40u64 {
        w.record(sample(10, 1, 0, t0 + Duration::from_secs(60 * i)));
    }
    w.record(sample(10, 2, 1, t0 + Duration::from_secs(60 * 40)));
    assert_unjudged(&w, 0);
}

/// **The estimate counts saturated passes, not samples.** Returning
/// `samples.len()` once let 39 unsaturated samples plus one saturated one
/// satisfy a floor of 20.
#[test]
fn the_estimate_counts_saturated_passes_only() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..39u64 {
        w.record(sample(10, 1, 0, t0 + Duration::from_millis(50 * i)));
    }
    w.record(sample(10, 2, 1, t0 + Duration::from_millis(50 * 39)));
    assert_eq!(w.samples().len(), 40);
    assert_eq!(judged(&w).0, 0);
}

/// 40 writes dispatched together, retired one per 150ms: passes 2-38 each
/// leave work behind, 37 of them in 5.55s, ~6.7/s, under the 10/s floor.
/// The time before the first landing is not charged: it holds the first
/// write's service.
#[test]
fn the_estimate_reads_a_slow_drain_below_the_floor() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for k in 0..40u64 {
        w.record(write(t0, 0, 150_000 * (k + 1), k));
    }
    assert_below_at(&w, 37.0 / 5.55, 37);
}

/// A burst whose every write reports zero elapsed in one shared batch is
/// one pass, and a rate of infinity is never read.
#[test]
fn an_unresolvable_burst_is_never_infinitely_fast() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..40 {
        w.record(batched(0, i + 1, 0, t0, 1));
    }
    assert_unjudged(&w, 0);
}

#[test]
fn the_window_retains_only_its_capacity() {
    let mut w = SloWindow::new(
        ClockOrigin::Ui,
        4,
        SERVICE_TIME_SLO_MS,
        THROUGHPUT_FLOOR_WRITES_PER_SEC,
    );
    let t0 = Instant::now();
    for i in 0..10u64 {
        w.record(sample(i, 1, 0, t0 + Duration::from_millis(i)));
    }
    assert_eq!(w.len(), 4);
    assert_eq!(w.samples()[0].ms, 6);
}

fn facade_sample(ms: u64, at: Instant) -> E2eSample {
    E2eSample {
        origin: ClockOrigin::Facade,
        ..sample(ms, 1, 0, at)
    }
}

/// **D119.a round 2.** The pipeline is shared even though the populations
/// are not: a sample dispatched alongside foreign traffic waited behind it,
/// so scoring it as uncontended service time would report queue wait as
/// service time. It is excluded.
#[test]
fn a_sample_that_shared_the_pipeline_is_not_service_time() {
    let s = contended(100, Instant::now());
    assert!(!s.is_service_time(), "foreign traffic is real contention");
    assert!(s.excluded_by_cross_origin());
}

/// And the exclusion is COUNTED and NAMED. A silently shrunk population is
/// indistinguishable from a quiet stretch, which is what made the
/// shared-registry behaviour a defect rather than a policy.
#[test]
fn cross_origin_exclusions_are_counted_and_named_in_the_report() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..40u64 {
        w.record(sample(10, 1, 0, t0 + Duration::from_millis(50 * i)));
    }
    for i in 0..7u64 {
        w.record(contended(500, t0 + Duration::from_secs(10 + i)));
    }

    assert_eq!(
        w.service_sample_count(),
        40,
        "the contended ones are excluded"
    );
    assert_eq!(w.cross_origin_excluded(), 7);
    let report = w.report();
    assert!(
        report.contains("7 excluded: facade traffic in the shared pipeline"),
        "the report must say why the population is thin, got: {report}"
    );
}

/// The exclusion must not silently empty a rung either: an all-contended
/// window reports Unjudged AND says what it dropped.
#[test]
fn an_all_contended_window_is_unjudged_and_says_why() {
    let mut w = SloWindow::default();
    let t0 = Instant::now();
    for i in 0..40u64 {
        w.record(contended(500, t0 + Duration::from_millis(50 * i)));
    }
    assert!(matches!(
        w.service_verdict(),
        RungVerdict::Unjudged { n: 0, .. }
    ));
    assert!(w.report().contains("40 excluded: facade traffic"));
}

/// The pooling guard. A window scores one origin, so handing it a foreign
/// sample is refused at the door rather than quietly widening the
/// population its p95 is taken over.
#[test]
#[should_panic(expected = "origins are scored in separate windows")]
fn a_ui_window_refuses_a_facade_sample() {
    let mut w = SloWindow::default();
    w.record(facade_sample(5, Instant::now()));
}

/// The routed front door keeps the two populations apart: a facade sample
/// that would dominate the UI p95 changes only the facade one.
#[test]
fn origin_windows_score_the_two_seams_separately() {
    let mut w = OriginWindows::new(512, SERVICE_TIME_SLO_MS, THROUGHPUT_FLOOR_WRITES_PER_SEC);
    let t0 = Instant::now();
    for i in 0..40u64 {
        w.record(sample(100, 1, 0, t0 + Duration::from_millis(50 * i)));
    }
    w.record(facade_sample(1, t0 + Duration::from_secs(10)));

    assert_eq!(w.ui().len(), 40);
    assert_eq!(w.facade().len(), 1);
    assert_eq!(w.ui().service_p95_ms(), Some(100));
    assert_eq!(w.facade().service_p95_ms(), Some(1));
    assert_eq!(
        w.report_lines().len(),
        2,
        "each origin is reported on its own line"
    );
}
