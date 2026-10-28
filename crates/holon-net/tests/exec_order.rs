//! Increment 1: producer order and run state are net constructions
//! (ADR 0032 §9 NET-run.a, REV11-running.a), not executor special cases.
//!
//! Twins: `HOLON_NET_TWIN=inhibitor-net` builds Example 3 with an inhibitor on
//! `Storage` instead of the `Storage.Open` / `Storage.Settled` complement;
//! `HOLON_NET_TWIN=hidden-reservation` flags a taken token instead of moving
//! it.

use std::collections::BTreeSet;

use exec_support::Driver;
use exec_support::Reads;
use exec_support::Spec;
use exec_support::Transition;
use exec_support::bind;
use exec_support::content;
use exec_support::int;
use exec_support::twin_name;
use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectResult;
use holon_net::exec::FireError;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Receiver;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

use crate::exec_support;

const APP: &str = "app";

fn gen_of(r: &mut dyn Reads, place: &str, s: &str) -> Option<Content> {
    let generation = r.field(place, s, "gen")?;
    Some(Content::from([("gen".to_string(), generation)]))
}

fn storage(backend: &str, s: &Subject, input: &Content) -> Vec<Output> {
    let generation = input["gen"].clone();
    let mut stored = input.clone();
    stored.insert("backend".to_string(), Value::String(backend.to_string()));
    vec![
        Output::new("Storage", s, stored),
        Output::new("Storage.Settled", s, content(&[("gen", generation)])),
    ]
}

/// Example 3: Turso first, Loro only through Turso's failure port.
fn complement_net() -> Spec {
    Spec::default()
        .place("Storage", Class::Process)
        .place("Storage.Open", Class::Process)
        .place("Storage.Settled", Class::Process)
        .place("TursoReady", Class::Process)
        .place("TursoFailed", Class::Process)
        .transition(
            Transition::new(
                "MakeTursoReady",
                Trigger::Auto,
                bind!(|r, s| gen_of(r, "Storage.Open", s)),
                Body::Effectful(Receiver::None),
            )
            .reads("Storage.Open")
            .inhibits("TursoReady")
            .inhibits("TursoFailed")
            .on_failure("TursoFailed"),
        )
        .transition(
            Transition::new(
                "FillFromTurso",
                Trigger::Auto,
                bind!(|r, s| gen_of(r, "Storage.Open", s)),
                Body::pure(|s, input| Ok(storage("turso", s, input))),
            )
            .consumes("TursoReady")
            .consumes("Storage.Open"),
        )
        .transition(
            Transition::new(
                "MakeLoroReady",
                Trigger::Auto,
                bind!(|r, s| gen_of(r, "Storage.Open", s)),
                Body::pure(|s, input| Ok(storage("loro", s, input))),
            )
            .consumes("TursoFailed")
            .consumes("Storage.Open"),
        )
        .transition(
            Transition::new(
                "Take",
                Trigger::Manual,
                bind!(|_r, _s| Some(Content::new())),
                Body::pure(|_, _| Ok(Vec::new())),
            )
            .consumes("Storage"),
        )
        .transition(
            Transition::new(
                "Reopen",
                Trigger::Manual,
                bind!(|r, s| gen_of(r, "Storage.Settled", s)),
                Body::pure(|s, input| {
                    let next = match input["gen"] {
                        Value::Integer(g) => g + 1,
                        ref other => panic!("gen is {other:?}"),
                    };
                    Ok(vec![Output::new(
                        "Storage.Open",
                        s,
                        content(&[("gen", Value::Integer(next))]),
                    )])
                }),
            )
            .consumes("Storage.Settled"),
        )
}

