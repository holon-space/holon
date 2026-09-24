//! The throughput rung as a CONTROLLED DRAIN TEST (Martin's ruling D207.a).
//!
//! The gate drives `N` writes, one per target, offered every
//! [`DRAIN_OFFER_EVERY`] (twice the floor `f`) with at most `W` =
//! [`DRAIN_WINDOW`] of them undelivered at once, and checks that every one is
//! visible within `L = N/f + s` of the first dispatch. Capacity is not inferred
//! from passive traces: the busy-period estimator in [`crate::latency_slo`] is
//! a disclosure only.
//!
//! "Capacity at least `f`" means a rate-latency service curve: a backlog of `B`
//! writes is retired within `S + B/f`, where `S` is the pipeline's latency
//! term. `S` is the pass already in flight plus the commit latency and
//! first-pass overhead of the new backlog. Each of the two parts is at most one
//! idle service time, so `S ≤ 2 ×` [`SERVICE_TIME_SLO_MS`] `= s = 400 ms`. Both
//! block mirrors apply rows in commit order, and the first one to apply a row
//! closes its clock.
//!
//! The gate's numbers: `f = 10/s`, `N = 600`, `W = 40`, so `L = 60 s + 0.4 s =
//! 60.4 s`, the stall bound `s + W/f = 4.4 s`, and `f·s = 4` writes.
//!
//! **A healthy pipeline cannot fail.** Every Fail below is a proof that the
//! curve does not hold:
//! * [`DrainVerdict::Late`]: if every write `k` is dispatched by `t0 + k/f`,
//!   the min-plus bound `D(t) ≥ inf_u [A(u) + f·(t − u − S)⁺]` reaches `N` at
//!   `t0 + S + N/f`, so the completion is at most `L`.
//! * [`DrainVerdict::Stalled`]: the window keeps every write's backlog at
//!   dispatch within `W`, so a healthy pipeline delivers it within `s + W/f`.
//! * [`DrainVerdict::WindowHeld`]: it is judged at the first write `k` not
//!   offered by `t0 + k/f`, so writes `0..k` met their deadlines and are the
//!   only ones offered. The Late bound then has a healthy pipeline deliver all
//!   but `f·S ≤ f·s = 4` of them by `t0 + k/f`, fewer than `W`.
//!
//! **A slow pipeline always fails.** No write exists before `t0`, and a feed
//! spends at least `1/μ` on each row, so the last row is visible no earlier
//! than `t0 + N/μ`: every `μ < N/L = 600 / 60.4 s ≈ 9.934/s` fails, host load
//! notwithstanding, so only rates in `[9.934, 10)/s` sit below the floor and
//! can still pass. A sustained slow stretch fails early: its backlog fills the
//! window, and the oldest write then waits past `s + W/f`, or the next write
//! misses its floor deadline.
//!
//! "Slow" is a rate sustained over the drive. A shorter dip below `f` fails
//! only once one of its writes waits past `s + W/f = 4.4 s`. With writes
//! offered every 50 ms, the `j`-th write of a dip to 5/s waits about
//! `j · (200 − 50) ms`, so a dip of up to 29 writes passes and one of 30
//! fails as `Stalled`.
//!
//! **Nothing observed is lost.** The window stays below the correlator's
//! per-origin capacity (`MAX_PENDING`), so no drive clock is evicted, and a
//! drive clock older than `s + W/f` ends the drive as `Stalled` long before the
//! correlator's 30 s expiry. The drive starts only once no clock is pending on
//! any of its targets. A lost drive clock is therefore an assertion failure,
//! never a verdict.
//!
//! **Invalid.** A driver that offered a write after `t0 + k/f` while the window
//! had room did not offer the load the Late proof needs, and no Fail proof
//! exists: the run is [`DrainVerdict::Invalid`] and judges nothing about the
//! tree.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use crate::latency_e2e::MAX_PENDING;
use crate::latency_slo::E2eSample;
use crate::latency_slo::SERVICE_TIME_SLO_MS;
use crate::latency_slo::THROUGHPUT_FLOOR_WRITES_PER_SEC;

/// The latency allowance `s`: twice the service-time SLO.
pub const DRAIN_TEST_SLACK: Duration = Duration::from_millis(2 * SERVICE_TIME_SLO_MS);

/// Writes the gate drives: 30 s of offered load at 20 writes/s.
pub const DRAIN_WRITES: usize = 600;

