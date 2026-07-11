# C4 — `Computation`: unify `Predicate`, `PrototypeValue`, and the silent SQL fallback

Spike design note. Status: DESIGN + de-risk. Ruling basis below.

## Ruling this implements

`docs/Proposals/VisionGapAnalysis-2026-07-11.md` (RULED Martin 2026-07-11):

> C4 derived fields — hide behind an interface; candidate design = generalize the
> existing Predicate trait to a **Computation** trait evaluable **in memory AND
> compilable to SQL**. Pipeline seat still open behind that interface.

Plus the registry ruling relayed by the coordinator: the registry question =
**Func-enum generalization by function shape** — the unifying type is a *closed
enum keyed by function shape*, not an open trait-object registry. This matches
"Parse, don't validate": make the set of computable shapes explicit and finite,
so both evaluators (in-memory + SQL) are total over a known match.

## Current state — three overlapping concepts, one silent hole

| # | Thing | Where | Evaluates | SQL | Notes |
|---|-------|-------|-----------|-----|-------|
| 1 | `Predicate<T>` **trait** | `crates/holon/src/core/traits.rs:121` | `test(&T)->bool` | `to_sql()->Option` | + `And/Or/Not` structs, `Lens<T,U>`, `Queryable<T>`. **No production impls** — only the test module + PBT infra. Dead static-dispatch design. |
| 2 | `Predicate` **enum** | `crates/holon-api/src/predicate.rs:17` | `evaluate(ctx)->bool` | — | The *actually-used* data-level, serializable predicate. Live callers: `row_pipeline.rs:86`, `render_interpreter.rs:773`. Built by the holon-profiles filter parser + `render_dsl`. |
| 3 | `ToSql for Predicate` | `traits.rs:22` | — | `to_sql_predicate()->Option<SqlPredicate>` | **SILENT partial-compile fallback** (see below). **No production callers** — dead SQL path. |
| 4 | `PrototypeValue` | `crates/holon-petri/src/lib.rs:292` | Rhai eval (in-memory f64) | — | `Literal(f64) \| Computed(CompiledExpr)`. The *computed-field* machinery (`rank_tasks`). No SQL. |

`CompiledExpr` (`crates/holon-expr`) is the shared Rhai vocabulary under (4);
`bounded_engine()` caps VM ops (vault data is untrusted).

### The silent hole (the thing the ruling calls out as "must become disclosed")

`traits.rs:65-93` — `Predicate::And`/`Or` collect children with
`filter_map(|p| p.to_sql_predicate())`, then compare lengths and return `None` if
any child failed; `Always => None`. A predicate that is perfectly valid
in-memory silently produces **no SQL filter**. A caller that then falls back to
"no WHERE clause" would silently widen the result set — exactly the "silently
degrades to look fine" failure the repo bans. Today no caller consumes it, so the
hole is latent; the moment C4 wires computed fields into matview/profile SQL it
becomes a live data-correctness bug.

## The unifying insight

A **predicate is a boolean-valued computation**. `PrototypeValue::Computed` is a
**numeric-valued computation**. `Literal`/`Eq`/`Var` are computations over
literals and field references. They are one algebra with two interpreters:

- **in-memory** — total over every shape (this is what the reactive pipeline and
  `rank_tasks` run today);
- **SQL** — *partial*: comparison/logic/arithmetic/field/literal lower cleanly;
  an arbitrary Rhai `Script` (switch, if/else chains) does not. This partiality
  is the real, irreducible fact — the fix is to **disclose** it, not hide it.

So PrototypeValue collapses into the new type and the boolean `Predicate` enum
becomes an *embedded* case of it; the `Predicate<T>` trait, `Lens`, `And/Or/Not`
structs, and the silent `ToSql` impl are deleted (no dual path).

### FRB constraint (decisive)

The boolean `Predicate` enum is `flutter_rust_bridge:non_opaque` — it crosses the
Rust↔Dart boundary as UI variant conditions. A Rhai `CompiledExpr` (opaque AST)
**cannot** live inside an FRB-exposed enum. Therefore `Computation` is a
**separate engine-side type that *embeds* `Predicate`**, rather than flattening
every shape into one FRB enum. This also means the existing `Predicate` enum and
all its call sites stay **unchanged** — generalization is by embedding, and the
migration churn is confined to the deletions + PrototypeValue.

## Design — `enum Computation` (function-shape keyed)

Lives in `holon-api` (already depends on `holon-expr` + `rhai`; no cycle — the SQL
*fragment* is just `String` + `Vec<Value>` like the existing `SqlPredicate`, so it
needs no turso dependency; only `to_params()` needs turso and stays in `holon`).
`Computation` is **not** FRB-exposed (engine-side only: row pipeline, rank_tasks).

```rust
pub enum Computation {
    Lit(Value),
    Field(String),                                       // column / context var
    Arith { op: ArithOp, lhs: Box<Computation>, rhs: Box<Computation> }, // + - * / (weights)
    Predicate(Predicate),   // the boolean-valued shape — embeds the FRB enum verbatim
    Script(CompiledExpr),   // arbitrary Rhai — in-memory ONLY
}
```

`Predicate` gains its own **disclosed** `compile_sql(&self) -> Result<SqlFragment,
SqlUnsupported>` (replacing the silent `ToSql`), and `Computation::compile_sql`
builds on it.

Two interpreters:

