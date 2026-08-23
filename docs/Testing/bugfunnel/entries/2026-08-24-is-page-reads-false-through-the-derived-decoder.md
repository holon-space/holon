---
id: 2026-08-24-is-page-reads-false-through-the-derived-decoder
date: 2026-08-24
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  `SqlBlockOperations::get_by_id(..).is_page()` is always false because the
  derived row decoder defaults every `#[edge_field]` to empty, so the
  no-pages-under-non-pages guard cannot fire on the non-prefetched SQL path.
---

## Bug

A page block reads back with NO tags through `SqlBlockOperations::get_by_id`,
even though the store holds `tags = ["Page"]`. Because `Block::is_page()` is
just `tags.contains(PAGE_TAG)`, every block looks like a non-page on that path.

`move_block_prefetched` takes `moved_is_page` from exactly that read when the
caller supplies no prefetch (`traits.rs:2383`). With it pinned false, the
no-pages-under-non-pages guard (`traits.rs:2439`) is unreachable on the
non-prefetched SQL path: **a page can be reparented under a non-page and
nothing refuses it.**

Found while writing the deterministic pin for the root-remodel (a) change
(root-remodel lane, 2026-08-23/24). The verifier had flagged that
`set_page(true)` "did not survive create→read" in the harness; I added a
non-vacuity assertion to the new test, which failed with `got tags Tags({})`,
and then measured where the tag actually goes.

## Root cause

Not a lost write, and not a macro bug — the writes and the projection are both
correct, and the macro is doing what it documents:

- MEASURED, the tag is stored and projected. `SELECT block_id, tag FROM
  block_tags` returns the row, and `SELECT id, tags FROM block` returns
  `tags = ["Page"]` for the same id (probe output in
  `.lane-logs/a-pin-diag.log`).
- `#[edge_field]` sets `skip_serialization` in the `Entity` derive
  (`crates/holon-macros/src/entity.rs:88-98`), whose own comment states the
  intent: such a field is "excluded from the schema and the row round-trip
  (defaulted on read)". The generated `from_entity` therefore pushes
  `Default::default()` for it (`entity.rs:175-182`) — for `tags`, an empty set.
- `QueryableCache` decodes rows with that derived `TryFromEntity`
  (`crates/holon/src/core/queryable_cache.rs:287`), and
  `SqlBlockOperations::get_by_id` is a straight delegation to the cache
  (`crates/holon/src/core/sql_block_operations.rs:254-256`).

So the defect is at the CONSUMER: `move_block_prefetched` asks a decoder that
is documented to drop edge fields for a fact that lives only in an edge field.

This is the same shape as the R4 finding in
`docs/Plans/root-remodel-feasibility-2026-08-23.md`: `Block` has two row
decoders that disagree, and code picks whichever is in reach. There the split
was over a NULL `parent_id`; here it is over edge fields. The hand-written
`impl TryFrom<StorageEntity> for Block` (`crates/holon-api/src/block.rs`) parses
`tags`; the derived `TryFromEntity` discards them.

## Missing piece

Two absences, either of which would have caught it:

1. **COVERAGE (primary)** — no transition ATTEMPTS a prohibited reparent. The
   guard is a write-side refusal, so observing that it has stopped firing
   requires generating a move of a page under a non-page. The composed
   generator provably never produces one: pages are seed-only and always at
   `no_parent` (ForkB-B1 R8). `inv-no-page-under-non-page` watches the resulting
   TOPOLOGY, which stays clean precisely because nothing ever tries the move —
   the invariant is green for the wrong reason.
2. **ORACLE (secondary)** — nothing asserts the two `Block` decoders agree on
   the same row. A single property (decode a row both ways, compare) would have
   reded on `tags` here and on `parent_id` in the R4 case.

## Remedy

OPEN — deliberately not fixed in the root-remodel lane; it gets its own
red-first dispatch.

The (a) change does not depend on the broken path: its pin
(`crates/holon-app/tests/move_block_to_root.rs::a_page_cannot_be_moved_to_root_because_root_is_not_a_page`)
supplies `moved_is_page` through `MovePrefetch` the way production does, and
asserts page-ness against the STORE rather than the decoder, so it cannot go
vacuous through this defect.

Candidate fixes, for whoever picks it up:

- Make `get_by_id` hydrate edge fields (route the block read through the
  hand-written `TryFrom<StorageEntity>`, or teach the derive to read an
  edge-field column when the row projects one) — the honest fix, since the
  `block` matview already carries them.
- Failing that, make the omission loud rather than silent: a block decoded
  without its edge fields should not be handed to a caller asking `is_page()`.

Whichever is chosen, close gap 1 first: add a transition that attempts a
prohibited reparent and asserts the refusal, so the keystone goes red before
the prod fix lands.
