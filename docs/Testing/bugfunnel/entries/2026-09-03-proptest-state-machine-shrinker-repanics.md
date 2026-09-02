---
id: 2026-09-03-proptest-state-machine-shrinker-repanics
date: 2026-09-03
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The proptest-state-machine shrinker re-panics on its own counter after any
  keystone red, so the minimal failing input is never printed and the run's log
  ends on a signature that says nothing about the product.
---

## Bug

Every keystone run that goes red ends with a second panic from the test
framework itself:

```
proptest-state-machine-0.7.0/src/strategy.rs:590:17:
Unexpected non-zero `seen_transitions_counter`
```

It arrives after the real verdict, so the log's last panic is the shrinker's,
not the product's, and the shrunk minimal sequence the shrinker exists to
produce is never emitted.

Found by the population A/B run of the `inv-sql-budget`
`OpenTabViaModifierClick` pin (docs-adr lane, 2026-09-03), where the
known-reds classifier reported it as a novel signature next to a red that was
itself pre-existing.

## Root cause

Not root-caused in the product. The panic is raised inside
`proptest-state-machine` 0.7.0 while shrinking, on a counter the crate
maintains itself; nothing in `crates/holon-integration-tests/` appears in the
frame. Whether the harness leaves the counter in a state the crate does not
expect, or the crate mis-handles a panicking test case, is open.

Measured on BOTH sides of the A/B, so it is not lane-caused:

- `scratchpad/ks-ab/base-28.log:512` — pure main `89e2efea`, after the primary
  `inv-sql-budget` red at `harness.rs:1137`.
- `scratchpad/ks-ab/tip-13.log:468` — the wave-8 chain, same shape.

## Missing piece

The classifier had no way to say "this signature is a shrink-tail artefact".
`scripts/keystone-known-reds.sh` extracts the first message line of every panic
and treats each as a candidate verdict, so a framework re-panic that carries no
information counted as a novel product regression — the classifier's own oracle
was wrong about what a signature means.

Downstream cost: no keystone red currently reports a shrunk minimal sequence,
so every red is triaged from the full generated run.

## Remedy

Registered as `proptest-sm-shrink-seen-transitions` in
`docs/Testing/KeystoneKnownReds.md` with its kind stated (shrink-tail artefact,
never a primary verdict), so a classified run reads the primary panic and this
line no longer masquerades as a regression.

Open: the shrinker still produces no minimal input. Fixing that needs a
reproduction against `proptest-state-machine` 0.7.0 in isolation and either a
harness change or an upstream issue.
