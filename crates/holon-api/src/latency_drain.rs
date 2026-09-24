//! The throughput rung as a CONTROLLED DRAIN TEST (Martin's ruling D207.a).
//!
//! The gate drives `N` writes on a known schedule at a rate above the floor
//! `f`, and checks that every one is visible within `N/f + s` of the first
//! dispatch. Capacity is not inferred from passive traces: the busy-period
//! estimator in [`crate::latency_slo`] is a disclosure only.
//!
//! **Healthy never fails.** "Capacity at least `f`" means: a backlog of `k`
//! writes is retired within `S + k/f`, where `S` is the pipeline's latency
//! term (commit latency, the pass already in flight, the first pass's
//! overhead — each bounded by one idle service time, so `S ≤ 2 ×`
//! [`SERVICE_TIME_SLO_MS`] `= s`). If every write `k` is dispatched by
//! `t0 + k/f`, then at `t* = t0 + S + N/f` the min-plus bound
//! `D(t*) ≥ inf_u [A(u) + f·(t* − u − S)⁺]` is at least `N` for every `u`, so
//! the completion `C ≤ S + N/f ≤ N/f + s`. No assumption about pass times,
//! batching, quiet batches or a second mirror enters.
//!
//! **Slow always fails.** No write exists before `t0`, and a feed applies rows
//! in commit order at a cost of at least `1/μ` each, so the last row is visible
//! no earlier than `t0 + N/μ`. Every `μ < N / (N/f + s)` fails, host load
//! notwithstanding.
//!
//! The schedule premise is measured, not assumed: a driver that fell behind
//! `t0 + k/f` makes the run [`DrainVerdict::Invalid`], never red.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use crate::latency_slo::E2eSample;
use crate::latency_slo::SERVICE_TIME_SLO_MS;

/// The latency allowance `s`: twice the service-time SLO. See the module doc.
pub const DRAIN_TEST_SLACK: Duration = Duration::from_millis(2 * SERVICE_TIME_SLO_MS);

/// One drain test: `writes` writes, each to its own target, judged against a
/// floor of `floor_per_sec` with latency allowance `slack`.
#[derive(Clone, Copy, Debug)]
pub struct DrainTest {
    writes: usize,
    floor_per_sec: f64,
    slack: Duration,
}

/// What a drain test observed.
#[derive(Clone, Debug, PartialEq)]
pub enum DrainVerdict {
    /// Every write was visible within `limit` of the first dispatch.
    Pass {
        completion: Duration,
        limit: Duration,
    },
    /// Some write was not visible within `limit`. `completion` is `None` when
    /// a write was never delivered at all.
    Fail {
        completion: Option<Duration>,
        delivered: usize,
        limit: Duration,
    },
    /// Write `write` was dispatched `late_by` after `t0 + write/f`: the driver
    /// did not offer the load the Pass proof assumes, so nothing was judged.
    Invalid { write: usize, late_by: Duration },
}

impl DrainTest {
    pub fn new(writes: usize, floor_per_sec: f64, slack: Duration) -> Self {
        assert!(writes > 0, "a drain test drives at least one write");
        assert!(floor_per_sec > 0.0, "the floor is a positive rate");
        Self {
            writes,
            floor_per_sec,
            slack,
        }
    }

    /// `N/f + s`: the latest completion a pipeline at the floor can have.
    pub fn limit(&self) -> Duration {
        Duration::from_secs_f64(self.writes as f64 / self.floor_per_sec) + self.slack
    }

    /// The latest instant write `k` may be dispatched for the run to be valid.
    fn scheduled_by(&self, t0: Instant, k: usize) -> Instant {
        t0 + Duration::from_secs_f64(k as f64 / self.floor_per_sec)
    }

