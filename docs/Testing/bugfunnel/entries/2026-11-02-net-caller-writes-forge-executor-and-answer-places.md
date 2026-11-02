---
id: 2026-11-02-net-caller-writes-forge-executor-and-answer-places
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a caller could write or retract the executor's own places (<T>.Running, .RetryPending, .OutcomeUnknown, .UndoPending, OutcomeUnknown) and write Answered / Expired tokens without their fields, and later calls panicked.
---

## Bug
The tester of card #867 found it; card #882 took it.

## Root cause
`write` / `retract` accepted any stored place, so a forged token reached the executor's readers: "'T.RetryPending' of s has no integer due" (crates/holon-net/src/exec.rs:2946 on the base f77be010c8f1), the UndoPending shape panic (exec.rs:1535), and `answer_settlement` "has no reason" (exec.rs:1983). Red on the base: `a_write_into_an_executor_owned_place_is_refused_and_changes_nothing`, `a_retract_of_an_executor_owned_token_is_refused_and_changes_nothing`, `no_call_after_a_write_into_an_executor_or_answer_place_panics`.

## Missing piece
No generator drew a write into an executor-owned or answer place. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`ExecError::ExecutorPlace` refuses a write or retract of an executor-owned place; `ExecError::AnswerField` refuses an Answered token without the answer id, seen hash or value, and an Expired token without an expire reason. Both change nothing.
