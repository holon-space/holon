# Adversarial verification: W1 prod fixes (A0 fallback, Rhai injection, MCP panics)

Reviewer: skeptical-senior-rust pass, 2026-07-06. Read-only source audit; no code run.
Scope: the three fixes described as landed in main. Verdict per fix below; findings ordered by severity.

---

## Fix 1 — A0 sort_key fallback removed (withhold + unsettle)

### F1.1 HIGH — share-projection worker has NO settled gate: withheld node becomes a REAL SQL DELETE
- Where: `crates/holon-loro/src/loro_share_backend.rs:337` (`snapshot_blocks_from_doc(&doc)` — settled flag discarded) and `:353` (`diff_snapshots_to_ops(&before, &after)` applied via `execute_batch_with_origin` with no delete withholding).
- The main-doc path (`loro_sync_controller.rs:404`) withholds deletes when `!after_settled`. The shared-tree ProjectionWorker does not: a node withheld from `after` (either the NEW missing-fi cause or the pre-existing transient meta-incomplete cause) diffs as `delete` against the fork-at-watermark `before` and deletes the block row from SQL.
- Concrete scenario: block inside a shared subtree is mid-mutation (meta not yet readable) or carries no fractional index when the debounced `share.project` tick fires → its SQL row is deleted; next settled tick re-creates it (add/remove CDC churn through the share path), or if the fi absence is persistent the row is permanently gone while the block is alive in Loro.
- Fix: use `snapshot_blocks_from_doc_settled` in the worker and gate the delete ops exactly like `loro_sync_controller.rs:404` (also freeze the watermark on unsettled, since this path advances `watermark` unconditionally on success).

### F1.2 MEDIUM-HIGH — `project_sort_keys` conflates the two meanings of `None` and silently aborts the whole batch
- Where: `crates/holon/src/core/sql_block_operations.rs:729-741`, specifically `:740` `None => return Ok(()), // SqlOnly — SQL already owns sort_key`.
- `live_sort_key` (block_cell_registry.rs:855) returns `Ok(None)` for BOTH (a) SqlOnly mode and (b) Loro mode where `block_sort_key` finds `tree.fractional_index(tree_id).is_none()` (loro_backend.rs:2368) — i.e. the exact degraded state Fix 1 is about. In case (b) this function: (1) silently skips the block whose sort_key it exists to write, (2) `return`s — aborting projection for ALL remaining ids in the batch, (3) logs nothing. That is a silent-swallow (CLAUDE.md fail-loud violation) hidden inside the fix itself.
- Concrete scenario: org-scan reconciler calls `project_sort_keys([a, b, c])`; `a` has no fi → `b` and `c` keep their stale/default "A0" keys with zero disclosure — the very mis-sort the function documents as its reason to exist.
- Fix: resolve mode ONCE up front (`has_loro_backing()`); in Loro mode treat inner `None` as `tracing::error!` + `continue` (or a hard `Err`), never `return Ok(())` mid-batch. Parse-don't-validate: make `live_sort_key` return an enum (`SqlOwnsKey | Key(String) | MissingFi`) instead of `Option<Option<…>>` flattened to `Option`.

### F1.3 MEDIUM — a PERSISTENT missing-fi node freezes the base and withholds deletes vault-wide, forever
- Where: `crates/holon-loro/src/loro_sync_controller.rs:404` (delete gate) and `:499` (`if after_settled … put_base`).
- The unsettle mechanism was designed for TRANSIENT states (mid-mutation meta). Fix 1 reuses it for an invariant violation that can be PERMANENT (fi truly absent in the CRDT state, replicated to every peer). If that happens: (a) real user deletes never project to SQL again (gate at :404 trips on every pass), (b) the base never advances (:499), so EVERY subsequent projection pass re-diffs the full doc against an ever-older base and re-applies a growing op set — unbounded re-projection churn plus one `tracing::error!` per pass. There is no repair/quarantine path and no user-visible banner (log-only disclosure; "falls back visibly" per CLAUDE.md is arguable but weak).
- Fix: distinguish `Unsettled::Transient` from `Unsettled::InvariantViolation`; for the latter surface a DegradedSignalBus banner and/or a one-shot repair (re-mint the fi via a Loro `mov` to its current position — that is an ordering write the Loro authority is entitled to make, not a fake sink-side "A0").

### F1.4 LOW — `diff_and_emit_after_import` ignores settled and `snapshot_blocks()` swallows errors (currently dead code)
- Where: `crates/holon-loro/src/loro_backend.rs:2341-2346` (`.unwrap_or_default()` — a `with_read` failure yields an EMPTY map; diffing against it emits Deleted for every block) and `:2391-2415` (delete loop with no settled gate).
- No prod caller found (only the definition; grep across crates). If this path is ever revived for inbound-sync CDC it reintroduces exactly the spurious-delete bug F1.1 describes, plus the unwrap_or_default error swallow. Delete it or port the settled gate now, while the context is fresh.

