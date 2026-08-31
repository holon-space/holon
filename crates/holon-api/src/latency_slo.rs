//! The latency SLO, as the two numbers it actually is.
//!
//! The project SLO ("interaction→projection-visible p95 < 200ms") used to be
//! scored by reading `ms` off every `stage="e2e"` event. That measures the
//! wrong thing under load: `ms` is service time PLUS the wait behind everything
//! queued ahead, so a pipeline driven faster than it drains reports a
//! queue-depth ramp — 12, 26, 40, 53 … 441ms — and every sample past the first
//! few breaches. A healthy pipeline typed at by an agent looked broken, and a
//! genuinely slow one driven slowly looked fine (BugFunnel 2026-08-31).
//!
//! Martin's ruling D50.a splits it into two independent rungs, scored here so
//! the runtime oracle and the land gate can never disagree about either:
//!
//! 1. **Service time** — [`SloWindow::service_p95_ms`]. The p95 of samples that
//!    were alone in the pipeline for their whole life (see
//!    [`E2eSample::is_service_time`]), so no sample carries another
//!    interaction's wait. Budget: [`SERVICE_TIME_SLO_MS`].
//! 2. **Throughput** — [`SloWindow::drain_rate_per_sec`]. Deliveries per second
//!    of wall time, summed over the INTERVALS in which the queue was non-empty
//!    (`backlog > 0`). An idle gap contributes neither its time nor its
//!    deliveries, so a mostly-quiet session cannot be scored as slow draining.
//!    Floor: [`THROUGHPUT_FLOOR_WRITES_PER_SEC`].
//!
//! Each rung reports [`RungVerdict::Unjudged`] below its sample floor rather
//! than passing on thin evidence: a gate that goes green because it collected
//! four samples is the failure mode this module exists to prevent.

use std::time::Duration;
use std::time::Instant;

/// Service-time budget. The project SLO, unchanged — what changed is which
/// samples are eligible to be scored against it.
pub const SERVICE_TIME_SLO_MS: u64 = 200;

/// Throughput floor: writes per second the pipeline must retire under a burst.
///
/// **10/s = 100ms per write. REPORT-ONLY, and under-calibrated — see below.**
///
/// Calibrated on the environment the gate runs in, not the one the bug was
/// found in: the ruling cites ~53 writes/s from a debug GPUI app, but the
/// headless harness carries OTel span layers and its own runtime, so only its
/// numbers can fail a build.
///
/// Observed by `latency_slo_rung_throughput_floor` on hosts the contention
/// covariate ADMITTED, unmodified tree, with THIS estimator: **42.8 · 64.4 ·
/// 460.3 · 460.7 · 809.6 · 1021.4 writes/s** — a 24x spread. (Earlier
/// observations of 7.7-27.0/s were taken with the
/// first-dispatch-to-last-delivery estimator that
/// [`SloWindow::saturated_drain`] replaced, and are void.)
///
/// A 24x spread cannot carry a gating floor at any value, which is why the rate
/// is REPORT-ONLY: `latency_slo_rung_throughput_floor` prints it and never
/// fails on it. 10/s is a floor-of-last-resort, below every observation, whose
/// only job is to give the printed line a reference point.
///
/// Two known contributors to the spread, neither yet attributed:
///  * the CDC actor's batching granularity — inter-batch gaps collapse toward
///    zero when batches arrive back-to-back, and the rate is deliveries over
///    exactly those gaps;
///  * burst write loss — 150 dispatches yield 63-64 deliveries, so the measured
///    workload is neither the dispatched one nor constant between runs.
///
/// **Promotion condition.** Gate this floor only once (a) the burst loss is
/// eliminated or held constant, (b) the batching contribution is understood,
/// and (c) five admitted runs on a quiet host agree within ~1.6x; then set the
/// floor to the worst of those divided by 1.6, per
/// `docs/Testing/latency-ceilings.txt`. The scorer is sound — its falsification
/// is `throughput_rung_fails_a_slow_drain` and the two false-red pins beside it
/// — and it is this constant, and the workload feeding it, that are
/// under-evidenced.
pub const THROUGHPUT_FLOOR_WRITES_PER_SEC: f64 = 10.0;

/// Service samples required before the p95 rung will return a verdict. The
/// ruling names n ≥ 30; a p95 over fewer samples is one sample's opinion.
pub const MIN_SERVICE_SAMPLES: usize = 30;

