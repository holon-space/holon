---
id: 2026-08-02-only-agent-facing-inspection-tool-can
date: 2026-08-02
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  `describe_ui` — the ONLY agent-facing UI-inspection tool — can never show a
  `live_query`'s rows, and reports a fully WORKING live_query as one
  placeholder item with empty columns. `describe_ui`
  (`frontends/mcp/src/tools.rs:2943`) routes to `snapshot_resolved`
  (`crates/holon-frontend/src/reactive.rs:445`), the headless shadow
  interpretation, and `shared_live_query_build` builds live_query content with
  `deeper_ctx.with_data_rows(vec![])`
  (`crates/holon-frontend/src/render_interpreter.rs:655`) — the empty row
  vector is unconditional, so the emitted subtree is `live_query > row >
  [spacer, icon, spacer, text "", spacer, text ""]` regardless of what the
  query returns. CONSEQUENCE, observed: this artifact directly caused a
  misdiagnosis. The dogfood lane read that output as evidence that "a nested
  live_query never binds its rows" and filed it as a data-binding defect; the
  actual bug is a zero-height layout collapse (see the companion COVERAGE
  row), and the "empty columns" it reasoned from were the tool's fixed output,
  not a symptom. An inspection tool that renders a working widget as broken is
  worse than one that omits it, because its output is indistinguishable from a
  real failure and is trusted as primary evidence.
source_line: 783
---

## Bug

(dogfood, ClaudeCode.org build-out; found by adversarial verification of
that lane's own headline claim) `describe_ui` — the ONLY agent-facing
UI-inspection tool — can never show a `live_query`'s rows, and reports a
fully WORKING live_query as one placeholder item with empty columns.
`describe_ui` (`frontends/mcp/src/tools.rs:2943`) routes to
`snapshot_resolved` (`crates/holon-frontend/src/reactive.rs:445`), the
headless shadow interpretation, and `shared_live_query_build` builds
live_query content with `deeper_ctx.with_data_rows(vec![])`
(`crates/holon-frontend/src/render_interpreter.rs:655`) — the empty row
vector is unconditional, so the emitted subtree is `live_query > row >
[spacer, icon, spacer, text "", spacer, text ""]` regardless of what the
query returns. CONSEQUENCE, observed: this artifact directly caused a
misdiagnosis. The dogfood lane read that output as evidence that "a nested
live_query never binds its rows" and filed it as a data-binding defect; the
actual bug is a zero-height layout collapse (see the companion COVERAGE
row), and the "empty columns" it reasoned from were the tool's fixed output,
not a symptom. An inspection tool that renders a working widget as broken is
worse than one that omits it, because its output is indistinguishable from a
real failure and is trusted as primary evidence.

## Missing piece

The tool reports a structurally-plausible EMPTY result where it cannot
produce a real one, instead of declining. Nothing in the output marks the
row vector as synthetic, so no consumer — human or agent — can tell "this
live_query has no rows" from "this tool does not evaluate live_query rows".

## Remedy

OPEN 2026-08-02 — no prod change. FIX DIRECTION (fail-loud, per the
project's error philosophy): `describe_ui` should either resolve live_query
rows for real, or emit an explicit unevaluated marker for that subtree —
never a silent empty-row placeholder that mimics a legitimate result. Until
then, treat any `describe_ui` output containing a `live_query` as UNKNOWN
for that subtree and verify rendering in a painted window instead.
