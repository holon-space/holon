---
id: 2026-10-03-non-finite-float-property-stored-as-null
date: 2026-10-03
gap: ORACLE
secondary: COVERAGE
status: PARTIAL
summary: >-
  A NaN or infinite float in a block property was stored as JSON null on both
  write legs and reported success, instead of being refused by name.
---

## Bug
Found by agent exploration in the DD tracer Inc 3b lane. A block property
holding `Value::Float(NaN)`, `inf` or `-inf` (at any depth) went into storage
as `null`. The write reported success. Ruling D37.a: refuse such a value at the
write boundary with a named error.

## Root cause
Both stores hold each property as JSON, and JSON has no NaN or infinity.
SQLite has no NaN either and stores a bound NaN as NULL. Each door that turns a
float into JSON, SQL text or a bound SQL value had its own conversion, and each
one mapped a non-finite float to null or to bad SQL text:
- Loro leg: `encode_property_value` (crates/holon-loro/src/loro_backend.rs)
  serializes with serde_json, which writes a non-finite `f64` as `null`.
- SQL leg, properties bag: `value_to_json`
  (crates/holon/src/core/sql_operation_provider.rs) mapped a failed
  `Number::from_f64` to `Value::Null`.
- SQL leg, typed columns: `value_to_sql` and `set_field` emitted a raw `inf`
  SQL literal. The write failed with the unrelated error "no such column: inf"
  or, on another column, was accepted.
- Scalar bind: `value_to_turso_param` (crates/holon-turso/src/turso.rs) and
  `value_to_turso` (crates/holon/src/core/traits.rs, used by the queryable
  cache) bound `Real(NaN)`, which SQLite stores as NULL.
- Matview literals: `value_to_sql_literal` (crates/holon-turso/src/sql_utils.rs)
  and `inline_parameters` (crates/holon/src/api/backend_engine.rs) inlined
  `inf`/`NaN` as SQL text.
- Petri: a Rhai computed property that returned a non-finite number was
  accepted, and the number text "inf" or "NaN" in a property parsed as a float.
- Derived-field sidecar: `crates/holon-turso/src/derived_reconciler.rs` stored
  `"null"` for a computation such as `x / 0.0`, and kept a stale row when a
  value stopped computing.
- MCP vtable (crates/holon-mcp-client/src/mcp_vtable.rs): a pushed-down filter
  constraint holding a non-finite float became JSON `null` in the tool params,
  and an `inf` SQL literal in INSERT text.
- `Value::Json` with invalid JSON text became `null` in `try_into_json`.
- Arithmetic evaluator: `arith_apply` and `eval_script`
  (crates/holon-api/src/computation.rs) returned `Ok(Float(inf))` or
  `Ok(Float(NaN))` for `x / 0.0`, `0.0 / 0.0` or an overflow, against the module
  doc that promises a loud `ComputeError::Arithmetic`.
- SQL-computed floats: SQL arithmetic such as `big * 10.0` computes `inf` from
  finite inputs. A derived `Arith` field lowers to a matview column
  (`sql_planted`), so Turso IVM stores `inf` in that column, and no write door
  sees it. Row hydration (`turso_value_to_value` on the query path and a
  duplicate of it in `parse_row_values_with_schema` on the CDC path,
  crates/holon-turso/src/turso.rs) returned it as `Value::Float(inf)`.
- Render-spec arithmetic: `eval_arithmetic` (crates/holon-api/src/render_eval.rs)
  guarded only the zero divisor, so finite operands such as `1e308 * 10.0`
  gave `Float(inf)`. A render-spec `BinaryOp` over a `ColumnRef`
  (crates/holon-frontend/src/render_interpreter.rs) showed it as the text
  `inf`, and the worker's `WatchEnvelope` serialize `panic!` fired on the prop.
  `eval_binary_op` now returns the `finite_float` refusal of `computation.rs`;
  the interpreter renders it as an error node.
- Petri literals: `resolve_prototype` (crates/holon-petri/src/lib.rs) did not
  check merged `PrototypeValue::Literal` values, so Rust code could pass
  `Literal(inf)` into the marking and ranking map. It now returns
  `PetriError::NonFiniteLiteral`.
- Operation log: `OperationLogEntry::new` (crates/holon-core/src/operation_log.rs)
  called `.expect("Operation must be serializable")`, which is false once
  `Value` serialization can fail. It now returns the error with context.
- MCP server: `holon_to_json_value` (frontends/mcp/src/tools.rs), the row
  serializer of every query tool and of `query_and_watch`, wrote a non-finite
  float as the number `0`.
- Nested render evaluator (round 8b): `eval_to_interp` and its callers
  (`eval_to_value`, `resolve_args`, `resolve_states`) had no error channel. An
  arithmetic refusal became `Null`, so an operation parameter built from an
  overflowing expression was `Null` instead of refused.
