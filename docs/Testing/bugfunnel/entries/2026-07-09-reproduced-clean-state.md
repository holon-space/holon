---
id: 2026-07-09-reproduced-clean-state
date: 2026-07-09
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  NOT reproduced from clean state
source_line: 872
---

## Bug

iOS split_block (real HW-Enter) INTERMITTENTLY leaves the ORIGIN block
un-truncated in the Loro doc — observed ONCE splitting the pre-existing
dirty block "hello9e7 EDITED": SQL `block_raw` momentarily correct
(origin="hello9e7" + new "EDITED") but Loro kept origin="hello9e7 EDITED",
so a later Loro→SQL reprojection overwrote SQL back to "hello9e7 EDITED",
permanently losing the truncation; renderer reads Loro so screen showed
"hello9e7 EDITED" AND a duplicate "EDITED". Confirmed `inspect_loro_blocks`
vs `execute_raw_sql`; persisted across relaunch (baked into on-disk
`.loro`), focus confirmed before HW Return keycode 40. **NOT reproduced from
clean state**: 3 clean fresh-block splits (WEDGETEST mid-text, ETEST ×2
rapid, all after `reset_vault`) were all CORRECT (Loro==SQL). So it is
intermittent / tied to particular block Loro history, NOT a deterministic
clean repro

## Missing piece

keystone drives SplitBlock via the direct op transition, not the live iOS
`KeyDown "enter"`→gpui-mobile→split path + async CDC/Loro→SQL reprojection
loop where the origin-truncation Loro write can be lost and SQL heals
backward; the Loro↔SQL content-parity invariant is not run against the live
wiring's reprojection race. Very likely the live manifestation of the known
flaky headless `keystone-splitblock-block-loss` RED (same
SqlOnly/sibling-order echo-loop area)

## Remedy

open; suspected flaky — needs a deterministic trigger
(block-history-dependent) before an invariant can pin it; would benefit from
(a) an insertText/HW-Enter live-input rung driving split over MCP + (b) a
post-SplitBlock Loro-origin-content parity assertion
