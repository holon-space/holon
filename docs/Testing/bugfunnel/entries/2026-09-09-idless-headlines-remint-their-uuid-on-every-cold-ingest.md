---
id: 2026-09-09-idless-headlines-remint-their-uuid-on-every-cold-ingest
date: 2026-09-09
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A headline with no authored `:ID:` drawer gets a fresh `Uuid::new_v4()` on
  every cold ingest, so the same vault booted into two fresh stores addresses
  the same headline under two different block ids — anything that keys on such
  an id across boots (a claim/complete cycle, a saved query result, a link)
  breaks.
---

## Bug

Reported by the `now-query` lane (report `lane-report-now-query.md` §"Run 2",
item 4 of "Vault defects recorded, not fixed"). Two ranked task rows came back
as `34c6ed7a-…` / `352ef4e0-…` in Run 2 where the SAME headlines had been
`99dbdbd9-…` / `c8199fcd-…` in Run 1 — two runs of the same query over the
same vault content, each against its own fresh store. The affected headlines
are the ones carrying no `:ID:` drawer.

Recorded, not fixed, by the `ingest-dupslug` lane: the lane's mandate was the
duplicate-slug merge, and this is an independent defect on the same ingest
path.

## Root cause

`extract_or_generate_id` (`crates/holon-org-format/src/parser.rs:1086`, called
from line 795) mints a `Uuid::new_v4()` for every headline that carries no
`:ID:` property.

The 2026-07-22 fix
(`2026-07-22-less-external-edit-ingest-duplicates-churns`) makes this stable
WITHIN a store's lifetime: `FileSyncController::ingest_file` reconciles each
freshly-minted id onto its already-minted twin by exact content plus sibling
position under the same parent, matching against the STORE's current children
(`block_reader.get_blocks`) before the by-id diff, via the pure
`compute_idless_remaps`
(`crates/holon-filesystem/src/file_sync_controller.rs:7166`).

That remap has nothing to match against on a COLD boot. A fresh store has no
children for any parent, so every remap misses and every ID-less headline
takes its new random id. The id is stable across re-ingests but not across
store lifetimes, which is precisely the guarantee an addressable id is
supposed to carry.

The write-back leg partly masks this — once Holon renders the file it stamps
the minted `:ID:` back to disk, so a headline is unstable only until its first
write-back. It is exactly the read-only workloads (the MCP query lane above,
any consumer that reads the vault without letting Holon rewrite it) that see
the churn.

## Missing piece

No transition tears a store down and re-boots the SAME vault into a fresh
store, so the two-cold-boots sequence is ungeneratable (COVERAGE). Secondary
ORACLE: no invariant states that a headline's block id is a function of its
file content, so even a generated double boot would pass.

## Remedy

OPEN. The parse-don't-validate answer is to derive an ID-less headline's id
deterministically from its content and structural position (a UUIDv5 over
document id + parent chain + content + sibling index) rather than minting a
random one and repairing it afterwards, which would make the id a function of
the file and delete the remap machinery. That changes every currently-stored
id for an ID-less headline, so it needs a ruling and a migration plan.
