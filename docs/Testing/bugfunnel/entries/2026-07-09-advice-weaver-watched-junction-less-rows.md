---
id: 2026-07-09-advice-weaver-watched-junction-less-rows
date: 2026-07-09
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Advice weaver watched the `advice_suppressed` JUNCTION (`SELECT anchor_id,
  lesson_id`) — id-less rows hit the `watch_query` enrichment boundary
  (`row_id().expect`), panicking on a `tokio-rt-worker`; the dying task
  dropped 3 blocks from the shared keystone stream and dominated the shrinker
source_line: 869
---

## Bug

Advice weaver watched the `advice_suppressed` JUNCTION (`SELECT anchor_id,
lesson_id`) — id-less rows hit the `watch_query` enrichment boundary
(`row_id().expect`), panicking on a `tokio-rt-worker`; the dying task
dropped 3 blocks from the shared keystone stream and dominated the shrinker

## Missing piece

background-task panic isolation masks it: the deterministic `advice_step6`
fired the panic (visible in its log) yet stayed GREEN because the spawned
weaver task's death doesn't fail the test; the id-less-row path exists only
in the live watch wiring, not the deterministic refresh path

## Remedy

FIXED (uncommitted): weaver now watches the entity-shaped canonical-read
matview (`advice_watch_sql`, `lesson_id AS id`) instead of the junction —
proven-incremental suppression anti-join (holon-advice
`probe_outer_antijoin_is_incrementally_maintained`); retires the proxy
trigger. Open gap: no assertion on background-weaver health / swallowed
spawned-task panics
