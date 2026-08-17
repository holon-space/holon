---
id: 2026-07-17-reds-family-boot-daily-journal-rule
date: 2026-07-17
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  11 `structural_pbt::teeth::*` reds (`wide_*_lockstep` family +
  `full_headless_static_catalog_probe`) on
  `inv-blocks-match-ref/{org,loro,block_raw,matview}` and
  `inv-history-no-phantom-rows/block_history`: the boot daily-journal rule
  mints today's day-page (`block:<uuid>` = "2026-01-15", child of
  `block:journals`) on every full_headless (Turso) boot, but the teeth oracle
  omitted it → SUT `+1` block false-diverges the block-set comparisons and the
  day-page's `block_history` create reads as a PHANTOM (its id unknown to the
  ref universe). Sibling of the composed-keystone `block:gen-` phantom row
  (2026-07-17, above) — SAME ORACLE-asymmetry class, different harness. Found
  on the clean main baseline (aborted by the `--lib` fail-fast at the first
  teeth red)
source_line: 810
---

## Bug

11 `structural_pbt::teeth::*` reds (`wide_*_lockstep` family +
`full_headless_static_catalog_probe`) on
`inv-blocks-match-ref/{org,loro,block_raw,matview}` and
`inv-history-no-phantom-rows/block_history`: the boot daily-journal rule
mints today's day-page (`block:<uuid>` = "2026-01-15", child of
`block:journals`) on every full_headless (Turso) boot, but the teeth oracle
omitted it → SUT `+1` block false-diverges the block-set comparisons and the
day-page's `block_history` create reads as a PHANTOM (its id unknown to the
ref universe). Sibling of the composed-keystone `block:gen-` phantom row
(2026-07-17, above) — SAME ORACLE-asymmetry class, different harness. Found
on the clean main baseline (aborted by the `--lib` fail-fast at the first
teeth red)

## Missing piece

the hand-driven teeth build their oracle via `frontend_wired(wide_ref())`,
which (unlike `wide_e2e_ref_for`) set only the wiring and NEVER called
`seed_boot_journal`, so the legit auto-minted day-page was absent from the
reference `blocks` map → absent from `all_block_ids` (block-set + history
universe). `boot_and_seed_wide` already keeps `keystone_boot_journal_id`
COMPARED (in its `tree` set, not scaffold-filtered) expecting the oracle to
model it — the oracle half was just missing for this entry point. NOT a prod
bug: the SUT correctly fires the rule + records the create; no prod
projection/ingest code was touched

## Remedy

FIXED 2026-07-17 (test-harness only). (1) `frontend_wired` (`wide_e2e.rs`)
now calls `seed_boot_journal(&mut state)` — the exact seed
`wide_e2e_ref_for` applies for a frontend draw — so every teeth oracle
models the day-page as a real non-seed child of `block:journals` (seq 1);
fixes the 10 `wide_*`/`frontend_structural_split`/`wide_e2e_transition`
lockstep tests. (2) `full_headless_static_catalog_probe`
(`structural_pbt.rs`) now builds `frontend_wired(wide_ref())` + fail-loud
awaits `keystone_boot_journal_id` in `block_raw` before its scaffold
snapshot; the day-page folds into `scaffold` (seed-classified via
`inject_scaffold_seed` → excluded from the `/org` comparison, which this
probe seeds no `Journals.org` for) while still counting for the history
universe (`all_block_ids` reads `blocks` regardless of seed class). All 48
teeth green; full `-p holon-integration-tests --lib` = 227 pass / 2 fail
(the 2 remaining `sql_loro_slice` task-state reds are pre-existing +
unrelated, confirmed by baseline revert).
