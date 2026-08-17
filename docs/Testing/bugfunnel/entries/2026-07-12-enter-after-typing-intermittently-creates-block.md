---
id: 2026-07-12-enter-after-typing-intermittently-creates-block
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Enter-after-typing intermittently creates NO block: editor dispatches
  `split_block` with its buffer position while committed content lags (`Split
  position 26 exceeds content length 22`, twice, both -4 delta); intent chain
  aborts, ERROR only in log, UI silent (fail-loud violation) — stale-buffer
  class recurrence at synthetic typing speed
source_line: 902
---

## Bug

Enter-after-typing intermittently creates NO block: editor dispatches
`split_block` with its buffer position while committed content lags (`Split
position 26 exceeds content length 22`, twice, both -4 delta); intent chain
aborts, ERROR only in log, UI silent (fail-loud violation) — stale-buffer
class recurrence at synthetic typing speed

## Missing piece

async per-char set_field commit vs split dispatch race absent from settled
headless drivers; no banner on aborted intent chains

## Remedy

OPEN
