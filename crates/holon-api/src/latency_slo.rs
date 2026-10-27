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
//! 2. **Throughput** — the pipeline's CAPACITY against
//!    [`THROUGHPUT_FLOOR_WRITES_PER_SEC`]. The verdict is the controlled drain
//!    test in [`crate::latency_drain`] (Martin's ruling D207.a).
//!    [`SloWindow::drain_estimate`] infers the drain rate from whatever traffic
//!    a window holds; it is a DISCLOSURE only and never a verdict.
//!
//! The service rung reports [`RungVerdict::Unjudged`] below its sample floor
//! rather than passing on thin evidence: a gate that goes green because it
//! collected four samples is the failure mode this module exists to prevent.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::time::Duration;
use std::time::Instant;

pub use crate::latency_e2e::ClockOrigin;
pub use crate::latency_e2e::QuietBatches;
pub use crate::latency_e2e::Superseded;

/// Service-time budget. The project SLO, unchanged — what changed is which
/// samples are eligible to be scored against it.
pub const SERVICE_TIME_SLO_MS: u64 = 200;

/// Throughput floor: writes per second the pipeline must retire. The SLO's
/// number; the drain test in [`crate::latency_drain`] fails every sustained
/// rate below `N/L = 600 / 60.4 s ≈ 9.934/s` and can pass one in
/// `[9.934, 10)/s`.
///
/// 10/s = 100ms per write. The passive estimator observed 56.5 · 58.7 · 61.7 ·
/// 68.6 · 74.0 writes/s on the gate's earlier sustained drive (test profile).
pub const THROUGHPUT_FLOOR_WRITES_PER_SEC: f64 = 10.0;

/// Service samples required before the p95 rung will return a verdict. The
/// ruling names n ≥ 30; a p95 over fewer samples is one sample's opinion.
pub const MIN_SERVICE_SAMPLES: usize = 30;

/// Saturated passes a busy period needs before the drain estimate judges it.
///
/// One slow pass, or a burst of two writes, is a SERVICE event. Three
/// consecutive passes with work queued behind each is sustained load.
const MIN_SATURATED_PASSES: usize = 3;

/// Slow the CDC delivery actor on purpose, so the service rung and the drain
/// test can be shown to have teeth.
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
    /// `ms` more, which caps its capacity at `1000 / ms` writes/s.
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
    /// The entity the interaction addressed.
    pub target: String,
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
    /// When the delivery closed.
    pub delivered_at: Instant,
    /// The correlator delivery that closed this sample. Samples one applied
    /// batch retired share it; their `delivered_at` differ by the emit loop's
    /// own microseconds, so the instant cannot identify the batch.
    pub delivery_batch: u64,
    /// The `LiveData` source whose delivery closed this sample.
    pub source: String,
    /// The feed that applied the delivery: see `latency_e2e::Feed`. A feed
    /// applies its batches one after another, and feeds run concurrently, so
    /// passes are sequenced per feed.
    pub feed: u64,
    /// The older interactions on the same target this closure retired with
    /// this one. Typing on one block closes one clock per pass however many
    /// keystrokes the pass made visible.
    pub superseded: Superseded,
    /// The feed's passes since its previous batch that closed a clock of this
    /// origin, none of which closed one.
    pub quiet: QuietBatches,
}

impl E2eSample {
    /// The latest instant this interaction can have been dispatched. `ms` is
    /// truncated to whole milliseconds, so the true dispatch lies up to one
    /// millisecond before this.
    pub fn latest_dispatch(&self) -> Instant {
        self.delivered_at - Duration::from_millis(self.ms)
    }

    /// Interactions this closure retired: this one plus the ones it superseded.
    pub fn retired(&self) -> NonZeroUsize {
        NonZeroUsize::MIN.saturating_add(self.superseded.ages_ms().len())
    }

