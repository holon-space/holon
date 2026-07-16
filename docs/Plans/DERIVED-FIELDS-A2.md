# C4 Derived Fields — Increment A2 (subset-first parser)

Status: LANDED (this increment). Ruling: Martin's C4 derived-fields rulings —
"A2 subset-first parser on the unchanged `=` surface"; the sidecar
`block_derived` target + CDC-watcher trigger is the **next** increment (ruled,
not built here).

This is the ground-level record for the A2 increment. The altitude view lives in
the vault (`Projects/Holon/`).

## What this increment delivers

A derived field declared as a `= …` property is now parsed by a **holon-owned
typed subset parser** into a `Computation` first, so the SQL-plantable shapes
(arithmetic, comparisons, `switch`, `if`) route to **seat A** (an IVM matview
column) instead of being forced onto the Rhai projection stage (seat B). The
user-facing `=` surface is UNCHANGED — anything outside the subset still works,
via a disclosed fall-back to the full Rhai compiler.

## 1. Spike verdict (gates the whole workstream)

`crates/holon-turso/tests/json_extract_matview_spike.rs` (green; each probe is an
executable record of the fork's IVM matview capability):

| Derived-column shape | Fork IVM matview | Result |
|---|---|---|
| `json_extract(properties, '$.p')` bare column | maintained + retracts | ✅ |
| `iif(cond, a, b)` (incl. over `json_extract`) | maintained + retracts | ✅ |
| searched `CASE WHEN … END` | rejected at DDL | ❌ |
| simple `CASE x WHEN … END` | rejected at DDL | ❌ |

The rejection is `Parse error: Cannot convert LogicalExpr to AST Expr: Case { … }`
— the fork's matview logical→AST conversion has no `CASE` arm.

Two consequences, both acted on in this increment:

1. **json_extract Field-binding is UNBLOCKED.** A block property (`block_raw`'s
   JSON `properties` column) can be planted as `json_extract(properties, '$.p')`
   and Turso IVM maintains it O(delta) with correct retraction. The next
   increment (sidecar target) can lower `Computation::Field` over a block
   property to `json_extract(…)` without a promoted-columns detour.
2. **`Computation::Case` lowers to nested `iif(...)`, NEVER to SQL `CASE`.** This
   is baked into `compile_sql` (see below), because `CASE` does not survive the
   fork's IVM planning.

## 2. Numeric semantics — type-faithful end-to-end (post-verifier)

A fresh-context verifier refuted the first cut's equivalence: `Computation::eval`
coerced every arithmetic operand to `f64`, so `5 / 2` gave `2.5` while Rhai (and
SQLite) give `2`; and whole-float SQL literals rendered without a decimal
(`3 / 2` = integer division) diverged from eval's `1.5`. The fix keeps
`Integer`/`Float` distinct through the whole pipeline, so all three evaluators
agree by construction (verified against the repo's Rhai engine):

- **`eval` arithmetic** (`arith_apply`): `int op int` → `int`, including integer
  division (`5 / 2 = 2`, `-5 / 2 = -2`); overflow and integer division-by-zero
  **fail loud** (`ComputeError::Arithmetic`), mirroring Rhai's default checked
  integer arithmetic. Any float operand promotes to a float result (IEEE,
  unchecked — `x/0.0` = ±inf, `0.0/0.0` = NaN, same as Rhai).
- **Equality has two faces** (both verified against Rhai): `Compare` `==`/`!=`
  and ordering are **numeric** (`5 == 5.0` is true) — matching Rhai `==` AND
  SQLite `=`. `Case` (`switch`) is **type-strict** (`switch 2 { 2.0 => … }` does
  NOT match) — matching Rhai `switch`. The two disagree cross-type, so the A2
  domain keeps a switch's scrutinee and case labels same-type.
- **SQL literal rendering** (`value_to_sql_literal`): floats render via `{:?}`
  (always a decimal point / exponent → SQLite REAL affinity); a non-finite float
  (±inf/NaN) has no SQLite literal and is a **loud plant error**
  (`InlineError::NonFiniteFloat`).
- **Unary minus** lowers to `Integer(0) - operand` (type-preserving: `-5` stays
  `Integer(-5)`), not `Float(0.0) - operand` (which wrongly promoted to float).
- **`eval_script` marshalling**: integers are now pushed into the Rhai scope as
  `INT` (i64), not silently coerced to `f64`. The old coercion made the Script
  (seat B) path disagree with the typed (seat A) path on integer `switch`
  semantics — a seat divergence the C4 design forbids. Now the two seats are
  observably identical.

## 3. `Computation::Case` design (`crates/holon-api/src/computation.rs`)

New variants on the `Computation` enum:

- `Compare { op: CmpOp, lhs, rhs }` — a boolean comparison between two
  sub-computations. Unlike `Predicate` (field-vs-literal), both sides are
  expressions, so `field > field` and `expr <= 0` are expressible. Needed for
  `if` conditions. Lowers to `(lhs op rhs)`.
- `Case { scrutinee, branches: Vec<(match_value, result)>, else_ }` — a
  multi-way conditional with **type-strict equality-match on the scrutinee**:
  - eval: evaluate `scrutinee`; take the `result` of the first branch whose
    `match_value` **structurally equals** it (`Value == Value`, type-sensitive),
    else `else_`. This mirrors Rhai's type-strict `switch` — deliberately NOT
    the numeric `values_match` used by `Compare`.
  - compile_sql: **nested `iif(...)`** — `iif(scrutinee = mv0, res0, iif(scrutinee
    = mv1, res1, else))`. The scrutinee fragment repeats per branch; params are
    emitted in left-to-right placeholder order so `SqlFragment::inline_sql`
    stays correct.
  - SQLite `=` is numeric, so eval and SQL agree only when a switch's scrutinee
    and case labels share a numeric type; the parser + A2 domain keep them
    same-type.

`switch` maps directly (`scrutinee = x`, literal case labels). `if c { r } else
if … else { e }` maps with `scrutinee = Lit(Boolean(true))` and each branch's
`match_value` being the boolean condition.

## 4. A2 subset parser (`crates/holon-api/src/expr_parser.rs`)

A hand-written recursive-descent parser over a deliberate **syntactic subset of
Rhai**, producing a `Computation` directly:

```
expr        := if_expr | switch_expr | comparison
if_expr     := 'if' comparison block ('else' 'if' comparison block)* 'else' block
switch_expr := 'switch' comparison '{' arm (',' arm)* ','? '}'
arm         := ('-'? number) '=>' expr | '_' '=>' expr     // labels: distinct numeric literals
block       := '{' expr '}'
comparison  := additive ( ('=='|'!='|'<'|'<='|'>'|'>=') additive )?
additive    := multiplicative ( ('+'|'-') multiplicative )*
multiplicative := unary ( ('*'|'/') unary )*
unary       := '-'? primary
primary     := number | identifier | '(' expr ')'
```

Notes:
- `if`/`switch` are value-position forms (top level, branch body, arm result,
  else, or inside `(...)`). A conditional in a bare arithmetic-operand position
  (`(if …) / x` without wrapping the conditional in its own parens) is **outside
  the subset** — Rhai accepts it, we fall back.
- Unary minus lowers to `Integer(0) - operand` (type-preserving; see §2).
- `switch` case labels are **numeric literals only** (Rhai requires constant
  cases) and **duplicates are rejected** at parse time (Rhai rejects them too) —
  parse, don't validate. `Integer(2)` and `Float(2.0)` are DISTINCT labels
  (type-strict), so they never collide.
- The parser is a **total function that fails loud with a typed
  `ExprParseError`** — it never swallows. `Err` is the caller's disclosed signal
  to fall back; it is NOT a user error.

## 5. Wiring into the petri `=` path (`crates/holon-petri/src/lib.rs`)

`PrototypeValue::Computed` now holds a `Computation` (was `CompiledExpr`).
`PrototypeValue::parse` tries the subset parser first; on reject it compiles the
full Rhai expression as a disclosed `Computation::Script` (seat B). A genuine
Rhai compile error stays loud, enriched with the subset rejection reason.

Seat-B routing is disclosed downstream at `DerivedFieldPlan::plan` time
(existing `tracing::info`), so no new log was added at the parse boundary.

### Which default petri props now plant (seat A) vs fall back (seat B)

All four default computed props are in the subset and route to **seat A**
(verified by `default_petri_props_parse_and_plant_to_seat_a`):

| Prop | Shape | Seat |
|---|---|---|
| `priority_weight` | `switch` → `Case` → nested `iif` | A |
| `urgency_weight` | `if/else if/else` → `Case` → nested `iif` | A |
| `position_weight` | arithmetic | A |
| `task_weight` | field arithmetic | A |

A user-declared `=` prop that uses a construct outside the subset (function
calls, string ops, `&&`/`||`, `let`, non-parenthesised conditional operands)
falls back to **seat B** (Rhai Script), disclosed.

## 6. Dual-eval equivalence PBT (the correctness artifact)

`crates/holon-api/tests/derived_field_dual_eval_pbt.rs` — a **differential**
property test: for every expression both parsers accept, the subset parser's
`Computation::eval` equals the Rhai evaluation.

- One generated abstract expression → one fully-parenthesised Rhai source string
  → fed to BOTH the subset parser and the Rhai compiler → evaluated over a shared
  context.
- Generator now covers **both integer and float** leaves, mixed int/float
  arithmetic, and int/float literal divisors (nonzero → no div-by-zero). Each
  `switch` is **monomorphic** (int scrutinee var + int labels, or float+float) —
  the only regime where Rhai's type-strict switch, SQLite's numeric `=`, and
  `Case`'s strict equality all agree. Comparisons and mixed arithmetic cross the
  two kinds freely (all three agree there). Conditionals appear only in value
  positions, never as arithmetic operands.
- 512 cases; plus **directed regressions** for the verifier's two counterexamples
  (`5 / 2`, `9 / 4 + 1`) and mixed/whole-float division; plus flagship tests
  asserting the `switch`/`if` defaults plant to seat A and eval-match Rhai.
- Non-vacuity verified by inversion (corrupting the equality core turns the
  property RED).

## 7. Third-evaluator (eval vs SQL) + a discovered fork bug

`crates/holon-turso/tests/derived_field_eval_vs_sql.rs` plants computations via
`DerivedFieldPlan::plan` into real matviews and compares SQLite's result to
`Computation::eval` on the same row — closing the eval↔SQL leg of the triangle.

**Discovered fork bug (flagged to the orchestrator, pinned as an executable
record):** the Turso fork's IVM **matview logical plan drops REAL affinity from
whole-number float literals** (`3.0` is planned as integer `3`). So a planted
`(3.0 / 2.0)` maintains as `1` and `(xi / 2.0)` (int column) as `4`, diverging
from eval's `1.5` / `4.5`. It is NOT a rendering bug — `inline_sql` correctly
emits `3.0` (holon-api unit test), and the direct query engine and any genuine
REAL column (`xf`) are correct; only the matview plan mis-types whole-float
*literals*. `matview_whole_float_literal_bug_is_pinned` records the wrong values
so the test flips RED when the fork is fixed.

Impact on A2: **none in production** — A2 computes the seat routing but does not
yet plant live derived matviews (that is the sidecar increment). The default
petri props remain numerically correct even under the bug (their whole-float
literals always meet a REAL column or fractional literal that re-promotes).

**Documented divergence — absent field:** `eval` on a missing field fails loud
(`MissingField`); `json_extract(props, '$.absent')` yields SQL NULL. A2 does not
yet lower `Field` to `json_extract`, so this is unreachable today; pinned by
`absent_field_divergence_is_documented` for reconciliation when the sidecar
binding lands (make them agree, or disclose the NULL-propagation).

## Next increment (ruled, NOT in this one)

The sidecar `block_derived` target + CDC-watcher trigger: plant derived columns
over block properties as an actual maintained matview, with `Computation::Field`
lowering to `json_extract(properties, '$.name')` (unblocked by the spike above).
The binding choice is settled — json_extract inline, not promoted columns.

**Blocking prerequisites for that increment (this increment's findings):**
1. Fix the Turso-fork whole-float-literal matview mis-typing (turso-fix
   workstream) — else planted float arithmetic over integer inputs is wrong.
2. Reconcile the absent-field divergence (eval fail-loud vs `json_extract` NULL).