/// The naive twin of Example 3: "not yet produced" is an inhibitor on the
/// product, so a consumer that takes the product re-enables the producer.
fn inhibitor_net() -> Spec {
    let storage_gen = bind!(|r, s| gen_of(r, "Config", s));
    Spec::default()
        .place("Config", Class::Process)
        .place("Storage", Class::Process)
        .place("TursoReady", Class::Process)
        .place("TursoFailed", Class::Process)
        .transition(
            Transition::new(
                "MakeTursoReady",
                Trigger::Auto,
                storage_gen.clone(),
                Body::Effectful(Receiver::None),
            )
            .reads("Config")
            .inhibits("Storage")
            .inhibits("TursoReady")
            .inhibits("TursoFailed")
            .on_failure("TursoFailed"),
        )
        .transition(
            Transition::new(
                "FillFromTurso",
                Trigger::Auto,
                storage_gen.clone(),
                Body::pure(|s, input| Ok(storage("turso", s, input)[..1].to_vec())),
            )
            .consumes("TursoReady"),
        )
        .transition(
            Transition::new(
                "MakeLoroReady",
                Trigger::Auto,
                storage_gen,
                Body::pure(|s, input| Ok(storage("loro", s, input)[..1].to_vec())),
            )
            .consumes("TursoFailed"),
        )
        .transition(
            Transition::new(
                "Take",
                Trigger::Manual,
                bind!(|_r, _s| Some(Content::new())),
                Body::pure(|_, _| Ok(Vec::new())),
            )
            .consumes("Storage"),
        )
}

fn inhibitor_variant() -> bool {
    twin_name().as_deref() == Some("inhibitor-net")
}

#[derive(Clone, Debug)]
enum StorageOp {
    TursoSucceeds,
    TursoFails,
    Take,
    Reopen,
}

struct StorageRef;

impl ReferenceStateMachine for StorageRef {
    type State = ();
    type Transition = StorageOp;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<StorageOp> {
        prop_oneof![
            Just(StorageOp::TursoSucceeds),
            Just(StorageOp::TursoFails),
            Just(StorageOp::Take),
            Just(StorageOp::Reopen),
        ]
        .boxed()
    }

    fn apply(state: (), _: &StorageOp) {
        state
    }
}

struct StorageSut {
    d: Driver,
    fires: u64,
    /// Generations whose Turso attempt settled failed.
    turso_failed: BTreeSet<i64>,
}

impl StorageSut {
    fn fire(&mut self, transition: &str) {
        if inhibitor_variant() && transition == "Reopen" {
            return;
        }
        self.fires += 1;
        let occ = OccurrenceId::caller(&format!("op#{}", self.fires));
        match self.d.fire(transition, APP, occ) {
            Ok(_) | Err(FireError::NotEnabled { .. }) => {}
            Err(e) => panic!("fire {transition}: {e}"),
        }
    }
}

struct StorageTest;

impl StateMachineTest for StorageTest {
    type SystemUnderTest = StorageSut;
    type Reference = StorageRef;

    fn init_test(_: &()) -> StorageSut {
        let mut d = Driver::new(if inhibitor_variant() {
            inhibitor_net()
        } else {
            complement_net()
        });
        let opener = if inhibitor_variant() {
            "Config"
        } else {
            "Storage.Open"
        };
        d.write(opener, APP, content(&[("gen", Value::Integer(0))]))
            .expect("the net quiesces");
        StorageSut {
            d,
            fires: 0,
            turso_failed: BTreeSet::new(),
        }
    }

    fn apply(mut sut: StorageSut, _: &(), op: StorageOp) -> StorageSut {
        match op {
            StorageOp::TursoSucceeds => sut
                .d
                .complete_oldest(|req| {
                    EffectResult::Done(vec![Output::new(
                        "TursoReady",
                        &req.subject,
                        req.input.clone(),
                    )])
                })
                .expect("the net quiesces"),
            StorageOp::TursoFails => {
                if let Some(req) = sut.d.pending.front() {
                    sut.turso_failed.insert(int(&req.input, "gen"));
                }
                sut.d
                    .complete_oldest(|_| EffectResult::Failed(Cause::definite("turso down")))
                    .expect("the net quiesces")
            }
            StorageOp::Take => sut.fire("Take"),
            StorageOp::Reopen => sut.fire("Reopen"),
        }
        sut
    }