- Props-only template path (round 9): `set_template` (GPUI
  `view_mode_switcher.rs`) re-interpreted the props of each live node without
  the full interpretation, so a template that evaluates to a refusal left the
  node as `text` with the props of an error node, and the cell showed nothing.
  A CDC update already took the full path and was correct.
- Rhai to `Value` converters (round 9): `entity.rs` and `entity_profile.rs`
  each had a private `Dynamic` to `Value` function, and both mapped a
  non-finite float to `Value::Float(inf)`. A computed field such as
  `base * 10.0` with `base = 1e308` stored `inf` in the row at the enrich seat
  and at the render seat.
- Render integer arithmetic (rounds 9 and 10): `eval_arithmetic` wrapped or
  panicked on integer overflow (`i64::MAX + 1`), and answered integer and float
  division by zero with `Null`.
- Computed-field refusal (round 10): once refused, a computed field showed
  `Null` and a `warn` in the log. A reader of the row could not tell it from an
  unbound field.
- Worker watch envelope (rounds 8 and 9): the `WatchEnvelope` serialize in
  `frontends/holon-worker/src/lib.rs` was inline in a napi seat, so it had no
  native test.

Neither JSON input (MCP, a submitted properties bag) nor org text can mint a
non-finite float: serde_json refuses `NaN`, `Infinity` and out-of-range numbers
such as `1e400`, and org drawer values decode to `Value::String`. The value
comes from a computation: Rust code, a Rhai script, or SQL arithmetic over
finite inputs.

## Missing piece
The test `a_non_finite_float_property_reads_back_as_null`
(crates/holon-loro/tests/block_source_boundaries.rs) reached the state and
asserted the silent `null` as correct — the oracle blessed the defect. The
keystone generates no typed (non-string) property values, so it cannot reach
the state at all.

## Remedy
One error type, `NotJson` (crates/holon-pattern/src/value.rs, re-exported as
`holon_api::NotJson`), names the key path and the refused value.
Every conversion listed above calls `NotJson::finite` or
`Value::try_into_json` and propagates the error. There is no
`From<Value> for serde_json::Value`, so a new caller must handle the error.
Two render-side conversions keep their own typed refusals:
`dynamic_to_value` (crates/holon-api/src/render_dsl.rs) and
`value_to_sql_literal` (crates/holon-api/src/computation.rs).
- Property writes: Loro `encode_property_value`, the SQL `partition_params`
  choke-point and `set_field` call `Value::reject_non_finite_float(key)`.
- Petri refuses non-finite literal text at its parse door, a non-finite
  context property (`NonFiniteContextProperty`) and a stored non-finite float
  (`InvalidPrototypeProperty`); the evaluator refuses a non-finite computed
  value.
- Float text parse doors refuse a non-finite result and name the text: the
  expression parser (`ExprParseErrorKind::NonFiniteLiteral`, no Rhai fallback
  in petri or `ComputedSpec::parse`), a UI condition literal
  (holon-profiles `parse_literal_value`), and a Logseq Transit float on import
  (`ImportError::NotStorable`).
- The derived-field reconciler deletes the row of a field that does not
  compute (eval error or non-finite value) and raises the condition
  `DerivedFieldNotComputed` until the field computes again.
- The arithmetic evaluator refuses a non-finite result of `Arith` or of a Rhai
  script with `ComputeError::Arithmetic`, which names the value and the
  expression.
- SQL reads: `turso_value_to_value` is the one row-hydration function for the
  query path and the CDC path. It refuses a non-finite REAL. A one-shot query
  fails as a whole and names the column and the query; a CDC change whose row
  holds one is left out of its batch, and the batch's `degraded` note names the
  relation, the rowid and the column. SQL can still compute and store `inf`
  in a matview column; the read refuses it.
- `Value`'s serde `Serialize` refuses a non-finite `Float` with an error that
  names `Value::Float` and the value, so no serde path writes `null`.
- The MCP server's row serializer refuses a non-finite float with the field
  path. A query tool and a watch's initial data return the error.
  `poll_changes` leaves out only the change that has no JSON form, names it
  in `omitted`, and passes each batch's `degraded` note through. The worker's
  MCP tools (frontends/holon-worker/src/lib.rs) do the same.
- The nested render evaluator returns `Result<_, ComputeError>` end to end.
  The render interpreter shows an error node, `parse_action_expr` refuses the
  operation with the error, the action watcher sets `RuleStatus::ExecError`,
  and the input, selectable, question and toggle builders show an error node.
- A template switch whose props-only re-interpretation fails
  (`NotAPropsUpdate`), or whose live node is an error node, replaces the item
  through the full interpretation (`interpret_and_attach`), as a CDC update
  does. The item recovers when the template becomes healthy again.
- `entity_profile::dynamic_to_value(d, source)` is the Rhai to `Value`
  converter of holon-api. It refuses a non-finite float through
  `computation::finite_float` and names the source expression.
