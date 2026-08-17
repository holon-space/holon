---
id: 2026-07-29-org-file-whose-doc-anchor-disjoint
date: 2026-07-29
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  An org file whose `#+ID:` doc anchor is DISJOINT from the subtree its blocks
  actually live under re-mints a fresh UUID for every ID-less headline on
  EVERY ingest, accreting duplicate subtrees without bound. Agent-found while
  triaging Martin's 2026-07-28 dogfood sighting: the live vault's
  `Projects/Holon/Prepare personal usage.org` held 10 copies of one headline,
  3 of another and 2 of two more, while disk held exactly ONE of each. NOT the
  hypothesised watcher double-fire race — the copies are 8 s and 12 s apart
  and the mechanism is deterministic. Chain: a SECOND, `Page`-tagged "Prepare
  personal usage" block (`block:be4e71b2`, created under the synthetic folder
  page of the still-open 2026-07-21 duplicate-sidebar-pages row) became the
  file's `#+ID:` anchor, while the file's real content stayed under the
  ORIGINAL untagged headline `block:712d5d93` — the name-chain lookup resolves
  through a `tag='Page'` view, so the untagged original is invisible and a
  Page-tagged twin is minted instead (same invisibility mechanism as the
  2026-07-28 `UnnamedPlaceholder` row). That anchor's only child is itself
  `Page`-tagged, and `CacheBlockReader::get_blocks` (`turso_seams.rs:220-245`)
  excludes `Page`-tagged blocks AND their subtrees, so the ID-less reconcile's
  candidate read (`file_sync_controller.rs:2004-2012`) returns EXACTLY EMPTY;
  in `tiered_match` `candidates.is_empty()` is the sole mint path, so all 81
  ID-less headlines of that 331-headline file minted fresh every pass, the
  creates landing under the parent `:ID:` written in the FILE
  (`block:8afe9f70`, in the OTHER root's subtree). Self-sustaining with no
  self-heal: write-back renders from the empty anchor and is refused by the
  lossless guard, so no `:ID:` is ever stamped back (81/331 headlines ID-less
  on disk) and the same headlines re-mint next pass; the cold-boot diff base
  is that same empty `get_blocks` read (behind a `.unwrap_or_default()`), so
  the delete pass has nothing to prune.
source_line: 1118
---

## Bug

An org file whose `#+ID:` doc anchor is DISJOINT from the subtree its blocks
actually live under re-mints a fresh UUID for every ID-less headline on
EVERY ingest, accreting duplicate subtrees without bound. Agent-found while
triaging Martin's 2026-07-28 dogfood sighting: the live vault's
`Projects/Holon/Prepare personal usage.org` held 10 copies of one headline,
3 of another and 2 of two more, while disk held exactly ONE of each. NOT the
hypothesised watcher double-fire race — the copies are 8 s and 12 s apart
and the mechanism is deterministic. Chain: a SECOND, `Page`-tagged "Prepare
personal usage" block (`block:be4e71b2`, created under the synthetic folder
page of the still-open 2026-07-21 duplicate-sidebar-pages row) became the
file's `#+ID:` anchor, while the file's real content stayed under the
ORIGINAL untagged headline `block:712d5d93` — the name-chain lookup resolves
through a `tag='Page'` view, so the untagged original is invisible and a
Page-tagged twin is minted instead (same invisibility mechanism as the
2026-07-28 `UnnamedPlaceholder` row). That anchor's only child is itself
`Page`-tagged, and `CacheBlockReader::get_blocks` (`turso_seams.rs:220-245`)
excludes `Page`-tagged blocks AND their subtrees, so the ID-less reconcile's
candidate read (`file_sync_controller.rs:2004-2012`) returns EXACTLY EMPTY;
in `tiered_match` `candidates.is_empty()` is the sole mint path, so all 81
ID-less headlines of that 331-headline file minted fresh every pass, the
creates landing under the parent `:ID:` written in the FILE
(`block:8afe9f70`, in the OTHER root's subtree). Self-sustaining with no
self-heal: write-back renders from the empty anchor and is refused by the
lossless guard, so no `:ID:` is ever stamped back (81/331 headlines ID-less
on disk) and the same headlines re-mint next pass; the cold-boot diff base
is that same empty `get_blocks` read (behind a `.unwrap_or_default()`), so
the delete pass has nothing to prune.

## Root cause

split doc root re-mints every ID-less headline on every ingest. A file whose
declared `#+ID:` anchor is DISJOINT from the subtree its own authored `:ID:`
blocks live in gets an EXACTLY EMPTY candidate read from
`CacheBlockReader::get_blocks` (`turso_seams.rs:220-245` excludes
`Page`-tagged blocks AND their subtrees, and the anchor's only child was
itself a page), and an empty candidate set is `tiered_match`'s sole
`MintFresh` path — so all 81 ID-less headlines of the live vault's
331-headline `Projects/Holon/Prepare personal usage.org` minted a fresh uuid
every pass, the creates landing under the parent `:ID:` written in the FILE
which sits in the OTHER root's subtree. Self-sustaining and with no
self-heal: write-back renders from the empty anchor and is refused by the
lossless guard so no `:ID:` is ever stamped back, and the cold-boot diff
base is that same empty `get_blocks` read (behind a `.unwrap_or_default()`),
so the delete pass has nothing to prune. COVERAGE primary: the keystone's
filename generator is the flat `[a-z_]+_[0-9]+\.org` with no directory
component, so a nested `Dir/Child.org` whose basename collides with an
untagged headline inside `Dir.org` is unreachable — the THIRD sighting of
that same missing nested-vault rung, after the 2026-07-21
duplicate-sidebar-pages row and the 2026-07-28 `UnnamedPlaceholder` row.
ORACLE secondary and independent of the rung: no invariant asserts that a
file's declared `#+ID:` anchor CONTAINS the store parents of that file's own
authored `:ID:` blocks, and none bounds block count per (parent, content),
so an empty candidate read silently minted instead of failing loud.)

