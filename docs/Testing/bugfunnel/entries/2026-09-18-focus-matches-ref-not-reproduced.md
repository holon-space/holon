---
id: 2026-09-18-focus-matches-ref-not-reproduced
date: 2026-09-18
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The `inv-focus-matches-ref` residual (1 of 36 in the triage lane's sweep) did
  not reproduce in 80 runs on the fixed base, biased and unbiased.
---

## Bug

The `tui-novel-reds-triage` lane listed `inv-focus-matches-ref` as one of four
load-correlated residuals, from a single unbiased observation in its 36-run
sweep (1/36). No captured log of that instance survived in that lane's
`lane-logs/` (the signature is absent from every `tui-sweep-*.log`), so the
payload behind the count is not decodable.

## Root cause

UNKNOWN — not reproduced. Attempted by the `fix-navigate-noop` lane on the base with the
D144.a navigation guard in place:

- 56 runs on the final binary (sha256 `8621b529b9b5…`) — 24 biased
  `HOLON_PBT_WEIGHTS=NavigateFocus:2000` at load 9.4-11.4
  (`lane-logs/f1navA-summary.txt`, `lane-logs/f1navB-summary.txt`), 8 biased
  `FocusEditableText:2000` (`lane-logs/f2feat-summary.txt`), 12 unbiased 8-step
  (`lane-logs/f3unb-summary.txt`), 12 minimal-recipe (`lane-logs/greenA|B-summary.txt`).
- 24 runs on the pre-fix binary under the same navigation bias
  (`lane-logs/base3A|B|C-summary.txt`).

Zero occurrences of the signature in all 80. What the runs do red (navigation
bias, in order of frequency) is `inv-inline-row-mount-present` (21/24),
`await_sidebar_intent` (3/24) and, unbiased, `await_sidebar_intent` plus
`nav_to: walked past target` — all recorded in their own entries.

## Missing piece

A decodable payload. The invariant itself fails loudly on a real focus
divergence (engine `focused_block` vs reference global focus,
`crates/holon-integration-tests/src/pbt/invariants/bodies/focus_matches_ref.rs`),
so its absence here is not evidence of coverage.

## Remedy

OPEN. Absence across 80 runs is not a root cause, so the entry stays in its
escape class rather than becoming a false alarm. If it recurs, the payload is
the thing to capture first: the failure message names both the reference focus
and the engine's.
