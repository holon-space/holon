---
id: 2026-07-17-page-smuggling-loophole-new-element-wise
date: 2026-07-17
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  `set_field("tags", […])` Page-smuggling loophole: the new element-wise
  `add_tag`/`remove_tag` block ops enforce the no-pages-under-non-pages
  nesting guard (interim ruling 2026-07-13) at BOTH write authorities, but the
  pre-existing whole-set `set_field("tags", [.., "Page"])` path — which
  create/ingest/sync/undo-inverse all legitimately use to write a WHOLE tag
  set — does NOT run the guard, so a caller (or a crafted intent) can still
  land the prohibited page-under-non-page topology by writing the Page tag
  through `set_field` instead of `add_tag`. Found by design/code review while
  adding the guard, not by an automated test. A NAIVE fix (guard
  `set_field("tags")` the same way) is BLOCKED by sync-origin replay:
  `LoroSyncController` and org-ingest replay whole tag sets through
  `set_field` with `EventOrigin::{Loro,Org}` and MUST be able to reproduce ANY
  state that was legal where it was authored (a peer/file may legitimately
  carry a topology this replica would reject at author-time) — an author-time
  guard on `set_field` would reject legitimate convergent replays and stall
  sync. The real fix is either an origin-scoped guard (enforce only for `User`
  origin, exempt sync/ingest replay) or a post-write topology INVARIANT
  (`no-page-under-non-page`) maintained independent of the write op, which the
  keystone already probes via `inv`/generator R8 but no SUT-side write-guard
  closes for `set_field`.
source_line: 1000
---

## Bug

`set_field("tags", […])` Page-smuggling loophole: the new element-wise
`add_tag`/`remove_tag` block ops enforce the no-pages-under-non-pages
nesting guard (interim ruling 2026-07-13) at BOTH write authorities, but the
pre-existing whole-set `set_field("tags", [.., "Page"])` path — which
create/ingest/sync/undo-inverse all legitimately use to write a WHOLE tag
set — does NOT run the guard, so a caller (or a crafted intent) can still
land the prohibited page-under-non-page topology by writing the Page tag
through `set_field` instead of `add_tag`. Found by design/code review while
adding the guard, not by an automated test. A NAIVE fix (guard
`set_field("tags")` the same way) is BLOCKED by sync-origin replay:
`LoroSyncController` and org-ingest replay whole tag sets through
`set_field` with `EventOrigin::{Loro,Org}` and MUST be able to reproduce ANY
state that was legal where it was authored (a peer/file may legitimately
carry a topology this replica would reject at author-time) — an author-time
guard on `set_field` would reject legitimate convergent replays and stall
sync. The real fix is either an origin-scoped guard (enforce only for `User`
origin, exempt sync/ingest replay) or a post-write topology INVARIANT
(`no-page-under-non-page`) maintained independent of the write op, which the
keystone already probes via `inv`/generator R8 but no SUT-side write-guard
closes for `set_field`.

## Missing piece

no transition drives a `set_field("tags")` Page-smuggle, and no write-side
guard/invariant closes the whole-set path (only the element-wise ops are
guarded); a naive set_field guard is precluded by sync-origin replay needing
to reproduce arbitrary authored topologies

## Remedy

OPEN — element-wise `add_tag`/`remove_tag` guarded (this workstream);
whole-set `set_field("tags")` hole documented, deferred pending an
origin-scoped guard or a maintained post-write topology invariant
