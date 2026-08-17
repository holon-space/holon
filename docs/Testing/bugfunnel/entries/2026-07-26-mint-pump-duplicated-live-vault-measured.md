---
id: 2026-07-26-mint-pump-duplicated-live-vault-measured
date: 2026-07-26
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Re-mint pump DUPLICATED ~34% of the live vault (measured read-only with
  `tursodb` against a copy of `~/.config/holon/holon.db`): 17,052 blocks,
  5,787 duplicate EXCESS rows (~34%), 19 duplicated `(content, parent_id)`
  groups, largest = 721 copies of ONE block with 721 distinct ids and 721
  distinct `created_at` spanning ~5.5 hours, same parent, same depth, each
  with its own fractional index. BOTH halves of a two-part mechanism are
  required — a single re-mint yields one duplicate, not 721: (a) an
  `:ID:`-less org headline is minted a fresh `Uuid::new_v4()` on EVERY parse
  (holon-org-format/src/parser.rs:741) — carve-out #1 of
  `PageIdentityDeterminism.md` §5.5, deliberately left open; (b) a file whose
  ingest FAILS never enters `last_projection`, so the 2-second
  `discovery_tick` (holon-orgmode/src/di.rs:1036-1045) re-ingests it FOREVER,
  and the `?` at holon-filesystem/src/file_sync_controller.rs:3368-3378
  propagates out of the discovery walk so every file later in walk order is
  never discovered at all. Half (a) is the already-logged 2026-07-22 ID-less
  re-ingest row; half (b) — the perpetual retry pump and the walk-aborting `?`
  — is new and is what turns one duplicate into 721. Relevant to any repair
  plan: the ruled `SqlOperationProvider::dedup_pages` would fix 0 of the 5,787
  rows — it is `Page`-tagged and same-parent-only, page-level duplication in
  the vault is a single pair, and it has no production caller.
source_line: 1110
---

## Bug

Re-mint pump DUPLICATED ~34% of the live vault (measured read-only with
`tursodb` against a copy of `~/.config/holon/holon.db`): 17,052 blocks,
5,787 duplicate EXCESS rows (~34%), 19 duplicated `(content, parent_id)`
groups, largest = 721 copies of ONE block with 721 distinct ids and 721
distinct `created_at` spanning ~5.5 hours, same parent, same depth, each
with its own fractional index. BOTH halves of a two-part mechanism are
required — a single re-mint yields one duplicate, not 721: (a) an
`:ID:`-less org headline is minted a fresh `Uuid::new_v4()` on EVERY parse
(holon-org-format/src/parser.rs:741) — carve-out #1 of
`PageIdentityDeterminism.md` §5.5, deliberately left open; (b) a file whose
ingest FAILS never enters `last_projection`, so the 2-second
`discovery_tick` (holon-orgmode/src/di.rs:1036-1045) re-ingests it FOREVER,
and the `?` at holon-filesystem/src/file_sync_controller.rs:3368-3378
propagates out of the discovery walk so every file later in walk order is
never discovered at all. Half (a) is the already-logged 2026-07-22 ID-less
re-ingest row; half (b) — the perpetual retry pump and the walk-aborting `?`
— is new and is what turns one duplicate into 721. Relevant to any repair
plan: the ruled `SqlOperationProvider::dedup_pages` would fix 0 of the 5,787
rows — it is `Page`-tagged and same-parent-only, page-level duplication in
the vault is a single pair, and it has no production caller.

## Root cause

re-mint pump duplicated ~34% of the LIVE vault — measured read-only on a
copy of `holon.db`: 17,052 blocks, 5,787 excess rows, largest group = 721
copies of one block over ~5.5 h. BOTH halves required: (a) `:ID:`-less
headlines re-minted `Uuid::new_v4()` on every parse (parser.rs:741, the
deliberate §5.5 carve-out, already logged 2026-07-22) and (b) NEW — a file
whose ingest fails never enters `last_projection` so the 2-second
`discovery_tick` (di.rs:1036-1045) re-ingests it forever, while the `?` at
file_sync_controller.rs:3368-3378 aborts the discovery walk so later files
are never discovered at all. No generated org file ever FAILS ingest, so the
pump arm is ungeneratable → COVERAGE primary; ENVIRONMENT secondary for the
real-vault scale + multi-hour tick count the 721× amplification needs. The
ruled `dedup_pages` repair would fix 0 of the 5,787 rows.)

## Missing piece

No generated org file ever FAILS ingest, so the failing-file arm of
`discovery_tick` — the pump half — is unreachable: `write_org_file.rs`
renders through the same org writer the parser reads back, so every case's
file parses. Missing pieces: (a) an ingest-FAILING external-write arm
(malformed/unparseable org, or a write that trips the `?` at
file_sync_controller.rs:3368-3378) plus an invariant that a file failing
ingest is retried at most N times AND that a failure does not abort
discovery of the remaining files; (b) an invariant that repeated re-ingest
of an unchanged ID-less file is idempotent in block COUNT (the 2026-07-22
row covers the identity half, not the unbounded-accumulation half).
ENVIRONMENT secondary: the 721× amplification needs real-vault scale plus a
long-lived process (~5.5 h of 2-second ticks) — no keystone case runs long
enough or over enough files for a per-tick leak to become visible.

## Remedy

OPEN (measured, not yet fixed; the pump half (b) is the high-leverage fix —
bound retries and stop the discovery walk from aborting — since carve-out
(a) is a deliberate §5.5 decision. Vault cleanup needs a real deduper:
`dedup_pages` is not it)