/// Saturated intervals the throughput rung needs before it will report a rate.
///
/// Low on purpose. The rate is REPORT-ONLY (see
/// [`THROUGHPUT_FLOOR_WRITES_PER_SEC`]), so this floor is not protecting a
/// verdict — it exists to catch a drive that stopped saturating at all, which
/// is the failure mode an integration rung owns. A higher floor would red an
/// unmodified tree: the same 150-write burst produced 5, 58, 60 and 61
/// saturated intervals across four runs, because how many inter-batch gaps a
/// burst opens depends on the CDC actor's batching. The interval and delivery
/// counts are printed beside every rate so thin evidence is visible rather than
/// silently averaged.
pub const MIN_DRAIN_INTERVALS: usize = 3;

/// Slow the CDC delivery actor on purpose, so the rungs above can be shown to
/// have teeth.
///
/// A gate nobody has watched fail is a decoration. The obvious way to prove
/// these two — a uniform delay switched on by the environment — does not work
/// here: boot performs hundreds of deliveries, so a delay large enough to
/// breach a 200ms service budget wedges the SUT before the measurement starts
/// (observed at 250ms: the boot settle failed and both rungs died for the wrong
/// reason). The delay therefore has to be armable AFTER boot, which is what
/// this is.
///
/// Feature-gated (`slo-fault-injection`, off by default): a release build has
/// no injector to call and no branch in the CDC apply path at all. Enabled only
/// by `holon-integration-tests`. Cost when the feature IS on and the injector
/// is disarmed: one relaxed atomic load per delivered batch.
#[cfg(feature = "slo-fault-injection")]
pub mod fault_injection {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    static DELIVERY_DELAY_MS: AtomicU64 = AtomicU64::new(0);

    /// Delay every subsequent CDC delivery by `ms` PER ROW it carries (`0`
    /// disarms) — so the armed pipeline behaves as though each write costs
    /// `ms` more, which is what the throughput rung measures.
    pub fn set_delivery_delay_ms(ms: u64) {
        DELIVERY_DELAY_MS.store(ms, Ordering::Relaxed);
    }

    /// The armed delay, or `None`. Called once per delivered batch.
    pub fn delivery_delay() -> Option<Duration> {
        match DELIVERY_DELAY_MS.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(Duration::from_millis(ms)),
        }
    }
}

/// One closed `stage="e2e"` measurement, as the correlator emits it.
#[derive(Clone, Debug)]
pub struct E2eSample {
    pub action: String,
    /// Wall time dispatch→projection-visible: service time plus queue wait.
    pub ms: u64,
    /// Interactions in flight when this one was dispatched, itself included.
    pub in_flight: usize,
    /// Interactions still pending after this one was delivered.
    pub backlog: usize,
    /// When the delivery closed. The throughput rung's clock.
    pub delivered_at: Instant,
}

impl E2eSample {
    /// Whether this sample is service time alone: the interaction was the only
    /// one in flight for its WHOLE life — nothing queued ahead of it at
    /// dispatch, nothing still pending when it was delivered.
    ///
    /// Both halves are load-bearing. `in_flight == 1` alone admits the head of
    /// a burst, which is dispatched into an empty queue and then overtaken by
    /// everything behind it: measured at 2150ms in a 40-write burst whose real
    /// per-write cost was ~62ms. Scoring that as service time is the queue-wait
    /// contamination this rung exists to exclude.
    pub fn is_service_time(&self) -> bool {
        self.in_flight == 1 && self.backlog == 0
    }
}

/// One rung's outcome. `Unjudged` is not a pass: it says the window never held
/// enough evidence to decide, and a caller must treat it as such.
#[derive(Clone, Debug, PartialEq)]
pub enum RungVerdict {
    Pass { measured: f64, n: usize },
    Fail { measured: f64, n: usize },
    Unjudged { n: usize, needed: usize },
}

impl RungVerdict {
    pub fn is_fail(&self) -> bool {
        matches!(self, RungVerdict::Fail { .. })
    }
}

/// A rolling window of `stage="e2e"` samples, scored as the two D50.a rungs.
///
/// `capacity` bounds retention so a long-lived process (the runtime oracle)
/// judges recent behaviour rather than the whole session; a gate rung sizes it
/// past its own sample count and keeps everything.
pub struct SloWindow {
    samples: Vec<E2eSample>,
    capacity: usize,
    slo_ms: u64,
    floor_per_sec: f64,
}

impl Default for SloWindow {
    fn default() -> Self {
        Self::new(512, SERVICE_TIME_SLO_MS, THROUGHPUT_FLOOR_WRITES_PER_SEC)
    }
}

