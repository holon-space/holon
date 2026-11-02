---
id: 2026-11-02-net-a-firing-output-into-a-place-the-net-does-not-store-panics
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a pure body, an effect result or a verified resolution whose Output names an undeclared, derived or executor-owned place panicked or kept a token outside the net.
---

## Bug
The tester of card #867 found it; card #882 took it.

## Root cause
`Executor::record` put every output without a check: an undeclared place reached `place_def` (crates/holon-net/src/exec.rs:550 on the base f77be010c8f1) or kept a token in a place the net does not declare, and a derived place reached `candidate_of` (exec.rs:576, "a firing writes the derived place"). Red on the base: `a_body_output_into_a_place_the_net_does_not_store_is_refused`.

## Missing piece
No generator drew an output target outside the net's stored places. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`Executor::record` checks every output first (`NetDef::refused_output`): an output into an undeclared, derived or executor-owned place, or an answer token without its fields, settles the firing Failed with a definite cause that names the place; nothing is put and the inputs go back.
