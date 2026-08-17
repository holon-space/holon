---
id: 2026-07-30-tagged-child-headline-inside-org-file
date: 2026-07-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A `:Page:`-tagged CHILD headline inside an org file is DELETED FROM DISK by
  the first boot's write-back and no file is ever created for it — only half
  of the page-promotion runs. Clean-room repro (fresh config+vault, single
  boot, zero user interaction): `Tagged.org` holding `#+TITLE:`/`#+ID:
  tagged-root` + root body + one `* Tagged Child :Page:` headline (`:ID:
  tagged-child`, with a body) is reduced after one boot to the single line
  `#+ID: tagged-root`, and no `Tagged Child.org` is created. Discriminating
  control in the SAME run: `Untagged.org`, byte-identical except its headline
  carries no `:Page:` tag, keeps headline and body — so the tag is the
  trigger, not the file shape. Recoverable in practice (blocks survive in the
  DB and a LATER boot re-renders them into the parent file, stripped of their
  `Page` tags), but between the two boots the user's on-disk content is gone,
  and for a git-tracked or file-synced vault the disk IS the source of truth.
  Second, smaller loss confirmed in the same repro and affecting BOTH files:
  the `#+TITLE:` line and the pre-first-headline root body text are dropped on
  write-back. Found incidentally while dogfooding the sidebar disclosure
  feature (round 2, rev 0c8f5bbb); NOT caused by it — the truncation
  timestamps precede any interaction and the feature diff touches no
  ingest/write-back code.
source_line: 1124
---

## Bug

A `:Page:`-tagged CHILD headline inside an org file is DELETED FROM DISK by
the first boot's write-back and no file is ever created for it — only half
of the page-promotion runs. Clean-room repro (fresh config+vault, single
boot, zero user interaction): `Tagged.org` holding `#+TITLE:`/`#+ID:
tagged-root` + root body + one `* Tagged Child :Page:` headline (`:ID:
tagged-child`, with a body) is reduced after one boot to the single line
`#+ID: tagged-root`, and no `Tagged Child.org` is created. Discriminating
control in the SAME run: `Untagged.org`, byte-identical except its headline
carries no `:Page:` tag, keeps headline and body — so the tag is the
trigger, not the file shape. Recoverable in practice (blocks survive in the
DB and a LATER boot re-renders them into the parent file, stripped of their
`Page` tags), but between the two boots the user's on-disk content is gone,
and for a git-tracked or file-synced vault the disk IS the source of truth.
Second, smaller loss confirmed in the same repro and affecting BOTH files:
the `#+TITLE:` line and the pre-first-headline root body text are dropped on
write-back. Found incidentally while dogfooding the sidebar disclosure
feature (round 2, rev 0c8f5bbb); NOT caused by it — the truncation
timestamps precede any interaction and the feature diff touches no
ingest/write-back code.

## Root cause

a `:Page:`-tagged CHILD headline inside an org file is DELETED FROM DISK by
the first boot's write-back and no file is ever created for it — only half
of the page-promotion runs. Clean-room repro
(`dogfood-sidebar-r2/scripts/repro_pagetag.sh`, fresh config+vault, single
boot, no user interaction): `Tagged.org` = `#+TITLE:`/`#+ID: tagged-root` +
body + one `* Tagged Child :Page:` headline with `:ID: tagged-child` and a
body; after one boot the file is reduced to the single line `#+ID:
tagged-root` and NO `Tagged Child.org` exists. The discriminating control in
the same run — `Untagged.org`, byte-identical except the headline carries no
`:Page:` tag — keeps its headline and body, so the tag is the trigger, not
the file shape. Recoverable in practice (the blocks stay in the DB and a
LATER boot re-renders them into the parent file, this time stripped of their
`Page` tags), but between the two boots the user's on-disk content is simply
gone, and for a vault that is git-tracked or file-synced the disk IS the
source of truth. Second, smaller loss confirmed in the same repro and
affecting BOTH files: the `#+TITLE:` line and the pre-first-headline root
body text are dropped on write-back. COVERAGE primary, no secondary:
`inv-every-page-has-its-own-file` already exists and WOULD have fired —
nothing generates the triggering state, because no transition writes an org
file containing a `:Page:`-tagged child headline, so a page born from an
INGESTED tag (as opposed to an op-driven `convert_block_to_page`) is
unreachable in the catalog. Same missing ingest-shape rung family as the
2026-07-21 duplicate-folder-page, 2026-07-28 `UnnamedPlaceholder` and
2026-07-29 split-doc-root rows. Found incidentally while dogfooding the
sidebar disclosure feature; NOT caused by it — the truncation timestamps
precede any interaction and the feature diff touches no ingest/write-back
code.)

