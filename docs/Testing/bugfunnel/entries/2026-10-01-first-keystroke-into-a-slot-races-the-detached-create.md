---
id: 2026-10-01-first-keystroke-into-a-slot-races-the-detached-create
date: 2026-10-01
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  On the dispatch (no-cell) leg, which is GPUI's production leg, the first
  keystroke into a creation slot races the detached `block.create`, so its
  `set_field` can reach the store before the block exists.
---

## Bug
Found by the RCA of three registered keystone reds (`red-rca-report.md` in the
session scratchpad, row 2). Measured there: JumpToSearchHit to an empty page then
TypeChars on Turso-only was red 3/5 at load 4; the Loro control was red 0/3.
Fix lane: i1-tier-guard / sequencer decision D7. The rate and any real text loss
in GPUI are not measured.

## Root cause
`birth_creation_affordance` (`crates/holon-frontend/src/reactive.rs:3065`)
spawns the create detached and returns at once; its own WARN says the first
keystroke "can reach the store before the block exists" (`:3136`). The next
write dispatches `set_field` with no ordering after the create, giving
`set_field ... matched no row in block_raw`. GPUI installs no cell registry
(`frontends/gpui/src/di.rs:128-134`), so this branch is its production leg.

## Missing piece
The keystone only reaches this leg on a Turso-only wiring (1 in 77 random
draws), and no windowed test types fast into a fresh slot on the GPUI wiring.

## Remedy
OPEN, fix lane: i1-tier-guard / sequencer decision D7. Order the first write
after the create (await the pending create, or send one create-with-content),
and measure the GPUI rate with a windowed PBT.
