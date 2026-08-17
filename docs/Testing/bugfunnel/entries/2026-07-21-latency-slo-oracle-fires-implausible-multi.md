---
id: 2026-07-21-latency-slo-oracle-fires-implausible-multi
date: 2026-07-21
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Latency-slo oracle fires implausible multi-second e2e values when the window
  is BACKGROUNDED — span ends at GPU frame-present so an unpresented frame
  inflates it into a false SLO violation; e2e stage coverage partial (fired
  once for navigate, projection stage never emitted).
source_line: 1060
---

## Bug

Latency-slo oracle fires implausible multi-second e2e values when the window
is BACKGROUNDED — span ends at GPU frame-present so an unpresented frame
inflates it into a false SLO violation; e2e stage coverage partial (fired
once for navigate, projection stage never emitted).

## Missing piece

end latency span at projection-visible/compute-complete, not frame-present;
emit e2e reliably per interaction

## Remedy

RESOLVED-AS-DOCUMENTED 2026-07-21 (cycle 3) — premise partially misread: the
SLO oracle ALREADY ends at projection-visible (rows_delivered from the
LiveData CDC actor; no frame-present stage exists). 28.7s = real
backgrounded-window pipeline stall (stays surfaced); once-only = warm nav
emits no CDC batch, expires loud as e2e_expired. Endpoint disclosure + 5
semantics-locking tests landed. Forks queued: warm-nav completeness hook;
backgrounded-stall disclosure flag
