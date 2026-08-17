---
id: 2026-07-12-journal-auto-create-promotion-dogfood-fix
date: 2026-07-12
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Journal auto-create promotion (dogfood #4 fix, ed390d15) breaks the journal
  on a REAL populated vault: prod now seeds `block:journals` + the holon_sql
  trigger/holon_rule action and fires today's journal on boot (works in DB —
  `action_watcher execute block.create origin=rule content "2026-07-12"`
  block:fdda212e), BUT the user's existing `holon-pkm/Journals.org` (`#+ID:
  journals`, 22 blocks of real entries) collides: cold-boot ingest must
  `Re-parenting 14 blocks from other documents to block:journals (… from
  seed_default_layout)` then FAILS the 2s feed barrier (`expected 22 blocks,
  cache has 5, feed_caught_up=true`) → `ingest FAILED partway — QUARANTINING`
  Journals.org (writeback skipped, DB view truncated to 5/22, today's journal
  can't persist to disk); then a flood of `SKIPPING write-back for quarantined
  file` ERRORs as the boot journal + rule keep dirtying block:journals.
  On-disk file is SAFE (quarantine is the fail-loud protection, not data
  loss).
source_line: 968
---

## Bug

Journal auto-create promotion (dogfood #4 fix, ed390d15) breaks the journal
on a REAL populated vault: prod now seeds `block:journals` + the holon_sql
trigger/holon_rule action and fires today's journal on boot (works in DB —
`action_watcher execute block.create origin=rule content "2026-07-12"`
block:fdda212e), BUT the user's existing `holon-pkm/Journals.org` (`#+ID:
journals`, 22 blocks of real entries) collides: cold-boot ingest must
`Re-parenting 14 blocks from other documents to block:journals (… from
seed_default_layout)` then FAILS the 2s feed barrier (`expected 22 blocks,
cache has 5, feed_caught_up=true`) → `ingest FAILED partway — QUARANTINING`
Journals.org (writeback skipped, DB view truncated to 5/22, today's journal
can't persist to disk); then a flood of `SKIPPING write-back for quarantined
file` ERRORs as the boot journal + rule keep dirtying block:journals.
On-disk file is SAFE (quarantine is the fail-loud protection, not data
loss).

## Missing piece

keystone boots a BARE `#+ID: journals\n` shell, never a POPULATED disk
`#+ID: journals` file that collides with the programmatic block:journals
seed; the 2s cold-boot per-file barrier (row 64) is untested at real-vault
scale where whole-vault ingest saturates it, and the re-parent-on-ingest of
seed_default_layout blocks (row 60/72 file-authority family) is unmodeled

## Remedy

OPEN→RESOLVED by the two FIXED rows below (barrier/count-gate +
file-authority streams): a populated `#+ID: journals` disk file now owns its
document, ingest succeeds, quarantine never engages, journal persists.
Separately: RECONCILED (this session) — `journals_auto_create_blocks` now
emits the ratified single-block `holon_rule` YAML (ADR 0024 §7.2,
`when:`/`emit:`), fired by `holon_rule_watcher` (old `action_watcher` proven
silent — no query-source sibling for its INNER JOIN); keystone CASES=8
FORCE_FULL fully green, capstones + lib tests pass. NOTE the single-block
form still emits `place: journals` → an INLINE child of `block:journals`
(`Place::parent_id()` = `block:journals`), NOT a `Journals/{today}.org`
page-file — a Place page-file extension is filed separately
