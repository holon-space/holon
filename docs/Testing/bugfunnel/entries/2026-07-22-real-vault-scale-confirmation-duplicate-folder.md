---
id: 2026-07-22-real-vault-scale-confirmation-duplicate-folder
date: 2026-07-22
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Real-vault-scale re-confirmation of the F5 duplicate-folder-page class
  (found while verifying the journals report above): a fresh ingest of the
  real 71-file vault produces TWO top-level "Areas" pages — the empty
  companion `Areas.org` root `block:3092ec5e` (childless) AND a phantom
  container `block:4f4f3fbe` (no `#+ID` in ANY org file) that owns ALL the
  real Areas children (`Aiuno` b05159b0, `Music` a9e683b4, a 2nd phantom
  `Music` 4b50f397 → OMR/Audio). So the sidebar shows a duplicated "Areas"
  (and "Music") subtree. Reproduced on a pristine run (fresh vault copy +
  fresh config). NOT a universal empty-companion bug: `Resources.org` is also
  a 43-byte empty companion yet `Resources/Templates` nests correctly under
  its `#+ID` `a9163ed8` with no phantom — so the trigger is
  content/ordering-specific, not "companion is empty". `Journals` is NOT
  affected on fresh ingest (companion `Journals.org` #+ID `journals` == the id
  children resolve to).
source_line: 1105
---

## Bug

Real-vault-scale re-confirmation of the F5 duplicate-folder-page class
(found while verifying the journals report above): a fresh ingest of the
real 71-file vault produces TWO top-level "Areas" pages — the empty
companion `Areas.org` root `block:3092ec5e` (childless) AND a phantom
container `block:4f4f3fbe` (no `#+ID` in ANY org file) that owns ALL the
real Areas children (`Aiuno` b05159b0, `Music` a9e683b4, a 2nd phantom
`Music` 4b50f397 → OMR/Audio). So the sidebar shows a duplicated "Areas"
(and "Music") subtree. Reproduced on a pristine run (fresh vault copy +
fresh config). NOT a universal empty-companion bug: `Resources.org` is also
a 43-byte empty companion yet `Resources/Templates` nests correctly under
its `#+ID` `a9163ed8` with no phantom — so the trigger is
content/ordering-specific, not "companion is empty". `Journals` is NOT
affected on fresh ingest (companion `Journals.org` #+ID `journals` == the id
children resolve to).

## Missing piece

Directory-page ↔ companion-`#+ID` reconciliation: children of a folder are
parented to a directory-derived container id that is supposed to be aliased
to the companion `.org`'s explicit `#+ID`; for `Areas` the alias didn't take
(children stuck on the path-derived `4f4f3fbe`, companion stranded as a
childless duplicate). Same class as F5 (Projects×2/Holon×2). Only surfaces
at real-vault scale with this exact folder set — keystone corpus doesn't
build folder-companion + subdir topologies that trip the reconciliation.
**ROOT-CAUSED 2026-07-22**: pure ingest ORDERING. The vault walk is unsorted
(`ignore::WalkBuilder`), so a child (`Areas/Music.org`) can ingest BEFORE
its folder companion (`Areas.org`). `FileSyncController::ingest_file`
resolves the child's parent chain via the doc-manager's
`get_or_create_by_name_chain`, which, on a miss, mints a path-derived
placeholder `PageId::for_path("Areas")` = `block:4f4f3fbe…` (the EXACT
reported phantom) and parents the children onto it. When `Areas.org` then
ingests, its authoritative `#+ID` `3092ec5e` misses `get_by_id` and
`create_forcing_id`s a SECOND, childless page (`create_forcing_id` correctly
keeps the `#+ID` for rename-stability — but nothing adopts the orphaned
placeholder's subtree). `Resources` reconciled only because its files
happened to scan companion-first (`find_by_parent_and_name` then matches the
real page by title); the `Music/` subtree phantom-recurses for the same
reason. Verified at the id level by
`crates/holon-orgmode/tests/directory_companion_adoption.rs` — child-first
ingest reproduces `block:4f4f3fbe…` exactly.

## Remedy

FIXED 2026-07-22 (PR fix-f5-phantom-container). Made parent-chain resolution
companion-AWARE and thus ORDER-INDEPENDENT:
`FileSyncController::resolve_dir_page_chain` (replacing the three
`get_or_create_by_name_chain` call sites in `ingest_file`) now, before
minting a path-derived placeholder for a directory segment, peeks the folder
companion on disk (`<segment>.<ext>` via `companion_doc_id`) and ADOPTS its
`#+ID` as the page identity — so whoever ingests first creates the folder
page under the id the companion resolves to, and no phantom is ever produced
(`crates/holon-filesystem/src/file_sync_controller.rs`). No `#+ID` (or no
companion) keeps the deterministic `PageId::for_path` id (an org page and a
`[[link]]`-created page still converge). Fail-loud: when an existing page
and a companion `#+ID` genuinely disagree, both ids are WARN-logged (no
silent adopt-by-guess); legacy accumulated-DB dups still need a one-time
dedup migration (see F2). Red-first proven:
`directory_companion_adoption.rs`
`child_before_companion_yields_single_area_page` RED (got 2 `Areas` pages
incl. `4f4f3fbe`) → GREEN (1); the `companion_before_child` control stays
green both ways. Parity remedy for the keystone COVERAGE gap: the dedicated
harness test locks child-first order (the keystone corpus builds no
folder/companion/subdir topology, so it cannot generate this).
