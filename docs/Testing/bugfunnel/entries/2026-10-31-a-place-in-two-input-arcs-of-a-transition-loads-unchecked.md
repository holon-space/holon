---
id: 2026-10-31-a-place-in-two-input-arcs-of-a-transition-loads-unchecked
date: 2026-10-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A transition that named one place twice in `reads` or `inhibits`, or in two
  of `consumes`, `reads` and `inhibits`, loaded and ran without a refusal.
---

## Bug

Nets whose transition repeated a place in `reads` or `inhibits`, or named one
place in `consumes` and `reads`, `consumes` and `inhibits`, or `reads` and
`inhibits`, all loaded and ran. Such a transition contradicts itself: it
inhibits a place it needs, or consumes a token it only reads. The file format
gave no refusal, so the author never learned of the contradiction.

Found by the tester replay of the net crate (holon-space/holon-work#727), not
by a test.

## Root cause

`TransitionSpec::check` (`crates/holon-net/src/file.rs:274`) checked only that
each arc names a declared place, and `NetDef::check`
(`crates/holon-net/src/exec.rs:562`) did the same for nets built in Rust.

## Missing piece

No generator drew input arc lists that share or repeat a place; the nets of
`caller_inputs.rs` give each place one role per transition.

## Remedy

Both checks refuse it with `InputTwice`, naming the transition and the two
fields (and, for a file, the file and the line of the transition). The same
place in two different transitions stays legal. Pinned by the two properties
of `crates/holon-net/tests/net_repeated_places.rs`, red before the fix
(holon-space/holon-work#868).