/// The gate's offer interval: 20 writes/s, twice the floor.
pub const DRAIN_OFFER_EVERY: Duration = Duration::from_millis(50);

/// Undelivered drive writes allowed at once.
pub const DRAIN_WINDOW: usize = 40;

/// How long setup's clocks on the drive targets may take to close.
const QUIESCE_WITHIN: Duration = Duration::from_secs(10);

/// How long the warm-up write may take to become visible.
const WARM_UP_WITHIN: Duration = Duration::from_secs(10);

/// One drain test: `writes` writes, each to its own target, offered every
/// `offer_every` with at most `window` undelivered, judged against a floor of
/// `floor_per_sec` with latency allowance `slack`.
#[derive(Clone, Copy, Debug)]
pub struct DrainTest {
    writes: usize,
    floor_per_sec: f64,
    slack: Duration,
    window: usize,
    offer_every: Duration,
}

/// What a drain test observed.
#[derive(Clone, Debug, PartialEq)]
pub enum DrainVerdict {
    /// Every write was visible within `limit` of the first dispatch.
    Pass {
        completion: Duration,
        limit: Duration,
    },
    /// The last write became visible after `limit`, or was still undelivered
    /// at it (`completion: None`).
    Late {
        completion: Option<Duration>,
        delivered: usize,
        limit: Duration,
    },
    /// Write `write` stayed undelivered for `waited`, past the `bound` a
    /// pipeline at the floor meets.
    Stalled {
        write: usize,
        waited: Duration,
        bound: Duration,
    },
    /// At write `write`'s floor deadline the pipeline still held `pending`
    /// writes, a full window, so the driver could not offer it.
    WindowHeld { write: usize, pending: usize },
    /// Write `write` was offered `late_by` after its floor deadline while the
    /// window had room: the driver fell behind, and nothing was judged.
    Invalid { write: usize, late_by: Duration },
}

impl DrainVerdict {
    pub fn is_fail(&self) -> bool {
        matches!(
            self,
            Self::Late { .. } | Self::Stalled { .. } | Self::WindowHeld { .. }
        )
    }
}

impl DrainTest {
    /// The land gate's drain test.
    pub fn gate() -> Self {
        Self::new(
            DRAIN_WRITES,
            THROUGHPUT_FLOOR_WRITES_PER_SEC,
            DRAIN_TEST_SLACK,
            DRAIN_WINDOW,
            DRAIN_OFFER_EVERY,
        )
    }

    pub fn new(
        writes: usize,
        floor_per_sec: f64,
        slack: Duration,
        window: usize,
        offer_every: Duration,
    ) -> Self {
        assert!(writes > 0, "a drain test drives at least one write");
        assert!(floor_per_sec > 0.0, "the floor is a positive rate");
        assert!(
            window < MAX_PENDING,
            "a window of {window} can evict drive clocks at the correlator's capacity {MAX_PENDING}"
        );
        assert!(
            window as f64 > floor_per_sec * slack.as_secs_f64() + 1.0,
            "a window of {window} can be held full by a healthy pipeline"
        );
        assert!(
            offer_every.as_secs_f64() < 1.0 / floor_per_sec,
            "the offered rate must exceed the floor"
        );
        Self {
            writes,
            floor_per_sec,
            slack,
            window,
            offer_every,
        }
    }

    /// `N/f + s`: the latest completion a pipeline at the floor can have.
    pub fn limit(&self) -> Duration {
        self.floor_time(self.writes) + self.slack
    }

    /// `s + W/f`: the longest a healthy pipeline leaves one write undelivered.
    pub fn stall_bound(&self) -> Duration {
        self.slack + self.floor_time(self.window)
    }

    pub fn writes(&self) -> usize {
        self.writes
    }

    fn floor_time(&self, writes: usize) -> Duration {
        Duration::from_secs_f64(writes as f64 / self.floor_per_sec)
    }

