//! Reader isolation during a take: T1 reads the row P0 that the taker T0
//! consumes. A reader that read through an open take commits only after the
//! take commits or fails, and it is stale when the take fails and puts its
//! held token back: it decided on a view that never committed. While the take
//! is parked, a condition discloses the wait.
//!
//! The net is drawn per case (P0's capacity, whether T0 exists, what T1
//! reads, T1's body, a fact of T1 into P0), so the generator draws a reader
//! beside a taker in most cases; the shared model runs on every operation.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::time::Duration;

use exec_support::Driver;
use exec_support::Spec;
use exec_support::Transition;
use exec_support::bind;
use exec_support::content;
use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Capacity;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Condition;
use holon_net::exec::Content;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::FireError;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Receiver;
use holon_net::exec::Resolution;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

use crate::exec_support;

const S: &str = "s";
const BACKOFF: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ReadKind {
    /// T1's bind reads the field `x` of P0.
    Field,
    /// T1's bind reads whether T0 runs.
    TakerRunning,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ReaderBody {
    Pure,
    Keyed,
    StateProbe,
}

#[derive(Clone, Debug)]
struct Net {
    p0: Capacity,
    taker: bool,
    read: ReadKind,
    body: ReaderBody,
    /// T1 puts a fact into P0; only with [`ReadKind::Field`], so the put
    /// compares and sets against the row T1 read.
    fact: bool,
    /// A second taker of P0, fired by hand, so a token can stay in P0 for
    /// T1 to read before T2 takes it.
    t2: bool,
}

const READER: Net = Net {
    p0: Capacity::Single,
    taker: true,
    read: ReadKind::Field,
    body: ReaderBody::Keyed,
    fact: false,
    t2: false,
};

fn x_of_p0(r: &mut dyn exec_support::Reads, s: &Subject) -> Option<Content> {
    Some(
        r.field("P0", s, "x")
            .map(|x| content(&[("x", x)]))
            .unwrap_or_default(),
    )
}

fn taker_runs(r: &mut dyn exec_support::Reads, s: &Subject) -> Option<Content> {
    Some(content(&[(
        "running",
        Value::Boolean(r.present("T0.Running", s)),
    )]))
}

fn fact_output(s: &Subject) -> Output {
    Output::new("P0", s, content(&[("x", Value::Boolean(true))]))
}

impl Net {
    fn spec(&self) -> Spec {
        let p1 = Spec::default().place("P1", Class::Process);
        let spec = match self.p0 {
            Capacity::Single => p1.place("P0", Class::Process),
            Capacity::Register => p1.register("P0", Class::Process),
            Capacity::Fifo => p1.fifo("P0", Class::Process),
        };
        let spec = if self.taker {
            spec.transition(
                Transition::new(
                    "T0",
                    Trigger::Auto,
                    bind!(|_r, _s| Some(Content::new())),
                    Body::Effectful(Receiver::Keyed),
                )
                .consumes("P0")
                .retry(Duration::from_secs(BACKOFF), 2),
            )
        } else {
            spec
        };
        let spec = if self.t2 {
            spec.transition(
                Transition::new(
                    "T2",
                    Trigger::Manual,
                    bind!(|_r, _s| Some(Content::new())),
                    Body::Effectful(Receiver::Keyed),
                )
                .consumes("P0")
                .retry(Duration::from_secs(BACKOFF), 2),
            )
        } else {
            spec
        };
        let fact = self.fact;
        let body = match self.body {
            ReaderBody::Pure => {
                Body::pure(move |s, _| Ok(if fact { vec![fact_output(s)] } else { vec![] }))
            }
            ReaderBody::Keyed => Body::Effectful(Receiver::Keyed),
            ReaderBody::StateProbe => Body::Effectful(Receiver::StateProbe),
        };
        // No read arc: an arc needs the token present, and the reads under
        // test include the absent row.
        let bind = match self.read {
            ReadKind::Field => bind!(x_of_p0),
            ReadKind::TakerRunning => bind!(taker_runs),
        };
        let t1 = Transition::new("T1", Trigger::Auto, bind, body).consumes("P1");
        let t1 = if self.body == ReaderBody::Keyed {
            t1.retry(Duration::from_secs(BACKOFF), 2)
        } else {
            t1
        };
        spec.transition(if self.fact { t1.fact("P0") } else { t1 })
    }

    fn outputs(&self, transition: &str) -> Vec<Output> {
        if transition == "T1" && self.fact {
            vec![fact_output(&Subject::new(S))]
        } else {
            vec![]
        }
    }
}

fn net() -> impl Strategy<Value = Net> {
    (
        prop::sample::select(vec![Capacity::Single, Capacity::Register, Capacity::Fifo]),
        any::<bool>(),
        prop::sample::select(vec![ReadKind::Field, ReadKind::TakerRunning]),
        prop::sample::select(vec![
            ReaderBody::Pure,
            ReaderBody::Keyed,
            ReaderBody::StateProbe,
        ]),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(p0, taker, read, body, fact, t2)| {
            let read = if taker { read } else { ReadKind::Field };
            Net {
                p0,
                taker,
                read,
                body,
                fact: fact && read == ReadKind::Field,
                t2,
            }
        })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Who {
    T0,
    T1,
    T2,
}

impl Who {
    fn name(self) -> &'static str {
        match self {
            Who::T0 => "T0",
            Who::T1 => "T1",
            Who::T2 => "T2",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Res {
    Done,
    Failed { retryable: bool },
    Unknown,
}

#[derive(Clone, Copy, Debug)]
enum How {
    Verify,
    Retry,
    Drop,
}

#[derive(Clone, Debug)]
enum Op {
    /// An outside write; `x: None` writes a token without `x`.
    Write { p0: bool, x: Option<bool> },
    /// Completes the oldest pending attempt of `who`; no-op when none.
    Complete { who: Who, res: Res },
    /// Completes an attempt the runner already got (answered or not) with
    /// Done.
    Duplicate { nth: prop::sample::Index },
    /// Moves past the retry backoff and fires the due timers.
    Step,
    /// Resolves the oldest parked occurrence of `who`; no-op when none.
    Resolve { who: Who, how: How },
    /// Fires T2 by hand; no-op when the net has no T2 or it is not enabled.
    FireT2,
}

use Op::Complete;
use Op::Write;

fn op() -> impl Strategy<Value = Op> {
    let who = prop::sample::select(vec![Who::T0, Who::T1, Who::T2]);
    let res = prop_oneof![
        Just(Res::Done),
        Just(Res::Failed { retryable: true }),
        Just(Res::Failed { retryable: false }),
        Just(Res::Unknown),
    ];
    let how = prop::sample::select(vec![How::Verify, How::Retry, How::Drop]);
    prop_oneof![
        3 => (any::<bool>(), prop::option::of(any::<bool>()))
            .prop_map(|(p0, x)| Op::Write { p0, x }),
        4 => (who.clone(), res).prop_map(|(who, res)| Op::Complete { who, res }),
        1 => any::<prop::sample::Index>().prop_map(|nth| Op::Duplicate { nth }),
        1 => Just(Op::Step),
        1 => (who, how).prop_map(|(who, how)| Op::Resolve { who, how }),
        1 => Just(Op::FireT2),
    ]
}

/// What the reader saw at its bind, by token identity: the P0 tokens that
/// had already left the row. Ids are never reused, so one of them back in
/// P0 is a restore of a take that was open at the bind.
struct Ghost {
    missed: BTreeSet<u64>,
    restored: bool,
}

struct Sut {
    d: Driver,
    net: Net,
    seen: BTreeSet<u64>,
    readers: BTreeMap<OccurrenceId, Ghost>,
    fired_t2: usize,
}

fn is_t1(o: &OccurrenceId) -> bool {
    matches!(o, OccurrenceId::Minted { transition, .. } if &**transition == "T1")
}

fn of(o: &OccurrenceId, who: Who) -> bool {
    match o {
        OccurrenceId::Minted { transition, .. } => &**transition == who.name(),
        OccurrenceId::Caller(id) => who == Who::T2 && id.starts_with("t2-"),
        _ => false,
    }
}

fn quiesced<T>(r: Result<T, ExecError>) {
    if let Err(ExecError::NoQuiescence(rounds)) = r {
        panic!("the net is still enabled after {rounds} rounds");
    }
}

impl Sut {
    fn new(net: Net) -> Self {
        Self {
            d: Driver::new(net.spec()),
            net,
            seen: BTreeSet::new(),
            readers: BTreeMap::new(),
            fired_t2: 0,
        }
    }

    fn p0_ids(&self) -> BTreeSet<u64> {
        self.d.exec().tokens().row("P0", S).map(|t| t.id).collect()
    }

    fn settled(&self, o: &OccurrenceId) -> Option<Settled> {
        self.d.exec().settled(o).expect("a known occurrence")
    }

    fn apply(&mut self, op: &Op) {
        match op {
            Op::Write { p0, x } => {
                let content = x.map_or_else(Content::new, |x| content(&[("x", Value::Boolean(x))]));
                quiesced(self.d.write(if *p0 { "P0" } else { "P1" }, S, content));
            }
            Op::Complete { who, res } => {
                if let Some(i) = self
                    .d
                    .pending
                    .iter()
                    .position(|r| &*r.transition == who.name())
                {
                    let req = self.d.pending.remove(i).expect("found above");
                    let result = match res {
                        Res::Done => EffectResult::Done(self.net.outputs(who.name())),
                        Res::Failed { retryable: true } => {
                            EffectResult::Failed(Cause::retryable("boom"))
                        }
                        Res::Failed { retryable: false } => {
                            EffectResult::Failed(Cause::definite("boom"))
                        }
                        Res::Unknown => EffectResult::Unknown(Cause::definite("reset")),
                    };
                    quiesced(self.d.complete(req.attempt, result));
                }
            }
            Op::Duplicate { nth } => {
                if !self.d.requests.is_empty() {
                    let req = self.d.requests[nth.index(self.d.requests.len())].clone();
                    self.d.pending.retain(|r| r.attempt != req.attempt);
                    let outputs = self.net.outputs(&req.transition);
                    quiesced(self.d.complete(req.attempt, EffectResult::Done(outputs)));
                }
            }
            Op::Step => quiesced(self.d.tick(BACKOFF + 1)),
            Op::FireT2 if self.net.t2 => {
                self.fired_t2 += 1;
                let o = OccurrenceId::caller(&format!("t2-{}", self.fired_t2));
                match self.d.fire("T2", S, o) {
                    Ok(_) | Err(FireError::NotEnabled { .. }) => {}
                    Err(e) => panic!("T2 fire: {e}"),
                }
            }
            Op::FireT2 => {}
            Op::Resolve { who, how } => {
                let parked = self
                    .d
                    .exec()
                    .occurrences()
                    .find(|(o, s)| of(o, *who) && matches!(s, Some(Settled::OutcomeUnknown(_))))
                    .map(|(o, _)| o.clone());
                if let Some(o) = parked {
                    let resolution = match how {
                        How::Verify => Resolution::Verify(self.net.outputs(who.name())),
                        How::Retry => Resolution::Retry,
                        How::Drop => Resolution::Drop,
                    };
                    self.resolve(&o, resolution);
                }
            }
        }
        self.observe();
    }

    /// Resolves `o` and checks that exactly its wait conditions go; a
    /// refused retry (a later run of the same transition holds `Running`)
    /// resolves nothing, so every condition stays.
    fn resolve(&mut self, o: &OccurrenceId, resolution: Resolution) {
        let before = self.d.exec().conditions().to_vec();
        let resolved = match self.d.resolve(o, resolution) {
            Ok(()) => true,
            Err(ExecError::RetryWhileRunning { .. }) => false,
            Err(e) => panic!("resolve of {o}: {e}"),
        };
        let after = self.d.exec().conditions();
        for c in &before {
            let this_wait = resolved
                && matches!(c, Condition::WaitsOnParkedTake { take_occurrence, .. } if take_occurrence == o);
            assert_eq!(
                after.contains(c),
                !this_wait,
                "resolve of {o}: {c} before, conditions after {after:?}"
            );
        }
    }

    /// One operation never both restores a P0 token and records T1: a
    /// restore comes from a settle of T0, a record of T1 from a settle of T1.
    fn observe(&mut self) {
        if self.net.read != ReadKind::Field {
            return;
        }
        let now = self.p0_ids();
        self.seen.extend(now.iter().copied());
        let t1: Vec<OccurrenceId> = self
            .d
            .exec()
            .occurrences()
            .filter(|(o, _)| is_t1(o))
            .map(|(o, _)| o.clone())
            .collect();
        for o in t1 {
            let open = matches!(self.settled(&o), None | Some(Settled::OutcomeUnknown(_)));
            let missed = &self.seen - &now;
            let ghost = self.readers.entry(o).or_insert(Ghost {
                missed,
                restored: false,
            });
            if open && ghost.missed.iter().any(|id| now.contains(id)) {
                ghost.restored = true;
            }
        }
    }

    /// P-restore: a reader whose read row got back a token that was taken
    /// at its bind, before it recorded, does not settle Done.
    fn check(&self) {
        for (o, g) in &self.readers {
            assert!(
                !(g.restored && self.settled(o) == Some(Settled::Done)),
                "P-restore: {o} settled Done although a failed take put back a P0 token \
                 it missed at its bind ({:?}); net {:?}",
                g.missed,
                self.net
            );
        }
    }

    fn t1_held(&self) -> bool {
        self.d.exec().held().iter().any(|(o, _)| is_t1(o))
    }

    /// The verdict of T1's first occurrence.
    fn verdict(&self) -> Verdict {
        let (o, s) = self
            .d
            .exec()
            .occurrences()
            .find(|(o, _)| is_t1(o))
            .unwrap_or_else(|| panic!("T1 never fired: {:?}", self.d.exec().why_not("T1", S)));
        match s {
            Some(Settled::Done) => Verdict::NotStale,
            Some(Settled::OverChangedInputs { .. }) => Verdict::Stale,
            Some(Settled::Failed(c)) if c.text.starts_with("stale inputs") => Verdict::Stale,
            other => panic!(
                "{o} did not record: {other:?}; occurrences {:?}",
                self.d.exec().occurrences().collect::<Vec<_>>()
            ),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Stale,
    NotStale,
}

struct Ref;

impl ReferenceStateMachine for Ref {
    type State = Net;
    type Transition = Op;

    fn init_state() -> BoxedStrategy<Net> {
        net().boxed()
    }

    fn transitions(_: &Net) -> BoxedStrategy<Op> {
        op().boxed()
    }

    fn apply(state: Net, _: &Op) -> Net {
        state
    }
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(net: &Net) -> Sut {
        Sut::new(net.clone())
    }

    fn apply(mut sut: Sut, _: &Net, op: Op) -> Sut {
        sut.apply(&op);
        sut
    }

    fn check_invariants(sut: &Sut, _: &Net) {
        sut.check();
    }
}

prop_state_machine! {
    // The default 256 cases often miss "bind through a take, the take
    // fails, the reader records".
    #![proptest_config(ProptestConfig {
        cases: ProptestConfig::default().cases.max(2048),
        .. ProptestConfig::default()
    })]
    #[test]
    fn a_read_through_a_failed_take_is_stale(sequential 1..24 => Test);
}

fn w(p0: bool) -> Op {
    Write { p0, x: None }
}

fn wx(p0: bool, x: bool) -> Op {
    Write { p0, x: Some(x) }
}

fn done(who: Who) -> Op {
    Complete {
        who,
        res: Res::Done,
    }
}

fn fails(who: Who, retryable: bool) -> Op {
    Complete {
        who,
        res: Res::Failed { retryable },
    }
}

/// A static example: the ops on the net, then the verdict of T1's first
/// occurrence and P-restore.
fn case(net: Net, ops: &[Op], expected: Verdict) {
    let mut sut = Sut::new(net);
    for op in ops {
        sut.apply(op);
    }
    assert_eq!(sut.verdict(), expected, "T1's verdict after {ops:?}");
    sut.check();
}

/// T1 reads P0 while T0 holds k1 and an outside write put k2 there; T0
/// commits k1 and its next run takes k2; then T1 records.
fn g_ops() -> Vec<Op> {
    vec![
        w(true),
        wx(true, false),
        w(false),
        done(Who::T0),
        done(Who::T1),
    ]
}

/// T1 reads P1 and P0 in order: T0 takes before T1 binds (`read_while`) or
/// T1 reads the token back from T0's retryable failure before the retry
/// takes it again.
fn beside(read_while: bool, t0: Res) -> Vec<Op> {
    let settle = Complete {
        who: Who::T0,
        res: t0,
    };
    if read_while {
        vec![w(true), w(false), settle, done(Who::T1)]
    } else {
        vec![
            w(true),
            fails(Who::T0, true),
            w(false),
            Op::Step,
            settle,
            done(Who::T1),
        ]
    }
}

const FIFO_READER: Net = Net {
    p0: Capacity::Fifo,
    taker: false,
    ..READER
};

#[test]
fn held_token_superseded_by_a_write() {
    let mut ops = g_ops();
    ops.insert(ops.len() - 1, wx(true, true));
    case(READER, &ops, Verdict::Stale);
}

#[test]
fn absent_read_then_written_and_taken() {
    case(READER, &[w(false), w(true), done(Who::T1)], Verdict::Stale);
}

/// T1's effect ends before T0 fails: T1 waits, then goes stale.
#[test]
fn dirty_absent_read_records_before_the_taker_fails() {
    let mut sut = Sut::new(READER);
    for op in [w(true), w(false), done(Who::T1)] {
        sut.apply(&op);
    }
    assert!(sut.t1_held(), "T1 waits while T0 holds the token");
    sut.apply(&fails(Who::T0, false));
    assert!(!sut.t1_held());
    assert_eq!(sut.verdict(), Verdict::Stale);
    sut.check();
}

/// A2 with a fact of T1 into the Single P0: T1 goes stale by the restore,
/// and its fact does not land over the restored token.
#[test]
fn a_stale_reader_puts_no_fact_over_the_restored_token() {
    let net = Net {
        fact: true,
        ..READER
    };
    let mut sut = Sut::new(net);
    for op in beside(true, Res::Failed { retryable: false }) {
        sut.apply(&op);
    }
    assert_eq!(sut.verdict(), Verdict::Stale);
    let p0 = sut
        .d
        .exec()
        .tokens()
        .token("P0", S)
        .expect("T0's token is back");
    assert_eq!(
        p0.record(),
        &Content::new(),
        "the restored token, not T1's fact"
    );
    sut.check();
}

/// T0 takes from two rows that T1 reads as holes, then fails: both tokens go
/// back, and T1's facts land on neither restored token. The drawn net has
/// one taken row, so this shape is a static example of its own.
#[test]
fn a_stale_reader_puts_no_fact_over_either_restored_token() {
    let x = || content(&[("x", Value::Boolean(true))]);
    let spec = Spec::default()
        .place("P1", Class::Process)
        .place("Q0", Class::Process)
        .place("Q1", Class::Process)
        .transition(
            Transition::new(
                "T0",
                Trigger::Auto,
                bind!(|_r, _s| Some(Content::new())),
                Body::Effectful(Receiver::None),
            )
            .consumes("Q0")
            .consumes("Q1"),
        )
        .transition(
            Transition::new(
                "T1",
                Trigger::Auto,
                bind!(|r, s| {
                    let q0 = r.field("Q0", s, "x");
                    let q1 = r.field("Q1", s, "x");
                    (q0.is_none() && q1.is_none()).then(Content::new)
                }),
                Body::Effectful(Receiver::None),
            )
            .consumes("P1")
            .fact("Q0")
            .fact("Q1"),
        );
    let mut d = Driver::new(spec);
    for p in ["Q0", "Q1", "P1"] {
        d.write(p, S, Content::new()).expect("the net quiesces");
    }
    let t1 = d
        .requests_of("T1")
        .next()
        .expect("T1 fired while T0 holds both tokens")
        .attempt;
    let s = Subject::new(S);
    d.complete(
        t1,
        EffectResult::Done(vec![Output::new("Q0", &s, x()), Output::new("Q1", &s, x())]),
    )
    .expect("the net quiesces");
    let t0 = d.requests_of("T0").next().expect("T0 runs").attempt;
    d.complete(t0, EffectResult::Failed(Cause::definite("rejected")))
        .expect("the net quiesces");
    for p in ["Q0", "Q1"] {
        let token = d.exec().tokens().token(p, S).expect("T0's token is back");
        assert_eq!(
            token.record(),
            &Content::new(),
            "{p}: the restored token, not T1's fact"
        );
    }
}

/// T0 holds k1, k2 is written, T1 reads k2, then T2 takes k2. T0 fails:
/// k1 is older than k2, which T2 holds, so k1 does not come back. T2's
/// commit empties P0, so T1's read of k2 is stale.
#[test]
fn superseded_token_stays_out_when_a_later_take_holds_the_newer_one() {
    let net = Net { t2: true, ..READER };
    let mut sut = Sut::new(net);
    for op in [
        w(true),
        w(true),
        w(false),
        Op::FireT2,
        fails(Who::T0, false),
        done(Who::T2),
        done(Who::T1),
    ] {
        sut.apply(&op);
    }
    assert_eq!(
        sut.p0_ids(),
        BTreeSet::new(),
        "k1 came back after k2 was consumed"
    );
    assert_eq!(sut.verdict(), Verdict::Stale);
    sut.check();
}

/// T0 holds k1, k2 is written and T2 takes it; T1 reads the hole and
/// waits on T2 only, since k1 can never come back. T2 commits first, then
/// T0 fails: k1 stays out, and T1's read of the empty row is true.
#[test]
fn superseded_token_stays_out_after_the_newer_one_is_consumed() {
    let net = Net {
        t2: true,
        body: ReaderBody::Pure,
        ..READER
    };
    let mut sut = Sut::new(net);
    for op in [
        w(true),
        w(true),
        Op::FireT2,
        w(false),
        done(Who::T2),
        fails(Who::T0, false),
    ] {
        sut.apply(&op);
    }
    assert_eq!(
        sut.p0_ids(),
        BTreeSet::new(),
        "k1 came back after k2 was consumed"
    );
    assert_eq!(sut.verdict(), Verdict::NotStale);
    sut.check();
}

/// T1's facts fill the FIFO P0; T2 takes one and parks, a later T2 run
/// takes the next, so a retry of the parked T2 is refused and the wait of
/// T1 on the parked take stays disclosed.
#[test]
fn a_refused_retry_keeps_the_wait_condition() {
    let net = Net {
        p0: Capacity::Fifo,
        taker: false,
        body: ReaderBody::Pure,
        fact: true,
        t2: true,
        ..READER
    };
    let mut sut = Sut::new(net);
    for op in [
        w(false),
        w(false),
        Op::FireT2,
        w(false),
        Complete {
            who: Who::T2,
            res: Res::Unknown,
        },
        fails(Who::T2, true),
        Op::FireT2,
        Op::Resolve {
            who: Who::T2,
            how: How::Retry,
        },
    ] {
        sut.apply(&op);
    }
    assert!(sut.t1_held(), "T1 still waits on the parked take");
    sut.check();
}

/// T1 reads k2, written while T0 holds k1: a failed T0 drops k1 for k2, so
/// T1's read cannot turn false and T1 does not wait for T0.
#[test]
fn read_of_a_token_written_during_a_take() {
    case(READER, &g_ops(), Verdict::NotStale);
    let mut sut = Sut::new(READER);
    for op in [w(true), wx(true, false), w(false), done(Who::T1)] {
        sut.apply(&op);
        assert!(
            !sut.t1_held(),
            "T1 waits on a take that cannot change its read"
        );
    }
    assert_eq!(sut.verdict(), Verdict::NotStale);
    sut.check();
}

#[test]
fn run_state_read_then_the_run_starts() {
    let net = Net {
        read: ReadKind::TakerRunning,
        ..READER
    };
    case(net, &[w(false), w(true), done(Who::T1)], Verdict::NotStale);
}

/// Conservative: an append behind the head T1 read moves the version.
#[test]
fn fact_into_fifo_appended_behind_head() {
    let net = Net {
        fact: true,
        ..FIFO_READER
    };
    case(
        net,
        &[w(true), w(false), wx(true, true), done(Who::T1)],
        Verdict::Stale,
    );
}

/// Conservative: an append behind the head T1 read moves the version.
#[test]
fn fifo_append_behind_the_read_head() {
    case(
        FIFO_READER,
        &[w(true), w(false), wx(true, true), done(Who::T1)],
        Verdict::Stale,
    );
}

#[test]
fn read_then_taken_taker_runs() {
    case(READER, &beside(false, Res::Unknown), Verdict::NotStale);
}

#[test]
fn read_then_taken_taker_done() {
    case(READER, &beside(false, Res::Done), Verdict::Stale);
}

#[test]
fn read_then_taken_taker_fails() {
    case(
        READER,
        &beside(false, Res::Failed { retryable: false }),
        Verdict::NotStale,
    );
}

#[test]
fn read_while_taken_taker_done() {
    case(READER, &beside(true, Res::Done), Verdict::NotStale);
}

#[test]
fn read_while_taken_taker_fails() {
    case(
        READER,
        &beside(true, Res::Failed { retryable: false }),
        Verdict::Stale,
    );
}

#[test]
fn read_while_taken_taker_retries() {
    case(
        READER,
        &beside(true, Res::Failed { retryable: true }),
        Verdict::Stale,
    );
}

/// T0 holds the FIFO front k0; T1 reads the next token k1 as the front;
/// T0 fails and k0 is the front again, so T1's present read is stale.
#[test]
fn present_fifo_read_whose_held_front_goes_back() {
    let net = Net {
        p0: Capacity::Fifo,
        ..READER
    };
    case(
        net,
        &[
            w(true),
            wx(true, true),
            w(false),
            fails(Who::T0, false),
            done(Who::T1),
        ],
        Verdict::Stale,
    );
}

#[derive(Clone, Copy, Debug)]
enum Resolve {
    Verify,
    Drop,
    /// Retry, then the retried attempt fails definitely: the earlier attempts
    /// may have taken effect, so the take parks again.
    RetryThenFails,
}

/// T0 takes P0 for `s`, T1 binds the hole, T0's two attempts end unknown so
/// T0 parks; T1's effect ends Done and T1 waits, disclosed. The same happens
/// for the subject `s2`, whose wait must outlive the resolve of `s`.
fn reader_waits_on_a_parked_take(resolve: Resolve) {
    let mut sut = Sut::new(READER);
    for s in [S, "s2"] {
        for p in ["P0", "P1"] {
            quiesced(sut.d.write(p, s, Content::new()));
        }
    }
    for _ in 0..2 {
        for _ in [S, "s2"] {
            sut.apply(&Complete {
                who: Who::T0,
                res: Res::Unknown,
            });
        }
    }
    sut.apply(&done(Who::T1));
    sut.apply(&done(Who::T1));
    let waits = |sut: &Sut, s: &str| {
        sut.d.exec().conditions().iter().any(|c| {
            matches!(c, Condition::WaitsOnParkedTake { reader, subject, take, .. }
                if &**reader == "T1" && subject.as_str() == s && &**take == "T0")
        })
    };
    assert!(
        waits(&sut, S) && waits(&sut, "s2"),
        "{:?}",
        sut.d.exec().conditions()
    );
    assert_eq!(sut.d.exec().held().len(), 2, "both readers wait");
    let take = sut
        .d
        .exec()
        .occurrences()
        .find(|(o, s)| of(o, Who::T0) && matches!(s, Some(Settled::OutcomeUnknown(_))))
        .map(|(o, _)| o.clone())
        .expect("T0 parked");
    let take_subject = sut
        .d
        .requests
        .iter()
        .find(|r| r.occurrence == take)
        .expect("T0 ran")
        .subject
        .clone();
    let other = if take_subject.as_str() == S { "s2" } else { S };
    let reader_of = |sut: &Sut, s: &str| {
        sut.d
            .requests
            .iter()
            .find(|r| &*r.transition == "T1" && r.subject.as_str() == s)
            .expect("T1 ran")
            .occurrence
            .clone()
    };
    let reader = reader_of(&sut, take_subject.as_str());
    let settled = |sut: &Sut| sut.settled(&reader);
    match resolve {
        Resolve::Verify => {
            sut.resolve(&take, Resolution::Verify(vec![]));
            assert_eq!(settled(&sut), Some(Settled::Done));
        }
        Resolve::Drop => {
            sut.resolve(&take, Resolution::Drop);
            assert_eq!(settled(&sut), Some(Settled::Done));
        }
        Resolve::RetryThenFails => {
            sut.resolve(&take, Resolution::Retry);
            assert!(!waits(&sut, take_subject.as_str()));
            assert_eq!(
                settled(&sut),
                None,
                "the retried take is open: T1 still waits"
            );
            let retried = sut
                .d
                .pending
                .iter()
                .position(|r| r.occurrence == take)
                .expect("the retry runs");
            let req = sut.d.pending.remove(retried).expect("found above");
            quiesced(
                sut.d
                    .complete(req.attempt, EffectResult::Failed(Cause::definite("boom"))),
            );
            assert!(
                waits(&sut, take_subject.as_str()),
                "parked again, disclosed again"
            );
            assert_eq!(settled(&sut), None, "T1 waits on the parked take");
            sut.resolve(&take, Resolution::Drop);
            assert_eq!(settled(&sut), Some(Settled::Done));
        }
    }
    assert!(!waits(&sut, take_subject.as_str()));
    assert!(waits(&sut, other), "the wait of the other subject stays");
    assert_eq!(sut.settled(&reader_of(&sut, other)), None);
    sut.check();
}

#[test]
fn reader_waits_on_a_parked_take_verify() {
    reader_waits_on_a_parked_take(Resolve::Verify);
}

#[test]
fn reader_waits_on_a_parked_take_drop() {
    reader_waits_on_a_parked_take(Resolve::Drop);
}

#[test]
fn reader_waits_on_a_parked_take_retry_then_fails() {
    reader_waits_on_a_parked_take(Resolve::RetryThenFails);
}
