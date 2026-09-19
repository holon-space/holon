---
id: 2026-09-19-badge-block-id-prop-is-stamped-unclassified
date: 2026-09-19
gap: COVERAGE
status: FIXED
summary: >-
  `badge("Page", #{block_id: col("id")})` stamped the vault's raw `id` text
  into the `block_id` prop, so `entity_id()` — which every reader of a marked
  badge calls — unwound on an id that forms no URI, and even a legitimate BARE
  id unwound there because the prop never carried its schemed form.
---

## Bug

Found by a VERIFIER probing the `block-ctor-sweep` lane's tree
(`lane-logs/verify-r2-probe.log`), not by any suite. Reachable from vault
content: `block_id` is written as `#{block_id: col("id")}` in a profile's own
render DSL — the builder's comment documents exactly that spelling — and the
`id` column is whatever the vault's SQL chose.

`badge` was a declarative widget, `fn badge(label: String, block_id: Option<String>)`
(`crates/holon-frontend/src/shadow_builders/badge.rs:8`), so the macro copied
the argument into the prop verbatim. The gpui builder then read it back with
`node.entity_id()` (`frontends/gpui/src/render/builders/badge.rs:21`), which
parses the prop strictly:

```
thread 'probe_e_badge_with_a_non_uri_block_id' panicked at
crates/holon-frontend/src/reactive_view_model.rs:1084:22:
live_block props["block_id"] must be a schemed EntityUri: Invalid URI "my task": unexpected character at index 2
```

Two more shapes had the same ending, and the fix for the first would not have
caught them:

- a BARE id (`abc`) is perfectly usable — `row_id_of_str` schemes it to
  `block:abc` — but the prop carried `abc`, and `entity_id()`'s strict
  `EntityUri::parse` rejects a value with no scheme. A legitimate vault id
  panicked.
- an EMPTY `id` column carried `""`, which `entity_id()` also parses.

Red for all three at `lane-logs/d13-badge-red.log:27`, `:49`, `:70`.

## Root cause

`entity_id()`'s `.expect` is correct where it was written: `ViewModel::live_block`
takes a typed `EntityUri`, so a `live_block` node's prop is always a serialized
URI and a malformed one is a programming error. `badge` then reused the same
prop NAME from a `String` parameter, and inherited a contract it does not meet
— the assertion's message still says `live_block props["block_id"]` while
reading a badge's.

The D142.a rule is that an id is classified once, at the boundary where the
text enters. For `badge` that boundary is the builder, and the builder was
declarative, so there was no place for the classification to happen.

## Missing piece

No test built a `badge` through the render DSL with an `id` column that is not
already a schemed URI. The row-id lane's windowed lock
(`frontends/gpui/tests/row_id_boundary_windowed.rs`) does put an unusable id
through a badge, but inside a COLLECTION, where the row pipeline refuses the
row before any leaf builder runs (`row_pipeline.rs:158`) — so it pins the row
refusal and never reaches the badge's own prop.

The keystone cannot draw it for the same reason as the sibling entries: every
id it mints is URI-safe by construction (`generators.rs:1029`,
`create_block_under_focus.rs:139`). DEFERRED, flagged on
`2026-09-19-live-block-id-that-forms-no-uri-kills-the-window`.

## Remedy

FIXED (lane `block-ctor-sweep`, delta 4). `badge` keeps its declarative
signature but now carries a body that classifies with `row_id_of_str`:
`Unusable` returns `ViewModel::refused_row(&refusal)` (the same surface
`drawer` uses), `Entity` carries the SCHEMED string, and `Absent` drops the
prop entirely so nothing downstream parses an empty value.

`badge` was also removed from `dispatch_resolve_props`
(`shadow_builders/mod.rs`). That props-only fast path calls the
macro-generated `resolve_props_from_args`, which copies the argument through
unclassified — leaving it wired would have let a row's id change to an
unusable value and re-stamp the raw text, bypassing the builder body. A badge
props recompute now takes the full interpret, so both paths agree.

Pinned by four tests in `crates/holon-frontend/src/shadow_builders/badge.rs`,
covering the unusable, bare, absent and empty shapes.