    /// Judge a drive. `dispatched[k]` is write `k`'s offer instant, for the
    /// writes offered by `until`; `delivered[k]` is its visibility, if seen by
    /// `until`.
    fn judge(
        &self,
        dispatched: &[Instant],
        delivered: &[Option<Instant>],
        until: Instant,
    ) -> DrainVerdict {
        assert_eq!(dispatched.len(), delivered.len());
        assert!(dispatched.len() <= self.writes);
        let t0 = dispatched[0];
        let delivered_by = |at: Instant, below: usize| {
            delivered[..below]
                .iter()
                .filter(|d| d.is_some_and(|d| d <= at))
                .count()
        };
        for (j, &at) in dispatched.iter().enumerate() {
            let backlog = j + 1 - delivered_by(at, j);
            assert!(
                backlog <= self.window,
                "write {j} was offered with {backlog} writes undelivered, past the window of {}",
                self.window
            );
        }
        let bound = self.stall_bound();
        for (write, (&at, done)) in dispatched.iter().zip(delivered).enumerate() {
            let waited = done.unwrap_or(until) - at;
            if waited > bound {
                return DrainVerdict::Stalled {
                    write,
                    waited,
                    bound,
                };
            }
        }
        for write in 0..self.writes {
            let deadline = t0 + self.floor_time(write);
            let offered = dispatched.get(write).copied();
            let late_by = match offered {
                Some(at) if at > deadline => at - deadline,
                Some(_) => continue,
                None if until >= deadline => until - deadline,
                None => panic!(
                    "the drive stopped at write {write} before its floor deadline with no \
                     proof of a slow pipeline"
                ),
            };
            let pending = write - delivered_by(deadline, write);
            return if pending >= self.window {
                DrainVerdict::WindowHeld { write, pending }
            } else {
                DrainVerdict::Invalid { write, late_by }
            };
        }
        let limit = self.limit();
        let count = delivered.iter().flatten().count();
        match delivered.iter().copied().collect::<Option<Vec<Instant>>>() {
            Some(all) => {
                let completion = all.into_iter().max().expect("at least one write") - t0;
                if completion <= limit {
                    DrainVerdict::Pass { completion, limit }
                } else {
                    DrainVerdict::Late {
                        completion: Some(completion),
                        delivered: count,
                        limit,
                    }
                }
            }
            None => {
                assert!(
                    until - t0 >= limit,
                    "the drive stopped {:?} after t0 with {} writes undelivered, before the \
                     {limit:?} limit",
                    until - t0,
                    self.writes - count
                );
                DrainVerdict::Late {
                    completion: None,
                    delivered: count,
                    limit,
                }
            }
        }
    }
}

/// What the driver does next.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Offer one content write to `target` now, and report the instant taken
    /// just before the offer to [`Drive::dispatched`].
    Dispatch { target: String },
    /// Nothing to do before `until` unless a delivery arrives first.
    Wait { until: Instant },
    /// The drive is over; [`Drive::verdict`] judges it.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Waiting for setup's clocks on the drive targets to close.
    Quiescing {
        since: Option<Instant>,
    },
    /// The warm-up write on the first target takes the cold path outside the
    /// measured drive. `at` is its offer instant once dispatched.
    WarmingUp {
        at: Option<Instant>,
    },
    Driving,
    Done,
}

/// The driver's state machine. The caller loops [`Drive::step`], offering a
/// write whenever it says so, until it returns [`Step::Done`].
pub struct Drive {
    test: DrainTest,
    warm_up: String,
    targets: Vec<String>,
    index: HashMap<String, usize>,
    phase: Phase,
    awaiting_dispatch: bool,
    dispatched: Vec<Instant>,
    delivered: Vec<Option<Instant>>,
    delivered_count: usize,
    oldest_pending: usize,
    seen: usize,
    observed_until: Option<Instant>,
}

impl Drive {
    /// `targets[k]` receives write `k`; `warm_up` receives one unmeasured
    /// write before them.
    pub fn new(test: DrainTest, warm_up: String, targets: Vec<String>) -> Self {
        assert_eq!(targets.len(), test.writes, "one target per write");
        let index: HashMap<String, usize> = targets
            .iter()
            .chain(std::iter::once(&warm_up))
            .enumerate()
            .map(|(k, t)| (t.clone(), k))
            .collect();
        assert_eq!(
            index.len(),
            targets.len() + 1,
            "the drive targets and the warm-up target are distinct"
        );
        Self {
            test,
            warm_up,
            targets,
            index,
            phase: Phase::Quiescing { since: None },
            awaiting_dispatch: false,
            dispatched: Vec::new(),
            delivered: vec![None; test.writes],
            delivered_count: 0,
            oldest_pending: 0,
            seen: 0,
            observed_until: None,
        }
    }

