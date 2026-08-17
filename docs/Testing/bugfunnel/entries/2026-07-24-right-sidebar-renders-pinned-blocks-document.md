---
id: 2026-07-24-right-sidebar-renders-pinned-blocks-document
date: 2026-07-24
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  Right sidebar renders PINNED blocks in document/ingest (`sort_key`) order,
  NOT the pin-recency order its backing query declares.
  `assets/default/index.org` `default-right-sidebar::src::0` ends `... RETURN
  d ORDER BY fr.added_ts DESC, d.sort_key` (most-recently-pinned root first —
  exactly what the reference models via `RefBoot::pin_block` move-to-top on
  `added_ts_logical`), but `default-right-sidebar::render::0` is
  `tree(#{sortkey: col("sort_key")})`, and `OutlineTree::from_rows`
  (`crates/holon-api/src/render_eval.rs:252-258`) sorts EVERY row — including
  the level-0 pin roots — by that single `sort_key` BEFORE partitioning
  roots/children, silently discarding the query's `ORDER BY`. So two pins
  whose `sort_key` (document) order is the reverse of their pin order render
  in the wrong order. Identical class to the FIXED left-sidebar bug B (seed
  now declares `sortkey: col("content")`) and BugFunnel F7 (journals feed
  ignores declared ORDER BY / sortkey). Found by analysis while
  premise-checking the left-sidebar sort lane on current main; not
  live-dogfooded (surfaces whenever ≥2 pins exist with divergent sort_key vs
  pin order).
source_line: 793
---

## Bug

Right sidebar renders PINNED blocks in document/ingest (`sort_key`) order,
NOT the pin-recency order its backing query declares.
`assets/default/index.org` `default-right-sidebar::src::0` ends `... RETURN
d ORDER BY fr.added_ts DESC, d.sort_key` (most-recently-pinned root first —
exactly what the reference models via `RefBoot::pin_block` move-to-top on
`added_ts_logical`), but `default-right-sidebar::render::0` is
`tree(#{sortkey: col("sort_key")})`, and `OutlineTree::from_rows`
(`crates/holon-api/src/render_eval.rs:252-258`) sorts EVERY row — including
the level-0 pin roots — by that single `sort_key` BEFORE partitioning
roots/children, silently discarding the query's `ORDER BY`. So two pins
whose `sort_key` (document) order is the reverse of their pin order render
in the wrong order. Identical class to the FIXED left-sidebar bug B (seed
now declares `sortkey: col("content")`) and BugFunnel F7 (journals feed
ignores declared ORDER BY / sortkey). Found by analysis while
premise-checking the left-sidebar sort lane on current main; not
live-dogfooded (surfaces whenever ≥2 pins exist with divergent sort_key vs
pin order).

## Root cause

right-sidebar rendered pin order ignores its query's declared `ORDER BY
fr.added_ts DESC` — the `tree(sortkey: col("sort_key"))` render's
`OutlineTree::from_rows` sorts ALL rows (incl. level-0 pin roots) by the
single `sort_key`, silently discarding the query sort, so pins render in
document/ingest order not pin-recency. SAME class as F7 (journals feed) and
the FIXED left-sidebar bug B. Found by analysis while premise-checking the
left-sidebar sort lane; no oracle asserted render-order == query's effective
declared sort, so the divergence was invisible. Oracle DELIVERED:
`structural_pbt.rs::right_sidebar_renders_pins_in_declared_added_ts_order`
(`#[ignore]`, RED-for-the-right-reason) locks it — any render sortkey
silently overriding a query ORDER BY is now a permanent red. FIX ESCALATED
to Martin: a single-column tree() sortkey cannot express "roots by added_ts
DESC, descendants by sort_key" — the fix is a render-DSL fork, ruling needed
before implementation. ORACLE primary / COVERAGE secondary.)

## Missing piece

No oracle asserted that a rendered tree/list's child order equals the
EFFECTIVE declared sort of its backing query — the right sidebar's
compile+execute was covered (`inv-focus-roots` checks the root SET,
unordered) but ORDER BY semantics were not, so a render-layer `sortkey`
silently overriding a SQL/GQL `ORDER BY` was invisible. Secondary COVERAGE:
the keystone catalog seeds no ≥2-pin right-sidebar topology with divergent
sort_key-vs-pin order.

## Remedy

OPEN 2026-07-24 — ORACLE DELIVERED, FIX ESCALATED. Oracle:
`crates/holon-integration-tests/src/pbt/frontend_slice/structural_pbt.rs::right_sidebar_renders_pins_in_declared_added_ts_order`
— pins `zebra` (lower sort_key) then `apple` (higher sort_key), asserts the
render shows `apple` (added_ts DESC) first; RED-for-the-right-reason on main
(renders `zebra` first by sort_key), `#[ignore]`d so the gate stays green
while the semantics question is open. With it in the catalog the override
class is now a permanent red. FIX is a genuine render-DSL FORK escalated to
Martin (NOT ruled here): a single tree() `sortkey` cannot express "roots by
`added_ts DESC`, descendants by `sort_key`" — options are (a) per-level
sortkey via the render's existing `rules` mechanism (already used here for
level-0 role/bullet overrides), or (b) make `OutlineTree` preserve the
backing query's incoming row order for roots and apply `sortkey` only within
sibling groups — a codebase-wide render-semantics change touching every
tree() consumer. Ruling needed before implementation.
