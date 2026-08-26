---
id: 2026-08-26-headless-services-render-live-query-blocks-as-error-widgets
date: 2026-08-26
gap: ENVIRONMENT
secondary: ORACLE
status: PARTIAL
summary: >-
  Every composed frontend-slice run renders the default left sidebar as an
  error widget, because the slice builds its trees through the MCP-path
  HeadlessBuilderServices, whose watch_query bails on live queries.
---

## Bug

In **every** headless composed run with the default layout, the left sidebar
renders as an error widget reading

```
Query error: HeadlessBuilderServices does not support live queries
```

carried by `block:default-left-sidebar`'s own live tree. The gates were green
throughout, because the only invariant watching for error widgets walked the
root layout alone — see
`2026-08-26-render-failure-invisible-warn-and-root-only-oracle`.

Surfaced the moment that invariant was widened to walk per-block live trees
(D35.a lane, this lane). First observation of the widened oracle, on its first
run: `just hand-authored` and the `logseq_parity` replay both fail with that
one signature and no other. `just keystone-smoke` stays green — the keystone
slice wires neither `SutViewSelection` nor `SutRenderer`, so the invariant
deselects there.

## Root cause

`HeadlessBuilderServices::watch_query` is an unconditional
`anyhow::bail!("HeadlessBuilderServices does not support live queries")`
(`crates/holon-app/src/headless_builder_services.rs:97`). The render
interpreter turns that `Err` into the error node
(`crates/holon-frontend/src/render_interpreter.rs:768`,
`Err(e) => Err(format!("Query error: {e}"))`), so any block whose render is a
nested `live_query` — the default sidebar among them — paints an error box
instead of its rows.

The prod/test divergence is that this stub is reached by the PBT slice at all.
Its own doc comment claims the opposite:

> This is NOT a test-fidelity concern: the E2E PBTs do not use this stub —
> they run windowless but drive the real `ReactiveEngine`
> (`headless_builder_services.rs:27-30`)

That claim is **false today**: the composed frontend slice's `services()`
constructs exactly this stub
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:1174`,
`Arc::new(HeadlessBuilderServices::new(self.engine.clone()))`), and every
`SutRenderer` / `SutViewSelection` tree the slice produces is interpreted
through it.

The limitation itself is not new and was measured before: the `#[ignore]`d
rung `journals_feed_cost_is_sublinear_in_history`
(`crates/holon-integration-tests/src/pbt/frontend_slice/structural_pbt.rs:4570-4596`)
records the identical string as its 2026-08-11 increment-0 result — per-day
content is unobservable headlessly because the nested `live_query` errors —
and parks the cost claim on a windowed rung. What was not known is that the
same incapacity paints a **user-visible error widget in the default layout of
every headless run**, unwatched.

## Missing piece

No headless implementation of `watch_query`, and no oracle reach to notice the
consequence. One-shot querying IS already available on the same stub
(`query_engine()` returns the engine, `headless_builder_services.rs:100-104`),
so a live query has a snapshot source; what is missing is an
`EnrichedChangeStream` built over it.

## Remedy

**PARTIAL.** Ruled D40.a (Martin, 2026-08-26) and implemented in the same lane:
`HeadlessBuilderServices::watch_query` now does a real one-shot compile +
execute via `QueryEngine::execute_query` (the no-matview, no-CDC path) and
delivers the rows as a single closed batch. The error widget is GONE — the
widened `inv-viewmodel-no-error-widgets` is green over the whole corpus, and
`just hand-authored` and the `logseq_parity` replay both pass again. The stub's
false "the E2E PBTs do not use this stub" doc comment is corrected.

Running the query for real is what keeps the failure loud: the interpreter's
`live_query` arm turns an `Err` into the block's error widget, so a malformed
or unexecutable query still surfaces. Measured cost: DDL and read distributions
across the whole `hand-authored` corpus are byte-identical before and after, so
the one-shot path adds no measurable SQL and creates no views.

**Residual, deliberately not closed here.** Headless still renders no query
ROWS. The interpreter's `live_query` arm calls `watch_query` only to
validate-by-doing — it drops the stream and interprets the item template
against EMPTY data rows, leaving the real subscription to the platform layer's
`BuilderServices::watch_query_live`, which `HeadlessBuilderServices` does not
implement. So a `live_query` block renders its empty table shell rather than an
error box.

Consequence, re-measured 2026-08-26: `journals_feed_cost_is_sublinear_in_history`
(`frontend_slice/structural_pbt.rs`) still cannot run, but its `#[ignore]`
reason was stale and has been corrected — the blocker is now the missing
platform watcher, not the bailing `watch_query`. Closing the residual means
implementing `watch_query_live` headlessly; that is a separate lane.