```rust
// TOTAL — every shape, in-memory. Script needs the bounded Rhai engine + a
// numeric scope built from ctx.
fn eval(&self, ctx: &Context) -> Result<Value, EvalError>;

// PARTIAL but DISCLOSED — the fix for the silent hole.
fn compile_sql(&self) -> Result<SqlFragment, SqlUnsupported>;
```

`SqlUnsupported` is a typed, informative error (`enum { Script(source), … }`) —
**never a bare `None`**. `Logic`/`Not` propagate a child's `SqlUnsupported`
upward (the `?` operator replaces `filter_map`), so an And with one Script term
fails loud, naming the offending sub-expression, instead of dropping it.

`Computation::eval` on the embedded `Predicate` returns `Value::Boolean(pred
.evaluate(ctx))` — reusing the existing, carefully-tuned boolean semantics
(`Var` truthiness, `compare_f64` fail-shut, `Ne` null rules) verbatim. `Script`
evaluates through the bounded Rhai engine over a numeric scope built from `ctx`
(same path `rank_tasks` uses today). `Arith` recurses and coerces via
`Value::as_f64`.

### Consuming the disclosed error (the caller contract)

A caller that wants SQL push-down calls `compile_sql()` and, on
`Err(SqlUnsupported)`, must choose a **disclosed** path per the repo's priority
order — either (a) evaluate that predicate in-memory over the candidate rows
(correct, slower — *disclosed degraded mode*, log + annotate), or (b) surface the
error. Never silently emit a WHERE clause missing the term. This is the same
CRDT-vs-LWW "degraded mode" precedent named in the C2b ruling.

## Migration (no dual path)

1. **DONE** — `Computation` (embedding `Predicate`) + `eval`/`compile_sql` +
   `predicate_to_sql` + 12 tests in `holon-api` (`crates/holon-api/src/computation.rs`).
2. **DONE / no churn** — the `Predicate` enum and every call site (row_pipeline,
   render_interpreter, profiles parser, render_dsl, PBT generators) are unchanged;
   generalization is by embedding.
3. **DONE** — deleted the silent enum→SQL path (`impl ToSql for Predicate`) AND
   the dead `Predicate<T>` trait, `Lens`, `And/Or/Not` structs, `Queryable<T>`,
   and `SqlPredicate` (all confirmed no production callers — `Queryable::query`
   had only a self-test, which now drives `Computation::compile_sql → query_raw`).
   `QueryableCache`/`nCache` is a *different* type and is untouched. The
   `Queryable` impl also carried its own silent "no SQL predicate → in-memory"
   fallback — removed with it.
4. **`PrototypeValue` — design refinement, NOT a naive fold** (see below).

### PrototypeValue: unify the semantics, keep the focused type

The obvious move — replace `PrototypeValue { Literal(f64), Computed(expr) }` with
`Computation` — is **wrong** by this repo's own "make illegal states
unrepresentable" rule: a prototype property is *only ever* a literal or a Rhai
script, so widening it to the 5-variant `Computation` would admit `Field` /
`Arith` / `Predicate` prototype values that every match site then has to guard
against. The correct unification is at the **evaluation semantics**, not the type:
`PrototypeValue` stays a focused 2-variant domain type whose `Computed(expr)`
shares `CompiledExpr` with `Computation::Script`, and its resolver can delegate to
`Computation::eval` to remove the *duplicate* Rhai-eval implementation (the real
"dual path"). That resolver dedup is a contained follow-up in `holon-petri`
(`resolve_prototype`) with a live E2E PBT (`petri_e2e_pbt.rs`) that independently
mirrors the loop — it deserves its own verified pass rather than being rushed, and
it does **not** block the C4 interface, which is landed.

## Pipeline seat — the one thing still OPEN (per ruling)

The ruling explicitly leaves open *where in the reactive pipeline* a computed
field is recomputed/retracted on input change. `Computation` is the **interface**
that makes that seat pluggable; it does not decide the seat. Two candidates,
recorded for the next ruling:

- **A. Turso matview column** — `compile_sql()` emits the field as a computed
  column; IVM maintains it. Fast, incremental, but only for SQL-compilable shapes
  (`Script` falls to degraded mode).
- **B. Reactive projection stage** — recompute via `eval` in the row pipeline
  when inputs change (analogue of ADR 0024 maintained display emission). Total
  (handles `Script`) but not IVM-incremental unless we track input deps.

The end-to-end proof below exercises seat B (total, in-memory) because it covers
every shape and is the one with an existing home (`row_pipeline`); seat A is the
SQL-push-down optimization gated on the compile path, proven separately by the
`compile_sql` round-trip test.

## Proof of correctness (deliverables)

- **Unit**: `eval` and `compile_sql` over each shape; `compile_sql` returns a
  *named* `SqlUnsupported::Script` (disclosed, asserted) — the anti-regression for
  the silent hole.
- **End-to-end computed field**: a `task_weight = priority_weight * (1 + urgency)`
  style `Script` field materialized through the pipeline, value asserted.
- **Incrementality**: change one input (`priority`) → recompute yields the new
  value and *only* the dependent field changes; unrelated fields are untouched.

## Risks

- `Script` in SQL is genuinely uncompilable — accepted; disclosed degraded mode.
- `Value` numeric coercion (Integer/Float/TEXT-affinity) must be identical across
  `eval` and Rhai's f64 world — covered by the existing coercion tests, extended.
- Deleting `Queryable<T>`/`Lens` — verify zero non-test callers before removal.
