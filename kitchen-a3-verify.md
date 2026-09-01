# Adversarial verification — lane kitchen-a3 (change `mvxvqnxm`)

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/kitchen-a3`
Base `@-` = `ed38a4dae833`. All evidence produced in this session; every number
copied from a named log under `$WS/lane-logs/`. No jj/git write command was run.
Probe file added and removed; `jj status` before/after is byte-identical
(`TREE-IDENTICAL`).

---

## Claim 1 — Tree identity: **CONFIRMED**

`grep -n "typed_rows" crates/holon-core/src/file_format.rs` on disk, before any
gate: hits at `:41` (`pub typed_rows: Vec<TypedRowSet>`) and `:78`
(`async fn replace_typed_rows`). `jj log -r @-` prints `ed38a4dae833`.
Toolchain in the lane: `nightly-2026-08-16-aarch64-apple-darwin (overridden by
.../kitchen-a3/rust-toolchain.toml)` — `lane-logs/v1.toolchain.log:1`. Not the
Homebrew-stable shadow.

## Claim 2 — vault `.cook` → rows joined by the minted id: **CONFIRMED**

Re-run: `cargo nextest run -p holon-integration-tests --test cook_vault_ingest`
→ `lane-logs/v2.cookvault.log`:
`Summary [   3.237s] 4 tests run: 4 passed, 0 skipped`.

The assertions DO pin the prefixed value; a bare id cannot pass. Two assertions
compose, in `crates/holon-integration-tests/tests/cook_vault_ingest.rs`:

* `:399` — `id.starts_with("recipe:")` on every `recipe.id`.
* `:406-415` — `rows_eventually(… "FROM ingredient_use iu JOIN recipe r ON
  r.id = iu.recipe_id …", 4, …)`, which panics unless exactly 4 rows join.

Since `r.id` is proven prefixed, a bare `iu.recipe_id` joins to 0 rows and the
`rows_eventually` deadline panics. The pair is sufficient. Independently
measured id shapes from my own probe run
(`lane-logs/v6.probe.log:142`):
`ingredient-use:Pancakes.cook::iu::0`, `…::iu::1` — both entity-prefixed.

## Claim 3 — "One writer": **CONFIRMED for writes**, with an undisclosed
## side-effect DEFECT (see D1)

* No new `SqlBlockOperations` and no direct SQL **write** anywhere in the diff.
  `DispatchingTypedRowSink::replace_typed_rows`
  (`crates/holon/src/core/typed_row_sink.rs`) issues only
  `self.dispatcher().await.execute_operation(entity, op, params)` via its `run`
  helper. The dispatcher is resolved per call (documented DI-cycle reason).
* The one raw-SQL statement it builds is a **read**: `ids_in_scope` runs
  `SELECT id FROM {table}_raw WHERE {owner_column} = '{owner_value}'`.
  `owner_column` is validated against the `TypeRegistry` in `checked_entity`
  before interpolation and `owner_value` is `'`-escaped, so this is not an
  injection seam. Minor: `checked_entity` validates against `type_def.fields`
  (all fields) while the raw table holds only `persistent_fields()` — naming a
  computed field would pass validation and fail loudly at SQL. Acceptable.
* **Loro**: a typed-row `create` for a declared type is NOT expected to touch
  Loro and does not. Loro authority covers *blocks* only — block writes route
  `SqlBlockOperations` → `BlockCellRegistry` → Loro, with
  `LoroSyncController::on_loro_changed` the sole SQL writer of block columns
  (`crates/holon/src/core/sql_block_operations.rs:17-20`,
  `crates/holon/src/core/sql_operation_provider.rs:1470-1476`).
  `recipe` / `ingredient_use` have no Loro document, so `SqlOperationProvider`
  writing their `_raw` tables bypasses nothing. Ordering is likewise
  block-only. No defect here.

### DEFECT D1 — vault ingest now writes onto the user's UNDO stack

Routing ingest through the shared dispatcher inherits `OperationLogObserver`,
whose `entity_filter()` returns `"*"` — "Observe all entities for undo/redo"
(`crates/holon/src/core/operation_log.rs:255-262`). Every dispatcher op is
logged unconditionally at `crates/holon/src/api/operation_dispatcher.rs:1150`;
there is no ingest-origin suppression and the sink passes
`AuthoredInput::Verbatim`.

Measured, not inferred — probe on a vault holding one `Pancakes.cook`,
immediately after boot (`lane-logs/v6.probe.log:143`):

```
PROBE undo-log total=5 kitchen-ops=3 sample=Some("{\"entity_name\":\"recipe\",\"op_name\":\"create\",…}")
```

3 of the 5 entries in the `operation` undo/redo table are machine-generated
ingest writes (`recipe create` + 2 × `ingredient_use create`). A user who
presses undo after opening a vault undoes a file-derived row, not their own
edit — and the next ingest rewrites it anyway. Nothing in
`docs/Plans/Kitchen.md` or the lane report discloses this. It scales with the
recipe count: a vault of 50 recipes with 8 ingredients each floods the stack
with ~450 entries on every boot that re-ingests.

## Claim 4 — "Idempotency = delete the scope, then create": **PARTIALLY
## REFUTED**

Ids across two ingests of an **unchanged** file: **stable**, and the lane's
claim holds — `lane-logs/v6.probe.log:142` vs `:144` are identical
(`…::iu::0` eggs, `…::iu::1` flour). No re-mint.

But two findings the lane did not state:

### DEFECT D2 — the byte-identical-re-save rung is VACUOUS

`lane-logs/v6.probe.log:143` vs `:145`: the undo-log row count is `5` before
and `5` after `write_org_file("Pancakes.cook", PANCAKES_COOK)` +
`wait_for_org_files_stable`. Zero dispatcher ops ran, so the content-hash fast
path skipped ingest entirely and `replace_typed_rows` was **never called** on
that leg. The test rung
`re_ingesting_a_recipe_replaces_its_rows_rather_than_duplicating_them`
(`crates/holon-integration-tests/tests/cook_vault_ingest.rs`, the "byte-identical
re-save must not duplicate rows" assertion) therefore proves nothing about
idempotency: it would pass even if `replace_typed_rows` duplicated rows on
every call. Only the *edited*-file leg exercises replacement. The test's own
comment there ("the write path's recognized-id upsert must land the same two
rows") also names a mechanism that is not the one implemented.

### DEFECT D3 — `ingredient_use` ids are POSITIONAL, so an ordinary edit
### re-assigns them across ingredients

`crates/holon-kitchen/src/rows.rs` mints
`format!("{rel_path}::iu::{index}")` from `.enumerate()`. Adding an ingredient
anywhere but the end shifts every later id onto a *different* ingredient.
Measured — adding `@butter{10%g}` as a new FIRST step
(`lane-logs/v6.probe.log:142` → `:146`):

| id | before | after front-insert |
|---|---|---|
| `ingredient-use:Pancakes.cook::iu::0` | `eggs` | `butter` |
| `ingredient-use:Pancakes.cook::iu::1` | `flour` | `eggs` |
| `ingredient-use:Pancakes.cook::iu::2` | — | `flour` |

The doc's stated id rule ("derived from the file's vault-relative path, never
minted fresh … re-ingest must land the same ids") is true only for an unchanged
file. Once Inc D lands `fields[].references`, anything holding
`ingredient-use:Pancakes.cook::iu::0` silently re-points to another ingredient.
The lane's own edited-file rung removes the FIRST ingredient (`@eggs`) — the
exact case that reshuffles — but asserts only `raw_name` and `quantity`, never
ids, so it accepts the shuffle silently.

### Partial failure between delete and create — disclosed, confirmed non-atomic

`replace_typed_rows` issues each delete and each create as its own
`execute_operation`. A failure on any create leaves the scope's earlier rows
deleted and later rows unwritten. The error propagates loudly (`with_context`
naming the type, then `file_sync_controller.rs` wrapping with the file path), so
it is disclosed at the call site rather than silent, and both the lane report and
`docs/Plans/Kitchen.md` state the limitation. Not a defect; noting that the
resulting store state is a partially-emptied recipe until the next ingest.

## Claim 5 — the `parent_id` short-circuit: **CONFIRMED**

The guard is keyed on the type's own write vocabulary, not on anything that
could disarm the block cascade:

* `crates/holon/src/core/sql_operation_provider.rs:2638` (`has_children`) and
  `:1692` (`prepare_delete`) both test `!self.write_schema.is_column("parent_id")`.
* `write_schema` for blocks is `WriteSchema::block()` — `blocks_known_columns()`
  from `holon_api::schema::BLOCK` (`:260-263`), which contains `parent_id`
  (proven behaviourally: `advertises_content_compounds` at `:438` requires it,
  and the block cascade tests below still pass). For a declared type it is
  `WriteSchema::from_type_def` = its `persistent_fields()` (`:266-274`).
* **A declared type WITH a `parent_id` column still cascades**: the guard is a
  pure column-presence test with no type-name special-casing.
* `queue.clear()` in `prepare_delete` is safe: `all_ids` collects only
  *descendants*, and the target row's own `DELETE` is emitted unconditionally
  after the loop (`:1740-1746`). Clearing the queue does not turn the delete
  into a no-op.

Runs: `cargo nextest run -p holon --features test-helpers --test
e2e_backend_engine_test --test undo_create_id_stability --test
undo_inverse_wave1 --test block_links_junction --test edge_field_e2e`
→ `lane-logs/v4.holon-delete.log`:
`Summary [   2.537s] 22 tests run: 17 passed (2 leaky), 5 failed, 0 skipped`.
All 5 failures are the allowlisted pre-existing reds: exactly 5 occurrences of
`cannot modify materialized view block`, in
`test_operation_triggers_stream_update`, `test_query_and_watch_stream`,
`test_create_and_delete_workflow`, `test_basic_query_execution`,
`test_multiple_operations_sequence` — the base-attributed root-remodel set named
in the lane rules. `test_create_and_delete_workflow` fails on the matview
signature, not on a cascade error. `just keystone-smoke` results below.

## Claim 6 — gate cross-check: **CONFIRMED** (with known reds)

| Gate | Log | Line I read |
|---|---|---|
| `cargo fmt --all --check` | `lane-logs/v1.fmt.log` | 0 lines of output, script exit 0 |
| `nextest -p holon-kitchen -p holon-core` | `lane-logs/v1.kitchen-core.log` | `Summary [   0.218s] 195 tests run: 195 passed, 0 skipped` |
| `nextest -p holon --lib` | `lane-logs/v1.holon-lib.log` | `Summary [   9.216s] 214 tests run: 214 passed, 0 skipped` |
| `cargo check -p holon-gpui -p holon-app` | `lane-logs/v1.gpui-app.log` | `Finished dev profile … in 6.02s`, 0 `^error` |
| `nextest --test cook_vault_ingest` | `lane-logs/v2.cookvault.log` | `Summary [   3.237s] 4 tests run: 4 passed, 0 skipped` |
| `nextest -p holon-filesystem` | `lane-logs/v5.filesystem.log` | `Summary [   3.237s] 93 tests run: 92 passed, 1 failed, 0 skipped` — the failure is `change_source::tests::notify_watcher_delivers_events_after_arm`, the flake named in the lane rules |
| `just keystone-smoke` ×3 | `lane-logs/v2.keystone.log`, `v5.keystone2.log`, `v5.keystone3.log` | 1 green, 2 red — both reds are documented known-reds (below) |
| consumers `cargo check --tests` | `lane-logs/v4.consumers.log`, `v5.orgmode-di.log`, `v5.holon-tests.log` | see below |

The lane report's own numbers reproduce: my 195 for `holon-kitchen`+`holon-core`
is consistent with its 221 for those two plus `holon-markdown`; `-p holon --lib`
matches 214 exactly; `holon-filesystem` matches 93/92/1 exactly.

**Keystone reds are NOT lane-attributable.** Both signatures are registered in
`docs/Testing/KeystoneKnownReds.md`:

* `lane-logs/v2.keystone.log:409-410` — `per-tick reconcile: one synthetic per
  minted real id (syn=[], real=[EntityUri("block:5b9c…")]); this tick RETIRED []
  and the SUT LOST []` = the `syn-real-mint` row's documented SECOND signature
  variant (`KeystoneKnownReds.md:105`, "unexplained-mint", pre-existing since
  ≤2026-07-25, measured 6.1 % red rate at `PROPTEST_CASES=1`).
* `lane-logs/v5.keystone3.log:295` — `inv-drawer-open-matches-ref … drawer
  block:default-left-sidebar rendered open=true but reference says open=false`
  = `drawer-open-matches-ref` (`KeystoneKnownReds.md:122`).

Neither touches recipes, typed rows, or the delete path, and the lane's diff
contains nothing sidebar- or block-loss-shaped. Note for the orchestrator: I saw
2 reds in 3 runs, above the documented 6.1 % — worth a wider sample before
reading anything into it, but not this lane's.

**Consumers of `crates/holon-core/src/file_format.rs`.** `ast-outline
reverse-deps` is a file-level in-crate graph and reports only `src/lib.rs`, as
the lane says. The real consumer set (`grep -rn FileFormatParseResult crates/`)
is `holon-core`, `holon-kitchen`, `holon-markdown`, `holon-orgmode`,
`holon-filesystem`, `holon`, plus downstream `holon-app` / `holon-gpui` /
`holon-integration-tests`. `cargo check --tests` is clean for
`holon-core`/`holon-kitchen`/`holon-markdown`/`holon-filesystem`,
`holon-orgmode --features di` (`lane-logs/v5.orgmode-di.log`, 0 errors) and
`holon --features test-helpers --tests` (`lane-logs/v5.holon-tests.log`,
0 errors).

**One pre-existing breakage found, NOT this lane's**: `cargo check -p
holon-orgmode --tests` with DEFAULT features fails —
`error[E0432]: unresolved import file_sync_controller` at
`crates/holon-orgmode/src/lib.rs:68` (`lane-logs/v4.consumers.log`). Verified
base-attributed: `git show ed38a4dae833:crates/holon-orgmode/src/lib.rs` carries
the identical line 68, and the lane's diff does not touch that file. The crate
only builds with `--features di`.

## Claim 7 — `docs/Plans/Kitchen.md` Inc A3: **CONFIRMED with a caveat**

The stated id rule and the idempotency mechanism both match the code: ids
derived from the vault-relative path (`crates/holon-kitchen/src/rows.rs`), the
`:`-in-path refusal is present, `recipe_id` holds `recipe:<path>`, and
"Idempotency = replace the scope … Not an upsert" matches
`replace_typed_rows`'s delete-then-create. The `parent_id` bullet matches the
guard. The non-atomicity is disclosed.

Two caveats:

* The **"What the red caught"** bullet is written as history ("The generic
  `delete` walked `parent_id` on every type … Both now short-circuit"), as is
  the **"Red-first"** bullet's lane-log narration. In a plan doc under the
  `holon-feature` red-first contract that is arguably required rather than rot,
  but it is not current-state-only as the claim asserts.
* The id rule as stated omits D3: it is stable only for an unchanged file.

### DEFECT D4 — stale "upsert" comments contradict the shipped mechanism

The mechanism changed from upsert to delete-then-create (the lane report says so
explicitly) but four comments still describe the abandoned design. Each will
mislead the next reader about why ids must be derived:

* `crates/holon-kitchen/src/rows.rs`, module doc: "…so the write path's
  recognized-id upsert updates the rows instead of growing a second copy".
* `crates/holon-core/src/file_format.rs`, `TypedRowSet::rows` doc: "both the
  replacement above and the write path's recognized-id upsert key on it".
* `crates/holon-integration-tests/tests/cook_vault_ingest.rs`, in the re-ingest
  rung: "same ids, so the write path's recognized-id upsert must land the same
  two rows".
* `crates/holon-core/src/file_format.rs`, `TypedRowSink::replace_typed_rows`
  doc: "Make each set's owned rows be exactly its `rows`, **atomically per
  set**." The implementation is explicitly NOT atomic — this is a false
  contract on the public trait, and the strongest of the four: a future
  implementor or caller may rely on it.

---

## Overall verdict: **REFUTED IN PART**

The load-bearing structural claims hold under independent exercise: the vault
row seam works and its test genuinely pins the prefixed join key (2); there is
exactly one *writer* and no Loro bypass, because these tables have no Loro
projection at all (3); the `parent_id` short-circuit is keyed on column presence
and leaves the block cascade intact (5); the gates reproduce, and every red I
saw is a registered pre-existing one (6).

Four defects, none of them a wrong-answer bug in the happy path, all of them
things the lane report claims or implies otherwise:

* **D1** (claim 3) — vault ingest writes onto the user's undo/redo stack;
  measured 3 of 5 log entries after booting a one-recipe vault. Undisclosed.
* **D2** (claim 4) — the byte-identical-re-save rung never invokes
  `replace_typed_rows` (content-hash fast path), so it cannot catch the
  duplication it is named for. Idempotency is proven only by the edited-file leg.
* **D3** (claim 4) — `::iu::<index>` ids are positional; inserting an ingredient
  re-assigns existing ids to different ingredients (measured). The stated id
  rule holds only for unchanged files, and the lane's edited-file rung exercises
  the reshuffling case without asserting ids.
* **D4** (claim 7) — four comments describe the abandoned upsert design, one of
  them an "atomically per set" contract on the public `TypedRowSink` trait that
  the implementation does not honour.

D1 and D3 are the two I would not land without a ruling; D2 and D4 are cheap.

---

# Re-verification — 2026-09-01, round 2 (D1–D4 delta)

Same workspace, same rules, change `mvxvqnxm` at its new state; `@-` still
`ed38a4dae833`. Two temporary probe files were added and removed; `jj status`
carries no `zz_verify` entry afterwards. Read-only on VCS throughout.

## (1) D1 — the origin gate: **CONFIRMED FIXED, and the widening is safe**

**Measured effect.** Probe `probe_replace_actually_reran`
(`lane-logs/r2b.probe.log:170`): after a boot that ingested a recipe AND a
prose-only re-save that re-derived its rows, the whole `operation` table holds
exactly one row — `{entity_name: "ingredient-use", op_name: "create"}` — which
is *my own* deliberate user-origin dispatcher call planting a sentinel. Zero
ingest entries, against 3 measured in round 1. The defect is closed.

**Enumeration of every reader of that table / of `OperationLogObserver`
output**, which is the part of the claim worth attacking, since the fix
deliberately widened beyond kitchen:

| Reader | Reads what | Load-bearing on machine-origin entries? |
|---|---|---|
| `OperationLogObserver` | sole writer, `entity_filter() == "*"` (`crates/holon/src/core/operation_log.rs:255-262`) | n/a — writer |
| `OperationLogStore` (`log_operation`, `mark_undone`, `mark_redone`, `clear_redo_stack`, `get_operation`, trimming) | `SELECT/UPDATE/DELETE … operation` (`operation_log.rs:77,96,122,175,193,211,317,352,366,402,436,473,485,535,575`) | **No production caller.** `grep -rn OperationLogOperations` outside that file returns only the trait declaration (`crates/holon-core/src/traits.rs:2723`) and the re-export (`lib.rs:89`); `grep` for `mark_undone`/`clear_redo_stack`/`log_operation` outside it returns only those trait lines. Every invocation is inside the store's own `#[cfg(test)]` module, which calls the store **directly** and never through the dispatcher — unaffected by an observer gate. Those tests are in `-p holon --lib`: `221`/`214` green below. |
| user-facing undo/redo (`can_undo`, `undo`) | `UndoStack`, `crates/holon-core/src/undo.rs` — a different structure, pushed by `DispatchingOperationEngine` under `origin.is_user()` | No. Already origin-gated before this lane; the sink never went through that engine. The lane's own correction to my round-1 framing is right: Cmd-Z was never polluted, the persistent log was. |
| MCP `query_history` / `count_history` | `block_history` via `TursoHistoryStore` (`crates/holon/src/api/holon_service.rs:183-200`) — an entirely separate relation, written from `holon-filesystem`/history modules, not from `notify_observers` | No. Confirmed green: `query_history_mcp` passes in the undo battery below. |
| provenance | `_provenance` stamped into `block_raw.properties` by the **engine**, before dispatch (`crates/holon/src/api/operation_engine.rs:2666-2671`) | No — never went through the observer. |
| sync / replication | no reference to the `operation` table anywhere (`grep -rn "FROM operation"` → only `operation_log.rs`, the lane's new rung, and unrelated JSON keys) | No. |
| observers generally | `provide_into_set::<dyn OperationObserver>` is registered **once**, `crates/holon/src/di/registration.rs:293-296`; the only other injection point, `OperationDispatcher::add_observer`, has zero callers | The gate therefore affects exactly one consumer. |

**Do any tests assert a non-user op is logged?** No. `grep -rn "FROM operation"`
over `crates/` finds one test reference: the lane's own new rung
`crates/holon-integration-tests/tests/cook_vault_ingest.rs:633`, which asserts
the log is **empty** after ingest. All other assertions live in the store's
direct-call unit tests.

**The gate is strictly a narrowing, never a widening.** Before, every dispatch
was logged. After, engine-routed dispatches are logged only when
`origin.is_user()`; direct dispatcher callers still reach
`execute_operation_with_input`, which now hardcodes `OpOrigin::User` and so are
logged exactly as before. Prod direct callers are
`operation_engine.rs:772,917,974,1237` (replay paths),
`core/type_declaration.rs:102` and `holon-loro-wiring/src/loro_block_query_source.rs:200-209`
— none newly logged, none newly silenced. So no path changes in the
"stops being recorded when it used to matter" direction beyond the intended
rule/sync/agent/ingest set.

**Origin pass-through does not change which origin BLOCK edits carry.**
`origin` was already the engine's own value and already had two consumers before
the dispatcher call: `stamp_provenance(op_name, params, &origin)`
(`operation_engine.rs:2671`) and the `AuthoredInput` choice
(`:2689-2692`, `User|Agent → Live`, `Rule|Sync|Ingest → Verbatim`). The diff
adds `origin.clone()` as a *fifth argument* and leaves both of those, and the
separately-computed `input`, untouched (`crates/holon/src/api/operation_engine.rs:2702-2705`).
A block edit therefore carries the same origin and the same `AuthoredInput` it
did before. Behavioural confirmation: `undo_foundation`,
`undo_create_id_stability`, `undo_gesture_atomicity`, `undo_inverse_wave1`,
`query_history_mcp`, `trust_gate` → `lane-logs/r2g.undo.log`:
`Summary [   5.046s] 33 tests run: 33 passed, 0 skipped`.

## (2) D2 — evidence beyond a passing assert: **CONFIRMED**

I did not take the rung's word for it. Positive control, in
`probe_replace_actually_reran`: after boot I planted a sentinel row through the
dispatcher, `ingredient-use:sentinel-probe` with
`recipe_id = "recipe:Pancakes.cook"` — inside the file's ownership scope but
produced by no parse. Then a prose-only re-save (different bytes, identical
ingredients).

* `lane-logs/r2b.probe.log:165` — before the re-save, 3 rows: `eggs-0`,
  `flour-0`, **`sentinel-probe`**.
* `:169` — after the re-save, 2 rows: `eggs-0`, `flour-0`. The sentinel is
  **gone**.

Only `replace_typed_rows` sweeps by `owner_column` scope, so the file genuinely
re-parsed and re-ran the replacement. The vacuity of round 1 is closed. The ids
are also byte-identical across the two ingests, which is the rung's own claim.

## (3) D3 — content-derived ids: **FIXED FOR THE REPORTED CASE, one residue**

Ids are now content-derived, measured (`lane-logs/r2b.probe.log:164`):
`ingredient-use:Pancakes.cook::iu::eggs-0`, `…::iu::flour-0`. The round-1
front-insert failure no longer reproduces.

**Slug collisions produce distinct rows, not a collision or a refusal** —
`lane-logs/r2b.probe.log:184`, one recipe naming `@Eggs`, `@eggs`, `@sea salt`,
`@sea-salt`, `@creme`:

```
creme-0 = creme | eggs-0 = Eggs | eggs-1 = eggs | sea-salt-0 = sea salt | sea-salt-1 = sea-salt
```

Both colliding pairs (`Eggs`/`eggs` via case-folding, `sea salt`/`sea-salt` via
the space→`-` mapping) get separate ids. No overwrite, no refusal. `crème`
slugs to `cr-me` by inspection of `id_slug` (any char outside
`[a-z0-9._-]` → `-`), so it is distinct from `creme`; it collides with `cr me`
and `cr-me`, the same residual class as below.

### DEFECT D3′ — for slug-colliding names the id is STILL positional

The occurrence counter is itself a position. Reordering two ingredients whose
slugs collide swaps their ids — `lane-logs/r2b.probe.log:184` → `:187`, after
swapping only the order of `@Eggs` and `@eggs` in the file:

| id | before | after reorder |
|---|---|---|
| `…::iu::eggs-0` | `Eggs` | `eggs` |
| `…::iu::eggs-1` | `eggs` | `Eggs` |

This is the exact failure mode D3 named, narrowed from *every* ingredient to
*same-slug* ingredients. The module doc in `crates/holon-kitchen/src/rows.rs`
states the stronger claim — "Position in the file is deliberately NOT part of
either" — which is false for this class, and `docs/Plans/Kitchen.md` inherits
it. Much smaller blast radius than round 1 (a recipe must name two
slug-equal ingredients), but it is the same silent re-pointing that Inc D's
`fields[].references` would build on, and nothing discloses it.

**`checked_local_id` refuses a space-containing filename loudly: CONFIRMED.**
A vault holding `My Pancakes.cook` boots with zero recipe rows *and* zero
blocks for it — and the refusal is disclosed five times at ERROR
(`lane-logs/r2b.probe.log:143-147`), naming the file and the fix:

* `ingest FAILED partway — QUARANTINING this file from write-back …
  error=derived recipe id "My Pancakes.cook" is not a storable URI path. Rename
  the file to one the id grammar admits.`
* `[OrgMode] OrgMode initial scan failed for 1 file(s): …`
* `holon_app::wiring: OrgMode initial scan degraded — some vault files were not
  ingested; other files continue syncing: …`

No panic, no silent drop, the healthy files keep ingesting, and the file is
quarantined until its content changes. This is the fail-loud contract honoured;
my round-1 concern that it might be silent is refuted by my own measurement (the
first probe simply had no tracing subscriber installed).

## (4) D4 — remaining wording: **CONFIRMED FIXED**

`jj diff -r @ --git | grep -n "upsert\|atomically\|atomic"` leaves exactly two
hits in shipped code, both correct as current-state statements:

* `crates/holon/src/core/typed_row_sink.rs` — "there is no in-place upsert to
  prefer over this", the justification for replacement, not a description of the
  mechanism.
* `crates/holon-core/src/file_format.rs` — "NOT atomic: retire and write are
  separate operations, so a failure part way …". The false "atomically per set"
  contract is gone.

Every other hit is in `docs/Plans/Kitchen.md` (correct: "Not an upsert … The
pair is NOT atomic") or inside `kitchen-a3-verify.md` itself, i.e. my own
round-1 quotations of the text that was removed.

## (5) Gates, re-run — every line copied from the named log

| Gate | Log | Summary line |
|---|---|---|
| `cargo fmt --all --check` | `lane-logs/r2g.fmt.log` | 0 lines of output |
| `nextest -p holon-kitchen -p holon-core -p holon-markdown` | `lane-logs/r2g.kitchen-core.log` | `Summary [   0.237s] 221 tests run: 221 passed, 0 skipped` |
| `nextest -p holon --lib` | `lane-logs/r2g.holon-lib.log` | `Summary [   9.069s] 214 tests run: 214 passed, 0 skipped` |
| `cargo check -p holon-gpui -p holon-app` | `lane-logs/r2g.gpui-app.log` | 0 `^error` |
| `cargo check -p holon-orgmode --features di --tests` | `lane-logs/r2g.orgmode.log` | 0 `^error` |
| `nextest --test cook_vault_ingest` | `lane-logs/r2g.cookvault.log` | `Summary [   1.768s] 5 tests run: 5 passed, 0 skipped` |
| undo battery + history + trust | `lane-logs/r2g.undo.log` | `Summary [   5.046s] 33 tests run: 33 passed, 0 skipped` |
| `nextest -p holon` e2e / links / edges | `lane-logs/r2g.e2e.log` | `Summary [   1.161s] 12 tests run: 7 passed, 5 failed, 0 skipped` — exactly 5 occurrences of `cannot modify materialized view block`, the allowlisted base reds |
| `nextest -p holon-filesystem` | `lane-logs/r2g.filesystem.log` | `Summary [   3.266s] 93 tests run: 93 passed, 0 skipped` — the known `notify_watcher_delivers_events_after_arm` flake did not fire this run |
| `just keystone-smoke` | `lane-logs/r2g.keystone.log` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |

The lane's own delta numbers reproduce (221, 214, 5/5 cook_vault, 5 matview
reds). The pre-existing default-feature breakage of `holon-orgmode` reported in
round 1 is unchanged and still base-attributed.

## Round-2 verdict: **CONFIRMED, with one narrowed residue (D3′)**

D1, D2 and D4 are closed, each on evidence I produced rather than on the lane's
asserts: an empty `operation` table after a real ingest, a swept sentinel row,
and a grep of the diff. The D1 widening is safe to land — the `operation` table
has no production reader at all, `query_history` reads a different relation, and
the user-facing undo stack was never in scope. D3 is fixed for the reported
case and for ordinary slug-distinct names.

The one open item is **D3′**: ids remain positional for ingredients whose names
slug identically (`Eggs`/`eggs`, `sea salt`/`sea-salt`), where the occurrence
counter re-points on reorder. The code and plan docs claim position plays no
part, which is now true of the common case and false of this one.
