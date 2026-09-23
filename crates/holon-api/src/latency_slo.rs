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
//! 2. **Throughput** — [`SloWindow::drain_rate_per_sec`]. Writes retired per
//!    second while the queue STAYED full: over busy periods of at least
//!    [`MIN_SATURATED_PASSES`] consecutive passes that each started with work
//!    already queued. A slow pass without sustained load is a service event,
//!    and an idle gap ends a busy period. Floor:
//!    [`THROUGHPUT_FLOOR_WRITES_PER_SEC`].
//!
//! Each rung reports [`RungVerdict::Unjudged`] below its sample floor rather
//! than passing on thin evidence: a gate that goes green because it collected
//! four samples is the failure mode this module exists to prevent.

use std::collections::BTreeMap;
use std::collections::HashMap;
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
/// Observed by `latency_slo_rung_throughput_floor` (test profile, the
/// sustained drive, the busy-period estimator): **56.5 · 58.7 · 61.7 · 68.6 ·
/// 74.0 writes/s**.
/// Observations under the earlier wave drive and interval estimators are void:
/// those estimators could inflate a rate by charging a pass to a 1ms span.
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

/// Saturated passes a busy period needs before the throughput rung judges it.
///
/// One slow pass, or a burst of two writes, is a SERVICE event: the queue did
/// not stay full. Three consecutive passes that each started with work already
/// queued mean arrivals kept up with retirement, which is the sustained load a
/// drain rate describes.
pub const MIN_SATURATED_PASSES: usize = 3;

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
    /// The `LiveData` source whose delivery closed this sample. A subscriber
    /// applies its batches one after another, and different sources run
    /// concurrently, so passes are sequenced per source.
    pub source: String,
}

impl E2eSample {
    /// The latest instant this interaction can have been dispatched. `ms` is
    /// truncated to whole milliseconds, so the true dispatch lies up to one
    /// millisecond before this.
    pub fn latest_dispatch(&self) -> Instant {
        self.delivered_at - Duration::from_millis(self.ms)
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

/// One applied delivery batch of one source — one PASS of its subscriber — as
/// the samples it closed describe it.
struct Pass {
    /// When it landed: its first sample's delivery.
    at: Instant,
    deliveries: usize,
    /// The earliest `latest_dispatch` of the writes it retired.
    first_dispatch: Instant,
}

/// What the judged busy periods of one source measured.
struct SourceDrain<'a> {
    source: &'a str,
    deliveries: usize,
    passes: usize,
    elapsed: Duration,
}

impl SourceDrain<'_> {
    fn rate_per_sec(&self) -> f64 {
        self.deliveries as f64 / self.elapsed.as_secs_f64()
    }
}

/// The throughput rung's reading of a window.
struct Drain<'a> {
    judged: Vec<SourceDrain<'a>>,
    /// The most saturated passes any busy period reached, judged or not.
    longest_busy_period: usize,
}

