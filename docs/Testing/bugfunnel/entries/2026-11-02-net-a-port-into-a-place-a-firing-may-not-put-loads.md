---
id: 2026-11-02-net-a-port-into-a-place-a-firing-may-not-put-loads
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a fact, failure or timeout port, a produces or a consume arc on a derived, executor-owned or answer place loaded, and the firing that used it later panicked.
---

## Bug
The navigator and tester of card #882 found it while they reviewed the card's port items.

## Root cause
`NetDef::check` refused ports only into undeclared places: a derived port reached `candidate_of` (crates/holon-net/src/exec.rs:576 on the base f77be010c8f1), and a port on an executor place or an answer place put a cause token that the executor's readers panic on (exec.rs:2946, :1983). Red: `a_port_or_take_on_an_executor_place_does_not_load` (logs of the lane tester, local-run-5).

## Missing piece
No generator drew a port onto a place that a firing may not put. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`NetDef::check` refuses them at load: `LoadError::ArcOnDerived`, `LoadError::WritesExecutorPlace`, `LoadError::PortLacksAnswerField`. Read and inhibit arcs on executor places still load.
