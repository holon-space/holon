---
id: 2026-10-31-a-transition-that-consumes-a-place-twice-panics-at-run-time
date: 2026-10-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A net whose transition listed one place twice under `consumes` loaded, and
  its first firing panicked in the executor instead of a load refusal.
---

## Bug

A net file with `consumes: [A, A]` passed the loader and `Executor::new`.
When `A` got a token, the firing took the token on the first arc and found
none on the second, so the executor panicked with "decide checked every
consumed token" in `Executor::start` (`crates/holon-net/src/exec.rs:2847`).
The same net built with the Rust builder panicked the same way.

Found by the tester replay of the net crate (holon-space/holon-work#727), not
by a test.

## Root cause

Neither the file check (`TransitionSpec::check`,
`crates/holon-net/src/file.rs:274`) nor `NetDef::check`
(`crates/holon-net/src/exec.rs:562`) looked at repeats inside the input arc
lists. `decide` tests each consume arc for an available token, which one token
satisfies twice; `start` then pops once per arc.

## Missing piece

No generator drew an arc list with a repeated place: `caller_inputs.rs` gives
each place one role per transition, so its nets are distinct by construction,
and the loader tests use hand-written nets only.

## Remedy

Both checks refuse a place that two input arcs of one transition name
(`InputTwice`): the file refusal names the file, the line of the transition and
the fields, the builder refusal the transition and the fields. The panic site
is unreachable because `Executor::new` refuses the net. Pinned by
`a_net_file_that_names_a_place_twice_is_refused_at_its_line` and
`a_built_net_that_names_a_place_twice_is_refused_by_its_check`
(`crates/holon-net/tests/net_repeated_places.rs`), red before the fix
(holon-space/holon-work#868).
