---
id: 2026-07-10-non-empty-org-seeded-vault-boot
date: 2026-07-10
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Non-empty (org-seeded) vault boot skips the default-seed journal
  infrastructure entirely: `block:journals` exists (writeback stub) but has
  zero children — no `::trigger/::action/::src/::render`, no today's
  `2026-07-10` block → real vaults never get a daily journal
source_line: 886
---

## Bug

Non-empty (org-seeded) vault boot skips the default-seed journal
infrastructure entirely: `block:journals` exists (writeback stub) but has
zero children — no `::trigger/::action/::src/::render`, no today's
`2026-07-10` block → real vaults never get a daily journal

## Missing piece

boot-seed path is conditional on a fully-empty vault; keystone only
exercises one of the two seed branches

## Remedy

FIXED (journal-seed stream + fork-A C, 2026-07-10/11): journals machinery
seeded PROGRAMMATICALLY on EVERY boot (presence-based, idempotent,
deterministic ids; shared spec across GPUI/worker/headless; no disk
`Journals.org` asset → duplicate-page bug fixed too); auto-create rule live;
the boot-critical residual (rule-machinery heading matview-compiling its
trigger) closed via `rule_sibling` profile exclusion + loud render_entity
belt. Capstone `advance_day_fires_one_journal_per_distinct_day_idempotently`
green. OPEN residue: file-authority echo if a stale bare `Journals.org` stub
exists on disk (echo-loop workstream family)
