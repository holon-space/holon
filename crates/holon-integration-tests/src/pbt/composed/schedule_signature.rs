//! The schedule a masked transition actually ran, as a string, plus the
//! process-wide set of the ones observed so far.
//!
//! A signature is the dispatch/completion interleaving read off the slice's
//! dispatch journal: `D` when an intent was dispatched, `C` when one settled.
//! `DDDDCCCC` is dispatch-all-then-settle; `DCDCDCDC` is a fully drained
//! schedule. Two runs that differ only in wall timing produce the same
//! signature, which is what makes it comparable across seeds.
//!
//! Coverage is judged on the [`Shape`] a signature falls into, never on the
//! signature itself: `DDDCCC` and `DDCC` are the same schedule dispatching
//! different numbers of characters, so counting them as two schedules would let
//! a generator that varies text length pass a coverage claim about
//! interleaving.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use holon_frontend::dispatch_journal::DispatchJournal;

/// Observations required before [`assert_coverage`] judges the run. Below this
/// the process has not yet seen enough masked multi-intent transitions for
/// "only one schedule is reachable" to be a claim about the harness rather than
/// about a short run.
const COVERAGE_FLOOR: usize = 4;

/// The schedule a signature describes, independent of how many intents the
/// transition happened to dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    /// Every dispatch precedes every completion.
    Burst,
    /// At least one intent completed while dispatches were still coming.
    Interleaved,
}

impl Shape {
    /// Classify by whether a `C` occurs before the last `D`.
    pub fn of(signature: &str) -> Self {
        let last_dispatch = signature.rfind('D');
        match last_dispatch {
            Some(i) if signature[..i].contains('C') => Self::Interleaved,
            _ => Self::Burst,
        }
    }
}

/// One masked transition's dispatch/completion stream, built by sampling the
/// journal while the transition runs.
pub struct Recording {
    mark: u64,
    kind: String,
    events: String,
    dispatched: usize,
    settled: usize,
}

impl Recording {
    pub fn start(journal: &DispatchJournal, kind: &str) -> Self {
        Self {
            mark: journal.mark(),
            kind: kind.to_string(),
            events: String::new(),
            dispatched: 0,
            settled: 0,
        }
    }

    /// Fold everything the journal has learned since the last sample into the
    /// event string. Dispatches are appended before completions: within one
    /// sampling window the two are indistinguishable, and an intent cannot
    /// settle before it was dispatched.
    pub fn sample(&mut self, journal: &DispatchJournal) {
        let entries = journal.since(self.mark).unwrap_or_else(|e| {
            panic!(
                "[schedule-signature] dispatch journal readback for {}: {e:#}",
                self.kind
            )
        });
        let dispatched = entries.len();
        let settled = entries.iter().filter(|e| !e.outcome.is_pending()).count();
        for _ in self.dispatched..dispatched {
            self.events.push('D');
        }
        for _ in self.settled..settled {
            self.events.push('C');
        }
        self.dispatched = dispatched;
        self.settled = settled;
    }

    /// Close the recording and register it. Completions that landed during the
    /// settle arrive here as one trailing run of `C`.
    pub fn finish(mut self, journal: &DispatchJournal) -> String {
        self.sample(journal);
        let intents = self.dispatched;
        let signature = self.events;
        if intents >= 2 {
            observed()
                .lock()
                .expect("schedule signature registry")
                .entry(Shape::of(&signature))
                .and_modify(|n| *n += 1)
                .or_insert(1);
        }
        signature
    }
}

fn observed() -> &'static Mutex<BTreeMap<Shape, usize>> {
    static OBSERVED: OnceLock<Mutex<BTreeMap<Shape, usize>>> = OnceLock::new();
    OBSERVED.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn exercised() -> &'static AtomicBool {
    static EXERCISED: AtomicBool = AtomicBool::new(false);
    &EXERCISED
}

/// Record that a multi-intent transition actually drew a waiting slot — the
/// precondition for judging coverage at all.
pub fn note_schedule_exercised() {
    exercised().store(true, Ordering::Release);
}

