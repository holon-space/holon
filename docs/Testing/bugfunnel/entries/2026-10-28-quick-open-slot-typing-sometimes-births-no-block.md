---
id: 2026-10-28-quick-open-slot-typing-sometimes-births-no-block
date: 2026-10-28
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  In the windowed quick-open tests, a character typed into the seated creation
  slot of an empty page is handled, but sometimes no child block is read back
  under the destination.
---

## Bug
A verifier of the one-binary integration-test change met three different
reds in `frontends/gpui/tests/quick_open_returns_focus_windowed.rs`, each in a
different run. The red is not a load-only red: it also occurs when the binary
runs alone.

- Windowed gate, first run: `a_second_keystroke_into_a_fresh_slot_appends_rather_than_prepends`
  panics at `quick_open_returns_focus_windowed.rs:793:5`, "the first character
  into a seated slot must land".
- Windowed gate, rerun: `typing_into_a_slot_creates_the_node_before_the_write`
  panics at `:759:5`, "typing must birth EXACTLY one block under the
  destination, got []".
- The binary alone, 3 runs: `enter_navigating_out_of_quick_open_seats_a_caret`
  panics in run 1 at `:551:5`, "the first keystroke must birth EXACTLY one
  block under the destination, got []"; runs 2 and 3 pass all 7 tests.

The tests passed in each other run, so the defect is intermittent. The
classifier reads each signature as novel.

## Root cause
Not found. Hypothesis to verify: `children_of` reads `block_raw`
(`quick_open_returns_focus_windowed.rs:370`), but `settle` (`:67`) waits only
until the painted element count is stable. It does not wait for the outbound
projector to write the new SQL row. If that is the cause, the test has a race
and the product has no defect. The other possibility is a product race: the
keystroke is handled before the creation slot is ready, and the birth is lost.
The `:793` signature ("must land") is the keystroke itself, not the SQL
read-back, so it supports the product-race hypothesis.

## Missing piece
- The keystone PBT does not exercise the windowed quick-open → creation-slot
  path, so the failing timing does not exist in its wiring.
- The windowed test settles on paint, not on the store write it then reads.

## Remedy
Open. Find which of the two hypotheses holds: run the binary alone in a loop
with the outbound-projector spans captured and check, for each red, whether
the Loro node exists while `block_raw` has no row. If the test races, settle
on the store write; if the product races, fix the slot birth and keep the
assertions.
