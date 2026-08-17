---
id: 2026-07-16-journals-seed-file-same-collision-fresh
date: 2026-07-16
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Journals seed/file same-id collision: fresh boot seeds `block:journals` +
  `journals::{src,render,action}::0` while the vault's Journals.org (same
  `#+ID: journals`, same block ids, LEGACY Rhai rule + old render) is
  hash-recorded as ingested but its day entries never land in the DB (4
  children vs file's ~10); after the (destructive) 19:32 rewrite + restart the
  file content re-ingests and "Journal Auto-Create" is DUPLICATED (seed + file
  copies side by side)
source_line: 820
---

## Bug

Journals seed/file same-id collision: fresh boot seeds `block:journals` +
`journals::{src,render,action}::0` while the vault's Journals.org (same
`#+ID: journals`, same block ids, LEGACY Rhai rule + old render) is
hash-recorded as ingested but its day entries never land in the DB (4
children vs file's ~10); after the (destructive) 19:32 rewrite + restart the
file content re-ingests and "Journal Auto-Create" is DUPLICATED (seed + file
copies side by side)

## Missing piece

no test boots the seed against a pre-existing vault Journals.org carrying
the same ids with divergent content

## Remedy

OPEN (live bug stands on its own). DECOUPLED from the keystone signature:
the "journals ingest-data-loss" keystone RED
(`block:journals::{auto-create,action::0}` absent from a non-frontend SUT)
is now root-caused as a TEST-ORACLE ASYMMETRY, not this live collision and
NOT an FK abort — the oracle modeled those frontend-only
`build_default_layout_blocks` seeds under non-frontend (Loro/storage-only)
draws that never seed them. FIXED this lane in `wide_e2e_ref_for` (drop the
auto-create RULE from the oracle on the non-frontend `else` arm; mirrors
forward-edge/boot-journal frontend-gating). This row's LIVE observations —
seed/file same-id collision, the vault Journals.org day entries never
landing (4 children vs ~10), and the DUPLICATED "Journal Auto-Create" after
the destructive rewrite+restart — remain OPEN as their own real bug, no
longer explained by or coupled to the keystone signature. ROOT-CAUSED
(2026-07-17): the real vault's `Journals.org` carries `#+ID: journals` and a
LEGACY `* Journal Auto-Create` heading with a RANDOM `:ID:` (`d67e1f08-…`)
containing holon_sql `journals::trigger::0` + Rhai `journals::action::0`;
the programmatic seed (`build_default_layout_blocks` →
`journals_auto_create_blocks`) unconditionally creates a
`block:journals::auto-create` heading (deterministic id) with a holon_rule
`journals::action::0`. So (A) the block id `journals::action::0` is claimed
by BOTH definitions and (B) two same-content "Journal Auto-Create" headings
coexist. The DATA LOSS (file day-entries never land) is the seed-before-scan
race the `index.org`/`user_index_org_exists` guard (`seed.rs:49`,
`wiring.rs:340`) handles for the ROOT layout but which has NO equivalent for
a user-authored `Journals.org`: the seed pre-creates `block:journals`, and
the file-authority ingest of `Journals.org` collides with the already-seeded
page and drops the file's own children. RED-first repro added (env-gap
closed):
`crates/holon-integration-tests/tests/journals_seed_file_collision.rs::seed_over_legacy_journals_org_no_dup_no_dataloss`
— boots SqlOnly over a legacy `Journals.org`, asserts exactly ONE
auto-create heading + file day-entry survives; `#[ignore]`d because green
requires a PRODUCT RULING (see fork). FORK (documented, NOT guessed):
seed-vs-file authority for `block:journals` + legacy-rule migration. (a)
Suppress the journals-machinery seed when a user `Journals.org` exists
(mirror the index.org guard) — kills the dup + data-loss, but re-breaks
dogfood #4 for legacy vaults (they keep their broken Rhai rule, never get
the new holon_rule). (b) Keep seeding + RECONCILE the file's legacy
auto-create onto the deterministic `journals::auto-create` id at ingest
(recognize the legacy rule and canonicalize) — correct but complex; needs a
rule to detect "this heading IS the legacy auto-create." (c) Dedupe by
(name,parent) at ingest. Recommendation: (b) as the durable answer, with (a)
as an interim only if migration is out of scope this cycle.