    /// Decide at `now`. `samples` are the e2e samples on the drive's targets in
    /// recording order, a prefix-stable list that only grows; `pending` lists
    /// the targets of every pending clock and is read only before the drive.
    pub fn step(
        &mut self,
        now: Instant,
        samples: &[E2eSample],
        pending: impl FnOnce() -> Vec<String>,
    ) -> Step {
        assert!(
            !self.awaiting_dispatch,
            "the previous Dispatch was not reported"
        );
        assert!(self.observed_until.is_none_or(|t| t <= now));
        self.observed_until = Some(now);
        self.observe(now, samples);
        match self.phase {
            Phase::Quiescing { since } => {
                let stale: Vec<String> = pending()
                    .into_iter()
                    .filter(|t| self.index.contains_key(t))
                    .collect();
                if stale.is_empty() {
                    self.phase = Phase::WarmingUp { at: None };
                    self.awaiting_dispatch = true;
                    return Step::Dispatch {
                        target: self.warm_up.clone(),
                    };
                }
                let since = since.unwrap_or(now);
                assert!(
                    now - since < QUIESCE_WITHIN,
                    "drive targets {stale:?} still had clocks pending {QUIESCE_WITHIN:?} after \
                     the drive asked for quiet: setup left an interaction in flight on them"
                );
                self.phase = Phase::Quiescing { since: Some(since) };
                Step::Wait {
                    until: since + QUIESCE_WITHIN,
                }
            }
            Phase::WarmingUp { at } => {
                let at = at.expect("the warm-up was dispatched");
                assert!(
                    now - at < WARM_UP_WITHIN,
                    "the warm-up write on {} was not visible within {WARM_UP_WITHIN:?}",
                    self.warm_up
                );
                Step::Wait {
                    until: at + WARM_UP_WITHIN,
                }
            }
            Phase::Driving => self.drive(now),
            Phase::Done => Step::Done,
        }
    }

    fn drive(&mut self, now: Instant) -> Step {
        let bound = self.test.stall_bound();
        let oldest = self.dispatched.get(self.oldest_pending).copied();
        if oldest.is_some_and(|at| now - at > bound) {
            self.phase = Phase::Done;
            return Step::Done;
        }
        let stall_at = oldest.map(|at| at + bound);
        let k = self.dispatched.len();
        let Some(&t0) = self.dispatched.first() else {
            return self.dispatch(0);
        };
        if k == self.test.writes {
            let limit_at = t0 + self.test.limit();
            if self.delivered_count == k || now >= limit_at {
                self.phase = Phase::Done;
                return Step::Done;
            }
            return Step::Wait {
                until: stall_at.map_or(limit_at, |s| s.min(limit_at)),
            };
        }
        let offer_at = t0 + self.test.offer_every * k as u32;
        if now < offer_at {
            return Step::Wait { until: offer_at };
        }
        if k - self.delivered_count < self.test.window {
            return self.dispatch(k);
        }
        let deadline = t0 + self.test.floor_time(k);
        if now >= deadline {
            self.phase = Phase::Done;
            return Step::Done;
        }
        Step::Wait {
            until: stall_at.map_or(deadline, |s| s.min(deadline)),
        }
    }

    fn dispatch(&mut self, k: usize) -> Step {
        self.awaiting_dispatch = true;
        Step::Dispatch {
            target: self.targets[k].clone(),
        }
    }

    /// Record the instant of the write the last [`Step::Dispatch`] asked for.
    pub fn dispatched(&mut self, at: Instant) {
        assert!(self.awaiting_dispatch, "no Dispatch is outstanding");
        self.awaiting_dispatch = false;
        match self.phase {
            Phase::WarmingUp { at: None } => self.phase = Phase::WarmingUp { at: Some(at) },
            Phase::Driving => {
                assert!(
                    self.dispatched.last().is_none_or(|&last| last <= at),
                    "writes are reported in dispatch order"
                );
                self.dispatched.push(at);
            }
            phase => panic!("a dispatch reported in {phase:?}"),
        }
    }

    fn observe(&mut self, now: Instant, samples: &[E2eSample]) {
        let mut last = self.seen.checked_sub(1).map(|i| samples[i].delivered_at);
        for s in &samples[self.seen..] {
            if s.delivered_at > now {
                break;
            }
            assert!(
                last.is_none_or(|l| l <= s.delivered_at),
                "samples are not in delivery order"
            );
            last = Some(s.delivered_at);
            self.seen += 1;
            self.consume(s);
        }
    }

