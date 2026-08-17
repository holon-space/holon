---
id: 2026-08-02-render-source-block-whose-dsl-fails
date: 2026-08-02
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  A render source block whose DSL fails to parse SILENTLY falls back to
  `table()` — the page shows a plausible, populated table and nothing tells
  the reader the authored render was discarded. Only a WARN reaches the log:
  `Failed to parse render_source, defaulting to table(): ...`
  (`crates/holon/src/api/block_domain.rs`, reached via `render_entity`). Hit
  17 times while authoring one page; each time the page looked fine. Direct
  violation of the repo's fail-loud rule (a parse failure is a config error,
  not a degradation) — the same block already has an `error` widget available
  for the disclosure.
source_line: 1138
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710) A
render source block whose DSL fails to parse SILENTLY falls back to
`table()` — the page shows a plausible, populated table and nothing tells
the reader the authored render was discarded. Only a WARN reaches the log:
`Failed to parse render_source, defaulting to table(): ...`
(`crates/holon/src/api/block_domain.rs`, reached via `render_entity`). Hit
17 times while authoring one page; each time the page looked fine. Direct
violation of the repo's fail-loud rule (a parse failure is a config error,
not a degradation) — the same block already has an `error` widget available
for the disclosure.

## Missing piece

No test asserts what a block RENDERS when its render_source is unparseable;
the DSL parser has its own unit tests, but the block-level fallback path is
unobserved. Missing piece = an invariant/keystone arm: a block with an
invalid render_source renders a visible error node, never `table()`.

## Remedy

OPEN — diagnosis only. Fix direction: return the parse error as
`ViewModel::error` (the widget exists,
`crates/holon-frontend/src/shadow_builders/error.rs`) instead of
`default_table()`.
