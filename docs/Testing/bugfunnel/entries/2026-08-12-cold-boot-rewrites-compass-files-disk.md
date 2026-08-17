---
id: 2026-08-12-cold-boot-rewrites-compass-files-disk
date: 2026-08-12
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A cold boot rewrites 6 of 7 Compass files on disk with zero user
  interaction, losing the authored drawer key order
source_line: 724
---

## Bug

(Compass dogfood lane; found by cold-booting a sandbox over a seeded Compass
vault; no automated test produced it) **A cold boot rewrites 6 of 7 Compass
files on disk with zero user interaction, losing the authored drawer key
order** — `:contributes-to:` moves to alphabetical position across 24
headings; key sets, values and content byte-identical; idempotent on a
second boot. VCS churn on every launch of an unedited vault. Legs in main:
SET at parse EXISTS (`parser.rs:982-990`), REPLAY at render EXISTS
(`org_renderer.rs:21-35`, `:408-415`), PERSIST is the broken leg and is
PATH-DEPENDENT — task #7's `org_store_org_round_trip.rs` proves survival
through a real store but writes via its own `through_the_store` helper, not
the production ingest path; on the live controller path `block:compass-g0`'s
stored properties JSON carries no `_drawer_order`, which is exactly what
makes the renderer fall back to alphabetical. `block_params.rs` has zero
`DRAWER_ORDER` references in main and in all eight lane workspaces, so no
production carrier fix exists anywhere.

## Root cause

secondary COVERAGE, Compass dogfood lane, found by COLD-BOOTING a sandbox
over a seeded Compass vault — no automated test produced it: **a cold boot
rewrites 6 of 7 Compass files on disk with zero user interaction, losing the
authored drawer key order.** Measured: seeded the sandbox from the real
vault, booted once, and diffed — the ONLY change across all 7 files is
`:contributes-to:` moving from its authored position to alphabetical order,
24 headings across 6 files; key sets, values and content are byte-identical,
and `Compass.org` (which carries no compass drawer keys) is untouched.
Idempotent, not oscillating: a second cold boot reproduced the vault
byte-for-byte. User impact is VCS churn on every launch of a vault the user
never edited. The carrier legs, checked in main: (a) SET at parse EXISTS —
`parser.rs:982-990` writes `org_props::DRAWER_ORDER`; (c) REPLAY at render
EXISTS — `org_renderer.rs:21-35` reads it and `:408-415` ranks drawer keys
by it, falling back to alphabetical when it is empty; (b) PERSIST is
PATH-DEPENDENT and is the broken leg.
`crates/holon-app/tests/org_store_org_round_trip.rs` (task #7) proves the
carrier survives a real Turso store, but it writes through its own
`through_the_store` helper into a fresh in-memory backend — NOT the
production ingest path. On the live controller path the carrier is gone:
`block:compass-g0`'s stored properties JSON read back
`{"last-reviewed":…,"provenance":…,"ID":…,"compass":…,"review-cadence":…,"sequence":0,"contributes-to":…}`
with no `_drawer_order`, which is exactly the input that makes
`authored_drawer_order()` return empty and the renderer emit alphabetical.
Consistent with `crates/holon-org-format/src/block_params.rs` containing
ZERO references to `DRAWER_ORDER` — the ingest param builder never carries
it. ENVIRONMENT primary: the interaction is generatable and the invariant
exists, but the write seam that drops the carrier (org ingest →
`build_block_params` → `SqlOperationProvider`) is not the seam the covering
test exercises. Secondary COVERAGE: no test boot-ingests a file whose
authored drawer is non-alphabetical. Missing piece: point the #7 round-trip
test at the PRODUCTION ingest path instead of a hand-rolled store insert,
then carry `DRAWER_ORDER` through `build_block_params`. FIXED 2026-08-12
(task #14, D6.a): the param builder is
`crates/holon-orgmode/src/block_params.rs` (the row's `holon-org-format`
path was wrong; the zero-hit grep result was right); `build_block_params`
now emits `org_props::DRAWER_ORDER` explicitly after the
`drawer_properties()` emit loop, which structurally cannot reach a
`_`-prefixed key. RED-FIRST: `org_store_org_round_trip.rs` gained a
`WriteLeg` selector and a third case,
`non_alphabetical_drawer_order_survives_the_ingest_leg`, that writes the
SAME fixture through `build_block_params` instead of the Loro projection
writer — red on the unmodified tree for exactly the carrier ("mechanism:
`build_block_params` must carry the authored-order carrier into the store",
`lane-logs/red-01.log`), green after. Gates: the file 3/3, holon-app +
holon-orgmode + holon-org-format 460/460, keystone-smoke pass-with-note.)

## Missing piece

the covering test exercises a hand-rolled store-insert seam instead of the
org-ingest → `build_block_params` → `SqlOperationProvider` seam that
actually drops the carrier; and no test boot-ingests a non-alphabetical
authored drawer

## Remedy

FIXED 2026-08-12 (task #14) — the param builder is
`crates/holon-orgmode/src/block_params.rs`, not the `holon-org-format` path
this row named; `build_block_params` now emits `org_props::DRAWER_ORDER`
explicitly after the `drawer_properties()` emit loop, which structurally
cannot reach a `_`-prefixed key. RED-FIRST: `org_store_org_round_trip.rs`
gained a `WriteLeg` selector and a third case,
`non_alphabetical_drawer_order_survives_the_ingest_leg`, writing the same
non-alphabetical fixture through `build_block_params` instead of the Loro
projection writer — red on the unmodified tree for exactly the carrier
("mechanism: `build_block_params` must carry the authored-order carrier into
the store"), green after. GREEN: the file 3/3, holon-app + holon-orgmode +
holon-org-format 460/460, keystone-smoke pass-with-note. The dogfood
cold-boot re-run that produced this row is NOT re-run by this lane.
