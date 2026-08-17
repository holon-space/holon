---
id: 2026-07-24-right-sidebar-renders-empty-pins-never
date: 2026-07-24
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  Right sidebar renders EMPTY — pins never appear. The seed GQL
  `assets/default/index.org` `default-right-sidebar::src::0` filters `MATCH
  (fr:focus_root) ... WHERE fr.region = 'right' ...`, but the production
  `focus_pin` op (`crates/holon/src/navigation/provider.rs:419`, the SAME op
  the PBT driver `SutNavHistoryDrive::pin_block` dispatches) inserts
  `navigation_history.region = Region::RightSidebar.as_str()`, which is
  `'right_sidebar'` (`crates/holon-api/src/types.rs:754`). The GQL
  `focus_root` node maps the `focus_roots` matview's `region` column VERBATIM
  (`crates/holon-turso/sql/schema/matview_focus_roots.sql`; generated matview
  SQL compares `_v0."region" = '<literal>'`), so `'right' = 'right_sidebar'`
  is always false → the real Turso GQL returns zero rows → the right sidebar
  shows no pins in both prod and the headless SUT. Every OTHER region consumer
  uses the full string (`'main'` = `Region::Main.as_str()`, `'right_sidebar'`
  elsewhere); `'right'` is the only short form — a typo. Found by static
  analysis while building the right-sidebar ordering oracle. CONFIRMED
  EMPIRICALLY: at load ~50 the COMMITTED
  `sidebar_renders_pages_in_declared_content_order` rendered GREEN (6.2s,
  left-sidebar pages present) while, in the SAME environment, the new
  right-sidebar oracle polled 22.7s (40× re-snapshots) and the right sidebar
  stayed EMPTY (`["block:default-right-sidebar"]`, zero pins) — ruling out
  load and pinning the cause to the region-literal mismatch.
source_line: 792
---

## Bug

Right sidebar renders EMPTY — pins never appear. The seed GQL
`assets/default/index.org` `default-right-sidebar::src::0` filters `MATCH
(fr:focus_root) ... WHERE fr.region = 'right' ...`, but the production
`focus_pin` op (`crates/holon/src/navigation/provider.rs:419`, the SAME op
the PBT driver `SutNavHistoryDrive::pin_block` dispatches) inserts
`navigation_history.region = Region::RightSidebar.as_str()`, which is
`'right_sidebar'` (`crates/holon-api/src/types.rs:754`). The GQL
`focus_root` node maps the `focus_roots` matview's `region` column VERBATIM
(`crates/holon-turso/sql/schema/matview_focus_roots.sql`; generated matview
SQL compares `_v0."region" = '<literal>'`), so `'right' = 'right_sidebar'`
is always false → the real Turso GQL returns zero rows → the right sidebar
shows no pins in both prod and the headless SUT. Every OTHER region consumer
uses the full string (`'main'` = `Region::Main.as_str()`, `'right_sidebar'`
elsewhere); `'right'` is the only short form — a typo. Found by static
analysis while building the right-sidebar ordering oracle. CONFIRMED
EMPIRICALLY: at load ~50 the COMMITTED
`sidebar_renders_pages_in_declared_content_order` rendered GREEN (6.2s,
left-sidebar pages present) while, in the SAME environment, the new
right-sidebar oracle polled 22.7s (40× re-snapshots) and the right sidebar
stayed EMPTY (`["block:default-right-sidebar"]`, zero pins) — ruling out
load and pinning the cause to the region-literal mismatch.

## Root cause

right sidebar renders EMPTY — region-literal mismatch. The seed GQL
`default-right-sidebar::src::0` filters `fr.region = 'right'`, but
production `focus_pin` writes `navigation_history.region =
Region::RightSidebar.as_str() = 'right_sidebar'` and the GQL `focus_root`
node maps the matview region column verbatim, so the real Turso GQL matches
NOTHING → the right sidebar shows no pins (prod + headless SUT). Invisible
to the composed keystone because the reference-side interpreter
`pbt::query.rs::gql_focus_region` parses whatever literal the GQL carries,
so the ref MIRRORS the SUT on `'right'` (both empty → agree, no divergence)
— a classic reference-mirrors-SUT ORACLE gap. Found by static analysis + the
new right-sidebar oracle rendering empty. FIX ESCALATED (not ruled):
correcting the literal spans the seed + `di/registration.rs` corpus + the
ref interpreter + the ref's pin-region keying, with real risk to the
currently-passing focus-root invariants; needs Martin's ruling. ORACLE
primary / COVERAGE secondary.)

## Missing piece

The composed keystone cannot catch it: the reference-side query interpreter
`crates/holon-integration-tests/src/pbt/query.rs::gql_focus_region` PARSES
whatever region literal the GQL carries (`'right'`) and keys the reference's
expected focus-root descendants on it, so the reference MIRRORS the SUT's
broken literal — ref and SUT both compute empty → they AGREE → no invariant
diverges. Classic reference-mirrors-the-SUT-bug ORACLE gap (same family as
the 2026-07-16 journals oracle-asymmetry). Secondary COVERAGE: no test ever
rendered the right-sidebar widget with pins and asserted non-empty.

## Remedy

OPEN 2026-07-24 — ESCALATED, no prod change landed (a one-token seed edit
`'right'`→`'right_sidebar'` was applied then REVERTED pending the ruling).
WHY escalated not ruled: (1) green-verification is impossible in this
session — a sustained load-~200 concurrent-agent build storm starves the
recursive-snapshot CDC settle, so no windowed/headless render resolves
nested rows (the COMMITTED `sidebar_renders_pages_in_declared_content_order`
also flaked empty under the same load, and passed at 5.3s in an earlier
lower-load window — direct evidence the flake is environmental); (2) the fix
is ENTANGLED — correcting the literal must touch the seed `index.org`, the
`di/registration.rs` canonical-query corpus, the ref interpreter `query.rs`,
AND the reference's pin-region keying in lockstep, or the currently-GREEN
focus-root invariants break; that lockstep + the ref-mirror convention is a
fork only Martin should rule. Red witness in the catalog:
`structural_pbt.rs::right_sidebar_renders_pins_in_declared_added_ts_order`
(`#[ignore]`) reds on the empty sidebar via its "both must render"
precondition. Once ruled+fixed, the SAME oracle advances to red on the
sortkey-override ordering (row below).
