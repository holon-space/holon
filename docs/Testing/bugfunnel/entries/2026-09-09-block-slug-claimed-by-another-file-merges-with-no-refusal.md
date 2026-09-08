---
id: 2026-09-09-block-slug-claimed-by-another-file-merges-with-no-refusal
date: 2026-09-09
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The duplicate-identity refusal on ingest is DOCUMENT-level only, so two org
  files that declare different `#+ID:` but share a headline `:ID:` slug merge
  block-by-block into whichever document ingested first, with no claimant
  check, no refusal and no disclosure.
---

## Bug

Found while root-causing
`2026-09-09-hidden-dir-vault-copy-invisible-at-boot-ingested-by-the-watcher`
(lane `ingest-dupslug`, from the `now-query` lane's vault-defect list). This is
the residual half of that bug: closing the walk/watcher divergence stops the
`.claude/worktrees/**` copies from reaching ingest at all, but it does not
address a duplicate slug between two ORDINARY vault files.

Reproduced end to end by the covering test below: with the refusal disabled the
second file's blocks land in the store and the shared slug is re-parented.

## Root cause

`crates/holon-filesystem/src/file_sync_controller.rs:2819` is the only
identity-collision gate on the ingest path. It resolves the file's
document-level `#+ID:` (`doc_id_from_content`), asks `live_claimant_of`
(line 1885) whether a DIFFERENT file already holds that document id, and on a
hit refuses the file outright (`IngestOutcome::RefusedWhileClaimed`) after a
one-shot ERROR from `disclose_duplicate_doc_id` (line 1981).

Every part of that machinery is keyed to the document: the disclosure site
constant is `DUPLICATE_ID_SITE = "duplicate-doc-id"` (line 287) and the claim
map is `doc_home: HashMap<EntityUri, CanonicalPath>`. There is no
`block_home`, no per-block claimant lookup, and no equivalent disclosure. A
headline `:ID:` therefore reaches the store through the ordinary block
upsert — `blocks.sql:6` declares `id TEXT PRIMARY KEY`, so the second file's
headline silently REPLACES the first file's row and inherits its parentage.

Two further properties make this worse than a plain last-writer-wins: the
claimant is "whichever file this session ingested FIRST" and the vault scan
order is arbitrary (the doc guard's own comment says so), so which file wins
can differ between boots; and because the losing file is still tracked, the
next write-back renders the merged block set into BOTH paths.

## Missing piece

No transition writes two files sharing a headline `:ID:` while declaring
distinct `#+ID:`, so the state is ungeneratable (COVERAGE). Secondary ORACLE:
even reached, no invariant asserts "a block id belongs to exactly one source
file", so the merge would pass unflagged.

## Remedy

FIXED by D102.a (Martin, 2026-09-08): the refusal drops the WHOLE second file,
walk order decides which file is second, and the existing ingest-refusal bus
(`WritebackDisclosure::ingest_refused`) carries a typed reason naming both
paths and the slug. Dropping only the colliding subtree was rejected: it leaves
a file whose on-disk content no longer matches the store, which the next
write-back rewrites.

`block_home: HashMap<EntityUri, CanonicalPath>` mirrors `doc_home` and is
replaced wholesale per file on every ingest, so a headline that moves between
files stops being claimed by the file it left. The check sits directly after
the parse and before the document is resolved, so a refused file leaves not one
block behind. Unlike the document-level guard, the claimant is RE-READ rather
than stat'ed, so the refusal lifts when the claimant edits the slug away —
without the claimant having to leave disk.

## Covering tests

- `crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs`
  `duplicate_block_slug_tests::a_second_file_claiming_a_blocks_slug_is_refused_whole_and_disclosed`
  — two real org files sharing `dupblk-shared`: the second is refused whole,
  nothing of it reaches the store, the claimant's bytes are untouched, and the
  banner names both paths and the slug.
- `duplicate_block_slug_tests::the_refusal_lifts_once_the_claimant_drops_the_slug`
  — the claimant renames the slug in place; the refused file then ingests.
