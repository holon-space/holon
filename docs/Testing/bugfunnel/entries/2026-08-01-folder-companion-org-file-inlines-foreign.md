---
id: 2026-08-01-folder-companion-org-file-inlines-foreign
date: 2026-08-01
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A folder-companion org file that INLINES a FOREIGN PAGE ROOT silently loses
  every block authored beneath that inline. Deterministic repro
  `region_writeback_loss.rs::companion_inlining_a_foreign_page_root_keeps_its_blocks`:
  boot `Projects/Frontends/GPUI.org` (`#+ID: aaaaaaaa-...`) alone so its root
  is store-resident and `Page`-tagged, then let `Projects/Frontends.org`
  appear carrying `* GPUI :ID: aaaaaaaa-...` with a child `** Inlined
  Descendant :ID: inlined-descendant` plus body. After write-back settles,
  `inlined-descendant` is in NO `block_raw` row, NOT in the owning page-file
  on disk, and DELETED from the companion on disk (rewritten down to
  `companion-head` + `companion-tail`). ZERO ERROR events captured. MECHANISM:
  `ingest_file` folds the inlined root and its parsed descendants into
  `foreign_subtree_ids` (`file_sync_controller.rs:2510-2533`) so no
  create/update/place/gate pass touches them; `get_blocks(companion_doc)` then
  cannot return them (its recursive walk stops at `Page` boundaries), so the
  re-render is a truncated projection. The ingest->write-back loss guard
  `check_writeback_lossless` (`file_sync_controller.rs:3422`) did NOT fire:
  `stale_removals` carries only the cross-doc-membership guard's own
  sanctioned prunes, and a foreign PAGE inline is explicitly excluded from
  that guard, so the removal is neither sanctioned nor caught. The comment at
  `:2559` says foreign page inlines are "deferred, not pruned" — on disk they
  ARE pruned. Distinct from the 2026-07-30 `:Page:`-tagged-child row: there
  the child was promoted to its own file and recoverable from the DB; here the
  block reaches NO store and NO file, so one boot destroys it irrecoverably.
source_line: 1131
---

## Bug