## Missing piece

`inv-every-page-has-its-own-file` ALREADY EXISTS and would have fired;
nothing generates the triggering state. No transition writes an org file
containing a `:Page:`-tagged child headline, so a page born from an INGESTED
tag (as opposed to an op-driven `convert_block_to_page`) is unreachable in
the catalog. Same missing ingest-shape rung family as the 2026-07-21
duplicate-folder-page, 2026-07-28 `UnnamedPlaceholder` and 2026-07-29
split-doc-root rows.

## Remedy

FIXED 2026-07-30 — coverage rung added RED, root-caused, fixed, green.
COVERAGE RUNG: `ingest_born_page_materializes_before_parent_prune{,_loro}`
(`structural_pbt.rs`) boot the real keystone frontend over the repro's exact
two-file vault (`boot_companion_topology` / `new_with_loro`) and assert on
DISK TRUTH (new recursive `disk_org_contents`) that the tagged child's name
and body survive and that `tagged-child` owns a file. RED on BOTH storage
wirings before the fix — the sibling `fileless_page_writeback_materializes`
had stayed green only because its child's `:ID:` EQUALED its heading text,
so a bare `#+ID: child-note` file satisfied every text assertion; that
fixture is de-masked (`* Child Note` / `:ID: child-note`) and now asserts
name+body too. ROOT CAUSE (superseded the "no file is created" framing — the
file WAS created, as a header-only `#+ID:` stub): a headline's body lives in
that headline block's OWN content (`title\nbody`), but a DOC-ROOT's content
was never round-tripped anywhere. The parser discarded the
pre-first-headline body (`parser.rs`,
`extract_section_content(doc.section())` took only source blocks/images),
`sync_document_metadata` (`holon-orgmode/src/file_format.rs`) synced only
`#+TODO:` keywords, and
`render_document_header`/`OrgRenderer::render_document` emitted header +
children only. The `:Page:` tag promotes a headline INTO a doc-root, so its
title and body fell into exactly that hole while the parent's re-render
pruned the headline away via the `Page`-excluding CTE — two halves composing
into deletion. The `#+TITLE:`/root-body loss on BOTH files was the SAME
defect one level up, not a second bug. FIX: parser captures the
pre-first-headline body into the doc-root's content;
`sync_document_metadata` syncs the doc-root's BODY half plus `file_title`
(body half only — the first content line is the page's name and
`authoritative_name_chain` builds the file path from it, so syncing the
title renamed every file whose `#+TITLE:` differed from its stem, observed
as a duplicate `Untagged Root.org`); `OrgRenderer::render_document` emits
that body between the `#+` directives and the first headline;
`LoroDocumentManager::update_metadata` persists doc-root content (it wrote
only properties+tags). NO synthetic `#+TITLE:` is emitted for a title-less
doc-root — a page's name is carried by its filename, and inventing one broke
`parse(render(doc)) == doc` for every title-less file (caught by
`org_block_round_trip_pbt` + `round_trip_pbt`, which is why the rungs assert
the page's name via its PATH). VERIFIED: 10/10 page-promotion tests green;
`holon-orgmode` + `holon-org-format` 166/166; keystone-smoke green
(`general_e2e_composed_pbt` ok, 4 passed) — `inv-org-render-fixed-point` did
not regress, as expected since render now matches what parse produces; rungs
stable over 4 consecutive runs (the pre-fix pass-then-fail flake does not
reproduce — the observable is now order-independent). Clean-room repro
re-run against `holon-gpui` from this worktree: `Tagged/Tagged Child.org` =
`#+ID: tagged-child` + `Child body.`, `Tagged.org` keeps `#+TITLE: Tagged
Root` + `Root body.`, control `Untagged.org` fully intact. Parent keeps NO
inline reference to the promoted child — full de-inline is the existing
page-promotion convention (`inv-companion-has-no-child-page-headings`), the
child is reached via the page tree. RESIDUAL (disclosed, follow-up): the
headless Loro test component reads its doc-root through a seam that never
sees the synced content, so the Loro rung does not assert the secondary
root-body survival; the real Loro-backed app DOES preserve it (repro output
above), so this is a harness-fidelity gap, not a product defect.
