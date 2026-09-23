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
//!    of wall time, summed over the INTERVALS in which the queue held more than
//!    one interaction (see [`SloWindow::drain_rate_per_sec`]). An idle gap
//!    contributes neither its time nor its deliveries, so a mostly-quiet
//!    session cannot be scored as slow draining. Floor:
//!    [`THROUGHPUT_FLOOR_WRITES_PER_SEC`].
//!
//! Each rung reports [`RungVerdict::Unjudged`] below its sample floor rather
//! than passing on thin evidence: a gate that goes green because it collected
//! four samples is the failure mode this module exists to prevent.

use std::time::Duration;
use std::time::Instant;

pub use crate::latency_e2e::ClockOrigin;

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
/// covariate ADMITTED (test profile, five waves of 30, batches identified by
/// `delivery_batch`, every dispatched write delivered): **9.5 · 13.5 · 26.4 ·
/// 30.8 · 31.5 · 45.6 · 52.9 · 65.1 · 69.3 · 71.4 · 75.1 · 75.8 · 84.9
/// writes/s** — a 9x spread, the low end on hosts with a load average of
/// 20-60. (Earlier observations were taken before samples carried their batch
/// identity, while the correlator evicted most of the burst, and are void.)
///
/// The rate is REPORT-ONLY: `latency_slo_rung_throughput_floor` prints it and
/// never fails on it. 10/s is a floor-of-last-resort, below every observation,
/// whose only job is to give the printed line a reference point.
///
/// **Promotion condition.** Gate this floor only once five admitted runs on a
/// quiet host agree within ~1.6x; then set the floor to the worst of those
/// divided by 1.6, per `docs/Testing/latency-ceilings.txt`. The scorer's
/// falsification is `throughput_rung_fails_a_slow_drain` and the false-red
/// pins beside it.
pub const THROUGHPUT_FLOOR_WRITES_PER_SEC: f64 = 10.0;

/// Service samples required before the p95 rung will return a verdict. The
/// ruling names n ≥ 30; a p95 over fewer samples is one sample's opinion.
pub const MIN_SERVICE_SAMPLES: usize = 30;

/// Saturated intervals the throughput rung needs before it will report a rate.
///
/// Low on purpose. The rate is REPORT-ONLY (see
/// [`THROUGHPUT_FLOOR_WRITES_PER_SEC`]), so this floor is not protecting a
/// verdict — it exists to catch a drive that stopped saturating at all, which
/// is the failure mode an integration rung owns. How many intervals one burst
/// opens depends on how the projector batches it — a pass can retire a whole
/// burst — so the gate drives its burst in waves that each close at least one.
/// The interval and delivery counts are printed beside every rate so thin
/// evidence is visible rather than silently averaged.
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
    /// The seam that opened this interaction's clock. Decides which window
    /// scores the sample; a [`SloWindow`] holds exactly one origin.
    pub origin: ClockOrigin,
    /// Wall time dispatch→projection-visible: service time plus queue wait.
    pub ms: u64,
    /// Interactions in flight when this one was dispatched, itself included.
    pub in_flight: usize,
    /// Interactions still pending after this one was delivered.
    pub backlog: usize,
    /// Whether any interaction of the OTHER origin overlapped this one's life.
    /// An event over the whole interval, not a reading at an instant — see
    /// `holon_api::latency_e2e`'s `Pending::contended`.
    pub contended: bool,
    /// When the delivery closed. The throughput rung's clock.
    pub delivered_at: Instant,
    /// The correlator delivery that closed this sample. Samples one applied
    /// batch retired share it; their `delivered_at` differ by the emit loop's
    /// own microseconds, so the instant cannot identify the batch.
    pub delivery_batch: u64,
}

impl E2eSample {
    /// The earliest instant this interaction can have been dispatched. `ms` is
    /// truncated to whole milliseconds, so the true dispatch lies up to one
    /// millisecond before `delivered_at - ms`; taking that bound makes every
    /// interval opened here as long as it could have been.
    pub fn earliest_dispatch(&self) -> Instant {
        self.delivered_at - Duration::from_millis(self.ms + 1)
    }