    /// The latest dispatch of every interaction this closure retired, each
    /// at its OWN dispatch, this one included.
    fn retired_dispatches(&self) -> impl Iterator<Item = Instant> + '_ {
        std::iter::once(self.ms)
            .chain(self.superseded.ages_ms().iter().copied())
            .map(|age| self.delivered_at - Duration::from_millis(age))
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

/// `ms` is truncated to whole milliseconds, so a true dispatch lies within this
/// much before its `latest_dispatch`.
const DISPATCH_RESOLUTION: Duration = Duration::from_millis(1);

/// One applied delivery batch of one feed — one PASS — as the samples it
/// closed describe it. A quiet pass closed none.
struct Pass {
    /// When it landed: its first sample's delivery.
    at: Instant,
    /// The earliest `latest_dispatch` of the clocks it closed.
    first_dispatch: Option<Instant>,
    /// The latest dispatch of every interaction it retired, superseded ones
    /// included, each at its own.
    retired: Vec<Instant>,
}

impl Pass {
    fn quiet(at: Instant) -> Self {
        Self {
            at,
            first_dispatch: None,
            retired: Vec::new(),
        }
    }

    fn deliveries(&self) -> usize {
        self.retired.len()
    }

    fn earliest_retired(&self) -> Option<Instant> {
        self.retired.iter().min().copied()
    }
}

/// Maximal runs of consecutive passes `a+1..=b`, within `from+1..=to`, for
/// which `holds(j)`, as `(a, b)`.
fn runs(from: usize, to: usize, holds: impl Fn(usize) -> bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut opened = None;
    for j in from + 1..=to {
        match (holds(j), opened) {
            (true, None) => opened = Some(j - 1),
            (false, Some(a)) => {
                out.push((a, j - 1));
                opened = None;
            }
            _ => {}
        }
    }
    if let Some(a) = opened {
        out.push((a, to));
    }
    out
}

/// What the judged busy periods of one feed measured.
struct FeedDrain<'a> {
    source: &'a str,
    deliveries: usize,
    passes: usize,
    elapsed: Duration,
}

impl FeedDrain<'_> {
    fn rate_per_sec(&self) -> f64 {
        self.deliveries as f64 / self.elapsed.as_secs_f64()
    }
}

/// The drain estimator's reading of a window.
struct Drain<'a> {
    judged: Vec<FeedDrain<'a>>,
    /// The most saturated passes any busy period reached, judged or not.
    longest_busy_period: usize,
}

impl Drain<'_> {
    fn worst(&self) -> Option<&FeedDrain<'_>> {
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

/// The passive drain estimator's reading of a window — a DISCLOSURE, never a
/// verdict (Martin's ruling D207.a). It infers capacity from whatever traffic
/// the window happened to hold, and adversarial traces have misjudged it both
/// ways. It has no failing variant on purpose: the verdict is the controlled
/// drain test in [`crate::latency_drain`].
#[derive(Clone, Debug, PartialEq)]
pub enum DrainEstimate {
    AtOrAbove {
        rate: f64,
        passes: usize,
        deliveries: usize,
    },
    Below {
        rate: f64,
        passes: usize,
        deliveries: usize,
    },
    /// No busy period was judged. `longest_busy_period` is the longest run of
    /// saturated passes found.
    Unjudged {
        longest_busy_period: usize,
        needed: usize,
    },
}