    fn consume(&mut self, s: &E2eSample) {
        let k = *self
            .index
            .get(&s.target)
            .unwrap_or_else(|| panic!("an e2e sample on {} is not on a drive target", s.target));
        assert_eq!(
            s.retired().get(),
            1,
            "the clock on {} was retired together with another clock on its target",
            s.target
        );
        match self.phase {
            Phase::Quiescing { .. } => {}
            Phase::WarmingUp { at } => {
                let at = at.expect("no sample before the warm-up is dispatched");
                assert!(
                    s.target == self.warm_up && s.latest_dispatch() >= at,
                    "a clock on {} closed during the warm-up, which only wrote {}",
                    s.target,
                    self.warm_up
                );
                self.phase = Phase::Driving;
            }
            Phase::Driving => {
                assert!(
                    s.target != self.warm_up,
                    "the warm-up write on {} closed twice",
                    self.warm_up
                );
                let at = *self.dispatched.get(k).unwrap_or_else(|| {
                    panic!("a clock on {} closed before the drive wrote it", s.target)
                });
                assert!(
                    self.delivered[k].is_none(),
                    "the drain write on {} was closed twice",
                    s.target
                );
                assert!(
                    s.latest_dispatch() >= at,
                    "the clock that closed {} opened before the driver dispatched its write",
                    s.target
                );
                self.delivered[k] = Some(s.delivered_at);
                self.delivered_count += 1;
                while self.oldest_pending < self.dispatched.len()
                    && self.delivered[self.oldest_pending].is_some()
                {
                    self.oldest_pending += 1;
                }
            }
            Phase::Done => panic!("a sample consumed after the drive ended"),
        }
    }

    /// The first dispatch of the measured drive.
    pub fn t0(&self) -> Option<Instant> {
        self.dispatched.first().copied()
    }

    /// Judge the drive as of its last observation.
    pub fn verdict(&self) -> DrainVerdict {
        assert_eq!(self.phase, Phase::Done, "the drive has not finished");
        self.test.judge(
            &self.dispatched,
            &self.delivered[..self.dispatched.len()],
            self.observed_until.expect("a finished drive has observed"),
        )
    }
}

#[cfg(test)]
mod tests {
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
                if committed.is_empty() {
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
                    Step::Done => return drive.verdict(),
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
    fn slow(
        draw: &mut impl FnMut(u64, u64) -> u64,
        per_row: (u64, u64),
        fast_rows: u64,
    ) -> Pipeline {
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
            matches!(verdict, DrainVerdict::Stalled { .. }),
            "{verdict:?}"
        );
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

    /// A 5/s dip at the end of a fast drive: 29 writes stay within the stall
    /// bound, 30 do not.
    #[test]
    fn a_short_dip_below_the_floor_passes_only_within_the_stall_bound() {
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
        let (dispatched, delivered) = throttled(tail(29));
        assert!(matches!(
            judged(&dispatched, &delivered, 60_400),
            DrainVerdict::Pass { .. }
        ));
        let (dispatched, delivered) = throttled(tail(30));
        assert!(matches!(
            judged(&dispatched, &delivered, 60_400),
            DrainVerdict::Stalled { .. }
        ));
    }

    /// A pipeline at the floor whose last write is offered at 56.4s and still
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

    #[test]
    fn a_write_undelivered_past_the_stall_bound_is_stalled() {
        let (dispatched, mut delivered) = throttled(|k| 50 * k + 20);
        delivered[3] = Some(150 + 4_401);
        assert_eq!(
            judged(&dispatched, &delivered, 60_400),
            DrainVerdict::Stalled {
                write: 3,
                waited: Duration::from_millis(4_401),
                bound: Duration::from_millis(4_400),
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

    /// Forty writes offered on schedule and none delivered by write 40's floor
    /// deadline at 4s: the drive stops there.
    #[test]
    fn a_window_held_full_at_a_floor_deadline_fails() {
        let dispatched: Vec<u64> = (0..DRAIN_WINDOW as u64).map(|k| k * 50).collect();
        let delivered = vec![None; DRAIN_WINDOW];
        assert_eq!(
            judged(&dispatched, &delivered, 4_000),
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
}
