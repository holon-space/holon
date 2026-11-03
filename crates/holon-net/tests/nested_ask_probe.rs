use std::panic::AssertUnwindSafe;
use std::time::Duration;

use exec_support::Spec;
use exec_support::Transition;
use exec_support::bind;
use exec_support::content;
use exec_support::text;
use holon_api::Value;
use holon_net::exec::AskMeta;
use holon_net::exec::Body;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::Output;
use holon_net::exec::Trigger;
use holon_net::exec::hash_from_text;
use holon_transition_contract::ANSWER_INPUT_KEY;
use holon_transition_contract::AnswerCall;
use holon_transition_contract::AnswerId;
use holon_transition_contract::SubjectName;
use holon_transition_contract::TransitionName;

use crate::exec_support;

const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn meta() -> AskMeta {
    AskMeta {
        input: None,
        irreversible: false,
        has_preview: false,
    }
}

fn call(work: &str, id: &str) -> AnswerCall {
    AnswerCall {
        transition: TransitionName(work.to_string()),
        subject: SubjectName("x".to_string()),
        seen: hash_from_text(HASH).expect("a hash text"),
        answer: AnswerId(id.to_string()),
        value: Some(text("v")),
    }
}

/// An ask whose `Answer.W` is itself the work of a second ask, with a bind
/// that returns the answer input, either does not load or its answer is a
/// typed refusal; `Executor::answer` never panics.
#[test]
fn an_answer_that_is_the_work_of_another_ask_does_not_panic() {
    let row = || {
        bind!(|r, s| r
            .present("Item", s)
            .then(|| content(&[("hash", text(HASH))])))
    };
    let spec = Spec::default()
        .place("Item", Class::Process)
        .place("Answered.W", Class::Process)
        .place("Expired.W", Class::Process)
        .place("Answered.Answer.W", Class::Process)
        .place("Expired.Answer.W", Class::Process)
        .derived("Asks.W", row())
        .derived("Asks.Answer.W", row())
        .ask("W", meta())
        .ask("Answer.W", meta())
        .transition(
            Transition::new(
                "Answer.Answer.W",
                Trigger::Manual,
                bind!(|r, s| r.present("Asks.Answer.W", s).then(Content::new)),
                Body::pure(|s, input| Ok(vec![Output::new("Answered.Answer.W", s, input.clone())])),
            )
            .inhibits("Answered.Answer.W"),
        )
        .transition(
            Transition::new(
                "Answer.W",
                Trigger::Manual,
                bind!(|r, s| r
                    .present("Asks.W", s)
                    .then(|| content(&[(ANSWER_INPUT_KEY, text("planted"))]))),
                Body::pure(|s, input| Ok(vec![Output::new("Answered.W", s, input.clone())])),
            )
            .consumes("Answered.Answer.W"),
        )
        .transition(
            Transition::new(
                "W",
                Trigger::Manual,
                bind!(|_r, _s| Some(Content::new())),
                Body::pure(|_, _| Ok(vec![])),
            )
            .consumes("Answered.W"),
        );
    let mut exec = match Executor::new(spec.netdef()) {
        Ok(exec) => exec,
        Err(e) => {
            eprintln!("the load refuses: {e}");
            return;
        }
    };
    let now = Duration::from_secs(1);
    let written = exec.write("Item", "x", content(&[("n", Value::Integer(1))]), now);
    assert!(written.expect("the write quiesces").requests.is_empty());
    let inner = exec.answer(call("Answer.W", "a0"), now);
    eprintln!("inner answer -> {inner:?}");
    let got = std::panic::catch_unwind(AssertUnwindSafe(|| exec.answer(call("W", "a1"), now)));
    assert!(
        got.is_ok(),
        "Executor::answer panicked on a net the load accepted"
    );
    eprintln!("outer answer -> {:?}", got.unwrap());
}