- `holon_engine::value::Value::try_from_dynamic` is the converter of the Petri
  engine (`RhaiEvaluator::eval_compiled_dynamic`, reached by `eval_postcond`).
  It refuses a non-finite float, and `Engine::fire` returns the error naming the
  expression and the value, so a postcondition or create-arc attribute never
  puts a non-finite token attribute into a marking.
- A refused computed field raises `ConditionKind::DerivedFieldNotComputed`
  (the condition of the derived-field reconciler), keyed by block id and
  field, from every seat of `ProfileResolver` (`resolve_computed_only`,
  `resolve_with_computed`, `resolve_with_variants`). The next successful
  evaluation of that field on that block clears it. The row keeps the
  field's `Null`; the condition is the disclosure. The resolver and the
  sidecar reconciler use distinct `DerivedFieldSeat` subjects, so each clears
  only its own condition. Known limitation: the resolver sees no block
  deletes, so its condition stays raised after the block is deleted or loses
  its profile (the reconciler clears its own on delete). `ComputedFields` carries
  the per-field outcomes from `resolve_computed_fields_with_scope` up to the
  resolver, which owns the `ConditionBus` handle.
- `eval_arithmetic` refuses integer overflow (`integer overflow: a op b`),
  integer division by zero (`integer division by zero: a / 0`) and a float
  division by zero (the `finite_float` refusal), each with its operands.
- The worker's `watch_envelope_json` is a free function with a native test.
- Bind refusals name their field (`build_where_clause`, insert, update) or,
  for positional parameters, their placeholder (`?2`).

Tests (each one red before its guard, see the teeth logs of the lane):
- crates/holon-loro/tests/block_source_boundaries.rs (Loro refusal).
- crates/holon/src/core/sql_operation_provider_diff_test.rs (create, nested
  update, set_field, column value, both stores refuse the same write).
- crates/holon-pattern/src/value.rs (`try_into_json` path and value).
- crates/holon-turso/src/turso.rs and sql_utils.rs (bind, literal).
- crates/holon/src/core/traits.rs (`value_to_turso`).
- crates/holon-petri/src/lib.rs (computed value, number text).
- crates/holon-turso/tests/derived_field_sidecar.rs (proptest: no non-finite
  value lands; a value turning non-finite drops its row and is disclosed).
- crates/holon-mcp-client/src/mcp_vtable.rs (literal and JSON).
- crates/holon-api/src/computation.rs (`Arith` and Rhai results).
- crates/holon-turso/tests/non_finite_float_read.rs (one-shot query, view
  read, watched view).
- frontends/mcp/src/tools.rs (row serializer names the field path; a poll
  leaves out and names only the bad change).
- frontends/holon-worker/src/lib.rs (the same watch poll contract).
- crates/holon-api/src/expr_parser.rs, crates/holon-profiles/src/lib.rs,
  crates/holon-logseq-db/src/project.rs (float text parse doors).
- crates/holon-orgmode/src/file_io.rs (object header arg).
- crates/holon-frontend/src/reactive_view.rs (a CDC update and a template
  switch into an overflow render the full-path error node; the switch back
  recovers).
- crates/holon-api/src/computed.rs and entity.rs (computed field refusal at the
  enrich, render and `TypeDefinition` seats).
- crates/holon-api/src/render_eval.rs (integer overflow, division by zero).
- crates/holon-profiles/src/lib.rs
  (`a_refused_computed_field_is_disclosed_until_it_computes_again`: all three
  resolver seats raise the condition and the next success clears it).
- frontends/holon-worker/src/lib.rs
  (`a_non_finite_watch_envelope_is_sent_as_an_error_view_model`).

No keystone transition added: no production-faithful driver (UI, org file,
MCP) can write a non-finite float, and the keystone does not author derived
fields, so a transition would need to inject a raw `Value` and bypass the
drivers the keystone exists to exercise.

## Computed-field seats without a bus
`TypeDefinition::enrich_with` (crates/holon-api/src/entity.rs) and the
lightweight `resolve_computed_fields` have no block id and no bus. They refuse
the float, log a `warn` and return `Null`. Only tests call them.

## Open gaps (ruling D45 pending)
SQL can compute a non-finite float from finite inputs. These shapes are
measured (verify-nan-5) and stay OPEN until ruling D45. No read door can see
them, because the engine has already turned the value into something else:
- `x / 0.0`, `0.0 / 0.0`, `-1.0 / 0.0` and NaN arithmetic (`9e999 - 9e999`,
  `9e999 * 0.0`) give NULL.
- `CAST('inf' AS REAL)`, `CAST('Infinity' AS REAL)` and `CAST('nan' AS REAL)`
  give `0.0`, also in a planted matview column.
- `CAST(9e999 AS INTEGER)` gives `i64::MAX`.
- `printf('%f', 9e999)` and `'' || 9e999` give the text `"Inf"`.
- One row with a non-finite REAL makes a whole one-shot view read fail, so
  the good rows are not shown either.
