---
id: 2026-11-03-net-an-ask-row-without-a-hash-text-panics
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, Executor::asks panicked on a hand-built Asks.T derive whose row held no hash text or a non-hex hash text, and hash_from_text panicked on any non-hex text.
---

## Bug
Found in round 2 while making hand-built ask inputs typed errors (ruling N2 and N5 of card #882).

## Root cause
`Executor::asks` panicked on a row without a string `HASH_FIELD` (crates/holon-net/src/exec.rs:2272 on the round-1 commit 19bba9bfa851), and the public `hash_from_text` panicked on text that is no hex hash (exec.rs:1930). The derive of `Asks.T` is a closure, so no load check sees its rows.

## Missing piece
The wide machine never drew a hand-built `Asks.T` derive. Litmus: COVERAGE, because no case reached the state; the no-panic oracle would have gone red had one reached it.

## Remedy
`hash_from_text` returns `Result`; `Executor::asks` maps a missing or non-hex hash text to the typed `DeriveError::AskRowHash`.
Improvement candidates: 1
