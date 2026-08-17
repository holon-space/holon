---
id: 2026-08-08-dead-both-remaining-callers-each-built
date: 2026-08-08
gap: COVERAGE
secondary: "ENVIRONMENT (ply half only)"
status: FIXED
summary: >-
  `from descendants` was DEAD in BOTH remaining callers:
  `frontends/mcp/src/describe_ui_expand.rs:143` and
  `frontends/ply/src/render/builders/live_query.rs:68` each built a
  `QueryContext` with `context_path_prefix: None`, which `bind_context_params`
  binds as the literal `"__NO_PATH__/"`, and the `descendants` stdlib entry
  filters `path | text.starts_with $context_path_prefix` — so every
  context-dependent query these callers resolve matches exactly zero rows,
  silently.
source_line: 757
---

## Bug

(task #42 lane, found by code exploration in the #16/#27 chevron lane —
cd4cd93e fixed the gpui builder and named its untouched siblings — and
independently visible in the #27 acceptance run, where `describe_ui`
reported the nested page's rows as empty while the window painted them)
**`from descendants` was DEAD in BOTH remaining callers:
`frontends/mcp/src/describe_ui_expand.rs:143` and
`frontends/ply/src/render/builders/live_query.rs:68` each built a
`QueryContext` with `context_path_prefix: None`, which `bind_context_params`
binds as the literal `"__NO_PATH__/"`, and the `descendants` stdlib entry
filters `path | text.starts_with $context_path_prefix` — so every
context-dependent query these callers resolve matches exactly zero rows,
silently.** The MCP half is the load-bearing one: `describe_ui` is the
dogfood instrument, so a working nested page reads back as empty content,
and every bug diagnosed through it inherits the lie.

## Root cause

task #42 lane, found by code exploration in the #16/#27 chevron lane —
cd4cd93e fixed the gpui builder and named its two untouched siblings — and
independently visible in the #27 acceptance run: **`from descendants` was
DEAD in the MCP `describe_ui` expander and in the ply live_query builder,
both passing `context_path_prefix: None` into the arm that binds the
`"__NO_PATH__/"` sentinel**, so every context-dependent query resolved by
either caller matched exactly zero rows, silently. The MCP half is the worse
one: `describe_ui` is our dogfood instrument, so it reported a populated
nested page as empty content while the real window painted it — an
instrument that manufactures PERCEPTION gaps for every future bug looked at
through it. ONE row for the pair: one defect, one mechanism, two callers.
COVERAGE, not ORACLE: `describe_ui_deferred` already expanded a live_query
against a real `BackendEngine` with real seeded rows and already asserted
those rows appear — the oracle was adequate and fired on the first
context-dependent case; no case had ever carried a `query_context_id`, so
the missing thing is a generator/case arm. Secondary ENVIRONMENT for the ply
half only: `frontends/ply` is out-of-workspace with no `tests/` dir, no
dev-dependencies and no headless seam — its sole entry point is
`#[macroquad::main]`, so no test of any class could have caught it and none
was written now. FIXED both sites in-lane per the gpui pattern
(`QueryContext::for_block_with_path` off a resolved `lookup_block_path`),
each with the failure arms its frontend's idiom demands — MCP surfaces a
lookup failure as a `ViewKind::Error` field in the reply (the reply IS the
instrument), ply paints its `error_text` widget and WARNs the disclosed
no-query-engine degradation. Red-for-the-right-reason exists for the MCP
site only (`list [0 items]` behind two passing engine-level controls); the
ply site is fixed on the shared mechanism with its coverage gap left open
and stated. SCOPE corrected by a round-2 verifier: fixed for MATERIALIZED
context blocks only — `BlockDomain::lookup_block_path` has a pre-existing
`ALLOW(fallback)` returning `Ok("/{block_id}")` for any missing row, so a
stale/deleted/unmaterialized context still gets a wrong prefix and renders
confidently empty rather than erroring (true of the landed gpui fix too);
filed as task #45, same silent-default family as #41. Evidence
`docs/Testing/fixture-logs-2026-08-08/task42-descendants-context-prefix-siblings.txt`.)

## Missing piece

COVERAGE: `describe_ui_deferred` already expanded a live_query against a
real `BackendEngine` with real seeded rows and already asserted those rows
appear — the oracle needed no change and fired on the first
context-dependent case. What was missing is a case arm: no test had ever
given the node a `query_context_id`, so no expansion ever exercised context
binding at all. Secondary ENVIRONMENT, ply only: `frontends/ply` is excluded
from the workspace (its manifest names a macOS 15+ SDK issue in its
dependency chain), has no `tests/` directory, no dev-dependencies and no
headless seam — its only entry point is `#[macroquad::main]`, so no test of
any class could have caught this and none exists now.

## Remedy

**FIXED both sites in-lane 2026-08-08 (task #42)** per the cd4cd93e gpui
pattern: the context is built from a resolved `lookup_block_path` via
`QueryContext::for_block_with_path`. Failure arms follow each frontend's
idiom — MCP returns an `Err` naming the block and the consequence, which its
caller renders as a `ViewKind::Error` field in the reply (the reply is the
instrument, so a log line would not do), while the no-query-engine/Loro-only
case was already an error node and is left as-is because describe_ui cannot
expand ANY live_query without an engine; ply paints its existing
`error_text` widget on lookup failure and WARNs + falls back to a
prefix-less context when `query_engine()` is `None` (a legitimately
supported mode — the gpui lane rejected a version that panicked there), with
the resolution moved INSIDE the watch-cache `Vacant` arm so this
immediate-mode frontend does not take a blocking matview read every frame.
Red-for-the-right-reason exists for the MCP site only (`list [0 items]`
behind two passing engine-level controls,
`frontends/mcp/tests/describe_ui_deferred.rs::expanded_descendants_query_resolves_the_contexts_path_prefix`);
the ply site is fixed on the proven shared mechanism and its coverage gap is
left OPEN and stated — a ply regression of this class is still only
catchable by running the app. **SCOPE, corrected by a round-2 verifier:
fixed for MATERIALIZED context blocks ONLY.** The "lookup failure surfaces a
visible error" arm is unreachable in the dominant failure mode, because
`BlockDomain::lookup_block_path`
(`crates/holon/src/api/block_domain.rs:49-67`) carries a pre-existing
`ALLOW(fallback)` that returns `Ok(format!("/{block_id}"))` for ANY missing
row — so a deleted, stale, or not-yet-materialized context block yields a
WRONG single-segment prefix instead of an `Err`, and a deep block is
provably wrong (real `/block:outer/block:inner` vs fabricated
`/block:inner`), rendering confidently empty and indistinguishable from
genuinely empty. This holds in the MCP fix, in ply, AND in the landed gpui
fix. That fallback is a THIRD sibling of the same silent-default family as
the #41 sentinel and lives in untouched core code — filed as task #45.
Second ply residual, disclosed: the prefix is resolved once and pinned for
the watch-cache entry's life, so a context block re-parented later keeps the
stale prefix and shows silently wrong rows — strictly better than
always-dead, but not correct. The sentinel arm itself
(`crates/holon/src/api/backend_engine.rs:470-481`) is likewise deliberately
untouched: task #41, pending ruling. Evidence
`docs/Testing/fixture-logs-2026-08-08/task42-descendants-context-prefix-siblings.txt`.
