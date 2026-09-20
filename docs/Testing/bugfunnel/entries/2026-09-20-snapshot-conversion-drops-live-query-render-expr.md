---
id: 2026-09-20-snapshot-conversion-drops-live-query-render-expr
date: 2026-09-20
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  The ReactiveViewModel→ViewModel conversion drops a live_query's `render_expr`,
  so every snapshot consumer sees a node that cannot describe its own rows —
  `describe_ui(expand_deferred=true)` expands no live_query at all.
---

## Bug

`describe_ui` with `expand_deferred=true` never expands a real `live_query`.
Every such node comes back as `Unevaluated { mechanism: LiveQueryRows }` with
the reason `"rows not evaluated: the node carries no query/query_lang/
render_expr, so it cannot describe its own result"`, even when the node has a
valid query and the query returns rows. The MCP expansion path, its
`EngineResolver`, and its tests are therefore dead in the shipped frontend.

The same `None` is why the headless keystone renders every `live_query` over an
empty row set — the shape recorded in
`2026-09-20-expanded-embedded-page-renders-none-of-its-children`. That entry
attributes the harness's empty rows to the missing watch lifecycle; this entry
records the layer beneath it, which also breaks a shipped frontend.

Found by agent exploration: lane `embedded-page-lazy-rca`, spike S0 for ruling
D164.a. A throwaway fill in `recompute_widget_snapshot` counted the props on
every `live_query` node the headless snapshot produces.

## Root cause

`crates/holon-frontend/src/reactive_view_model.rs:1579`, in the `"live_query"`
arm of the `ReactiveViewModel` → `ViewKind` conversion:

```rust
render_expr: None, // TODO: store in props as serialized
```

The value is not missing upstream. `crates/holon-frontend/src/shadow_builders/
live_query.rs` serialises it into the node props alongside the others:

```rust
__props.insert("render_expr".to_string(),
    Value::String(serde_json::to_string(&result.render_expr).unwrap_or_default()));
```

The conversion reads `query`, `query_lang` and `query_context_id` back out of
those props and drops this one on the floor.

Consumer that breaks: `frontends/mcp/src/describe_ui_expand.rs:304` requires
`(Some(query), Some(query_lang), Some(render_expr))` before it will call
`EngineResolver::expand_live_query`.

GPUI is unaffected, and that is the parity hazard: its builder reads
`render_expr` off its own props in `frontends/gpui/src/render/builders/
live_query.rs`, never through this snapshot conversion. So the one shipped
frontend that works is the one the conversion does not serve, and every
snapshot consumer — MCP, PBT harness, TUI — gets a node it cannot act on.

Evidence: 456 `ViewKind::LiveQuery` nodes walked across 404 snapshot passes of
the hand-authored keystone suite, 1–2 per pass, in 404 of 404 passes; `query`
and `query_lang` present on all of them, `render_expr` `None` on all 456
(`lane-logs/spike-ha-fillstats.jsonl`, first fill arm; summary in
`lane-logs/shared-live-query-spikes-report.md`). With the field restored from
the props, 1087 of 1087 nodes resolved, 0 failed, 110 rows delivered, and the
suite advanced from case 25 to case 90 with no new red invariant
(`lane-logs/spike-ha-base.log` vs `lane-logs/spike-ha-fill.log`).

## Missing piece

No invariant asserts that a `live_query` node in the snapshot can describe its
own rows — that the props the builder wrote survive the conversion the
frontends read. The conversion is a lossy boundary with nothing policing its
totality, so a `TODO` sat in a shipped path with no alarm on it. The MCP
`describe_ui` deferred-expansion tests all construct their `ViewKind::LiveQuery`
by hand (`frontends/mcp/tests/describe_ui_deferred.rs`), so they set
`render_expr` themselves and never exercise the conversion that drops it.

## Remedy

Fixed.

Gap-closing rung:
`describe_ui_expand::interpreted_node_tests::an_interpreted_live_query_expands_to_its_real_rows`
(`frontends/mcp/src/describe_ui_expand.rs`). It interprets `live_query(#{sql:
…, item_template: …})` from render-DSL source and snapshots it, so the
conversion is on the path; it then runs `resolve_deferred` under
`DeferredPolicy::Expand` and asserts both seeded rows render. Red before the
fix with the reason string `"the node carries no query/query_lang/
render_expr"` (`lane-logs/i05-red.log`), green after
(`lane-logs/i05-green.log`).

Fix, in three places:

- `crates/holon-frontend/src/reactive_view_model.rs`, `"live_query"` arm: the
  `render_expr` prop is REQUIRED and parsed at the boundary into `RenderExpr`.
  Absent, non-string and unparsable all become `ViewKind::Error` naming the
  prop, the serde error and the node (its context block, else its query or
  source) — no `None` is left in the field whose silent `None` was the bug.
  Pinned by
  `reactive_view_model::tests::a_live_query_snapshot_without_a_readable_render_expr_is_an_error_node`
  (red: `lane-logs/i05b-red-conversion.log`, green: `lane-logs/i05b-green.log`).
- `crates/holon-api/src/render_dsl.rs`, `dynamic_to_value`: a non-finite float
  literal is refused where the DSL literal is parsed. `text(1e400)` parsed to
  `Literal { Float(inf) }`, which `serde_json` writes as `null` and reports
  success — so the template a consumer read back was not the one the author
  wrote, and no error fired anywhere. Pinned by
  `render_dsl::tests::a_non_finite_float_literal_is_refused`
  (red: `lane-logs/i05b-red.log`, green: `lane-logs/i05b-green.log`).
- `crates/holon-frontend/src/shadow_builders/live_query.rs`: the producer's
  `serde_json::to_string(..).unwrap_or_default()` made an empty string
  representable. With the non-finite literal refused upstream, a `RenderExpr`
  always has a JSON form, so the call states that invariant with `expect`
  rather than carrying a branch nothing can enter.

Known limitations, none of them this defect:

- `Value` is `#[serde(untagged)]` (`crates/holon-pattern/src/value.rs`), so a
  `Value::DateTime` or `Value::Json` in a template would return as
  `Value::String`. Neither is reachable through this prop: `dynamic_to_value`
  emits only Integer/Float/Boolean/String/Null, and no Rust-built `RenderExpr`
  reaches a `live_query` item template. Recorded in
  `~/.claude/plans/shared-live-query-lifecycle.md` §12.
- A `RowSourceSpec::Named` live_query carries `source` props, not
  `query`/`query_lang`, so `describe_ui_expand.rs:304` still reports "rows not
  evaluated" for named-source nodes even with a valid `render_expr`.
- `EngineResolver` builds its `RenderContext` without the `context_entity`
  that `watch_query_live` and `shared_live_query_build` both set — the third
  copy of the live_query lifecycle, which Inc 5 deletes (plan §12).
