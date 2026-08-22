---
id: 2026-08-24-split-block-indent-no-previous-sibling
date: 2026-08-24
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Splitting the same block twice reaches a state where the split's own indent
  step has no previous sibling to indent under, and the production op refuses
  with a hard error.
---

## Bug

`SplitBlock` on a block that has already been split can drive the split's
follow-on `indent` into a position with no previous sibling. The production op
refuses, loudly:

```
panicked at crates/holon-integration-tests/src/pbt/op_write_cap.rs:299:17:
[SplitBlock/keystroke] enter [] failed: dispatch_intent_sync: block.indent failed:
Operation 'indent' on entity 'block' failed: Cannot indent: no previous sibling to become parent
```

This is a HARD SUT error — the operation refused — not an oracle divergence, so
no invariant is at fault and no model change would make it green. Something a
user can reach by pressing Enter twice in the same place.

Found by the keystone PBT under forced weights, during 2b.4 I2b verification
(lane `inc2b-i1`, verifier round 2). It is NOT a re-homing defect: the shrunk
counterexample contains no `RehomeEntity` and `native_homed: {}`.

## Root cause

Not diagnosed here — this entry records the escape and the repro; a separate
lane owns the fix. The shape is that the split compound's `indent` constituent
assumes a previous sibling exists at the position the split just created, which
a second split at the same block can violate.

Reproduction, the verifier's shrunk 4-transition counterexample:

```
SplitBlock { block_id: block:c1, position: 0 }
SplitBlock { block_id: block:c1, position: 2 }
InstantiateTemplate { parent_id: block::sp… }
SplitBlock { … }
```

Discovering run:

```
HOLON_PBT_FORCE_FULL=1 \
HOLON_PBT_WEIGHTS='RehomeEntity:120,SplitBlock:60,BlockToPage:40,InstantiateTemplate:40,WriteOrgFile:30,ExpandToggle:30' \
PROPTEST_CASES=24 cargo test -p holon-integration-tests --features pbt --test general_e2e_composed_pbt
```

## Missing piece

REACHABILITY, and it is worth being precise about what changed. The transition
and the invariants were always there; what was missing was a draw distribution
that reached this state. The `RehomeEntity` weight raised from 2 to 10 in
2b.4 I2b (verifier finding D6, "the default keystone never draws
`RehomeEntity`") shifted the global distribution enough to reach it — the
transition being weighted is unrelated to the bug, but re-weighting one member
re-weights every draw.

Measured novelty at the time of filing:

- `grep -c "no previous sibling to become parent" docs/Testing/KeystoneKnownReds.md` → 0
- present in exactly one lane log, `.lane-logs/d6-w10b-34636-20260824-054320.log`, a weight
  experiment from the same round
- absent from every pre-weight-change lane log in that workspace

## Remedy

Open. A separate lane takes the production fix. Until then this is an
unregistered red that a sufficiently split-heavy draw can hit; whoever picks it
up should decide whether the split compound should skip the indent when no
previous sibling exists, or whether the split should not have produced that
position at all.
