use std::panic::AssertUnwindSafe;

use exec_support::Driver;
use exec_support::answer_call;
use exec_support::answer_spec;
use exec_support::bind;
use exec_support::content;
use exec_support::text;
use holon_api::Value;
use holon_net::exec::Executor;
use holon_transition_contract::ANSWER_ID_KEY;

use crate::exec_support;

/// A manual `Answer.T` whose bind already holds the answer id either does not
/// load or its answer is a typed refusal; `Executor::answer` never panics.
#[test]
fn an_answer_whose_bind_holds_the_answer_id_does_not_panic() {
    let mut spec = answer_spec();
    let answer = spec
        .transitions
        .iter_mut()
        .find(|t| t.name == "Answer.Accept")
        .expect("the ask net has Answer.Accept");
    answer.bind = bind!(|r, s| r
        .present("Asks.Accept", s)
        .then(|| content(&[(ANSWER_ID_KEY, text("planted"))])));
    if let Err(e) = Executor::new(spec.netdef()) {
        eprintln!("the load refuses: {e}");
        return;
    }
    let mut d = Driver::unchecked(spec);
    d.write("Item", "x", content(&[("n", Value::Integer(1))]))
        .expect("the net quiesces");
    let call = answer_call(&d, "x", "a0", false, None);
    let got = std::panic::catch_unwind(AssertUnwindSafe(|| d.answer(call)));
    assert!(
        got.is_ok(),
        "Executor::answer panicked on a net the load accepted"
    );
    eprintln!("answer -> {:?}", got.unwrap());
}
