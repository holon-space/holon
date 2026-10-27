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
//! The gate's numbers: `f = 10/s`, `N = 600`, `W = 60`, `P =`
//! [`DRAIN_PATIENCE`] `= 20 s`, so `L = 60 s + 0.4 s = 60.4 s`, `f·s = 4`
//! writes, and a full window is retired within `s + W/f = 6.4 s`.
//!
//! **A healthy pipeline cannot fail.** Every Fail below is a proof that the
//! curve does not hold:
//! * [`DrainVerdict::Late`]: if every write `k` is dispatched by `t0 + k/f`,
//!   the min-plus bound `D(t) ≥ inf_u [A(u) + f·(t − u − S)⁺]` reaches `N` at
//!   `t0 + S + N/f`, so the completion is at most `L`.
//! * [`DrainVerdict::WindowHeld`]: it is judged at the first write `k` not
//!   offered by `t0 + k/f`, so writes `0..k` met their deadlines and are the
//!   only ones offered. The Late bound then has a healthy pipeline deliver all
//!   but `f·S ≤ f·s = 4` of them by `t0 + k/f`, fewer than `W = 60`. The
//!   earliest such deadline is write `W`'s at `t0 + W/f = 6.0 s`, so over the
//!   first window a write may take up to 6.0 s.
//! * [`DrainVerdict::Undelivered`]: a write is offered with at most `W` writes
//!   undelivered, so a healthy pipeline retires it within `s + W/f = 6.4 s`,
//!   and a write still undelivered `P = 20 s` after its dispatch disproves the
//!   curve.
//!
//! **A slow pipeline always fails.** No write exists before `t0`, and a feed
//! spends at least `1/μ` on each row, so the last row is visible no earlier
//! than `t0 + N/μ`: every `μ < N/L = 600 / 60.4 s ≈ 9.934/s` fails, host load
//! notwithstanding, so only rates in `[9.934, 10)/s` sit below the floor and
//! can still pass. A sustained slow stretch fails early: its backlog fills the
//! window, and the next write misses its floor deadline as `WindowHeld`. A
//! pipeline that stops delivering fails as `Undelivered` at most `P` after the
//! dispatch of its first write it never delivers.
//!
//! "Slow" is a rate sustained over the drive. A dip below `f` fails only once
//! it pushes the completion past `L` or holds the window full at a floor
//! deadline. The writes are all offered by about 30 s, so a drive can end with
//! up to 202 writes at 5/s and pass (`29.97 s + 150 ms × 202 ≤ L`); 203 fail as
//! `Late`. One write's wait is judged only past `P`, over three times the
//! healthy bound: under the gate's drive in the `test` profile, per-write e2e
//! measured about 1.7 s p50 and 3.7 s max, far beyond `s`, so the longest wait
//! is reported, and judged only as a hang.
//!
//! **Nothing observed is lost.** The window stays below the correlator's
//! per-origin capacity (`MAX_PENDING`), so no drive clock is evicted, and `P`
//! stays below its 30 s expiry, so the drive ends before any drive clock can
//! expire. The drive starts only once no clock is pending on any of its
//! targets.
//!
//! **Invalid.** A driver that offered a write after `t0 + k/f` while the window
//! had room did not offer the load the Late proof needs, and no Fail proof
//! exists: the run is [`DrainVerdict::Invalid`] and judges nothing about the
//! tree.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use crate::latency_e2e::EXPIRY;
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
pub const DRAIN_WINDOW: usize = 60;

/// How long a dispatched write may stay undelivered before the drive fails.
pub const DRAIN_PATIENCE: Duration = Duration::from_secs(20);

/// How long setup's clocks on the drive targets may take to close.
const QUIESCE_WITHIN: Duration = Duration::from_secs(10);

/// How long the warm-up write may take to become visible.
const WARM_UP_WITHIN: Duration = Duration::from_secs(10);

