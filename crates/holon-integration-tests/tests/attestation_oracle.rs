//! Shared parts of the self-attestation tests: the refusal text written out
//! from its specification, fresh kind names, and the tally operations a caller
//! can apply to the process-wide draw counter.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread::JoinHandle;

use holon_integration_tests::pbt::alphabet_coverage::note_declined;
use holon_integration_tests::pbt::alphabet_coverage::note_drawn;
use holon_integration_tests::pbt::alphabet_coverage::note_offered;
use holon_integration_tests::pbt::alphabet_coverage::note_promised;
use holon_pbt_core::validation::Reason;
use nonempty_collections::NEVec;
use proptest::prelude::*;

/// What a refused self-attestation of `kind` says.
pub fn refusal_text(kind: &str) -> String {
    format!(
        "{kind}: this suite vouches for itself as the kind's coverage \
         (`CoveredBy::SelfAttestingSuite` in `EXPECTED_ABSENT`), but its run drew it 0 times — \
         draw the kind here, or move the claim to the suite that does"
    )
}

/// A kind name no other test of this process notes. The tally is
/// process-wide, so a shared name would let one test's draws satisfy another
/// test's attestation; `#` never occurs in a transition kind name.
pub fn fresh_kind(prefix: &str) -> &'static str {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    Box::leak(format!("{prefix}#fresh-{n}").into_boxed_str())
}

pub fn leak(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

pub fn any_kind_text() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => any::<String>(),
        1 => Just(String::new()),
        1 => prop::sample::select(vec![
            "StructuralKeyBurst",
            "ToggleState",
            "HoldDispatch",
            "{}",
            "{kind}",
            "%s",
            "\n",
            "\0",
        ])
        .prop_map(str::to_owned),
        1 => "[a-zA-Z]{1,4}".prop_map(|s| s.repeat(500)),
    ]
}

#[derive(Clone, Copy, Debug)]
pub enum WorkerEnd {
    Draw,
    DrawThenPanic,
    PanicWithoutDraw,
}

impl WorkerEnd {
    pub fn draws(self) -> bool {
        matches!(self, WorkerEnd::Draw | WorkerEnd::DrawThenPanic)
    }
}

/// One public operation on the tally, on kind `kind` of a per-case pool.
#[derive(Clone, Debug)]
pub enum TallyOp {
    Promise(usize),
    Offer(usize),
    Decline(usize, Vec<Reason>),
    Draw(usize),
    StartWorker(usize),
    FinishWorker(usize, WorkerEnd),
    /// Every open worker draws at once, so their draws race on the counter.
    FinishAllWorkers,
}

pub fn tally_op(pool: usize) -> impl Strategy<Value = TallyOp> {
    let kind = 0..pool;
    let reason = prop::sample::select(vec![
        Reason::NoTogglableStates,
        Reason::HandAuthoredOnly,
        Reason::AppNotStarted,
    ]);
    let end = prop_oneof![
        Just(WorkerEnd::Draw),
        Just(WorkerEnd::DrawThenPanic),
        Just(WorkerEnd::PanicWithoutDraw),
    ];
    prop_oneof![
        1 => kind.clone().prop_map(TallyOp::Promise),
        1 => kind.clone().prop_map(TallyOp::Offer),
        1 => (kind.clone(), prop::collection::vec(reason, 1..4))
            .prop_map(|(k, r)| TallyOp::Decline(k, r)),
        2 => kind.clone().prop_map(TallyOp::Draw),
        1 => kind.prop_map(TallyOp::StartWorker),
        1 => (any::<usize>(), end).prop_map(|(i, e)| TallyOp::FinishWorker(i, e)),
        1 => Just(TallyOp::FinishAllWorkers),
    ]
}

struct Worker {
    kind: &'static str,
    end: mpsc::Sender<WorkerEnd>,
    handle: JoinHandle<()>,
}

/// Applies tally operations and tracks the draws they made per kind name.
/// A worker draws only when its end is applied, so an open worker is a draw
/// that has not happened yet; dropping the tally abandons open workers.
#[derive(Default)]
pub struct Tally {
    draws: std::collections::BTreeMap<String, u64>,
    workers: Vec<Worker>,
}

impl Tally {
    pub fn apply(&mut self, op: &TallyOp, kinds: &[&'static str]) {
        match op {
            TallyOp::Promise(k) => note_promised(kinds[*k]),
            TallyOp::Offer(k) => note_offered(kinds[*k]),
            TallyOp::Decline(k, reasons) => {
                let reasons = NEVec::try_from_vec(reasons.clone()).expect("1..4 reasons");
                note_declined(kinds[*k], &reasons);
            }
            TallyOp::Draw(k) => {
                note_drawn(kinds[*k]);
                *self.draws.entry(kinds[*k].to_owned()).or_default() += 1;
            }
            TallyOp::StartWorker(k) => {
                let kind = kinds[*k];
                let (end, rx) = mpsc::channel::<WorkerEnd>();
                let handle = std::thread::spawn(move || {
                    let Ok(end) = rx.recv() else { return };
                    if end.draws() {
                        note_drawn(kind);
                    }
                    assert!(
                        !matches!(end, WorkerEnd::DrawThenPanic | WorkerEnd::PanicWithoutDraw),
                        "a worker of the vouching suite failed"
                    );
                });
                self.workers.push(Worker { kind, end, handle });
            }
            TallyOp::FinishWorker(i, end) => {
                if self.workers.is_empty() {
                    return;
                }
                let worker = self.workers.remove(i % self.workers.len());
                worker
                    .end
                    .send(*end)
                    .expect("an open worker waits for its end");
                let joined = worker.handle.join();
                assert_eq!(
                    joined.is_err(),
                    !matches!(end, WorkerEnd::Draw),
                    "a worker ends as it was told"
                );
                if end.draws() {
                    *self.draws.entry(worker.kind.to_owned()).or_default() += 1;
                }
            }
            TallyOp::FinishAllWorkers => {
                let workers = std::mem::take(&mut self.workers);
                for worker in &workers {
                    worker
                        .end
                        .send(WorkerEnd::Draw)
                        .expect("an open worker waits for its end");
                }
                for worker in workers {
                    worker.handle.join().expect("a drawing worker ends cleanly");
                    *self.draws.entry(worker.kind.to_owned()).or_default() += 1;
                }
            }
        }
    }

    pub fn drawn(&self, kind: &str) -> u64 {
        self.draws.get(kind).copied().unwrap_or(0)
    }
}

impl Drop for Tally {
    fn drop(&mut self) {
        for worker in self.workers.drain(..) {
            drop(worker.end);
            worker
                .handle
                .join()
                .expect("an abandoned worker exits without drawing");
        }
    }
}
