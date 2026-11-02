---
id: 2026-11-02-net-an-answer-whose-ask-reads-an-undeclared-place-panics
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, `Executor::answer` whose answer bind read an undeclared place panicked, and a read of an undeclared place inside a traced sub-read was dropped silently.
---

## Bug
The tester of card #867 found the panic; the tester of card #882 found the dropped read (its F8).

## Root cause
`Executor::answer` panicked on `FireError::NotEnabled { why: ReadsUndeclared }` (crates/holon-net/src/exec.rs:1934 on the base f77be010c8f1). `Reader::traced` (exec.rs:1112) did not carry the sub-trace's undeclared read to its caller. Red on the base: `an_answer_whose_ask_reads_an_undeclared_place_is_refused`.

## Missing piece
No generator drew an ask whose situation reads an undeclared place. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`UftError::ReadsUndeclared { transition, subject, place }`; `Reader::traced` carries the undeclared read, so every caller gets its ReadsUndeclared form.
