---
id: 2026-07-18-cold-focus-descendant-matview-navigation-latency
date: 2026-07-18
gap: ENVIRONMENT
secondary: COVERAGE
status: UNCLASSIFIED
summary: >-
  Cold focus-descendant matview navigation latency (RC5, latency-above-SLO,
  found by Martin/dogfooding): the first navigation to a fresh page root must
  materialize the main-panel focus-descendant content watch — the
  `default-main-panel` `WITH RECURSIVE focus_descendants` query
  (`assets/default/index.org:20-34`) that Turso IVM compiles into a recursive
  CTE. The holon_sql form is correctly SEEDED from `focus_roots(region=main)`;
  the all-blocks recursive scan is introduced by IVM's recursive-CTE
  compilation, so the cold materialization measured ~11.9s @1038 live blocks
  in the live app. The ONE keystone boots a 3-block focus doc, so this
  whole-vault materialization never manifests at test scale.
source_line: 1002
---

## Bug

Cold focus-descendant matview navigation latency (RC5, latency-above-SLO,
found by Martin/dogfooding): the first navigation to a fresh page root must
materialize the main-panel focus-descendant content watch — the
`default-main-panel` `WITH RECURSIVE focus_descendants` query
(`assets/default/index.org:20-34`) that Turso IVM compiles into a recursive
CTE. The holon_sql form is correctly SEEDED from `focus_roots(region=main)`;
the all-blocks recursive scan is introduced by IVM's recursive-CTE
compilation, so the cold materialization measured ~11.9s @1038 live blocks
in the live app. The ONE keystone boots a 3-block focus doc, so this
whole-vault materialization never manifests at test scale.

## Missing piece

keystone vault too small to surface the O(vault) descendant materialization;
AND the headless nav path (`apply_navigate_focus` → `settle_focus_matviews`)
polls only the lightweight `current_focus`/`focus_roots` tables and NEVER
registers the main-panel descendant content watch (boot registers only
`SEEDED_SIDEBAR_WATCH_ID`), so a bare NavigateFocus is structurally blind to
the cliff

## Remedy

NAV SOAK RUNG LANDED 2026-07-18 (`soak_nav_latency` in
`general_e2e_composed_pbt.rs`; test-only, env-gated skip-by-default via
`HOLON_SOAK_NAV=reproduce\ | zero` AND `HOLON_SOAK_SEED_BLOCKS>0`; RELEASE
profile; sibling of the reseed `soak_reseed_reproduction`). Drives 10 REAL
sidebar-click `NavigateFocus` to fresh `block:soak-doc-K` page roots at 2000
blocks (2036 live) via `SutFocusWrite::apply_navigate_focus`, and — CLOSING
the coldness gap above — after each nav registers the main-panel
focus-descendant watch (`FocusRootDescendants` GQL, the documented panel
shape `row_origin.rs:458`; `to_sql` has no recursive surface so the
query-equivalent GQL `MATCH (fr:focus_root),(root)<-[:CHILD_OF*0..N]-(d)` is
used, which IVM compiles to the same recursive descendant matview) through
the SAME production `ReactiveEngine::watch_query_live` path the real UI
uses, measuring register→settle. **NEGATIVE REPRODUCTION** at 2000 blocks ×
10 fresh roots (release): focus-write(nav) p50=474ms/p95=696ms;
focus-descendant matview (cliff locus) p50=56ms/p95=271ms/max=271ms (first
~271ms, steady ~50-72ms); nav→content-visible p50=566ms/p95=816ms — ALL
sub-second, ~2 orders of magnitude below the prod ~11.9s. Vault scale ALONE
(WIDE shape, shallow depth ≤4) does NOT reproduce it (271ms) — but **DEPTH
does**. RC5 CONFIRMED depth-driven at the compiled-CTE level: the recursive
`focus_descendants` matview's cycle guard `',' | | visited | | ',' NOT LIKE
'%,' | | id | | ',%'` is an O(path-length) string scan PER recursion step,
so cold-materialization cost is ~QUADRATIC in tree DEPTH (cf. the turso
counter test: 21× populate_work on a 13-node CHAIN). Added a
`HOLON_SOAK_SHAPE=wide\ | deep\ | mixed` seed-shape knob (`HOLON_SOAK_DEPTH`
default 200) — `deep` = a few linear nesting chains mirroring real outlines.
**REPRODUCTION (release, reproduce mode):** `deep` 1000 blocks / 5 chains ×
depth 200 (1031 live) → FIRST navigation's cold focus-descendant matview =
**19,029ms** (content 19,242ms), subsequent navs ~35ms (compiled IVM view
reused); `mixed` = **1,556ms**; `wide` 2000 blocks = 271ms (negative). So
the cliff is FIRST-navigation-after-boot × DEEP tree, worse than the prod
~11.9s@1038 — NOT concurrency-specific as first hypothesised, and NOT
count-driven. FIX = Turso IVM recursive-CTE compiler scoping / a
non-quadratic cycle-guard representation (e.g. a visited-SET not a
delimited-string scan). `reproduce` mode PASSES on `deep`
(descendant-matview p95=19029ms ≥ 1500ms floor); `zero` mode FAILS on `deep`
(content p95 ≫ 2000ms budget = cliff present) and PASSES on `wide` — a TRUE
red guard until the CTE fix lands. Reproducing invocation:
`HOLON_SOAK_NAV=reproduce HOLON_SOAK_SHAPE=deep HOLON_SOAK_SEED_BLOCKS=1000
HOLON_SOAK_DEPTH=200 HOLON_SOAK_SETTLE_MS=30000 cargo nextest run -p
holon-integration-tests --test general_e2e_composed_pbt --release
--no-capture -E 'test(soak_nav_latency)'`. SECONDARY (not triaged): the
main-panel descendant watch's `is_image` computed field logs a DISCLOSED
degraded-mode WARN (`Variable not found: content_type`) on the soak rows.
NEXT: the fix workstream is the Turso IVM recursive-CTE cycle-guard
(quadratic string scan); this rung (`HOLON_SOAK_SHAPE=deep`) is its ready
red guard.
