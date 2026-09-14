---
id: 2026-09-14-nested-live-query-context-col-silently-binds-row-id
date: 2026-09-14
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A nested live_query written as `context: col("some_field")` silently ignores
  the column and binds the enclosing row's `id` instead, so the child query
  answers for the wrong subject with no error anywhere.
---

## Bug

The render DSL advertises a per-row nested query: a `live_query` inside another
collection's `item_template`, scoped to the enclosing row by a `context`
argument. Writing that the obvious way —

```
live_query(#{sql: "...", context: col("provider_name")}, ...)
```

— does not scope the child query to `provider_name`. The `col(...)` expression
is discarded and the child query is bound to the enclosing row's `id` column
instead. Nothing warns, nothing fails, and the child list renders rows: just
the wrong ones, for whatever entity happens to share that id.

Found by code audit on 2026-09-14, while verifying whether template nesting
could join provider rows to conditions for D120.a item 3 (lane
`error-remedy`, design doc `~/.claude/plans/error-remedy-design.md` §3.4). It
is latent, not observed in the product: no template in the tree uses
`context: col(...)` today, which is why it has never bitten.

## Root cause

`context` is a legal template argument. It is in the allowlist at
`crates/holon-api/src/render_eval.rs:763`, and
`frontends/mcp/src/describe_ui_expand.rs:29` documents the shape as "the
ordinary per-row nested query".

The builder never evaluates it. `shared_live_query_build`
(`crates/holon-frontend/src/render_interpreter.rs:674`) resolves the argument at
`:702-712` with `ba.args.get_string("context")`, which reads only the `named`
scalar map. A `col("provider_name")` argument is stored unevaluated in
`ba.args.templates`, so `get_string` returns `None` and control reaches the
fallback `ba.ctx.row().get("id")`.

The fallback is correct on its own terms — an id-scoped child query is the
common case — but it is reached for two very different reasons: "the author
asked for the default" and "the author asked for a column and we could not read
it". Collapsing those is what makes the failure silent. This is the
`_ => default` shape CLAUDE.md's parse-don't-validate section warns about: an
argument the author wrote is dropped instead of refused.

## Missing piece

**Primary (COVERAGE):** no transition authors a template containing a nested
`live_query`, and no seeded asset contains one, so no keystone case can reach
the code path at all. The alphabet cannot produce the interaction.

**Secondary (ORACLE):** even if a case reached it, no invariant compares the
rows a child collection renders against the subject its parent row names, so a
wrong-subject binding would pass. The defect would still have escaped after the
coverage gap was closed.

## Remedy

Open. Not fixed in this lane, and deliberately not on its critical path:
D121.a ratified the grouped-named-source join (`conditions_by_subject` over
`LiveData::group_by`) precisely because the nesting alternative is a bug to fix
before it is a feature to use.

When it is fixed, the fix is to evaluate the `context` template argument against
the enclosing row and to **refuse loudly** when it names a column the row does
not carry, rather than falling back. The default must stay reachable only when
no `context` was written at all. Closing the coverage gap needs a seeded
template with a nested `live_query` so the keystone can reach the path; closing
the oracle gap needs an invariant that a child collection's rows belong to the
subject its parent row names.