    /// Judge a run. `writes` are the driver's `(target, dispatch instant)` in
    /// dispatch order, one per distinct target; `samples` are the e2e samples
    /// that closed them, and nothing else.
    pub fn verdict(&self, writes: &[(String, Instant)], samples: &[E2eSample]) -> DrainVerdict {
        assert_eq!(
            writes.len(),
            self.writes,
            "the drain test was sized for {} writes",
            self.writes
        );
        assert!(
            writes.windows(2).all(|w| w[0].1 <= w[1].1),
            "the driver's writes are not in dispatch order"
        );
        let index: HashMap<&str, usize> = writes
            .iter()
            .enumerate()
            .map(|(k, (target, _))| (target.as_str(), k))
            .collect();
        assert_eq!(
            index.len(),
            writes.len(),
            "two drain-test writes share a target, so one clock could retire the other"
        );
        let mut delivered: Vec<Option<Instant>> = vec![None; writes.len()];
        for s in samples {
            let k = *index.get(s.target.as_str()).unwrap_or_else(|| {
                panic!(
                    "an e2e sample on {} is not one of the drain test's writes",
                    s.target
                )
            });
            assert_eq!(
                s.retired().get(),
                1,
                "the drain write on {} was retired together with another clock on its target",
                s.target
            );
            assert!(
                delivered[k].is_none(),
                "the drain write on {} was closed twice",
                s.target
            );
            assert!(
                s.latest_dispatch() >= writes[k].1,
                "the clock that closed {} opened before the driver dispatched its write",
                s.target
            );
            delivered[k] = Some(s.delivered_at);
        }
        self.judge(
            &writes.iter().map(|(_, at)| *at).collect::<Vec<_>>(),
            &delivered,
        )
    }

