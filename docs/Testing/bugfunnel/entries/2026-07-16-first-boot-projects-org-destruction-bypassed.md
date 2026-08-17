---
id: 2026-07-16-first-boot-projects-org-destruction-bypassed
date: 2026-07-16
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  First-boot Projects.org destruction BYPASSED the mass-truncation tripwire:
  on the restart boot the tripwire demonstrably works on the block-driven
  write-back path ("MASS TRUNCATION … 36 of 37 … VETOED — QUARANTINING"), yet
  the first boot's 6,245-line rewrite went through unimpeded — a write-back
  path exists that the tripwire does not cover
source_line: 824
---

## Bug

First-boot Projects.org destruction BYPASSED the mass-truncation tripwire:
on the restart boot the tripwire demonstrably works on the block-driven
write-back path ("MASS TRUNCATION … 36 of 37 … VETOED — QUARANTINING"), yet
the first boot's 6,245-line rewrite went through unimpeded — a write-back
path exists that the tripwire does not cover

## Missing piece

the tripwire WAS on both block-driven paths but its threshold policy
tolerated name_chain-failed drops

## Remedy

FIXED (same fix as row 23). The tripwire didn't "not cover" the path — it
RAN (the 749 name_chain failures came from its own
`writeback_sibling_grounding`) but its 25%-of-file threshold, meant to
tolerate benign ungrounded drops (cross-doc moves, matview lag), also
tolerated drops caused by a LOUD name_chain grounding failure. On the
restart boot the ungrounded count happened to exceed 25% (36 of 37) so it
fired; on first boot it fell under. Fix promotes any name_chain-failed
(UNRESOLVABLE) drop to a hard veto regardless of count — a grounding failure
can never slip under the threshold again.