### F1.5 SOLID — the parts I could not break
- `Vec<Option<String>>` consumption: exactly two consumers. Snapshot path (loro_backend.rs:878-902) destructures `Some(Some(k))` and withholds otherwise; `block_sort_key` (:2363-2384) pre-checks `fractional_index().is_none()`, and `keys[i]` cannot panic (`effective_sibling_sort_keys` maps 1:1 over `siblings`, `position()` guarantees `i < len`). No unwrap/index on the Option shape anywhere else (grep).
- Tie-suffix ordering claim verified: `.` = 0x2E sorts below `0`-`f`, so `"{fi}.{run:06x}"` stays between `fi`-prefixed neighbours; run_pos capped at 16^6 siblings.
- Can `None` hit VALID data? In loro 1.11.1 (pinned `=`), every handler create/move path mints a position (`FiIfNotConfigured::UseJitterZero`/`Zero`; `Throw` errors instead), and `TreeState::new` defaults to `GenerateFractionalIndex{jitter:0}` — `enable_fractional_index` is belt-and-braces. `NodePosition.position = None` is only representable from a `TreeOp` that carries `position: None`, which 1.x handlers never emit; realistic source is a pre-1.0 legacy doc. `fork_at` snapshots read STORED positions, so fork-based diffs (share backend :348) are unaffected by config. Orphan-children worry is moot on the main path: the withheld node's existing SQL row survives (delete withheld), so children never point at a missing row — except via F1.1's share path, where the parent row IS deleted and children remain: one more reason F1.1 is the real hole.

---

## Fix 2 — Rhai injection (AttrInit Literal/Expr + rhai_string_literal)

### F2.1 MEDIUM (latent, one-way door) — `#[serde(untagged)]` makes every `Literal(String)` deserialize as `Expr`
- Where: `crates/holon-engine/src/arc.rs:186-192` (`#[serde(untagged)] enum AttrInit { Expr(String), Literal(Value) }`) with `crates/holon-engine/src/value.rs:4` (`Value` itself untagged).
- `Literal(Value::String("Robert\") + evil()"))` serializes to the bare YAML/JSON string; deserializing tries `Expr(String)` FIRST and always wins for strings. Any future round-trip of a programmatically-built net (persistence, IPC, a `save-net` CLI verb) silently re-classifies escaped user DATA as CODE — the injection returns through the serde back door with zero compile errors.
- Today: not exploitable — `CreateArc` is only DESERIALIZED (yaml/net.rs:79, author-controlled config); `state.rs`/`history.rs` serialize tokens/events, not arcs; the rank_tasks net is in-memory only. But the type is a loaded gun.
- Fix: custom `Deserialize` keeping the YAML string-means-Expr convention but a distinguishable serialized form for Literal (e.g. map `{lit: …}` / YAML `!lit` tag), or at minimum `impl Serialize` that returns an error for `Literal(Value::String)` so the round-trip fails loud instead of flipping semantics.

### F2.2 SOLID — `rhai_string_literal` (holon-petri/src/lib.rs:1008-1024): could not break it
- Escapes `"` `\` `\n` `\r` `\t`. Rhai double-quoted strings have NO interpolation (`${}` is live only in back-tick literals), so interpolation syntax is inert data. Raw control chars (NUL, VT, U+2028…) and all unicode pass through inside the literal as data — Rhai's lexer only terminates on unescaped `"` or line-break, both covered. No `\u` handling needed since chars are emitted raw. The only splice, `build_objective_expr` (:1282), wraps block_id in it; scope-variable side uses `rhai_ident_fragment` (:997-1003, strictly `[A-Za-z0-9_]`) with injectivity enforced by `PetriError::FragmentCollision` (:865). Regression test at :1447 (quotes/backslashes in names) exists.

### F2.3 verified — no surviving user-text→Rhai splice on the rank path
- Person names / block ids all flow as `AttrInit::Literal` (:1135, :1139, :1223, :1227, :1241) or as data-compared preconds — `PrecondSpec::Exact` is plain string equality in guard.rs:138-152, never source. `id_expr`s embed only `rhai_ident_fragment` output. Bind names (`delegate_…`, `wait_…`, wiki-link binds) become rhai *Scope* keys, which are data, never parsed.
- Residual splice that DOES exist: `PrecondSpec::from_str` compiles `format!("x {} {}", op, rhs)` (arc.rs:87) — `rhs` is spliced into source. Reachable only from author-controlled YAML nets (serde via FromStr), not vault text. Acceptable, but worth a comment saying so.

### F2.4 LOW — NaN/Inf weights produce a broken objective (error, not injection)
- `numeric_prop` (:548) accepts `"NaN"`/`"inf"` strings; `PrototypeValue::parse` accepts computed exprs returning inf. `format!("{weight:.6}")` then emits `NaN`/`inf` into the objective source (:1280) → compiles as an identifier → whole `rank_tasks` fails at eval with "Variable not found: inf". Fail-loud (good) but the error is misleading and one bad task kills ranking for all tasks. Fix: reject non-finite at `numeric_prop`/post-`resolve_prototype`.
- `serde(untagged)` mis-parse of a person "named like an expression": not possible at construction on the rank path (no serde involved); only via F2.1's round-trip.