impl SloWindow {
    pub fn new(capacity: usize, slo_ms: u64, floor_per_sec: f64) -> Self {
        assert!(capacity > 0, "SloWindow capacity must be non-zero");
        Self {
            samples: Vec::new(),
            capacity,
            slo_ms,
            floor_per_sec,
        }
    }

    pub fn record(&mut self, sample: E2eSample) {
        if self.samples.len() == self.capacity {
            self.samples.remove(0);
        }
        self.samples.push(sample);
    }

    /// Drop every sample. A consumer measuring a specific stretch of work calls
    /// this at its start so setup deliveries cannot be scored as the workload.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn samples(&self) -> &[E2eSample] {
        &self.samples
    }

    /// RUNG 1 — p95 of the service-time samples, nearest-rank.
    pub fn service_p95_ms(&self) -> Option<u64> {
        let mut ms: Vec<u64> = self
            .samples
            .iter()
            .filter(|s| s.is_service_time())
            .map(|s| s.ms)
            .collect();
        if ms.is_empty() {
            return None;
        }
        ms.sort_unstable();
        let rank = ((ms.len() as f64) * 0.95).ceil() as usize;
        Some(ms[rank.clamp(1, ms.len()) - 1])
    }

    pub fn service_sample_count(&self) -> usize {
        self.samples.iter().filter(|s| s.is_service_time()).count()
    }

    /// Median and max of the service-time samples. Not gated — printed beside
    /// the p95 so a red says whether the whole distribution moved or one tail
    /// sample did.
    pub fn service_p50_max_ms(&self) -> Option<(u64, u64)> {
        let mut ms: Vec<u64> = self
            .samples
            .iter()
            .filter(|s| s.is_service_time())
            .map(|s| s.ms)
            .collect();
        if ms.is_empty() {
            return None;
        }
        ms.sort_unstable();
        Some((ms[ms.len() / 2], ms[ms.len() - 1]))
    }

    pub fn service_verdict(&self) -> RungVerdict {
        let n = self.service_sample_count();
        if n < MIN_SERVICE_SAMPLES {
            return RungVerdict::Unjudged {
                n,
                needed: MIN_SERVICE_SAMPLES,
            };
        }
        let p95 = self.service_p95_ms().expect("n >= MIN_SERVICE_SAMPLES > 0") as f64;
        if p95 < self.slo_ms as f64 {
            RungVerdict::Pass { measured: p95, n }
        } else {
            RungVerdict::Fail { measured: p95, n }
        }
    }

    /// The window's CDC delivery batches, in order: `(instant, deliveries,
    /// backlog_after)`. Every closure the actor made in one `apply_batch`
    /// shares an instant, which is why the batch — not the sample — is the unit
    /// this rung reasons about.
    fn batches(&self) -> Vec<(Instant, usize, usize)> {
        let mut out: Vec<(Instant, usize, usize)> = Vec::new();
        for s in &self.samples {
            match out.last_mut() {
                Some(b) if b.0 == s.delivered_at => {
                    b.1 += 1;
                    b.2 = s.backlog;
                }
                _ => out.push((s.delivered_at, 1, s.backlog)),
            }
        }
        out
    }

    /// The drain measurement: deliveries retired across SATURATED intervals,
    /// how many such intervals there were, and their total wall time.
    ///
    /// An interval is the gap between two consecutive delivery batches, and it
    /// counts ONLY when the earlier batch left work still queued. An idle gap
    /// contributes nothing — not its time and not its deliveries — so a session
    /// that was quiet for an hour and then queued once cannot be scored as an
    /// hour of slow draining.
    ///
    /// Three estimators failed here before this one; each is worth knowing
    /// because each looked right:
    ///  * the MEAN OF PER-DELIVERY GAPS reported 99,156 writes/s for a pipeline
    ///    sleeping 300ms per batch — deliveries inside one batch share an
    ///    instant, so most gaps were zero;
    ///  * FIRST-TO-LAST DELIVERY reported up to 179,834 writes/s for the same
    ///    reason: a burst landing in one batch spans microseconds;
    ///  * FIRST DISPATCH TO LAST DELIVERY over the whole window fixed both but
    ///    stopped excluding idle time, so 40 healthy writes one per minute plus
    ///    a single queued one scored `0.0 writes/s` and raised a banner on a
    ///    perfectly healthy pipeline — the exact false-red class this module
    ///    exists to remove.
    ///
    /// Per-interval gating is what satisfies all three constraints at once.
    fn saturated_drain(&self) -> Option<(usize, usize, Duration)> {
        let batches = self.batches();
        let mut deliveries = 0usize;
        let mut intervals = 0usize;
        let mut elapsed = Duration::ZERO;
        for pair in batches.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            if prev.2 == 0 {
                continue;
            }
            deliveries += next.1;
            intervals += 1;
            elapsed += next.0.saturating_duration_since(prev.0);
        }
        (intervals > 0).then_some((deliveries, intervals, elapsed))
    }

    /// RUNG 2 — writes per second the pipeline retires while saturated.
    pub fn drain_rate_per_sec(&self) -> Option<f64> {
        let (deliveries, _, elapsed) = self.saturated_drain()?;
        let secs = elapsed.as_secs_f64();
        // Saturated work inside one clock tick is unmeasurable, never
        // "infinitely fast" — a caller must not read a pass out of it.
        if secs <= 0.0 {
            return None;
        }
        Some(deliveries as f64 / secs)
    }

    /// Intervals during which the pipeline had work queued — the count the
    /// rung's guard and [`MIN_DRAIN_INTERVALS`] are about. Says what it means:
    /// an unsaturated window returns 0 however many samples it holds.
    pub fn drain_interval_count(&self) -> usize {
        self.saturated_drain()
            .map_or(0, |(_, intervals, _)| intervals)
    }

    /// Deliveries retired across those intervals — the verdict's `n`.
    pub fn drain_delivery_count(&self) -> usize {
        self.saturated_drain()
            .map_or(0, |(deliveries, _, _)| deliveries)
    }

    pub fn throughput_verdict(&self) -> RungVerdict {
        let n = self.drain_interval_count();
        if n < MIN_DRAIN_INTERVALS {
            return RungVerdict::Unjudged {
                n,
                needed: MIN_DRAIN_INTERVALS,
            };
        }
        let Some(rate) = self.drain_rate_per_sec() else {
            return RungVerdict::Unjudged {
                n: 0,
                needed: MIN_DRAIN_INTERVALS,
            };
        };
        if rate >= self.floor_per_sec {
            RungVerdict::Pass { measured: rate, n }
        } else {
            RungVerdict::Fail { measured: rate, n }
        }
    }

    /// Both rungs on one line, for a banner or a gate log.
    pub fn report(&self) -> String {
        let service = match self.service_verdict() {
            RungVerdict::Pass { measured, n } => {
                format!("service p95 {measured:.0}ms < {}ms over n={n}", self.slo_ms)
            }
            RungVerdict::Fail { measured, n } => format!(
                "service p95 {measured:.0}ms EXCEEDS {}ms over n={n}",
                self.slo_ms
            ),
            RungVerdict::Unjudged { n, needed } => {
                format!("service p95 unjudged (n={n} < {needed})")
            }
        };
        let spread = match self.service_p50_max_ms() {
            Some((p50, max)) => format!(" [p50 {p50}ms max {max}ms]"),
            None => String::new(),
        };
        let throughput = match self.throughput_verdict() {
            RungVerdict::Pass { measured, n } => format!(
                "drain {measured:.1}/s >= {:.1}/s over {n} saturated intervals ({} deliveries)",
                self.floor_per_sec,
                self.drain_delivery_count(),
            ),
            RungVerdict::Fail { measured, n } => format!(
                "drain {measured:.1}/s BELOW {:.1}/s over {n} saturated intervals ({} deliveries)",
                self.floor_per_sec,
                self.drain_delivery_count(),
            ),
            RungVerdict::Unjudged { n, needed } => {
                format!("drain rate unjudged ({n} saturated intervals < {needed})")
            }
        };
        format!("{service}{spread} | {throughput}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ms: u64, in_flight: usize, backlog: usize, at: Instant) -> E2eSample {
        E2eSample {
            action: "set_field".to_string(),
            ms,
            in_flight,
            backlog,
            delivered_at: at,
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
        assert!(matches!(w.throughput_verdict(), RungVerdict::Pass { .. }));
    }

    #[test]
    fn throughput_rung_ignores_idle_gaps() {
        // Paced samples leave an empty backlog, so their 50ms gaps are the
        // driver waiting and form no saturated stretch at all.
        let w = paced(&[11; 60]);
        assert_eq!(w.drain_interval_count(), 0);
        assert!(matches!(
            w.throughput_verdict(),
            RungVerdict::Unjudged { .. }
        ));
    }

    /// Batched deliveries share one instant. Averaging the per-delivery gaps
    /// counted those zeros as infinitely fast work and reported 99,156 writes/s
    /// for a pipeline sleeping 300ms per batch; wall time across the stretch
    /// reports what actually happened.
    #[test]
    fn batched_deliveries_do_not_inflate_the_drain_rate() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        // Four batches of 10, 300ms apart. Three saturated intervals of 300ms,
        // each retiring the 10 deliveries of the batch that closes it: 30
        // deliveries over 0.9s. The first batch's own 10 precede any measured
        // interval and are correctly not attributed to one.
        for batch in 0..4u64 {
            for i in 0..10usize {
                let done = batch as usize * 10 + i;
                w.record(sample(
                    50,
                    5,
                    40 - done - 1,
                    t0 + Duration::from_millis(300 * batch),
                ));
            }
        }
        let rate = w.drain_rate_per_sec().expect("a measurable stretch");
        assert!(
            (rate - 33.3).abs() < 1.0,
            "expected 30 deliveries over 0.9s = ~33/s, got {rate}"
        );
        assert_eq!(w.drain_interval_count(), 3);
        assert_eq!(w.drain_delivery_count(), 30);
    }

    /// **The false-red the third estimator shipped** (verifier probe 1). A
    /// healthy session — 40 writes of 10ms, one per minute — plus a single
    /// delivery that happened to leave a backlog. Scoring the whole window
    /// reported `0.0 writes/s` and raised a THROUGHPUT banner on a pipeline
    /// doing 10ms of work per write. Idle gaps must contribute nothing.
    #[test]
    fn an_idle_session_with_one_queued_delivery_is_not_a_slow_drain() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        for i in 0..40u64 {
            w.record(sample(10, 1, 0, t0 + Duration::from_secs(60 * i)));
        }
        w.record(sample(10, 2, 1, t0 + Duration::from_secs(60 * 40)));
        assert_eq!(
            w.drain_interval_count(),
            0,
            "no interval opened on a non-empty queue, so none is scoreable: {}",
            w.report()
        );
        assert!(
            matches!(w.throughput_verdict(), RungVerdict::Unjudged { .. }),
            "an idle session must be UNJUDGED, never a throughput failure: {}",
            w.report()
        );
    }

    /// **`drain_interval_count` must count intervals, not samples** (verifier
    /// probe 3). It names the quantity `MIN_DRAIN_INTERVALS` gates and the
    /// quantity the rung's guard message quotes; returning `samples.len()` let
    /// 39 unsaturated samples plus one saturated one satisfy a floor of 20.
    #[test]
    fn drain_interval_count_counts_saturated_intervals_only() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        for i in 0..39u64 {
            w.record(sample(10, 1, 0, t0 + Duration::from_millis(50 * i)));
        }
        w.record(sample(10, 2, 1, t0 + Duration::from_millis(50 * 39)));
        assert_eq!(w.samples().len(), 40);
        assert_eq!(w.drain_interval_count(), 0);
    }

    #[test]
    fn throughput_rung_fails_a_slow_drain() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        // 40 writes dispatched 150ms apart, each taking 50ms: ~5.9s of wall
        // time for 40 writes = ~6.8/s, under the 10/s floor.
        for i in 0..40 {
            w.record(sample(
                50,
                i + 1,
                40 - i - 1,
                t0 + Duration::from_millis(150 * i as u64),
            ));
        }
        let v = w.throughput_verdict();
        assert!(v.is_fail(), "expected a fail, got {v:?}");
    }

    /// A burst whose every write reports zero elapsed and one shared delivery
    /// instant carries no time at all. That is unmeasurable, and a rate of
    /// infinity must never be read as a pass.
    #[test]
    fn an_unresolvable_burst_is_unjudged_never_infinitely_fast() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        for i in 0..40 {
            w.record(sample(0, i + 1, 40 - i - 1, t0));
        }
        assert_eq!(w.drain_rate_per_sec(), None);
        assert!(matches!(
            w.throughput_verdict(),
            RungVerdict::Unjudged { .. }
        ));
    }

    #[test]
    fn the_window_retains_only_its_capacity() {
        let mut w = SloWindow::new(4, SERVICE_TIME_SLO_MS, THROUGHPUT_FLOOR_WRITES_PER_SEC);
        let t0 = Instant::now();
        for i in 0..10u64 {
            w.record(sample(i, 1, 0, t0 + Duration::from_millis(i)));
        }
        assert_eq!(w.len(), 4);
        assert_eq!(w.samples()[0].ms, 6);
    }
}
