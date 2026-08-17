---
id: 2026-07-12-cold-boot-over-martin-real-vault
date: 2026-07-12
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Cold boot over Martin's REAL vault: `Journals.org` (`#+ID: journals`, 22
  blocks) ingest FAILED forever — "block feed did not catch up within 2s
  (expected 22 blocks, cache has 5, feed_caught_up=true)" → file QUARANTINED
  from write-back (disk intact, fail-loud worked) + `poll_new_files` retried
  every ~10s (794 identical attempts) + 3600 "SKIPPING write-back" ERROR flood
  lines; DB held 5/22 for the session. Root cause is NOT timing (interim
  "runtime barrier needs the batched treatment" hypothesis superseded): the
  post-ingest count gate expected the whole parse (22) but the file inlines 3
  foreign page-file doc-roots + their 14 descendants, which `get_blocks`'s
  Page-boundary recursive walk can STRUCTURALLY never return for this doc —
  the gate was unsatisfiable, so every retry failed at exactly 5/22 (=
  22−3−14). Secondarily, the barrier itself was a fixed 2s wall-clock ceiling
  — a timing assumption that also breaks under real cold-boot load
  (`boot_write` 2–5s/file observed)
source_line: 969
---

## Bug

Cold boot over Martin's REAL vault: `Journals.org` (`#+ID: journals`, 22
blocks) ingest FAILED forever — "block feed did not catch up within 2s
(expected 22 blocks, cache has 5, feed_caught_up=true)" → file QUARANTINED
from write-back (disk intact, fail-loud worked) + `poll_new_files` retried
every ~10s (794 identical attempts) + 3600 "SKIPPING write-back" ERROR flood
lines; DB held 5/22 for the session. Root cause is NOT timing (interim
"runtime barrier needs the batched treatment" hypothesis superseded): the
post-ingest count gate expected the whole parse (22) but the file inlines 3
foreign page-file doc-roots + their 14 descendants, which `get_blocks`'s
Page-boundary recursive walk can STRUCTURALLY never return for this doc —
the gate was unsatisfiable, so every retry failed at exactly 5/22 (=
22−3−14). Secondarily, the barrier itself was a fixed 2s wall-clock ceiling
— a timing assumption that also breaks under real cold-boot load
(`boot_write` 2–5s/file observed)

## Missing piece

keystone boots a tiny 2-file vault: no folder-companion file sharing ids
with page-file docs, no vault-scale ingest cadence; the misleading error
text ("feed did not catch up") masked the real gate for two triage rounds

## Remedy

FIXED (2026-07-12): (1) post-ingest gate
(`expected_block_count`/`expected_present_ids`) now excludes
`gate_excluded_ids` — foreign-page subtrees + Page-tagged parse blocks +
their parsed descendants — i.e. it only expects blocks the doc walk CAN
return, and the bail message now reports the doc-walk ground truth + first
missing ids instead of blaming the feed; (2) feed barrier redesigned
progress-grounded: new `BlockReader::blocks_in_feed_count` +
`wait_for_feed_progress` — waits in stall-window slices and keeps waiting as
long as expected ids keep landing, NO total wall-clock cap (total ingest
time is a function of vault size, not health); declares failure only on a
full no-progress window with ids missing; `finish_initial_scan(budget)` →
stall semantics; same progress barrier serves the runtime
`on_file_changed`/`poll_new_files` path (2s stall window); (3) quarantine
recovery: retry path (`poll_new_files` re-ingest) now succeeds on first
clean pass and clears the quarantine; the per-skip "SKIPPING write-back"
ERROR is rate-limited to once per quarantine episode (repeats at debug).
Pinned by `companion_inlining_foreign_page_subtree_ingests_clean` (A/B: RED
on pre-fix code with the exact prod signature) +
`scaled_cold_boot_slow_feed_converges_via_progress` (250 files/1250 blocks,
slow-but-progressing feed: old fixed-budget wait fails first slice, progress
barrier converges) in
`crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs`