impl Drain<'_> {
    fn worst(&self) -> Option<&SourceDrain<'_>> {
        self.judged
            .iter()
            .min_by(|a, b| a.rate_per_sec().total_cmp(&b.rate_per_sec()))
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

    /// Each source's passes, ordered by when they landed. The batch — not the
    /// sample — is the unit this rung reasons about: every closure one applied
    /// batch made retired at the same moment.
    fn passes_by_source(&self) -> BTreeMap<&str, Vec<Pass>> {
        let mut by_batch: HashMap<u64, (&str, Pass)> = HashMap::new();
        for s in &self.samples {
            let (source, pass) = by_batch.entry(s.delivery_batch).or_insert((
                s.source.as_str(),
                Pass {
                    at: s.delivered_at,
                    deliveries: 0,
                    first_dispatch: s.latest_dispatch(),
                },
            ));
            assert_eq!(
                *source, s.source,
                "delivery batch {} names two sources — the correlator stamps one source per \
                 `rows_delivered` call",
                s.delivery_batch,
            );
            pass.at = pass.at.min(s.delivered_at);
            pass.deliveries += 1;
            pass.first_dispatch = pass.first_dispatch.min(s.latest_dispatch());
        }
        let mut out: BTreeMap<&str, Vec<Pass>> = BTreeMap::new();
        for (source, pass) in by_batch.into_values() {
            out.entry(source).or_default().push(pass);
        }
        for passes in out.values_mut() {
            passes.sort_by_key(|p| p.at);
        }
        out
    }

    /// The drain measurement, per source.
    ///
    /// Pass `j` is SATURATED when a write it or a later pass retired had
    /// certainly been dispatched by the time pass `j-1` landed: the queue was
    /// not empty then, so the subscriber went straight on to pass `j`, and
    /// `at_j - at_(j-1)` is exactly pass `j`'s duration. A busy period is a
    /// run of consecutive saturated passes. It is judged only with at least
    /// [`MIN_SATURATED_PASSES`]; its rate is the deliveries of its saturated
    /// passes over the time from the landing before them to the last landing.
    ///
    /// * No delivery is charged to less than its own pass, so a rate cannot
    ///   inflate: the previous estimator charged whole passes to 1ms spans and
    ///   passed a 3.3/s pipeline at 500/s.
    /// * A slow pass without sustained load stays a SERVICE event: one slow
    ///   write, or a slow write with one follower, cannot make three saturated
    ///   passes.
    /// * Only delivered samples count, so a clock that never closes cannot hold
    ///   a busy period open, and an idle gap ends it.
    /// * A dispatch is resolved to `ms`'s millisecond and taken at its latest,
    ///   so a doubtful arrival ends a busy period rather than extending one.
    fn drain(&self) -> Drain<'_> {
        let mut judged = Vec::new();
        let mut longest_busy_period = 0;
        for (source, passes) in self.passes_by_source() {
            // queued_from[j]: the earliest dispatch among writes pass j or a
            // later pass retired.
            let mut queued_from: Vec<Instant> = passes.iter().map(|p| p.first_dispatch).collect();
            for j in (0..queued_from.len().saturating_sub(1)).rev() {
                queued_from[j] = queued_from[j].min(queued_from[j + 1]);
            }
            let mut drain = SourceDrain {
                source,
                deliveries: 0,
                passes: 0,
                elapsed: Duration::ZERO,
            };
            let mut judge = |a: usize, b: usize| {
                longest_busy_period = longest_busy_period.max(b - a);
                if b - a >= MIN_SATURATED_PASSES {
                    drain.deliveries += passes[a + 1..=b]
                        .iter()
                        .map(|p| p.deliveries)
                        .sum::<usize>();
                    drain.passes += b - a;
                    drain.elapsed += passes[b].at - passes[a].at;
                }
            };
            let mut opened: Option<usize> = None;
            for j in 1..passes.len() {
                let saturated = queued_from[j] <= passes[j - 1].at;
                match (saturated, opened) {
                    (true, None) => opened = Some(j - 1),
                    (false, Some(a)) => {
                        judge(a, j - 1);
                        opened = None;
                    }
                    _ => {}
                }
            }
            if let Some(a) = opened {
                judge(a, passes.len() - 1);
            }
            if drain.passes > 0 {
                assert!(
                    drain.elapsed > Duration::ZERO,
                    "source {source}: {} saturated passes landed at one instant",
                    drain.passes,
                );
                judged.push(drain);
            }
        }
        Drain {
            judged,
            longest_busy_period,
        }
    }

    /// RUNG 2 — the slowest judged source's drain rate, in writes per second.
    pub fn drain_rate_per_sec(&self) -> Option<f64> {
        self.drain().worst().map(SourceDrain::rate_per_sec)
    }

    /// Saturated passes across every judged busy period — the verdict's `n`.
    pub fn drain_pass_count(&self) -> usize {
        self.drain().judged.iter().map(|d| d.passes).sum()
    }

    /// Deliveries retired by those passes.
    pub fn drain_delivery_count(&self) -> usize {
        self.drain().judged.iter().map(|d| d.deliveries).sum()
    }

    /// Each source with a judged busy period is judged on its own; the rung
    /// fails when the slowest of them is below the floor.
    pub fn throughput_verdict(&self) -> RungVerdict {
        let drain = self.drain();
        let n = drain.judged.iter().map(|d| d.passes).sum();
        let Some(worst) = drain.worst() else {
            return RungVerdict::Unjudged {
                n: drain.longest_busy_period,
                needed: MIN_SATURATED_PASSES,
            };
        };
        let measured = worst.rate_per_sec();
        if measured >= self.floor_per_sec {
            RungVerdict::Pass { measured, n }
        } else {
            RungVerdict::Fail { measured, n }
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
        let worst_source = self
            .drain()
            .worst()
            .map_or(String::new(), |d| format!(", slowest source {}", d.source));
        let throughput = match self.throughput_verdict() {
            RungVerdict::Pass { measured, n } => format!(
                "drain {measured:.1}/s >= {:.1}/s over {n} saturated passes ({} deliveries{worst_source})",
                self.floor_per_sec,
                self.drain_delivery_count(),
            ),
            RungVerdict::Fail { measured, n } => format!(
                "drain {measured:.1}/s BELOW {:.1}/s over {n} saturated passes ({} deliveries{worst_source})",
                self.floor_per_sec,
                self.drain_delivery_count(),
            ),
            RungVerdict::Unjudged { n, needed } => {
                format!("drain rate unjudged (longest busy period {n} saturated passes < {needed})")
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
            source: "block".to_string(),
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

    fn assert_unjudged(w: &SloWindow, longest: usize) {
        assert_eq!(
            w.throughput_verdict(),
            RungVerdict::Unjudged {
                n: longest,
                needed: MIN_SATURATED_PASSES
            },
            "{}",
            w.report()
        );
        assert_eq!(w.drain_rate_per_sec(), None);
    }

    fn assert_fails_at(w: &SloWindow, rate: f64, passes: usize) {
        assert_eq!(
            w.throughput_verdict(),
            RungVerdict::Fail {
                measured: w.drain_rate_per_sec().expect("a judged busy period"),
                n: passes
            },
            "{}",
            w.report()
        );
        let measured = w.drain_rate_per_sec().expect("a judged busy period");
        assert!(
            (measured - rate).abs() < 0.01,
            "expected {rate}/s, got {measured}/s"
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
        // 40 writes dispatched at t0, retired in four passes of 10, 300ms
        // apart: passes 2-4 are saturated, 30 deliveries over 900ms.
        for pass in 0..4u64 {
            for i in 0..10u64 {
                w.record(write(t0, 0, 300_000 * (pass + 1) + i, pass));
            }
        }
        assert_eq!(w.drain_pass_count(), 3);
        assert_eq!(w.drain_delivery_count(), 30);
        let rate = w.drain_rate_per_sec().expect("a judged busy period");
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

    /// Probe Q1. A serial 300ms pipeline fed so that it never idles: write
    /// `k` arrives 1ms before pass `k-1` lands. 3.3 writes/s, under the floor.
    #[test]
    fn a_serial_pipeline_that_never_idles_is_a_slow_drain() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        serial(&mut w, t0, 0, 40, 300, 1, 0);
        assert_fails_at(&w, 1000.0 / 300.0, 39);
    }

    /// Probe Q2. Arrivals every 299ms against a 300ms serial pipeline: the
    /// queue grows without limit, and the rate stays the pipeline's own.
    #[test]
    fn an_overloaded_serial_pipeline_is_a_slow_drain() {
        for n in [40u64, 100, 150] {
            let mut w = SloWindow::new(
                ClockOrigin::Ui,
                512,
                SERVICE_TIME_SLO_MS,
                THROUGHPUT_FLOOR_WRITES_PER_SEC,
            );
            let t0 = Instant::now();
            for k in 0..n {
                w.record(write(t0, 299_000 * k, 300_000 * (k + 1), k));
            }
            assert_fails_at(&w, 1000.0 / 300.0, n as usize - 1);
        }
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

    /// A write dispatched in the very instant the previous pass lands is
    /// queued when the next pass starts.
    #[test]
    fn a_write_dispatched_as_a_pass_lands_keeps_the_busy_period_open() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        serial(&mut w, t0, 0, 5, 300, 0, 0);
        assert_fails_at(&w, 1000.0 / 300.0, 4);
    }

    /// [`MIN_SATURATED_PASSES`] is the line between a burst and sustained
    /// load: two saturated passes are not judged, three are.
    #[test]
    fn two_saturated_passes_are_not_a_busy_period_and_three_are() {
        let t0 = Instant::now();
        let mut w = SloWindow::default();
        for k in 0..3u64 {
            w.record(write(t0, 0, 300_000 * (k + 1), k));
        }
        assert_unjudged(&w, 2);
        w.record(write(t0, 0, 1_200_000, 3));
        assert_fails_at(&w, 1000.0 / 300.0, 3);
    }

    /// An idle gap ends a busy period. Two short ones either side of it are
    /// not one long one, and two judged ones do not charge the gap.
    #[test]
    fn an_idle_gap_splits_busy_periods() {
        let t0 = Instant::now();
        let mut w = SloWindow::default();
        serial(&mut w, t0, 0, 3, 300, 1, 0);
        serial(&mut w, t0, 10_000, 3, 300, 1, 10);
        assert_unjudged(&w, 2);

        let mut w = SloWindow::default();
        serial(&mut w, t0, 0, 4, 300, 1, 0);
        serial(&mut w, t0, 10_000, 4, 300, 1, 10);
        assert_fails_at(&w, 1000.0 / 300.0, 6);
    }

    /// Passes are sequenced per source: a subscriber applies its own batches in
    /// order, and two sources run side by side. The slowest judged source
    /// decides the verdict.
    #[test]
    fn sources_are_sequenced_and_judged_separately() {
        let t0 = Instant::now();
        let mut w = SloWindow::default();
        serial(&mut w, t0, 0, 5, 300, 1, 0);
        for k in 0..5u64 {
            w.record(E2eSample {
                source: "focus_roots".to_string(),
                ..write(t0, 150_000, 160_000 + 10_000 * k, 100 + k)
            });
        }
        assert_fails_at(&w, 1000.0 / 300.0, 8);
        assert!(
            w.report().contains("slowest source block"),
            "{}",
            w.report()
        );
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

    /// **`drain_pass_count` counts saturated passes, not samples.** Returning
    /// `samples.len()` once let 39 unsaturated samples plus one saturated one
    /// satisfy a floor of 20.
    #[test]
    fn drain_pass_count_counts_saturated_passes_only() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        for i in 0..39u64 {
            w.record(sample(10, 1, 0, t0 + Duration::from_millis(50 * i)));
        }
        w.record(sample(10, 2, 1, t0 + Duration::from_millis(50 * 39)));
        assert_eq!(w.samples().len(), 40);
        assert_eq!(w.drain_pass_count(), 0);
    }

    /// 40 writes dispatched together, retired one per 150ms: 39 saturated
    /// passes in 5.85s, ~6.7/s, under the 10/s floor. The time before the
    /// first landing is not charged: it holds the first write's service.
    #[test]
    fn throughput_rung_fails_a_slow_drain() {
        let mut w = SloWindow::default();
        let t0 = Instant::now();
        for k in 0..40u64 {
            w.record(write(t0, 0, 150_000 * (k + 1), k));
        }
        assert_fails_at(&w, 39.0 / 5.85, 39);
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
}
