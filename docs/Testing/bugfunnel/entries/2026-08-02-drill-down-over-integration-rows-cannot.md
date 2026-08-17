---
id: 2026-08-02-drill-down-over-integration-rows-cannot
date: 2026-08-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A drill-down UI over integration rows cannot be driven from the MCP surface
  at all, so it cannot be verified by an agent. `click {"entity_id":
  "cc-project:..."}` HANGS forever (no response, no error, no timeout — the
  app stays healthy; `driver.click_entity(..).await` in
  `frontends/mcp/src/tools.rs:3320+` never resolves for a non-block
  EntityUri), and the `x`/`y` fallback never hits an `expand_toggle` chevron
  (five attempts across three coordinate systems, verified by re-reading the
  chevron glyph in `describe_ui`). `describe_ui` is also blind to
  lazily-materialised content, so the drill-in state cannot even be OBSERVED
  headlessly.
source_line: 1146
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710) A
drill-down UI over integration rows cannot be driven from the MCP surface at
all, so it cannot be verified by an agent. `click {"entity_id":
"cc-project:..."}` HANGS forever (no response, no error, no timeout — the
app stays healthy; `driver.click_entity(..).await` in
`frontends/mcp/src/tools.rs:3320+` never resolves for a non-block
EntityUri), and the `x`/`y` fallback never hits an `expand_toggle` chevron
(five attempts across three coordinate systems, verified by re-reading the
chevron glyph in `describe_ui`). `describe_ui` is also blind to
lazily-materialised content, so the drill-in state cannot even be OBSERVED
headlessly.

## Missing piece

The windowed PBT drives blocks by entity id; a collection row whose entity
is an integration entity (not a block) is outside every driver's reach, so
no automated layer can open one. Missing piece = `click_entity` resolving
(or failing loudly) for non-block EntityUris, plus a `describe_ui` mode that
materialises lazy slots.

## Remedy

OPEN — diagnosis only. This is what forced the nested-live_query defect
above to be proven by screenshot rather than by an assertion.
