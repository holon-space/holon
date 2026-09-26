---
id: 2026-09-29-typing-after-a-jump-to-a-day-page-fails-on-turso
date: 2026-09-29
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  On a Turso-only (SQL authority) session, the first character typed after
  jumping to a journal day page from search fails: the keystroke's
  `set_field('content')` targets a block that `block_raw` does not hold.
---

## Bug
Found by the Inc 6 round-5 lane while it hunted an unexplained keystone red
(`lane-logs/inc6r5-hunt-6-pbt.log`, `lane-logs/inc6-r5-report.md`, "Before
verify"). The keystone's JumpToSearchHit onto the day page
`block:368857d2-…` seats the caret, and the next TypeChars fails with
`[SutEditorMirrorWrite::apply_type_chars] send_raw_keystroke('€') failed: …
set_field('content') on 'block:<uuid>' matched no row in block_raw: the
subject does not exist`. In the app this is a keystroke that is refused right
after a jump — a user-facing bug candidate.

## Root cause
Not root-caused. Timing-dependent (4 of 5 replays red). Suspect: the jump
seats the editor on a block that is minted or re-keyed but not yet in the SQL
store (the day page's first child, or its creation slot) when the keystroke
dispatches.

## Missing piece
The keystone draws Turso-only wirings rarely (Turso inclusion 0.2) and drew
JumpToSearchHit → TypeChars on them too seldom for the red to register; it
appeared once the dense tools were added to every Turso draw (Inc 6 r5).
Pre-existing: 3 of 3 red with main's dense code and no dense tools
(`lane-logs/inc6r5-norow-main-ab.log`).

## Remedy
Open. Registered as the known red `jump-typechars-no-row`
(`docs/Testing/KeystoneKnownReds.md`); deterministic lock
`crates/holon-integration-tests/hand-authored-regressions/known-red-jump-typechars-no-row.jsonl`.