/// `inv-schedule-coverage`: when a schedule asked to wait, the run must
/// actually reach an interleaved schedule.
///
/// Called per sequence. Judges the PROCESS, not the sequence: a single sequence
/// rarely holds enough multi-intent masked transitions to distinguish "no
/// interleaving is reachable" from "this sequence was short".
///
/// The claim is causal rather than a count of shapes. "At least two distinct
/// shapes" would false-red a `serial` run, which asks every slot to drain and
/// so reaches exactly one shape on purpose; what actually matters is that
/// asking to wait CHANGES the schedule that results.
pub fn assert_coverage() {
    // A run whose schedules never asked to wait cannot be evidence that waiting
    // fails to change the schedule — `burst` is exactly that run, by
    // construction. Judging it would turn the compatibility arm into a red.
    if !exercised().load(Ordering::Acquire) {
        return;
    }
    let observed: BTreeMap<Shape, usize> = observed()
        .lock()
        .expect("schedule signature registry")
        .clone();
    let total: usize = observed.values().sum();
    if total < COVERAGE_FLOOR {
        return;
    }
    assert!(
        observed.contains_key(&Shape::Interleaved),
        "[inv-schedule-coverage] {total} masked multi-intent transitions drew waiting slots, yet \
         every one of them still produced a {:?} schedule — only dispatch-all-then-settle is \
         reachable, so the schedule points are not suspending the driver and a green run over \
         them proves nothing about the other schedules.",
        observed.keys().collect::<Vec<_>>(),
    );
}

#[cfg(test)]
mod tests {
    use holon_api::EntityName;
    use holon_frontend::operations::OperationIntent;

    use super::*;

    fn intent() -> OperationIntent {
        OperationIntent::new(
            EntityName::from("block"),
            "set_field".to_string(),
            Default::default(),
        )
    }

    #[test]
    fn a_burst_of_dispatches_then_completions_reads_as_d_then_c() {
        let journal = DispatchJournal::new();
        let mut rec = Recording::start(&journal, "TypeChars");
        let seqs: Vec<u64> = (0..3).map(|_| journal.record(&intent())).collect();
        rec.sample(&journal);
        for seq in seqs {
            journal.settle(seq, Ok(()));
        }

        assert_eq!(rec.finish(&journal), "DDDCCC");
    }

    #[test]
    fn draining_between_dispatches_reads_as_alternating() {
        let journal = DispatchJournal::new();
        let mut rec = Recording::start(&journal, "TypeChars");
        for _ in 0..3 {
            let seq = journal.record(&intent());
            rec.sample(&journal);
            journal.settle(seq, Ok(()));
            rec.sample(&journal);
        }

        assert_eq!(rec.finish(&journal), "DCDCDC");
    }

    #[test]
    fn bursts_of_different_lengths_are_one_shape() {
        assert_eq!(Shape::of("DDDCCC"), Shape::Burst);
        assert_eq!(Shape::of("DDCC"), Shape::Burst);
        assert_eq!(Shape::of("DC"), Shape::Burst);
    }

    #[test]
    fn a_completion_before_the_last_dispatch_is_interleaved() {
        assert_eq!(Shape::of("DCDC"), Shape::Interleaved);
        assert_eq!(Shape::of("DDCDCC"), Shape::Interleaved);
    }

    /// Completions the settle delivers land after every dispatch, so a
    /// transition whose intents all settle late stays a burst however many
    /// trailing completions arrive.
    #[test]
    fn trailing_completions_do_not_make_a_burst_interleaved() {
        assert_eq!(Shape::of("DDDDCCCCCCC"), Shape::Burst);
    }

    /// A failed intent is a completion: the schedule is about when work
    /// finished, not whether it succeeded.
    #[test]
    fn a_failed_intent_completes_the_slot() {
        let journal = DispatchJournal::new();
        let mut rec = Recording::start(&journal, "TypeChars");
        let seq = journal.record(&intent());
        rec.sample(&journal);
        journal.settle(seq, Err("boom".to_string()));

        assert_eq!(rec.finish(&journal), "DC");
    }
}
