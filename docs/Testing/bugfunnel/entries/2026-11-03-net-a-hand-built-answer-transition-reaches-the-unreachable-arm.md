---
id: 2026-11-03-net-a-hand-built-answer-transition-reaches-the-unreachable-arm
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, an ask net built by hand with a bind of Answer.T that returned an answer field loaded, and its first answer panicked in the unreachable arm of Executor::answer.
---

## Bug
The verifier of card #882 found it in round 1 by reading the load check against the arm.

## Root cause
`NetDef::check` required only `Trigger::Manual` of `Answer.T` (crates/holon-net/src/exec.rs:811 on the round-1 commit 19bba9bfa851). The arm of `Executor::answer` (exec.rs:2222) relied on a manual pure `Answer.T`, so a bind returning `@answer`, `seen` or the answer-input key (`FireError::ValueCollides`) and an effectful body (`WhyNot::InFlight`) reached it. Red on the round-1 commit: `an_answer_whose_bind_holds_the_answer_id_does_not_panic` (answer_bind_probe.rs, the verifier's patch).

## Missing piece
The wide machine built asks only through `lower()`, whose `Answer.T` has an empty bind and a pure body, so no case drew a hand-built `Answer.T`. Litmus: COVERAGE, because no case reached the state; the no-panic oracle would have gone red had one reached it.

## Remedy
`NetDef::check` refuses an ask whose `Answer.T` is not manual, pure and without retry (`LoadError::IncompleteAsk`). A bind that returns an answer field cannot be seen by the load, so the answer is the typed `UftError::AnswerBindCollides`. `NetDef::check` also refuses an ask whose work is the `Answer.T` of another ask (`LoadError::AskWorkIsAnswer`), so the `unreachable!` arm of `Executor::answer` holds. The wide generator draws hand-built `Answer.T` transitions.
