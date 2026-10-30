---
id: 2026-10-30-net-a-bind-reading-an-undeclared-place-panics
date: 2026-10-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a bind that read a place the net does not declare panicked in the reader during the call that triggered the round.
---

## Bug
The tester replay (wave 14) found it with a probe on main 30b64bdd: an auto transition whose bind calls `Reader::field("Ghost", ..)` panicked during `write` with "the net reads place 'Ghost', which it does not declare".

## Root cause
`Reader::field` and `Reader::present` looked the place up with `NetDef::place_def` (crates/holon-net/src/exec.rs:532 on the base), which panics for an undeclared place. The loader cannot see what a bind closure reads.

## Missing piece
No test drew a bind that reads an undeclared place. Litmus: COVERAGE, because no generator produced such a bind.

## Remedy
The reader marks the read in the trace, reads the place as empty, and the executor refuses the firing with `WhyNot::ReadsUndeclared(place)`. A round that refuses an auto transition for this reason discloses `Condition::ReadsUndeclared`, and `derive`, `derive_all` and `asks` return `DeriveError::ReadsUndeclared`. The pinning test is `a_bind_reading_an_undeclared_place_never_fires` in crates/holon-net/tests/caller_inputs.rs.
