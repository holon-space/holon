---
id: 2026-08-04-live-oracle-false-positives-backgrounded-window
date: 2026-08-04
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  The live `latency-slo` oracle false-positives on a backgrounded window, and
  the false positive is indistinguishable from a real breach.
source_line: 781
---

## Bug

(dogfood, latency measurement) **The live `latency-slo` oracle
false-positives on a backgrounded window, and the false positive is
indistinguishable from a real breach.** Two `navigate` interactions were
reported by the in-app oracle as `took 150475ms` and `161558ms end-to-end
(SLO: p95 <200ms)`, raising a persistent red `ORACLE VIOLATION` banner
across the top of the window (screenshots `06-sidebar.png`,
`07-sidebar-bottom.png`); `scripts/measure_latency.py` reports the same two
samples as PROD END-TO-END `navigate p50 156016ms`. Both numbers are
artifacts: GPUI does not paint while its window is not frontmost, so the
interaction→visible span stays open until the window is re-fronted, and the
measured duration is the agent's (or a user's) time spent in another
application. DISCLOSED: because of this, no genuine interaction-latency
verdict could be produced in this session — the `e2e` stage has only these
two poisoned samples, and the only clean pipeline number is `rows (CDC batch
apply)` p95 0 ms over 697 batches.

## Missing piece

The oracle measures wall-clock to first paint with no notion of window
occlusion, so any backgrounded interval is charged to the interaction.
Missing piece = the visible stage must either exclude time in which the
window is not painting (pause the span on occlusion/background and resume on
foreground) or the oracle must suppress/annotate samples spanning a
background interval. Until then every dogfood and every real user who
alt-tabs mid-navigation gets a red banner, which is the fastest way to train
everyone to ignore it — an oracle that cries wolf is worse than no banner.

## Remedy

OPEN 2026-08-04 — diagnosis only. Also note for the fix lane:
`measure_latency.py` reports `no stage=action_total events` and the named
`projection` stage never fires even with `RUST_LOG=holon_latency=debug`, so
e2e coverage remains partial.