## Missing piece

Keystone cannot express it: the filename generator is the flat
`[a-z_]+_[0-9]+\.org`, with no directory component, so a nested
`Dir/Child.org` whose basename collides with an untagged headline inside
`Dir.org` is unreachable — the SAME missing nested-vault rung already named
by the 2026-07-21 duplicate-sidebar-pages row and the 2026-07-28
`UnnamedPlaceholder` row (third sighting). ORACLE secondary and independent
of the rung: no invariant asserts that a file's declared `#+ID:` anchor
CONTAINS the store parents of that file's own authored `:ID:` blocks, and
none bounds block count per (parent, content) — so an empty candidate read
silently minted instead of failing loud, in violation of the fail-loud
contract.

## Remedy

FIXED (quarantine only) 2026-07-29 — new guard
`FileSyncController::assert_mint_parents_inside_doc_anchor` resolves each
ID-less headline's nearest AUTHORED ancestor (the block the mint would
actually be parented by) and refuses the ingest when that block's store
owner is not this file's anchor; the `Err` quarantines the file from
write-back through the existing `on_file_changed` path, disclosing anchor,
offending block and real owner. Scoped to the mint's real parent so a stale
cross-doc copy elsewhere in the file (already handled by the
cross-doc-membership guard) does not wedge ingest. The
`.unwrap_or_default()` on the cold-boot diff-base read is replaced by
`with_context`. Regression `tests/split_doc_root_idless_duplicates.rs`, 3
cases: `split_doc_root_is_quarantined_never_minted` (zero blocks, all three
ids present in the refusal, page file byte-identical),
`quarantine_is_stable_across_reingests` (a FRESH refusal per round, so "the
file silently stopped being ingested" cannot pass it),
`healthy_doc_root_still_mints_once` (discriminating control).
Red-for-the-right-reason before the fix: first ingest 2 blocks, +2 per
further ingest. Keystone repro attempted per the CLAUDE.md rule and
structurally impossible — the nested-vault generator rung stays OPEN, shared
with the two rows named at left. DEFERRED to Martin: (a) REPAIR semantics —
reconciling ID-less headlines onto the authored parent's subtree so a split
root ingests normally rather than being refused — is deliberately out of
scope pending his ruling; (b) the live vault's data repair is his, not the
lane's. Until that repair, the file stays quarantined: growth STOPS, but the
~17 already-accreted duplicates remain.
