---
id: 2026-09-26-org-ingest-never-clears-a-removed-planning-line
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  On the org leg, deleting a `SCHEDULED:` or `DEADLINE:` line from a headline
  in the file left the stored value in place, and a drawer-spelled priority
  the file dropped left its `_priority_drawer_only` / `_drawer_order` carriers
  behind.
---

## Bug
Found by reading `build_block_params` in lane `ingest-clear-props`, while
checking a sibling suspicion about `priority`. `scheduled` and `deadline` were
emitted only when the headline carries them, with no removal branch, the
defect `2026-09-26-org-ingest-never-clears-a-removed-task-keyword` fixed for
the task keyword.

Red on both storage arms through the real ingest seam (two `WriteOrgFile`
steps on the same block): `lane-logs/org-planning-red-scheduled-loro-and-priority-green.log`,
`lane-logs/org-planning-red-scheduled-sqlonly.log`,
`lane-logs/org-planning-red-deadline-loro.log`,
`lane-logs/org-planning-red-deadline-sqlonly.log`. Each shows
`inv-blocks-match-ref` with `sut={"scheduled": "<2026-09-26 Sat>", …}` and a
reference with no `scheduled`.

The same test run showed the `_`-prefixed priority carriers surviving an
ingest whose file no longer spells a drawer priority
(`lane-logs/h1-red-org-store.log`: `"_priority_drawer_only": "t"`,
`"_drawer_order": "[\"priority\"]"`). The renderer reads
`_priority_drawer_only` to suppress the `[#X]` cookie
(`crates/holon-org-format/src/models.rs`, the priority arm of the headline
renderer), so a later cookie priority would be written back in the drawer.
That consequence is read from the code, not reproduced end to end.

## Root cause
`crates/holon-orgmode/src/block_params.rs` emitted `scheduled` / `deadline`
under `if let Some(..)` only. Its removal loop walks
`previous.drawer_properties()`, which excludes both planning keys (they are
`INTERNAL_KEYS` in `crates/holon-org-format/src/models.rs`) and every
`_`-prefixed key. So an ingest `update` carried nothing that could clear them,
and the store merge only inserts.

## Missing piece
The keystone's `WriteOrgFile` draws fresh files with fresh ids; no transition
rewrites an existing headline with a planning line removed. The invariant that
catches it (`inv-blocks-match-ref`) fires at once when a hand-authored case
reaches the state.

## Remedy
`build_block_params` emits `Value::REMOVED` for `priority`, `scheduled`,
`deadline`, `_drawer_order` and `_priority_drawer_only` when `previous`
carried the key and the file no longer does. Pinned by the hand-authored cases
`ingest-clears-a-removed-{scheduled,deadline,priority}-{loro,sqlonly}-arm`
(`lane-logs/org-keystone-green.log`) and by
`holon-app::org_store_org_round_trip::a_file_that_lost_its_priority_clears_the_stored_rank_on_re_ingest`
for the carriers.

### The adoption path
The lane verifier (`lane-logs/ingest-verify.md`, probe 1) found the one
production `update` that passed no `previous`: the ingest's re-parent path
(`find_foreign_blocks` → `update` for a block absent from the diff base). A
stored priority, planning line or property survived it. Before this lane the
unconditional `priority = Null` hid the stale rank, so the first fix
regressed `priority` on that path. `scheduled`, `deadline` and properties
were already broken there.

A headline moved between two page files never reaches this path (see
`2026-09-26-a-headline-moved-between-org-files-is-lost-when-the-target-is-ingested-first`).
It is reached by a block the store holds outside any page.
Red on both arms through the controller:
`holon-integration-tests::org_suite::reparent_clears_removed_values`
(`lane-logs/reparent-red.log`: `"priority": Integer(1)`, `"scheduled"`,
`"aisle"` kept after `Beta.org` adopted the block).

Fix: `BlockReader::find_foreign_blocks` returns the stored block with each
conflict (its default implementation already held it), and the controller
passes that block as `previous`. This is the same baseline a cold-boot ingest
uses, since the diff base is then seeded from the store. Green:
`lane-logs/reparent-green.log`. The keystone cannot express it:
`WriteOrgFile` refuses an id another document owns, and no catalog
transition I found creates a block outside a page.

OPEN follow-up, shared with the task-keyword entry: a keystone transition that
rewrites an existing headline's keyword, priority or planning, so the
generator reaches this shape without a hand-authored case.