(task #97, agent exploration) A folder-companion org file that INLINES a
FOREIGN PAGE ROOT silently loses every block authored beneath that inline.
Deterministic repro
`region_writeback_loss.rs::companion_inlining_a_foreign_page_root_keeps_its_blocks`:
boot `Projects/Frontends/GPUI.org` (`#+ID: aaaaaaaa-...`) alone so its root
is store-resident and `Page`-tagged, then let `Projects/Frontends.org`
appear carrying `* GPUI :ID: aaaaaaaa-...` with a child `** Inlined
Descendant :ID: inlined-descendant` plus body. After write-back settles,
`inlined-descendant` is in NO `block_raw` row, NOT in the owning page-file
on disk, and DELETED from the companion on disk (rewritten down to
`companion-head` + `companion-tail`). ZERO ERROR events captured. MECHANISM:
`ingest_file` folds the inlined root and its parsed descendants into
`foreign_subtree_ids` (`file_sync_controller.rs:2510-2533`) so no
create/update/place/gate pass touches them; `get_blocks(companion_doc)` then
cannot return them (its recursive walk stops at `Page` boundaries), so the
re-render is a truncated projection. The ingest->write-back loss guard
`check_writeback_lossless` (`file_sync_controller.rs:3422`) did NOT fire:
`stale_removals` carries only the cross-doc-membership guard's own
sanctioned prunes, and a foreign PAGE inline is explicitly excluded from
that guard, so the removal is neither sanctioned nor caught. The comment at
`:2559` says foreign page inlines are "deferred, not pruned" — on disk they
ARE pruned. Distinct from the 2026-07-30 `:Page:`-tagged-child row: there
the child was promoted to its own file and recoverable from the DB; here the
block reaches NO store and NO file, so one boot destroys it irrecoverably.

## Root cause

secondary ORACLE: a folder-companion org file that INLINES a FOREIGN PAGE
ROOT silently loses every block authored beneath that inline — no error, no
warning, no quarantine. Repro
(`region_writeback_loss.rs::companion_inlining_a_foreign_page_root_keeps_its_blocks`,
deterministic): boot `Projects/Frontends/GPUI.org` (`#+ID: aaaaaaaa-…`)
alone so its root is store-resident and `Page`-tagged, then let the
companion `Projects/Frontends.org` appear carrying `* GPUI :ID: aaaaaaaa-…`
with a child `** Inlined Descendant :ID: inlined-descendant` and its body.
After the write-back settles, `inlined-descendant` is in NO `block_raw` row,
NOT in the owning page-file on disk, and DELETED from the companion on disk
— the companion is rewritten down to `companion-head` + `companion-tail`.
Zero ERROR events captured for the run. MECHANISM: `ingest_file` classifies
the inlined root and its parsed descendants into `foreign_subtree_ids`
(`file_sync_controller.rs:2510-2533`) so no create/update/place/gate pass
touches them — the owning page-file stays authoritative — but nothing else
picks them up either, and `get_blocks(companion_doc)` cannot return them
because its recursive walk stops at `Page` boundaries. The re-render is
therefore a truncated projection, and the ingest→write-back loss guard
`check_writeback_lossless` (`file_sync_controller.rs:3422`) did NOT fire on
it: `stale_removals` carries only the cross-doc-membership guard's OWN
sanctioned prunes, and a foreign PAGE inline is explicitly excluded from
that guard (the de-inline workstream's deferred concern), so the removal is
neither sanctioned nor caught. The comment at `:2559` says foreign page
inlines are "deferred, not pruned" — on disk they ARE pruned. Distinct from
the 2026-07-30 `:Page:`-tagged-child row: there the child was promoted to
its own file and recoverable from the DB; here the block reaches NO store
and NO file, so a single boot destroys it irrecoverably. Same ingest-shape
family as the 2026-07-21 duplicate-folder-page / 2026-07-28
`UnnamedPlaceholder` / 2026-07-29 split-doc-root / 2026-07-30
`:Page:`-tagged-child rows: no transition writes a file whose headline
`:ID:` equals another page-file's `#+ID:`, so the state is ungeneratable in
the keystone catalog — COVERAGE. ORACLE secondary: prod's own lossless guard
is the oracle that should have caught it and has a hole, and no invariant
asserts that a block the ingest SKIPS is preserved on disk. Found by agent
exploration while repairing task #97's quarantine test, not by any test.
OPEN 2026-08-01 — repro landed RED and disclosed, NOT fixed; the fix needs a
ruling on whether the companion keeps the inlined subtree, the page-file
adopts it, or the ingest is refused and quarantined.)

## Missing piece

Same ingest-shape family as the 2026-07-21 duplicate-folder-page, 2026-07-28
`UnnamedPlaceholder`, 2026-07-29 split-doc-root and 2026-07-30
`:Page:`-tagged-child rows: no transition writes a file whose headline
`:ID:` equals another page-file's `#+ID:`, so the state is ungeneratable in
the keystone catalog. ORACLE secondary: prod's own lossless guard is the
oracle that should have caught this and has a hole, and no invariant asserts
that a block the ingest SKIPS is preserved on disk.

## Remedy

FIXED 2026-08-01 (Martin's ruling, option d) — the block-driven write-back
guard is now ZERO-TOLERANCE: `FileSyncController::veto_ungrounded_removals`
(formerly `tripwire_mass_truncation`) refuses + quarantines on ANY on-disk
block the projection drops that no delivered `Remove` op sanctioned and no
sibling file carries, instead of only above `max(3, 25%)` — the threshold
was what let this 2-block companion loss through silently. Disk is left
byte-intact and the refusal reuses the ingest quarantine's disclosure
wording. The pinned repro now asserts the contract (quarantine disclosed for
`Frontends.org`, `inlined-descendant` still on disk) and
`block_driven_writeback_small_drop_passes_silently` is flipped to
`..._vetoes_single_ungrounded_drop`. Ownership semantics (companion keeps
the inline vs page-file adopts it) are NOT decided here — still parked as
Fork B. Keystone repro attempted per the CLAUDE.md rule and structurally
impossible for the same reason as the sibling rows.