    /// Whether this sample is service time alone: the interaction was the only
    /// one in flight **anywhere in the pipeline** for its WHOLE life — nothing
    /// queued ahead of it at dispatch, nothing still pending when it was
    /// delivered, and no traffic of the OTHER origin either.
    ///
    /// Both time halves are load-bearing. `in_flight == 1` alone admits the
    /// head of a burst, which is dispatched into an empty queue and then
    /// overtaken by everything behind it: measured at 2150ms in a 40-write
    /// burst whose real per-write cost was ~62ms. Scoring that as service time
    /// is the queue-wait contamination this rung exists to exclude.
    ///
    /// The cross-origin half is load-bearing for the same reason (D119.a round
    /// 2). Populations are partitioned by origin so no percentile pools two
    /// spans — but the PIPELINE is shared, so a facade op in flight really does
    /// make a concurrent UI interaction wait. Judging that sample as
    /// uncontended would report queue wait as service time, which is a fake in
    /// the other direction. It is excluded instead — and, unlike before,
    /// COUNTED and named: see [`SloWindow::cross_origin_excluded`], which every
    /// report prints so a thin `n` always says why it is thin.
    ///
    /// `contended` is an event recorded over the interaction's whole life, so
    /// this really does mean what it says. An earlier version sampled the other
    /// origin's depth at two instants — dispatch and delivery — and a foreign
    /// clock that opened and closed BETWEEN them was invisible, which is the
    /// common shape rather than an exotic one (round 3 verification).
    pub fn is_service_time(&self) -> bool {
        self.in_flight == 1 && self.backlog == 0 && !self.contended
    }

    /// Whether this sample would have been service time but for traffic of the
    /// other origin. Exactly the population
    /// [`SloWindow::cross_origin_excluded`] counts.
    pub fn excluded_by_cross_origin(&self) -> bool {
        self.in_flight == 1 && self.backlog == 0 && self.contended
    }
}

/// One applied delivery batch, as the samples it closed describe it.
struct Batch {
    id: u64,
    /// When it landed: its first sample's delivery.
    at: Instant,
    deliveries: usize,
    /// Interactions of this origin still pending after it.
    backlog: usize,
    earliest_dispatch: Instant,
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

/// A rolling window of `stage="e2e"` samples **of one [`ClockOrigin`]**, scored
/// as the two D50.a rungs.
///
/// `capacity` bounds retention so a long-lived process (the runtime oracle)
/// judges recent behaviour rather than the whole session; a gate rung sizes it
/// past its own sample count and keeps everything.
///
/// # Why the origin is a window property, not a filter
///
/// A UI sample times a whole interaction; a facade sample starts above the
/// frontend dispatch seam and is therefore systematically shorter. Pooling them
/// makes the p95 depend on the agent/human traffic mix rather than on the
/// pipeline (Martin's ruling D119.a, 2026-09-12). Every percentile in this
/// module reads `self.samples`, and [`Self::record`] refuses a sample of any
/// other origin — so there is no pooled window to take a percentile of, and no
/// filter a consumer can forget to apply.
pub struct SloWindow {
    origin: ClockOrigin,
    samples: Vec<E2eSample>,
    capacity: usize,
    slo_ms: u64,
    floor_per_sec: f64,
}

impl Default for SloWindow {
    fn default() -> Self {
        Self::new(
            ClockOrigin::Ui,
            512,
            SERVICE_TIME_SLO_MS,
            THROUGHPUT_FLOOR_WRITES_PER_SEC,
        )
    }
}

impl SloWindow {
    pub fn new(origin: ClockOrigin, capacity: usize, slo_ms: u64, floor_per_sec: f64) -> Self {
        assert!(capacity > 0, "SloWindow capacity must be non-zero");
        Self {
            origin,
            samples: Vec::new(),
            capacity,
            slo_ms,
            floor_per_sec,
        }
    }

    /// The one origin this window scores.
    pub fn origin(&self) -> ClockOrigin {
        self.origin
    }

