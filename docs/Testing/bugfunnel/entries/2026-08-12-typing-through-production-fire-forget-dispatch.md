---
id: 2026-08-12-typing-through-production-fire-forget-dispatch
date: 2026-08-12
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  Typing through the production fire-and-forget dispatch door LOSES THE TYPED
  TEXT: 9 keystroke `set_field` intents in flight concurrently, and the block
  settles EMPTY.
source_line: 1201
---

## Bug

(det-sched Increment 2 lane, found by the FIRST armed interleaving run —
`HOLON_PBT_SCHED_KINDS=TypeChars HOLON_PBT_SCHED_SEED=1 just hand-authored`,
per `lane-logs/GREEN-hand-authored-armed-typechars.log`. Cite the LOG, not
the seed: the seed fixes the pump budget only — every `SEED=1` run prints
the same `seed=126653198849245441 steps=1`, yet separate `SEED=1` runs land
on different blocks and different first-divergent layers (measured:
`block:promo-loro`, `block:promo-sql`, `block:wwf0`), because the pump
yields race real OS thread scheduling. Do not read this row as "seed=1
reproduces it" or any other seed as "does not" — see PBT.md's "the seed
widens the interleaving; it does not replay it") **Typing through the
production fire-and-forget dispatch door LOSES THE TYPED TEXT: 9 keystroke
`set_field` intents in flight concurrently, and the block settles EMPTY.**
Case `promote-todo-keyword-loro`, block `block:promo-loro`: reference
`"milk"` (task_state TODO), SUT `content=""` in Loro, `block_raw`, matview,
org AND the live editor (`inv-editor-text/mirror` SUT MutableText `"TODO"`
vs ref `"TODO milk"`; `inv-editor-caret/mirror` caret 4 vs 9);
first-divergent layer store/CRDT (`inv-blocks-match-ref/loro`), so it is a
WRITE race, not a projection lag. The keystone could never generate it:
unarmed it drives `dispatch_intent_sync` and awaits every keystroke, so peak
in-flight is 1 by construction (measured: `intents=5 peak_in_flight=1`
before the door swap, `intents=5/9 peak_in_flight=5/9` after). Production
GPUI types through the fire-and-forget door (`editor_view.rs:1070`), so the
interleaving under test is the one a fast typist produces. NOT yet
adjudicated prod-bug vs oracle-asymmetry — the reference applies keystrokes
sequentially and a genuine reorder makes it the user's expectation, which
argues SUT; funding a fix needs that ruling first. Evidence:
`lane-logs/RED-hand-authored-no-door-swap.log` (the
red-for-the-right-reason: overlap never happened),
`lane-logs/GREEN-hand-authored-armed-typechars.log` (overlap real,
divergence surfaced).

## Missing piece

The keystone had no way to put two writes of one gesture in flight — the
settle+await closed every window. Increment 2's `HOLON_PBT_SCHED_KINDS` mask
now can; the mask is EMPTY by default, so this is an opt-in observation, not
a gate.

## Remedy

OPEN — armed observation only; arming stays off by default, no fix attempted
(plan §3: fixing armed reds is out of scope for this increment) | #20 |
deterministic scheduling (task #20)