/// One drain test: `writes` writes, each to its own target, offered every
/// `offer_every` with at most `window` undelivered, judged against a floor of
/// `floor_per_sec` with latency allowance `slack`, each within `patience` of
/// its dispatch.
#[derive(Clone, Copy, Debug)]
pub struct DrainTest {
    writes: usize,
    floor_per_sec: f64,
    slack: Duration,
    window: usize,
    offer_every: Duration,
    patience: Duration,
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
    /// At write `write`'s floor deadline the pipeline still held `pending`
    /// writes, a full window, so the driver could not offer it.
    WindowHeld { write: usize, pending: usize },
    /// Write `write` was still undelivered `after` its dispatch: the pipeline
    /// stopped delivering.
    Undelivered { write: usize, after: Duration },
    /// Write `write` was offered `late_by` after its floor deadline while the
    /// window had room: the driver fell behind, and nothing was judged.
    Invalid { write: usize, late_by: Duration },
}

impl DrainVerdict {
    pub fn is_fail(&self) -> bool {
        matches!(
            self,
            Self::Late { .. } | Self::WindowHeld { .. } | Self::Undelivered { .. }
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
            DRAIN_PATIENCE,
        )
    }

    pub fn new(
        writes: usize,
        floor_per_sec: f64,
        slack: Duration,
        window: usize,
        offer_every: Duration,
        patience: Duration,
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
        let full_window = slack + Duration::from_secs_f64(window as f64 / floor_per_sec);
        assert!(
            patience > full_window,
            "a patience of {patience:?} fails a healthy pipeline, which retires a full window \
             within {full_window:?}"
        );
        assert!(
            patience < EXPIRY,
            "a patience of {patience:?} lets the correlator expire a drive clock at {EXPIRY:?} \
             before the drive judges it"
        );
        Self {
            writes,
            floor_per_sec,
            slack,
            window,
            offer_every,
            patience,
        }
    }

    /// `N/f + s`: the latest completion a pipeline at the floor can have.
    pub fn limit(&self) -> Duration {
        self.floor_time(self.writes) + self.slack
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
        let stuck = (0..dispatched.len()).find(|&j| {
            let give_up = dispatched[j] + self.patience;
            until >= give_up && delivered[j].is_none_or(|d| d >= give_up)
        });
        if let Some(write) = stuck {
            return DrainVerdict::Undelivered {
                write,
                after: self.patience,
            };
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
    oldest_undelivered: usize,
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
            oldest_undelivered: 0,
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
        let Some(&t0) = self.dispatched.first() else {
            return self.dispatch(0);
        };
        while self.delivered[..self.dispatched.len()]
            .get(self.oldest_undelivered)
            .is_some_and(Option::is_some)
        {
            self.oldest_undelivered += 1;
        }
        let give_up = self
            .dispatched
            .get(self.oldest_undelivered)
            .map(|&at| at + self.test.patience);
        if give_up.is_some_and(|g| now >= g) {
            self.phase = Phase::Done;
            return Step::Done;
        }
        match self.schedule(now, t0) {
            Step::Wait { until } => Step::Wait {
                until: give_up.map_or(until, |g| g.min(until)),
            },
            step => step,
        }
    }

    fn schedule(&mut self, now: Instant, t0: Instant) -> Step {
        let k = self.dispatched.len();
        if k == self.test.writes {
            let limit_at = t0 + self.test.limit();
            if self.delivered_count == k || now >= limit_at {
                self.phase = Phase::Done;
                return Step::Done;
            }
            return Step::Wait { until: limit_at };
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
        Step::Wait { until: deadline }
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
            }
            Phase::Done => panic!("a sample consumed after the drive ended"),
        }
    }

    /// The first dispatch of the measured drive.
    pub fn t0(&self) -> Option<Instant> {
        self.dispatched.first().copied()
    }

    /// The longest any dispatched write waited for its delivery, an
    /// undelivered one counted up to the last observation.
    pub fn longest_wait(&self) -> Duration {
        let until = self
            .observed_until
            .expect("a drive that dispatched has observed");
        self.dispatched
            .iter()
            .zip(&self.delivered)
            .map(|(&at, done)| done.unwrap_or(until) - at)
            .max()
            .expect("the drive dispatched a write")
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
mod tests;
