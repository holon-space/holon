---
id: 2026-07-18-seed-enumerates-descendants-block
date: 2026-07-18
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  seed enumerates descendants FROM EVERY block
source_line: 808
---

## Bug

Page navigation 5–10s on real vault (release build, SqlOnly, ~1038-block
vault): the main-panel page body is rendered from
`watch_view_e06c78ae5d0bb287` — a `WITH RECURSIVE` matview (region='main'
focus-descendants; twin `watch_view_7d520cb583fa91ee` for the right region)
whose recursive **seed enumerates descendants FROM EVERY block** (`SELECT …
FROM block AS _v1`) with an O(path-length) string cycle-guard (a
comma-joined `visited` path-set grown by string-concat each step, with a
`NOT LIKE` anti-membership check per row) and filters to the focused
`focus_roots.root_id` only at the very end → O(N×subtree). Measured LIVE via
the holon MCP `execute_raw_sql` `duration_ms`: cold `count(*)` = **11894ms**
at 1038 blocks (returns 0 rows — pure IVM maintenance, no result), vs
**6.9ms** for the non-recursive `block` matview and **8.8ms** for
`block_with_path` (which seeds correctly from roots); warm re-read
**0.13ms**. The single DatabaseActor serializes, so the 12s compute freezes
every concurrent query (probes returned `Transport error: no pending
response`). Definitional bug visible in `list_tables`: `block_with_path`
seeds `WHERE parent_id LIKE 'sentinel:%'` (cheap) while the focus-descendant
matviews seed from all blocks (expensive). Confirmed SqlOnly live ("Loro is
not enabled") — NOT the row-71 CRDT reseed.

## Missing piece

Budget holds at test scale but not vault scale: the ONE keystone focus doc
is 3 blocks and the prior "SqlOnly meets SLO" verdict measured 1.5k
SYNTHETIC blocks — neither makes the O(N×subtree) all-blocks seed bite;
needs a vault-scale focus-descendant rung asserting per-navigation
matview-materialize cost vs accumulated block count. AND (ORACLE) no
invariant would catch it even at scale because **navigation is structurally
uninstrumented**: `holon_api::latency_e2e::interaction_dispatched`
(latency_e2e.rs:107) has only 3 callers (reactive.rs:2439/2533,
operations.rs:28), ALL write-path edits, so a page-open emits no
`stage="e2e"` event; and `LatencySloLayer` is `#[cfg(debug_assertions)]`
(frontends/gpui logging.rs:200-204) so it is compiled OUT of release builds
— a release+warm-boot log shows NO latency line for a slow navigation
(absence of signal ≠ speed). Needs a navigation e2e span + a
release-reachable SLO oracle.

## Remedy

OPEN — root-caused live 2026-07-18 (investigation only; NO prod code
changed). Fix direction (SEPARATE decision for Martin, NOT applied): scope
the recursive seed to `focus_roots.root_id` (walk from the focused root, not
`FROM block`), turning O(N×subtree) into O(subtree); consider replacing the
string-visited cycle guard with the `block_with_path` pattern. Confirm the
recompute lands on the live navigation path (vs once-per-boot cold
materialize) with the wall-clock chrome-trace (`cargo run --release
--features chrome-trace`, `CHROME_TRACE_FILE=…`, then
`scripts/analyze-chrome-trace.py`). Inspection capability added this
session: "Matview-maintenance cost probe" recipe in
`.claude/skills/holon-diagnostics/SKILL.md` (SQL/IVM section). This row
SUBSUMES the instrumentation-hole finding (nav emits no e2e; oracle
release-stripped) as its ORACLE secondary — not given a separate row per the
skill's one-row-per-bug + "latency is ORACLE/ENVIRONMENT".
