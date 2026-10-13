//! Increment 4: retry, run and abandon timers on an injected clock, with a
//! count bound per run (ADR 0032 §9 NET-run.a).
//!
//! Twin: `HOLON_NET_TWIN=no-count-bound` retries without the count bound.

mod exec_support;

use std::time::Duration;

use exec_support::Driver;
use exec_support::Spec;
use exec_support::Transition;
use exec_support::bind;
use exec_support::content;
use exec_support::text;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::EffectResult;
use holon_net::exec::Output;
use holon_net::exec::Receiver;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const APP: &str = "app";
const MAX_ATTEMPTS: u32 = 3;

/// Turso gets three attempts two seconds apart inside a 20 s run; its failure
/// and timeout ports both feed Loro.
fn net() -> Spec {
    Spec::default()
        .place("Want", Class::Process)
        .place("TursoReady", Class::Process)
        .place("TursoDown", Class::Process)
        .place("LoroReady", Class::Process)
        .transition(
            Transition::new(
                "MakeTursoReady",
                Trigger::Auto,
                bind!(|r, s| {
                    let url = r.text("Want", s, "url")?;
                    Some(content(&[("url", text(&url))]))
                }),
                Body::Effectful(Receiver::Keyed),
            )
            .reads("Want")
            .inhibits("TursoReady")
            .inhibits("TursoDown")
            .retry(Duration::from_secs(2), MAX_ATTEMPTS)
            .run_timer(Duration::from_secs(20))
            .abandon(Duration::from_secs(3))
            .on_failure("TursoDown")
            .on_timeout("TursoDown"),
        )
        .transition(
            Transition::new(
                "MakeLoroReady",
                Trigger::Auto,
                bind!(|_r, _s| Some(content(&[]))),
                Body::pure(|s, _| Ok(vec![Output::new("LoroReady", s, content(&[]))])),
            )
            .consumes("TursoDown"),
        )
}

#[derive(Clone, Debug)]
enum Op {
    Tick(u64),
    Done,
    FailRetryable,
    Unknown,
}

struct Ref;

impl ReferenceStateMachine for Ref {
    type State = ();
    type Transition = Op;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<Op> {
        prop_oneof![
            (1u64..5).prop_map(Op::Tick),
            Just(Op::Done),
            Just(Op::FailRetryable),
            Just(Op::Unknown),
        ]
        .boxed()
    }

    fn apply(state: (), _: &Op) {
        state
    }
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Driver;
    type Reference = Ref;

    fn init_test(_: &()) -> Driver {
        let mut d = Driver::new(net());
        d.write("Want", APP, content(&[("url", text("libsql://local"))]));
        d.step().expect("the net quiesces");
        d
    }

    fn apply(mut d: Driver, _: &(), op: Op) -> Driver {
        match op {
            Op::Tick(secs) => d.tick(secs),
            Op::Done => d.complete_oldest(|req| {
                EffectResult::Done(vec![Output::new("TursoReady", &req.subject, content(&[]))])
            }),
            Op::FailRetryable => {
                d.complete_oldest(|_| EffectResult::Failed(Cause::retryable("connection refused")))
            }
            Op::Unknown => {
                d.complete_oldest(|_| EffectResult::Unknown(Cause::definite("connection reset")))
            }
        }
        .expect("the net quiesces");
        d
    }

    fn check_invariants(d: &Driver, _: &()) {
        let tokens = d.exec().tokens();
        assert!(
            tokens.token("TursoReady", APP).is_none() || tokens.token("LoroReady", APP).is_none(),
            "one_backend: both TursoReady and LoroReady hold a token"
        );
    }
}

prop_state_machine! {
    #[test]
    fn retries_stay_within_the_count_bound(sequential 1..40 => Test);
}

/// A run spans Open → Settled (NET-run.a): a firing while the run is
/// unsettled is an attempt of the same run, under the same run timer.
#[test]
fn a_firing_in_an_unsettled_run_shares_its_timer() {
    let net = Spec::default()
        .place("Doc", Class::Process)
        .place("TimedOut", Class::Process)
        .transition(
            Transition::new(
                "Publish",
                Trigger::Auto,
                bind!(|r, s| {
                    let title = r.text("Doc", s, "title")?;
                    Some(content(&[("title", text(&title))]))
                }),
                Body::Effectful(Receiver::None),
            )
            .reads("Doc")
            .run_timer(Duration::from_secs(20))
            .on_timeout("TimedOut"),
        );
    let mut d = Driver::new(net);
    d.write("Doc", APP, content(&[("title", text("v1"))]));
    d.step().expect("the net quiesces");
    d.complete_oldest(|_| EffectResult::Unknown(Cause::definite("connection reset")))
        .expect("the net quiesces");
    d.tick(10).expect("the net quiesces");
    d.write("Doc", APP, content(&[("title", text("v2"))]));
    d.step().expect("the net quiesces");
    assert_eq!(
        d.requests_of("Publish").count(),
        2,
        "v2 fires a second attempt"
    );

    d.tick(11).expect("the net quiesces");
    assert!(
        d.exec().tokens().token("TimedOut", APP).is_some(),
        "run_spans_its_attempts: the run timer of the first firing did not fire at 21 s"
    );
}
