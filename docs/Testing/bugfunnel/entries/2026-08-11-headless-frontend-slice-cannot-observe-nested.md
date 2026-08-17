---
id: 2026-08-11-headless-frontend-slice-cannot-observe-nested
date: 2026-08-11
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The headless frontend slice cannot observe nested `live_query` content at
  all, so the journals feed's dominant cost term is structurally invisible to
  every headless rung.
source_line: 727
---

## Bug

(journals-view lane, increment 0; found by MEASUREMENT — a purpose-built
cost rung tripped its own vacuity guard instead of its claim) **The headless
frontend slice cannot observe nested `live_query` content at all, so the
journals feed's dominant cost term is structurally invisible to every
headless rung.** Rendering `block:journals` over a synthetic history counts
ZERO materialised day-children at every history size (3 days: 3 listed / 0
children; 12 days: 12 listed / 0 children); the rendered tree names the
cause verbatim under every `expand_toggle`: `ERROR: Query error:
HeadlessBuilderServices does not support live queries`. The toggles are
present and report `expanded=true`, so any assertion that merely counts
toggles passes — only one reaching THROUGH the toggle sees the hole.
Consequence beyond journals: any headless rung asserting content nested
inside an `expand_toggle`/`live_query` passes VACUOUSLY today.

## Root cause

journals-view lane, increment 0, found by MEASUREMENT — a purpose-built cost
rung tripped its own vacuity guard instead of its claim: **the headless
frontend slice cannot observe nested `live_query` content at all, so the
journals feed's dominant cost term is structurally invisible to every
headless rung.** Rendering `block:journals` over a synthetic history and
counting materialised day-CHILDREN returns ZERO at every history size (3
days: 3 day-pages listed / 0 children; 12 days: 12 listed / 0 children), and
the rendered tree names the cause under every day's `expand_toggle`,
verbatim: `ERROR: Query error: HeadlessBuilderServices does not support live
queries`. The rows and the `expand_toggle`s are all present and report
`expanded=true`, so the feed LOOKS right to any assertion that counts
toggles — only an assertion that reaches THROUGH the toggle into its
`content:` slot sees the hole. ENVIRONMENT, not COVERAGE: the interaction is
perfectly generatable and the profile variant fires correctly; it is the
per-day `live_query(from descendants)` — one `ReactiveShell`, one watched
matview and one CDC subscription per day page in production — that has no
counterpart in the headless wiring, so the failing path never runs in the
test environment. Consequence beyond this lane: any headless rung asserting
on content nested inside an `expand_toggle` / `live_query` passes VACUOUSLY
today, and the journals laziness claim (render cost must not scale with
history) has no headless home — it must be carried by a windowed rung.
Missing piece: either live-query support in `HeadlessBuilderServices`, or a
loud harness guard that fails a rung which reads through an unmaterialised
`live_query` rather than silently returning nothing. Rung
`journals_feed_cost_is_sublinear_in_history` is landed and `#[ignore]`d with
this finding as its reason — deliberately NOT presented as a red, per
`holon-feature` §1. OPEN.)

## Missing piece

the per-day `live_query(from descendants)` — one ReactiveShell, one watched
matview, one CDC subscription per day page in production — has no
counterpart in the headless wiring, so the failing path never runs in the
test environment; the laziness claim (render cost must not scale with
history) has no headless home and must be carried by a windowed rung.
Missing piece: live-query support in `HeadlessBuilderServices`, or a loud
harness guard failing any rung that reads through an unmaterialised
`live_query`.

## Remedy

OPEN — rung `journals_feed_cost_is_sublinear_in_history` landed `#[ignore]`d
with this finding as its reason (a red that is not red-for-the-right-reason
is never presented as one, per `holon-feature` §1).
