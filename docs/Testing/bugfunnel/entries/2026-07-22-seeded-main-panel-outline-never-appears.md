---
id: 2026-07-22-seeded-main-panel-outline-never-appears
date: 2026-07-22
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Seeded main-panel outline never appears on a real (~66-file) vault after a
  fresh-seed reset: `default-main-panel::src::0` was a `WITH RECURSIVE
  focus_descendants` CTE; the Turso IVM plans it as a fullscan and the watch
  never delivers first rows (navigations expired 60-90s). Invisible to
  pre-backlinks vaults.
source_line: 1097
---

## Bug

Seeded main-panel outline never appears on a real (~66-file) vault after a
fresh-seed reset: `default-main-panel::src::0` was a `WITH RECURSIVE
focus_descendants` CTE; the Turso IVM plans it as a fullscan and the watch
never delivers first rows (navigations expired 60-90s). Invisible to
pre-backlinks vaults.

## Missing piece

Only bites at real-vault scale on a fresh seed — the keystone registers its
main-panel watch as GQL and never loads the seeded SQL body, so no test
executed this exact CTE at scale.

## Remedy

FIXED 2026-07-22 — swapped `src::0` (main + right sidebar) to the proven
anchored-varlen GQL form (`MATCH
(fr:focus_root),(root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE
fr.region=... AND root.id=fr.root_id RETURN d`), compiles to PR #55 CTE
(198ms/7 rows live). Accepted regression: GQL descends THROUGH Page-tagged
children (old CTE stopped at Page boundary) and caps depth at 20 — pending
Turso CTE-fullscan engine fix. New corpus test
`every_seeded_source_block_compiles_and_executes_against_booted_vault`
(compile+execute floor; cannot catch the scale hang).