impl DrainEstimate {
    pub fn is_below(&self) -> bool {
        matches!(self, DrainEstimate::Below { .. })
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
        assert!(
            sample
                .superseded
                .ages_ms()
                .iter()
                .all(|&age| age >= sample.ms),
            "a superseded interaction is older than the one that closed it: ms {} but \
             superseded ages {}",
            sample.ms,
            sample.superseded,
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

    /// Each feed's source and passes, ordered by when they landed, quiet ones
    /// included. The batch — not the sample — is the unit this estimate reasons
    /// about: every closure one applied batch made retired at the same moment.
    fn passes_by_feed(&self) -> BTreeMap<u64, (&str, Vec<Pass>)> {
        let mut by_batch: HashMap<u64, (u64, &str, &QuietBatches, Pass)> = HashMap::new();
        for s in &self.samples {
            let (feed, source, quiet, pass) = by_batch.entry(s.delivery_batch).or_insert((
                s.feed,
                s.source.as_str(),
                &s.quiet,
                Pass {
                    at: s.delivered_at,
                    first_dispatch: Some(s.latest_dispatch()),
                    retired: Vec::new(),
                },
            ));
            assert!(
                (*feed, *source, *quiet) == (s.feed, s.source.as_str(), &s.quiet),
                "delivery batch {} names two feeds, sources or quiet logs ({feed} {source} [{quiet}] \
                 vs {} {} [{}]) — the correlator stamps one of each per `rows_delivered` call",
                s.delivery_batch,
                s.feed,
                s.source,
                s.quiet,
            );
            pass.at = pass.at.min(s.delivered_at);
            pass.first_dispatch = pass.first_dispatch.map(|f| f.min(s.latest_dispatch()));
            pass.retired.extend(s.retired_dispatches());
        }
        let mut out: BTreeMap<u64, (&str, Vec<Pass>)> = BTreeMap::new();
        for (feed, source, quiet, pass) in by_batch.into_values() {
            let (named, passes) = out.entry(feed).or_insert((source, Vec::new()));
            assert_eq!(*named, source, "feed {feed} names two sources");
            passes.extend(
                quiet
                    .ages_us()
                    .iter()
                    .map(|&age| Pass::quiet(pass.at - Duration::from_micros(age))),
            );
            passes.push(pass);
        }
        for (_, passes) in out.values_mut() {
            passes.sort_by_key(|p| p.at);
        }
        out
    }

    /// The drain measurement, per feed.
    ///
    /// Pass `j` is SATURATED when a write that it or a later pass retired had
    /// certainly been dispatched by the time pass `j-1` landed: the feed went
    /// straight on to pass `j`, so `at_j - at_(j-1)` is its whole duration. A
    /// busy period is a run of saturated passes `a+1..=b`. Its rate is
    /// `min(arrivals, capacity)`, so it shows the CAPACITY only when:
    ///
    /// * its arrival rate reaches the floor. The arrival rate counts the
    ///   interactions passes `a+1..=b` retired that were certainly dispatched
    ///   in `(f_a, at_(b-1)]`, each at its own dispatch, over `at_(b-1) - f_a`.
    ///   `f_a` is the earliest dispatch retired by the last pass up to `a` that
    ///   retired one: pass `a` started no earlier, so when every pass takes all
    ///   that is queued the window spans at least the busy period's `at_b -
    ///   at_a`, and the rate cannot fall below the arrival rate. A rate below
    ///   the floor with arrivals at or above it proves the capacity is below
    ///   the floor; or
    /// * its passes leave work behind: pass `j` retired a write, and a write
    ///   retired two or more passes after `j` was dispatched by `at_(j-1)`. A
    ///   write commits within one pass time, so passes `j` and `j+1` together
    ///   could not take all that was queued. A quiet pass can be a batch of
    ///   untracked rows shorter than a commit, so it shows no load. When the
    ///   arrivals are below the floor, only runs of these passes are judged.
    ///
    /// A judged run needs at least [`MIN_SATURATED_PASSES`] passes. Its rate
    /// is the deliveries of its passes over `at_b - at_a`.
    ///
    /// * No delivery is charged to less than its own pass, so a rate cannot
    ///   inflate: a previous estimator charged whole passes to 1ms spans and
    ///   passed a 3.3/s pipeline at 500/s.
    /// * A user typing slower than the floor into longer passes is neither:
    ///   each pass shows everything queued, so it measures the typing.
    /// * Only delivered samples count, so a clock that never closes cannot hold
    ///   a busy period open, and an idle gap ends it.
    /// * A dispatch is resolved to `ms`'s millisecond and taken at its latest,
    ///   so a doubtful arrival ends a busy period rather than extending one.
    fn drain(&self) -> Drain<'_> {
        let mut judged = Vec::new();
        let mut longest_busy_period = 0;
        for (source, passes) in self.passes_by_feed().into_values() {
            // queued_from[j]: the earliest dispatch among clocks pass j or a
            // later pass closed.
            let mut queued_from: Vec<Option<Instant>> =
                passes.iter().map(|p| p.first_dispatch).collect();
            for j in (0..queued_from.len().saturating_sub(1)).rev() {
                queued_from[j] = match (queued_from[j], queued_from[j + 1]) {
                    (Some(own), Some(later)) => Some(own.min(later)),
                    (own, later) => own.or(later),
                };
            }
            let queued_by = |j: usize, landed: Instant| {
                queued_from
                    .get(j)
                    .copied()
                    .flatten()
                    .is_some_and(|q| q <= landed)
            };
            let saturated = |j: usize| queued_by(j, passes[j - 1].at);
            let leaves_work =
                |j: usize| passes[j].deliveries() > 0 && queued_by(j + 2, passes[j - 1].at);
            let arrivals_reach_floor = |a: usize, b: usize| {
                let Some(first) = passes[..=a].iter().rev().find_map(Pass::earliest_retired) else {
                    return false;
                };
                let to = passes[b - 1].at;
                let arrived = passes[a + 1..=b]
                    .iter()
                    .flat_map(|p| &p.retired)
                    .filter(|&&at| at - DISPATCH_RESOLUTION >= first && at <= to)
                    .count();
                b - 1 > a && arrived as f64 >= self.floor_per_sec * (to - first).as_secs_f64()
            };
            let mut drain = FeedDrain {
                source,
                deliveries: 0,
                passes: 0,
                elapsed: Duration::ZERO,
            };
            let mut judge = |a: usize, b: usize| {
                if b - a >= MIN_SATURATED_PASSES {
                    drain.deliveries += passes[a + 1..=b]
                        .iter()
                        .map(Pass::deliveries)
                        .sum::<usize>();
                    drain.passes += b - a;
                    drain.elapsed += passes[b].at - passes[a].at;
                }
            };
            for (a, b) in runs(0, passes.len().saturating_sub(1), saturated) {
                longest_busy_period = longest_busy_period.max(b - a);
                if arrivals_reach_floor(a, b) {
                    judge(a, b);
                } else {
                    for (a, b) in runs(a, b, leaves_work) {
                        judge(a, b);
                    }
                }
            }
            if drain.passes > 0 {
                assert!(
                    drain.elapsed > Duration::ZERO,
                    "a {source} feed: {} saturated passes landed at one instant",
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

    /// The slowest judged feed's drain rate. A disclosure: see
    /// [`DrainEstimate`].
    pub fn drain_estimate(&self) -> DrainEstimate {
        let drain = self.drain();
        let passes = drain.judged.iter().map(|d| d.passes).sum();
        let deliveries = drain.judged.iter().map(|d| d.deliveries).sum();
        let Some(worst) = drain.worst() else {
            return DrainEstimate::Unjudged {
                longest_busy_period: drain.longest_busy_period,
                needed: MIN_SATURATED_PASSES,
            };
        };
        let rate = worst.rate_per_sec();
        if rate >= self.floor_per_sec {
            DrainEstimate::AtOrAbove {
                rate,
                passes,
                deliveries,
            }
        } else {
            DrainEstimate::Below {
                rate,
                passes,
                deliveries,
            }
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
        let throughput = match self.drain_estimate() {
            DrainEstimate::AtOrAbove {
                rate,
                passes,
                deliveries,
            } => format!(
                "drain estimate (disclosure) {rate:.1}/s >= {:.1}/s over {passes} saturated passes ({deliveries} deliveries{worst_source})",
                self.floor_per_sec,
            ),
            DrainEstimate::Below {
                rate,
                passes,
                deliveries,
            } => format!(
                "drain estimate (disclosure) {rate:.1}/s BELOW {:.1}/s over {passes} saturated passes ({deliveries} deliveries{worst_source})",
                self.floor_per_sec,
            ),
            DrainEstimate::Unjudged {
                longest_busy_period,
                needed,
            } => format!(
                "drain estimate (disclosure) unjudged (no busy period of {needed}+ saturated passes was judged; longest {longest_busy_period})"
            ),
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
mod tests;
