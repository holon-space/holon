---
id: 2026-10-06-embedded-page-under-journals-feed-misses-expand-toggle-under-load
date: 2026-10-06
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The teeth test embedded_page_under_journals_feed_renders_expanded failed once
  in a full library run (no expand_toggle for a day page after 5 s) and passes
  3 of 3 alone on base and on the epoch-flip tree; cause not found.
---

## Bug

Found by the epoch-flip verifier in a full
`cargo nextest --lib -E 'package(holon-integration-tests)'` run
(437 tests, ~576 s): `pbt::frontend_slice::structural_pbt::teeth::embedded_page_under_journals_feed_renders_expanded`
panicked at `structural_pbt.rs:4194`. The violation was
`[inv-embedded-page-collapsed-lazy] ... embedded page block:day-0714 ... has NO
expand_toggle in the main-panel widget tree`, PERSISTED for 5 s.

## Root cause

Not found. Measured A/B (lane-logs/ab4.sh, 3 runs each, test alone):
base `e0313eaf` 3/3 pass, epoch-flip tree 3/3 pass. The failure appeared only
in the loaded full run. Candidate causes: a main-panel render that does not
settle under CPU load within the 5 s persistence window, or a real race in the
`embedded_page` profile variant wrapping. This signature differs from the
registered `journal-feed-main-panel-still-loading` row (that one reports
`loading placeholders=N`), so it is a separate family.

## Missing piece

No load-reproducible run and no second observation, so the cause is not
separable from noise. It stays OPEN in its escape class rather than
FALSE-ALARM, because nothing shows the test asserts more than the product
promises.

## Remedy

Open. Next step: re-run the full library suite on base to see whether the
failure also occurs there under load, and capture the widget tree at failure.
