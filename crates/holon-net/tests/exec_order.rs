//! Increment 1: producer order and run state are net constructions
//! (ADR 0032 §9 NET-run.a, REV11-running.a), not executor special cases.
//!
//! Twins: `HOLON_NET_TWIN=inhibitor-net` builds Example 3 with an inhibitor on
//! `Storage` instead of the `Storage.Open` / `Storage.Settled` complement;
//! `HOLON_NET_TWIN=hidden-reservation` flags a taken token instead of moving
//! it.

mod exec_support;

use std::collections::BTreeSet;

use exec_support::Driver;
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
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const APP: &str = "app";

fn gen_of(r: &mut Reader<'_>, place: &str, s: &Subject) -> Option<Content> {
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
fn complement_net() -> NetDef {
    NetDef::default()
        .place("Storage", Class::Process)
        .place("Storage.Open", Class::Process)
        .place("Storage.Settled", Class::Process)
        .place("TursoReady", Class::Process)
        .place("TursoFailed", Class::Process)
        .transition(
            TransitionDef::new(
                "MakeTursoReady",
                Trigger::Auto,
                |r, s| gen_of(r, "Storage.Open", s),
                Body::Effectful(Receiver::None),
            )
            .reads("Storage.Open")
            .inhibits("TursoReady")
            .inhibits("TursoFailed")
            .on_failure("TursoFailed"),
        )
        .transition(
            TransitionDef::new(
                "FillFromTurso",
                Trigger::Auto,
                |r, s| gen_of(r, "Storage.Open", s),
                Body::Pure(|s, input| Ok(storage("turso", s, input))),
            )
            .consumes("TursoReady")
            .consumes("Storage.Open"),
        )
        .transition(
            TransitionDef::new(
                "MakeLoroReady",
                Trigger::Auto,
                |r, s| gen_of(r, "Storage.Open", s),
                Body::Pure(|s, input| Ok(storage("loro", s, input))),
            )
            .consumes("TursoFailed")
            .consumes("Storage.Open"),
        )
        .transition(
            TransitionDef::new(
                "Take",
                Trigger::Manual,
                |_, _| Some(Content::new()),
                Body::Pure(|_, _| Ok(Vec::new())),
            )
            .consumes("Storage"),
        )
        .transition(
            TransitionDef::new(
                "Reopen",
                Trigger::Manual,
                |r, s| gen_of(r, "Storage.Settled", s),
                Body::Pure(|s, input| {
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
fn inhibitor_net() -> NetDef {
    let storage_gen = |r: &mut Reader<'_>, s: &Subject| gen_of(r, "Config", s);
    NetDef::default()
        .place("Config", Class::Process)
        .place("Storage", Class::Process)
        .place("TursoReady", Class::Process)
        .place("TursoFailed", Class::Process)
        .transition(
            TransitionDef::new(
                "MakeTursoReady",
                Trigger::Auto,
                storage_gen,
                Body::Effectful(Receiver::None),
            )
            .reads("Config")
            .inhibits("Storage")
            .inhibits("TursoReady")
            .inhibits("TursoFailed")
            .on_failure("TursoFailed"),
        )
        .transition(
            TransitionDef::new(
                "FillFromTurso",
                Trigger::Auto,
                storage_gen,
                Body::Pure(|s, input| Ok(storage("turso", s, input)[..1].to_vec())),
            )
            .consumes("TursoReady"),
        )
        .transition(
            TransitionDef::new(
                "MakeLoroReady",
                Trigger::Auto,
                storage_gen,
                Body::Pure(|s, input| Ok(storage("loro", s, input)[..1].to_vec())),
            )
            .consumes("TursoFailed"),
        )
        .transition(
            TransitionDef::new(
                "Take",
                Trigger::Manual,
                |_, _| Some(Content::new()),
                Body::Pure(|_, _| Ok(Vec::new())),
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
        match self.d.exec.fire(transition, APP, occ, self.d.now) {
            Ok(_) | Err(FireError::NotEnabled { .. }) => {}
            Err(e) => panic!("fire {transition}: {e}"),
        }
        self.d.step().expect("the net quiesces");
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
        d.exec
            .write(opener, APP, content(&[("gen", Value::Integer(0))]));
        d.step().expect("the net quiesces");
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
        let tokens = sut.d.exec.tokens();
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

/// REV11-running: a take moves the token, so a reader cannot approve a token
/// a running firing holds.
fn running_net() -> NetDef {
    let n = |r: &mut Reader<'_>, s: &Subject| {
        let n = r.field("Intent", s, "n")?;
        Some(Content::from([("n".to_string(), n)]))
    };
    NetDef::default()
        .place("Intent", Class::Process)
        .place("Sent", Class::Process)
        .place("Archived", Class::Process)
        .transition(
            TransitionDef::new("Send", Trigger::Auto, n, Body::Effectful(Receiver::None))
                .consumes("Intent"),
        )
        .transition(
            TransitionDef::new(
                "Archive",
                Trigger::Auto,
                n,
                Body::Pure(|s, input| Ok(vec![Output::new("Archived", s, input.clone())])),
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
        let archived_id =
            |sut: &IntentSut, s: &str| sut.d.exec.tokens().token("Archived", s).map(|t| t.id);
        let before = SUBJECTS.map(|s| archived_id(&sut, s));
        match op {
            IntentOp::Write(s) => {
                sut.next_n += 1;
                sut.d
                    .exec
                    .write("Intent", s, content(&[("n", Value::Integer(sut.next_n))]));
                sut.d.step().expect("the net quiesces");
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
        for (s, was) in SUBJECTS.into_iter().zip(before) {
            if archived_id(&sut, s) == was {
                continue;
            }
            let archived = &sut
                .d
                .exec
                .tokens()
                .token("Archived", s)
                .expect("changed")
                .content;
            for req in sut.d.pending.iter().filter(|r| r.subject.as_str() == s) {
                assert!(
                    archived["n"] != req.input["n"],
                    "archive_never_approves_held_token: Archive approved intent {:?} of '{s}' \
                     while Send holds it",
                    req.input["n"]
                );
            }
        }
        sut
    }
}

prop_state_machine! {
    #[test]
    fn a_take_moves_the_token_into_running(sequential 1..20 => IntentTest);
}