    fn judge(&self, dispatched: &[Instant], delivered: &[Option<Instant>]) -> DrainVerdict {
        let t0 = dispatched[0];
        if let Some((write, at)) = dispatched
            .iter()
            .enumerate()
            .find(|&(k, &at)| at > self.scheduled_by(t0, k))
        {
            return DrainVerdict::Invalid {
                write,
                late_by: *at - self.scheduled_by(t0, write),
            };
        }
        let limit = self.limit();
        let completion = delivered
            .iter()
            .copied()
            .collect::<Option<Vec<Instant>>>()
            .map(|all| all.into_iter().max().expect("at least one write") - t0);
        match completion {
            Some(completion) if completion <= limit => DrainVerdict::Pass { completion, limit },
            _ => DrainVerdict::Fail {
                completion,
                delivered: delivered.iter().flatten().count(),
                limit,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latency_slo::ClockOrigin;
    use crate::latency_slo::QuietBatches;
    use crate::latency_slo::Superseded;
    use crate::latency_slo::THROUGHPUT_FLOOR_WRITES_PER_SEC;

    const N: usize = 60;
    /// The gate's offered rate: 20 writes/s.
    const GAP_US: u64 = 50_000;

    fn test() -> DrainTest {
        DrainTest::new(N, THROUGHPUT_FLOOR_WRITES_PER_SEC, DRAIN_TEST_SLACK)
    }

    /// SplitMix64: a fixed seed gives a fixed trace.
    fn split_mix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// How one feed of the simulated pipeline applies rows. All times in µs.
    #[derive(Clone, Copy, Debug)]
    enum Shape {
        /// One row per pass; a pass takes a time drawn per pass in `lo..=hi`.
        Serial { lo: u64, hi: u64 },
        /// Every committed row per pass; a pass takes a base drawn per pass in
        /// `lo..=hi` plus `per_row` for each row it carries.
        Batching { lo: u64, hi: u64, per_row: u64 },
    }

    #[derive(Clone, Debug)]
    struct Pipeline {
        /// One shape per feed. Two feeds are the two `block` mirrors: the one
        /// that applies a row first closes its clock; the other's batch is
        /// quiet.
        feeds: Vec<Shape>,
        /// Commit latency per write, drawn in `0..=commit_max`.
        commit_max: u64,
        /// Untracked batches of `quiet.0..=quiet.1` µs, one before a pass with
        /// probability `quiet.2` percent.
        quiet: (u64, u64, u64),
    }

    struct SimRun {
        writes: Vec<(String, Instant)>,
        samples: Vec<E2eSample>,
    }

    /// Run `N` writes on the gate's schedule through `pipeline`, and emit what
    /// the correlator would: one sample per clock, closed by the first feed
    /// that applies its row, with each feed's quiet batches logged.
    fn simulate(pipeline: &Pipeline, seed: u64) -> SimRun {
        let mut rng = seed;
        let mut draw = |lo: u64, hi: u64| lo + split_mix(&mut rng) % (hi - lo + 1);
        let t0 = Instant::now();
        let dispatch: Vec<u64> = (0..N as u64).map(|k| k * GAP_US).collect();
        let commit: Vec<u64> = dispatch
            .iter()
            .map(|d| d + draw(0, pipeline.commit_max))
            .collect();
        struct Batch {
            feed: usize,
            end: u64,
            rows: Vec<usize>,
        }
        let mut batches = Vec::new();
        for (feed, shape) in pipeline.feeds.iter().enumerate() {
            let mut applied = [false; N];
            let mut now = 0;
            while applied.iter().any(|a| !a) {
                let committed: Vec<usize> = (0..N)
                    .filter(|&k| !applied[k] && commit[k] <= now)
                    .collect();
                if committed.is_empty() {
                    now = (0..N)
                        .filter(|&k| !applied[k])
                        .map(|k| commit[k])
                        .min()
                        .expect("an unapplied row");
                    continue;
                }
                if draw(0, 99) < pipeline.quiet.2 {
                    now += draw(pipeline.quiet.0, pipeline.quiet.1);
                    batches.push(Batch {
                        feed,
                        end: now,
                        rows: Vec::new(),
                    });
                }
                let rows = match *shape {
                    Shape::Serial { lo, hi } => {
                        now += draw(lo, hi);
                        vec![committed[0]]
                    }
                    Shape::Batching { lo, hi, per_row } => {
                        now += draw(lo, hi) + per_row * committed.len() as u64;
                        committed
                    }
                };
                for &k in &rows {
                    applied[k] = true;
                }
                batches.push(Batch {
                    feed,
                    end: now,
                    rows,
                });
            }
        }
        batches.sort_by_key(|b| b.end);
        let mut closed = [false; N];
        let mut quiet_since: Vec<Vec<u64>> = vec![Vec::new(); pipeline.feeds.len()];
        let mut samples = Vec::new();
        for (batch_id, b) in batches.iter().enumerate() {
            let closes: Vec<usize> = b.rows.iter().copied().filter(|&k| !closed[k]).collect();
            if closes.is_empty() {
                quiet_since[b.feed].push(b.end);
                continue;
            }
            let quiet =
                QuietBatches::new(quiet_since[b.feed].drain(..).map(|q| b.end - q).collect());
            for (i, &k) in closes.iter().enumerate() {
                closed[k] = true;
                let delivered_us = b.end + i as u64;
                samples.push(E2eSample {
                    action: "set_field".to_string(),
                    target: format!("block:drain-{k}"),
                    origin: ClockOrigin::Ui,
                    ms: (delivered_us - dispatch[k]) / 1000,
                    in_flight: 2,
                    backlog: 1,
                    contended: false,
                    delivered_at: t0 + Duration::from_micros(delivered_us),
                    delivery_batch: batch_id as u64,
                    source: "block".to_string(),
                    feed: b.feed as u64,
                    superseded: Superseded::default(),
                    quiet: quiet.clone(),
                });
            }
        }
        SimRun {
            writes: (0..N)
                .map(|k| {
                    (
                        format!("block:drain-{k}"),
                        t0 + Duration::from_micros(dispatch[k]),
                    )
                })
                .collect(),
            samples,
        }
    }

    /// What the land gate concludes from a run: red, green, or no verdict.
    #[derive(Debug, PartialEq)]
    enum Gate {
        Red(String),
        Green,
        NoVerdict(String),
    }

    /// THE SEAM: the land gate's throughput verdict over one run.
    fn gate_verdict(run: &SimRun) -> Gate {
        match test().verdict(&run.writes, &run.samples) {
            DrainVerdict::Pass { .. } => Gate::Green,
            v @ DrainVerdict::Fail { .. } => Gate::Red(format!("{v:?}")),
            v @ DrainVerdict::Invalid { .. } => Gate::NoVerdict(format!("{v:?}")),
        }
    }

    /// A healthy pipeline: every feed retires at least `f` writes/s with a
    /// latency term within `s`, background batches included.
    fn healthy(draw: &mut impl FnMut(u64, u64) -> u64) -> Pipeline {
        let quiet = (1_000, 15_000, draw(0, 50));
        let shape = |draw: &mut dyn FnMut(u64, u64) -> u64| match draw(0, 1) {
            // At most 85ms a pass plus one 15ms quiet batch: 100ms a row.
            0 => {
                let hi = draw(1_000, 85_000);
                Shape::Serial {
                    lo: draw(1_000, hi),
                    hi,
                }
            }
            // Per-row cost at most half the gap between writes; bases vary
            // per pass from 1 to 400ms, the pass-time variation that broke
            // the passive estimator.
            _ => {
                let hi = draw(1_000, 400_000);
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
            quiet,
        }
    }

    /// A pipeline whose every feed spends at least `per_row` on each row:
    /// capacity `1/per_row`.
    fn slow(draw: &mut impl FnMut(u64, u64) -> u64, per_row: (u64, u64)) -> Pipeline {
        let quiet = (1_000, 15_000, draw(0, 100));
        let feeds = (0..draw(1, 2))
            .map(|_| {
                let cost = draw(per_row.0, per_row.1);
                match draw(0, 1) {
                    0 => Shape::Serial {
                        lo: cost,
                        hi: cost + draw(0, 100_000),
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
            quiet,
        }
    }

    /// **The gate's contract, as a property.** A healthy pipeline — pass times
    /// that vary per pass, commit latency, quiet batches, a second mirror —
    /// never turns the gate red, and a pipeline at or below 9.35 writes/s
    /// (2× too slow included) always does.
    #[test]
    fn the_drain_gate_passes_every_healthy_pipeline_and_fails_every_slow_one() {
        let mut wrong: std::collections::BTreeMap<&str, (usize, Option<String>)> =
            std::collections::BTreeMap::new();
        for seed in 0..10_000u64 {
            let mut rng = seed;
            let mut draw = |lo: u64, hi: u64| lo + split_mix(&mut rng) % (hi - lo + 1);
            let (pipeline, must_fail) = match seed % 3 {
                0 => (healthy(&mut draw), false),
                // 2× to 4× too slow.
                1 => (slow(&mut draw, (200_000, 400_000)), true),
                // Just below the floor: 5 to 9.35 writes/s.
                _ => (slow(&mut draw, (107_000, 200_000)), true),
            };
            let run = simulate(&pipeline, draw(0, u64::MAX - 1));
            let gate = gate_verdict(&run);
            let family = match (must_fail, &gate) {
                (true, Gate::Red(_)) | (false, Gate::Green) => continue,
                (true, Gate::Green) => "slow pipeline green",
                (true, _) => "slow pipeline without a verdict",
                (false, Gate::Red(_)) => "healthy pipeline red",
                (false, _) => "healthy pipeline without a verdict",
            };
            let (count, first) = wrong.entry(family).or_insert((0, None));
            *count += 1;
            first.get_or_insert(format!("seed {seed}: {pipeline:?} -> {gate:?}"));
        }
        assert!(wrong.is_empty(), "of 10000 traces: {wrong:#?}");
    }

    /// The verifier's DEFECT 1 shape on the gate's schedule: an unbounded
    /// pipeline whose pass time varies from near idle to loaded. The passive
    /// estimator failed this shape on a sparser schedule.
    #[test]
    fn a_pass_time_that_varies_inside_a_burst_is_not_red() {
        let run = simulate(
            &Pipeline {
                feeds: vec![Shape::Batching {
                    lo: 20_000,
                    hi: 300_000,
                    per_row: 0,
                }],
                commit_max: 0,
                quiet: (0, 0, 0),
            },
            605,
        );
        assert_eq!(gate_verdict(&run), Gate::Green);
    }

    /// The verifier's DEFECT 3 shape: a serial pipeline 2× too slow, with a
    /// 5ms untracked batch before every pass. The passive estimator could
    /// never judge it.
    #[test]
    fn a_slow_pipeline_interleaved_with_quiet_batches_is_red() {
        let run = simulate(
            &Pipeline {
                feeds: vec![Shape::Serial {
                    lo: 200_000,
                    hi: 200_000,
                }],
                commit_max: 0,
                quiet: (5_000, 5_000, 100),
            },
            3,
        );
        assert!(matches!(gate_verdict(&run), Gate::Red(_)));
    }

    /// Driver instants and deliveries in ms after `t0`, straight to the judge.
    fn judged(dispatch_ms: &[u64], delivered_ms: &[Option<u64>]) -> DrainVerdict {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        test().judge(
            &dispatch_ms.iter().map(|&d| at(d)).collect::<Vec<_>>(),
            &delivered_ms.iter().map(|d| d.map(at)).collect::<Vec<_>>(),
        )
    }

    fn on_schedule() -> Vec<u64> {
        (0..N as u64).map(|k| k * 50).collect()
    }

    /// A serial pipeline at exactly the floor, 100ms a row, whose first row
    /// commits after the whole 400ms allowance: it completes at exactly
    /// `N/f + s` and passes. Pins the limit's `N`, its slack and its `<=`.
    #[test]
    fn a_pipeline_at_the_floor_with_the_full_latency_allowance_passes() {
        let delivered: Vec<Option<u64>> = (1..=N as u64).map(|k| Some(400 + 100 * k)).collect();
        assert_eq!(
            judged(&on_schedule(), &delivered),
            DrainVerdict::Pass {
                completion: Duration::from_millis(6_400),
                limit: Duration::from_millis(6_400),
            }
        );
    }

    /// 9.3 writes/s from the first dispatch, no latency at all: 59 rows after
    /// the first land 107.5ms apart, and the last one lands past the limit.
    /// Pins that completion is measured from the first DISPATCH.
    #[test]
    fn a_pipeline_just_below_the_floor_fails() {
        let delivered: Vec<Option<u64>> =
            (0..N as u64).map(|k| Some(108 + k * 1075 / 10)).collect();
        assert!(matches!(
            judged(&on_schedule(), &delivered),
            DrainVerdict::Fail {
                completion: Some(_),
                delivered: N,
                ..
            }
        ));
    }

    #[test]
    fn an_undelivered_write_fails() {
        let mut delivered: Vec<Option<u64>> = on_schedule().iter().map(|d| Some(d + 20)).collect();
        delivered[17] = None;
        assert!(matches!(
            judged(&on_schedule(), &delivered),
            DrainVerdict::Fail {
                completion: None,
                delivered: 59,
                ..
            }
        ));
    }

    /// Write 3 dispatched at 301ms, 1ms behind `t0 + 3/f`: the Pass proof's
    /// premise is void, so the run is not judged, however fast it drained.
    #[test]
    fn a_driver_behind_the_floor_schedule_is_invalid() {
        let mut dispatch = on_schedule();
        for d in dispatch.iter_mut().skip(3) {
            *d += 151;
        }
        let delivered: Vec<Option<u64>> = dispatch.iter().map(|d| Some(d + 20)).collect();
        assert_eq!(
            judged(&dispatch, &delivered),
            DrainVerdict::Invalid {
                write: 3,
                late_by: Duration::from_millis(1),
            }
        );
    }

    #[test]
    #[should_panic(expected = "closed twice")]
    fn a_write_closed_twice_is_refused() {
        let mut run = simulate(
            &Pipeline {
                feeds: vec![Shape::Serial {
                    lo: 1_000,
                    hi: 1_000,
                }],
                commit_max: 0,
                quiet: (0, 0, 0),
            },
            1,
        );
        let again = run.samples[0].clone();
        run.samples.push(again);
        test().verdict(&run.writes, &run.samples);
    }
}
