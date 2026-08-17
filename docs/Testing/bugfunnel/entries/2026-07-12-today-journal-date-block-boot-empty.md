---
id: 2026-07-12-today-journal-date-block-boot-empty
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  No today's journal date block on boot (empty AND seeded vault): journal
  auto-create machinery is dormant in prod (seed exists only behind test-gated
  `HOLON_JOURNALS_MACHINERY_SEED`, `wide_e2e.rs`) — regression vs dogfood #2
  where empty-vault boot created today's block; interim state of the journal
  rework (rows 60/62, Fork B) left prod with NO journal function at all
source_line: 905
---

## Bug

No today's journal date block on boot (empty AND seeded vault): journal
auto-create machinery is dormant in prod (seed exists only behind test-gated
`HOLON_JOURNALS_MACHINERY_SEED`, `wide_e2e.rs`) — regression vs dogfood #2
where empty-vault boot created today's block; interim state of the journal
rework (rows 60/62, Fork B) left prod with NO journal function at all

## Missing piece

prod boot seeds page+src/render but no machinery; keystone gates the
machinery behind an env var prod never sets

## Remedy

FIXED (2026-07-12): `build_default_layout_blocks` now seeds
`journals_auto_create_blocks()` (trigger + action) on EVERY boot (mirrored
in `frontends/holon-worker/src/seed.rs`), so the clock-day trigger fires
`block.create` and every vault gets today's journal; the prior render-panic
blocker is gone (fork-A `is_program`/`rule_sibling` exclusion +
`RowIdentity`-keyed reactive rows). KEYSTONE PROMOTED — env gate
`HOLON_JOURNALS_MACHINERY_SEED` DELETED
(`journals_machinery_enabled`/`JOURNALS_MACHINERY_ORG`/`seed_journals_machinery`
removed): every composed frontend boot injects a fixed `TestClock` (Fork A,
`keystone_boot_clock`), fires the real rule, and the oracle models the
boot-fired journal by its exact deterministic id (`keystone_boot_journal_id`
via `holon-api::effect_id`) as a non-seed child of `block:journals`
(sequenced after the seeded rule), with the boot converge awaiting the async
firing (Fork B). `general_e2e_composed_pbt` CASES=8 FORCE_FULL green apart
from the pre-existing `inv-editor-text/mirror` residual (separate
editor-stale-buffer stream). Directed capstones
`advance_day_fires_one_journal_per_distinct_day_idempotently` +
`rule_trigger_never_reaches_display_evaluation` now boot on the shipped
programmatic rule (bare `Journals.org`). Follow-up: the AdvanceDay multi-day
journal-count invariant (`SutJournalCount`) is still dormant (referenced
only in a comment); live-MCP arm still boots on real `SystemClock` so its
boot-journal id won't match `keystone_boot_journal_id`