    fn check_invariants(sut: &StorageSut, _: &()) {
        let mut seen = BTreeSet::new();
        for req in sut.d.requests_of("MakeTursoReady") {
            let generation = int(&req.input, "gen");
            assert!(
                seen.insert(generation),
                "turso_fires_once_per_open: MakeTursoReady fired twice for generation \
                 {generation}; requests {:?}",
                sut.d.requests
            );
        }
        let tokens = sut.d.exec().tokens();
        if let Some(stored) = tokens.token("Storage", APP)
            && stored.content["backend"] == Value::String("loro".to_string())
        {
            let generation = int(&stored.content, "gen");
            assert!(
                sut.turso_failed.contains(&generation),
                "loro_only_after_turso_failed: Loro produced generation {generation} \
                 without a failed Turso attempt"
            );
        }
        if !inhibitor_variant() {
            let open = tokens.token("Storage.Open", APP).is_some();
            let settled = tokens.token("Storage.Settled", APP).is_some();
            assert!(
                open != settled,
                "open_xor_settled: Storage.Open {open}, Storage.Settled {settled}"
            );
        }
    }
}

prop_state_machine! {
    #[test]
    fn producer_order_is_a_net_construction(sequential 1..30 => StorageTest);
}

/// REV11-running: a take moves the token into `Send.Running`, so `Archive`
/// never sees a token a running firing holds.
fn running_net() -> Spec {
    let n = bind!(|r, s| {
        let n = r.field("Intent", s, "n")?;
        Some(Content::from([("n".to_string(), n)]))
    });
    Spec::default()
        .place("Intent", Class::Process)
        .place("Sent", Class::Process)
        .place("Archived", Class::Process)
        .transition(
            Transition::new(
                "Send",
                Trigger::Auto,
                n.clone(),
                Body::Effectful(Receiver::None),
            )
            .consumes("Intent"),
        )
        .transition(
            Transition::new(
                "Archive",
                Trigger::Auto,
                n,
                Body::pure(|s, input| Ok(vec![Output::new("Archived", s, input.clone())])),
            )
            .reads("Intent")
            .inhibits("Archived"),
        )
}

#[derive(Clone, Debug)]
enum IntentOp {
    Write(&'static str),
    SendSucceeds,
    SendFails,
}

struct IntentRef;

impl ReferenceStateMachine for IntentRef {
    type State = ();
    type Transition = IntentOp;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<IntentOp> {
        prop_oneof![
            prop::sample::select(SUBJECTS.to_vec()).prop_map(IntentOp::Write),
            Just(IntentOp::SendSucceeds),
            Just(IntentOp::SendFails),
        ]
        .boxed()
    }

    fn apply(state: (), _: &IntentOp) {
        state
    }
}

const SUBJECTS: [&str; 2] = ["a", "b"];

struct IntentSut {
    d: Driver,
    next_n: i64,
}

struct IntentTest;

impl StateMachineTest for IntentTest {
    type SystemUnderTest = IntentSut;
    type Reference = IntentRef;

    fn init_test(_: &()) -> IntentSut {
        IntentSut {
            d: Driver::new(running_net()),
            next_n: 0,
        }
    }

    fn apply(mut sut: IntentSut, _: &(), op: IntentOp) -> IntentSut {
        match op {
            IntentOp::Write(s) => {
                sut.next_n += 1;
                sut.d
                    .write("Intent", s, content(&[("n", Value::Integer(sut.next_n))]))
                    .expect("the net quiesces");
            }
            IntentOp::SendSucceeds => sut
                .d
                .complete_oldest(|req| {
                    EffectResult::Done(vec![Output::new("Sent", &req.subject, req.input.clone())])
                })
                .expect("the net quiesces"),
            IntentOp::SendFails => sut
                .d
                .complete_oldest(|_| EffectResult::Failed(Cause::definite("refused")))
                .expect("the net quiesces"),
        }
        sut
    }
}

prop_state_machine! {
    #[test]
    fn a_take_moves_the_token_into_running(sequential 1..20 => IntentTest);
}