---

## Fix 3 — MCP-path panics (PetriError through materialize)

### F3.1 HIGH — chrono panic survives, reachable from `rank_tasks` with a stored property
- Where: `crates/holon-engine/src/engine.rs:214-215` — `marking.set_clock(time + chrono::Duration::minutes(duration as i64))`, called from `Engine::rank` → `fire` simulation (:245).
- `duration` is user vault data: `TaskInfo::from_block` (:773) → `integer_prop` → any i64 (string form accepted: `numeric_prop` parses `"2e14"`). chrono 0.4 `Duration::minutes` PANICS out of bounds (> ~1.5e14 min), and `DateTime + TimeDelta` panics on date overflow from ~1.4e11 min (~year 262143). `default_duration_minutes` / computed prototype values reach the same sink as f64 → `as i64` saturation → same panic.
- Failing input: a task block with drawer property `duration: 200000000000000` → `rank_tasks` MCP tool → process/task abort. Exactly the class Fix 3 claims closed.
- Fix: bound-check at the parse boundary (`TaskInfo::from_block` — e.g. reject duration outside (0, 10y]) returning `PetriError::…`, and/or `TimeDelta::try_minutes` + `checked_add_signed` in `fire` returning the engine's `Err(String)`.

### F3.2 MEDIUM — DoS, not panic: user Rhai compiles as a FULL script with no operation limit
- Where: `crates/holon-expr/src/lib.rs:54-66` — `engine.compile(&source)` (full script, statements allowed), engines built with plain `Engine::new()` everywhere (guard.rs:22, petri lib.rs:848, main.rs:40) — `max_operations` never set (grep: none).
- Failing input: prototype block property `task_weight: "= while true {}"` → `resolve_prototype` eval never returns → `rank_tasks` MCP call hangs forever, holding the tool. "Never panic" was achieved for this path but "never wedge" was not.
- Fix: `engine.set_max_operations(…)` (+ `set_max_expr_depths`) on every evaluator, or `compile_expression` where a script is not needed (loops then rejected at compile → `PetriError::ObjectiveCompile`/`InvalidPrototypeProperty`, fail-loud).

### F3.3 MEDIUM-LOW — cyclic computed props are SILENTLY dropped (fail-loud violation introduced by the Result plumbing's neighbour, not by it)
- Where: `crates/holon-core/src/util.rs:56` (`topo_sort_kahn` — Kahn's algorithm simply omits nodes on a cycle) consumed at `crates/holon-petri/src/lib.rs:405` (`resolve_prototype`), which iterates only `sorted` and never checks `sorted.len() == computed.len()`.
- Failing input: prototype props `a: "= b"`, `b: "= a"`, `task_weight: "= a"` → `a`,`b` vanish, `task_weight` eval then errors OR (if `task_weight` itself is on the cycle) silently defaults to 1.0 at materialize_at:899. No error, no log — silently-swallowed user error, the exact anti-pattern the fix was policing.
- Fix: `assert`/`Err(PetriError::…Cycle)` when the sort output is shorter than the input.

### F3.4 LOW — residual boundary casts
- `lib.rs:731` `Priority::from_int(i as i32)`: i64→i32 truncation BEFORE validation; `priority: 4294967297` is accepted as priority 1 (validation sees the truncated value). Use `i32::try_from` → `InvalidPriority`.
- `lib.rs:1358` `mental_slots_capacity as usize`: a negative stored capacity wraps to ~1.8e19. Cosmetic (display only) but trivially parse-don't-validate at `SelfDescriptor::from_block`.

### F3.5 SOLID — error threading itself is clean
- `PetriError` (lib.rs:47-95, thiserror) covers every stored-property parse on the materialize path (numeric/integer/priority/deadline/prototype/computed/objective/fragment-collision); `rank_tasks` maps to `String` (:1310-1321), `block_domain.rs:517` wraps in anyhow with context — messages are enriched at each hop, nothing is `.ok()`-swallowed or downgraded to a default. Remaining panics inside the engine (`set_attr` panic lib.rs:246, `create_token` assert :251, `net.transition().unwrap()` :1329, `expect("now within range")` :837) are genuine internal invariants NOT reachable from external data on the rank path: rank-built nets have empty postconds, create ids are `completed_/knowledge_/waiting_for_ + frag` with frag-injectivity enforced, and the transition id in :1329 came from the net itself. The panic->Result conversion did not silently swallow anything (`Err` strings propagate to the MCP caller).

---

## Bottom line per fix
- Fix 1: core snapshot/withhold logic is correct and well-reasoned, but INCOMPLETE — the share-projection worker (F1.1) and `project_sort_keys` (F1.2) were not brought along, and one of them turns "withhold" into "delete".
- Fix 2: the escaping and Literal plumbing are solid on the live path; the untagged serde shape (F2.1) is a loaded gun for the first future net round-trip.
- Fix 3: the materialize path is genuinely panic-free, but the engine's clock arithmetic (F3.1) still panics on a single stored property, and unbounded Rhai (F3.2) can wedge the MCP tool.
