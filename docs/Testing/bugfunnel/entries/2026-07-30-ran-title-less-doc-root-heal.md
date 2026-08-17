---
id: 2026-07-30-ran-title-less-doc-root-heal
date: 2026-07-30
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  `on_file_changed` ran the title-less doc-root heal BEFORE the Model.md
  invariant-11 registered-mount guard, so a REGISTERED shared-subtree mount
  file — a one-way projection sink whose truth is the shared Loro doc — was
  both read and WRITTEN by a heal that re-derives the doc-root's content from
  the FILE NAME. That is the projection-sink→store direction invariant 11
  exists to forbid: the heal's `get_by_id` probe fired on every post-boot
  change of a mount file, and on a mount whose store doc-root is an
  empty-content `Page` it went on to rewrite that row's content and parent
  through `apply_ingest_batch` before the guard ever ran. Agent-found while
  triaging why the guard's own suite had gone degenerate; no automated test
  could see it.
source_line: 1119
---

## Bug

`on_file_changed` ran the title-less doc-root heal BEFORE the Model.md
invariant-11 registered-mount guard, so a REGISTERED shared-subtree mount
file — a one-way projection sink whose truth is the shared Loro doc — was
both read and WRITTEN by a heal that re-derives the doc-root's content from
the FILE NAME. That is the projection-sink→store direction invariant 11
exists to forbid: the heal's `get_by_id` probe fired on every post-boot
change of a mount file, and on a mount whose store doc-root is an
empty-content `Page` it went on to rewrite that row's content and parent
through `apply_ingest_batch` before the guard ever ran. Agent-found while
triaging why the guard's own suite had gone degenerate; no automated test
could see it.

## Root cause

`on_file_changed` ran the title-less doc-root heal BEFORE the Model.md
invariant-11 registered-mount guard, so a registered shared-subtree mount
file — whose truth is the shared Loro doc — was read AND written by a heal
that re-derives the doc-root's content from the FILE NAME, i.e. from the
projection sink, exactly the direction invariant 11 forbids. ENVIRONMENT
primary: the keystone's wiring has NO mount registry (`mount_registry` is
`None` in SqlOnly/tests), so no generated interaction can reach the
registered-mount arm at all — the seam, not the interaction, is missing.
ORACLE secondary: the guard's own dedicated suite could not flag it either,
because its observable was the COUNT OF ALL `DocumentManager` CALLS — the
heal's first probe is a `get_by_id` on the same collaborator, so the count
went degenerate the moment the heal was added: the skip test red at
left:1/right:0 while the guard was actually correct, and the two
ingest-expecting tests passed VACUOUSLY on the heal's call alone. Remedy
landed with the fix: the guard moved ahead of the heal in `on_file_changed`,
and the three tests were rewritten onto a STATE observable (store rows after
`on_file_changed`), which is blind to who called whom.)

## Missing piece

The keystone's wiring has NO mount registry at all (`mount_registry` stays
`None` outside the sharing wiring, and `None` ⇒ never skip), so the
registered-mount arm is not reachable by ANY generated interaction — the
missing piece is the seam, not a transition. ORACLE secondary and
independent: the guard's dedicated suite `shared_projection_ingest_guard.rs`
DID exercise the exact path and still could not flag it, because its
observable was the count of ALL `DocumentManager` calls. The heal's first
probe is a `get_by_id` on that same collaborator, so once the heal was wired
in the count stopped discriminating: `registered_mount_file_is_skipped` went
red at left:1/right:0 while the guard was in fact correct, and the two
ingest-expecting tests passed VACUOUSLY on the heal's single call. A
collaborator-traffic observable cannot survive a new caller; a state
observable can.

## Remedy

FIXED 2026-07-30 — the invariant-11 check is extracted to
`FileSyncController::skip_registered_mount` and runs in `on_file_changed`
BEFORE `heal_title_less_doc_root`, so a registered mount triggers neither
heal nor ingest; `ingest_file` keeps the same call for the initial scan and
its other callers. Both pre-ingest steps now share ONE disk read
(`read_if_present`), so the reorder costs no extra IO — the heal takes the
content instead of re-reading it, and the boot sweep reads per file. Landed
WITH the observable rewrite: the three guard tests now assert STORE STATE
after `on_file_changed` (blocks present ⇒ ingested / absent ⇒ skipped)
against a `FakeStore` that serves the `BlockOrdering` write seam,
`DocumentManager` and `BlockReader` from one map, plus a fourth test
`registered_mount_file_is_not_healed` seeded with exactly the row the heal
would rewrite. Red-for-the-right-reason logs: heal-order red (`store rows
written: ["block:mount-xyz"]`) before the reorder; non-vacuity per test by
disabling the guard (skip test reds on `holds ["block:child-1"]`) and by
disabling ingest (both ingest tests red on `store blocks = []`). The CI
exclusion `-E 'not test(registered_mount_file_is_skipped)'` is REMOVED — the
suite is green as written (104/104 with `--features di`).