    /// Record a sample of THIS window's origin.
    ///
    /// A foreign origin is a programming error, not data to be filtered: it
    /// means a caller routed samples by hand and got it wrong, which is exactly
    /// the pooling D119.a forbids. Route through [`OriginWindows`] instead.
    pub fn record(&mut self, sample: E2eSample) {
        assert_eq!(
            sample.origin, self.origin,
            "a {:?} SloWindow was handed a {:?} sample — origins are scored in separate windows \
             (D119.a); route samples through OriginWindows",
            self.origin, sample.origin,
        );
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

    /// Samples this window declined to score because the OTHER origin had
    /// traffic in the shared pipeline at the time.
    ///
    /// Reported, never silent. An exclusion shrinks the population a percentile
    /// rests on, so a reader who sees only `n` cannot tell a quiet stretch from
    /// one crowded out by agent traffic — and that indistinguishability is what
    /// made the shared-registry version of this a defect rather than a policy.
    pub fn cross_origin_excluded(&self) -> usize {
        self.samples
            .iter()
            .filter(|s| s.excluded_by_cross_origin())
            .count()
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

    /// The window's delivery batches, ordered by when they landed. The batch —
    /// not the sample — is the unit this rung reasons about: every closure one
    /// applied batch made retired at the same moment.
    fn batches(&self) -> Vec<Batch> {
        let mut out: Vec<Batch> = Vec::new();
        for s in &self.samples {
            match out.iter_mut().find(|b| b.id == s.delivery_batch) {
                Some(b) => {
                    assert_eq!(
                        b.backlog, s.backlog,
                        "samples of delivery batch {} disagree on the backlog it left — the \
                         correlator stamps one backlog per origin per batch",
                        b.id,
                    );
                    b.at = b.at.min(s.delivered_at);
                    b.deliveries += 1;
                    b.earliest_dispatch = b.earliest_dispatch.min(s.earliest_dispatch());
                }
                None => out.push(Batch {
                    id: s.delivery_batch,
                    at: s.delivered_at,
                    deliveries: 1,
                    backlog: s.backlog,
                    earliest_dispatch: s.earliest_dispatch(),
                }),
            }
        }
        out.sort_by_key(|b| b.at);
        out
    }

    /// The drain measurement: deliveries retired across SATURATED intervals,
    /// how many such intervals there were, and their total wall time.
    ///
    /// Each batch closes one interval, and the interval counts ONLY while the
    /// queue held more than one interaction:
    ///  * after a batch that left work queued, the interval runs from that
    ///    batch to the next one;
    ///  * after an empty queue (or at the window's start), a batch opens a
    ///    stretch. It counts when it retired two or more interactions or left
    ///    some queued, and runs from the earliest dispatch still pending — the
    ///    queue was empty before that, so no idle time enters.
    ///
    /// An idle gap therefore contributes nothing — not its time and not its
    /// deliveries — so a session that was quiet for an hour and then queued
    /// once cannot be scored as an hour of slow draining. Counting the opening
    /// stretch is what lets a burst the pipeline retires in ONE pass be scored
    /// at all, instead of reading as a drive that never saturated.
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
    fn saturated_drain(&self) -> Option<(usize, usize, Duration)> {
        let batches = self.batches();
        let mut deliveries = 0usize;
        let mut intervals = 0usize;
        let mut elapsed = Duration::ZERO;
        for (i, b) in batches.iter().enumerate() {
            let prev = i.checked_sub(1).map(|p| &batches[p]);
            let start = match prev {
                Some(prev) if prev.backlog > 0 => prev.at,
                _ => {
                    if b.deliveries < 2 && b.backlog == 0 {
                        continue;
                    }
                    let opened = batches[i..]
                        .iter()
                        .map(|later| later.earliest_dispatch)
                        .min()
                        .expect("batches[i..] holds at least b");
                    prev.map_or(opened, |prev| opened.max(prev.at))
                }
            };
            deliveries += b.deliveries;
            intervals += 1;
            elapsed += b.at.saturating_duration_since(start);
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
        let excluded = match self.cross_origin_excluded() {
            0 => String::new(),
            n => format!(
                " ({n} excluded: {} traffic in the shared pipeline)",
                self.origin.other().as_str()
            ),
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
        format!(
            "[origin={}] {service}{spread}{excluded} | {throughput}",
            self.origin.as_str()
        )
    }
}

/// The scoring front door: one [`SloWindow`] per [`ClockOrigin`], with the
/// routing done here instead of at every call site.
///
/// This is what makes pooling unreachable rather than merely discouraged. A
/// consumer records samples into `OriginWindows`, which files each one by its
/// own `origin`; to read a statistic it must name an origin and receives that
/// origin's window. No method on either type returns a number computed over
/// more than one origin.
pub struct OriginWindows {
    ui: SloWindow,
    facade: SloWindow,
}

impl OriginWindows {
    pub fn new(capacity: usize, slo_ms: u64, floor_per_sec: f64) -> Self {
        Self {
            ui: SloWindow::new(ClockOrigin::Ui, capacity, slo_ms, floor_per_sec),
            facade: SloWindow::new(ClockOrigin::Facade, capacity, slo_ms, floor_per_sec),
        }
    }

    /// File one sample into the window its own origin names.
    pub fn record(&mut self, sample: E2eSample) {
        match sample.origin {
            ClockOrigin::Ui => self.ui.record(sample),
            ClockOrigin::Facade => self.facade.record(sample),
        }
    }

    pub fn window(&self, origin: ClockOrigin) -> &SloWindow {
        match origin {
            ClockOrigin::Ui => &self.ui,
            ClockOrigin::Facade => &self.facade,
        }
    }

    pub fn ui(&self) -> &SloWindow {
        &self.ui
    }

    pub fn facade(&self) -> &SloWindow {
        &self.facade
    }

    pub fn clear(&mut self) {
        self.ui.clear();
        self.facade.clear();
    }

    /// Both origins, each on its OWN line. Never one merged line: a reader who
    /// sees a single number has been told the pipeline has a single p95, and it
    /// does not.
    pub fn report_lines(&self) -> Vec<String> {
        [ClockOrigin::Ui, ClockOrigin::Facade]
            .into_iter()
            .map(|o| self.window(o).report())
            .collect()
    }
}

#[cfg(test)]
mod tests {
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
            origin: ClockOrigin::Ui,
            ms,
            in_flight,
            backlog,
            contended: false,
            delivered_at: at,
            delivery_batch: batch,
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

    /// Batched deliveries retire together. Averaging the per-delivery gaps
    /// counted the zeros between them as infinitely fast work and reported
    /// 99,156 writes/s for a pipeline sleeping 300ms per batch; wall time
    /// across the stretch reports what actually happened.
    #[test]
    fn batched_deliveries_do_not_inflate_the_drain_rate() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        // 40 writes dispatched at t0, retired in four batches of 10, 300ms
        // apart. The first batch opens the stretch at the earliest possible
        // dispatch (t0 - 1ms, `ms` being truncated), so: 40 deliveries over
        // 1.201s.
        for batch in 0..4u64 {
            let at = Duration::from_millis(300 * (batch + 1));
            for i in 0..10u64 {
                w.record(batched(
                    at.as_millis() as u64,
                    (batch * 10 + i + 1) as usize,
                    (40 - 10 * (batch + 1)) as usize,
                    t0 + at + Duration::from_micros(i),
                    batch,
                ));
            }
        }
        let rate = w.drain_rate_per_sec().expect("a measurable stretch");
        assert!(
            (rate - 33.3).abs() < 1.0,
            "expected 40 deliveries over 1.201s = ~33/s, got {rate}"
        );
        assert_eq!(w.drain_interval_count(), 4);
        assert_eq!(w.drain_delivery_count(), 40);
    }

    /// **A burst the pipeline retires in ONE pass is a saturated stretch.**
    /// The projector drains its whole queue per pass, so a fast pipeline can
    /// close 64 queued interactions in one batch. Every sample then reports
    /// `backlog == 0` — and scoring only batch-to-batch gaps read that as "the
    /// drive never saturated", the false structural red the land gate hit.
    /// The samples also carry distinct `delivered_at` (the emit loop runs
    /// between them), so the batch must be identified by `delivery_batch`.
    #[test]
    fn a_burst_retired_in_one_pass_is_one_saturated_interval() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        let landed = t0 + Duration::from_millis(100);
        for i in 0..64u64 {
            let dispatched = t0 + Duration::from_micros(20 * i);
            let at = landed + Duration::from_micros(i);
            w.record(batched(
                at.duration_since(dispatched).as_millis() as u64,
                i as usize + 1,
                0,
                at,
                7,
            ));
        }
        assert_eq!(w.drain_interval_count(), 1);
        assert_eq!(w.drain_delivery_count(), 64);
        let rate = w.drain_rate_per_sec().expect("a measurable stretch");
        assert!(
            (600.0..=640.0).contains(&rate),
            "64 deliveries over ~101ms from the first dispatch, got {rate}"
        );
    }

    /// Three such passes, each dispatched after the previous one landed, are
    /// three saturated intervals: enough for a verdict, and a slow one fails.
    #[test]
    fn three_one_pass_waves_are_judged_and_a_slow_one_fails() {
        let judge = |pass_ms: u64| {
            let mut w = SloWindow::default();
            let t0 = Instant::now();
            for wave in 0..3u64 {
                let start = t0 + Duration::from_millis(wave * (pass_ms + 5));
                let at = start + Duration::from_millis(pass_ms);
                for i in 0..50u64 {
                    w.record(batched(pass_ms, i as usize + 1, 0, at, wave));
                }
            }
            w.throughput_verdict()
        };
        assert!(matches!(judge(100), RungVerdict::Pass { n: 3, .. }));
        // 50 writes per 6s pass is ~8.3/s, under the 10/s floor.
        assert!(judge(6000).is_fail(), "got {:?}", judge(6000));
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
            1,
            "only the queued delivery's own stretch is saturated: {}",
            w.report()
        );
        let rate = w.drain_rate_per_sec().expect("one measurable stretch");
        assert!(
            rate > THROUGHPUT_FLOOR_WRITES_PER_SEC,
            "the idle minutes entered the rate ({rate}/s): {}",
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
        assert_eq!(w.drain_interval_count(), 1);
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

    /// A burst whose every write reports zero elapsed in one shared batch
    /// resolved below the clock's millisecond. It is scored at its lower bound
    /// — one full millisecond — and a rate of infinity is never read.
    #[test]
    fn an_unresolvable_burst_is_scored_at_its_lower_bound_never_infinitely_fast() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        for i in 0..40 {
            w.record(batched(0, i + 1, 0, t0, 1));
        }
        let rate = w.drain_rate_per_sec().expect("a bounded stretch");
        assert!((rate - 40_000.0).abs() < 1.0, "got {rate}");
        assert!(matches!(
            w.throughput_verdict(),
            RungVerdict::Unjudged { n: 1, .. }
        ));
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
}
