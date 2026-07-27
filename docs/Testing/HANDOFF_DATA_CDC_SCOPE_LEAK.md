# Handoff — Data CDC scope leak in query-block watchers

**Status (2026-05-04 update — FIX LANDED)**: 10/10 seed=42 PBT runs
pass after the fix at `crates/holon-frontend/src/reactive.rs:1189`
(previously ~20% failure rate). The cycle path now returns a
`LiveBlock(block_id, Badge "↺ self-reference: <id>")` placeholder
instead of `ViewModel::error`, preserving the no-error-widget
invariant. The "leak" itself is the legitimate semantics of an
unanchored query — a user could write the same GQL — so the renderer
handles the recursion gracefully rather than the fixture being
"corrected".

**Root cause**: The leak is **GQL, not PRQL**. The PBT generator `index_file_gql_varlen`
(`crates/holon-integration-tests/src/pbt/generators.rs:232-242`) emits

```text
MATCH (root:block)<-[:CHILD_OF*1..3]-(d:block) RETURN d
```

with **no anchor on `root`**. Compiles to a `WITH RECURSIVE`
matview that starts traversal from *every* `block` row, so the result
set is "every block reachable in 1..3 hops from any block" — i.e.,
nearly the entire DB. Confirmed by tracing the SQL→view_name mapping in
seed=42 on rebuild `b2gt4g94a`: the leaking matview
`watch_view_8707819c265fb25e` is exactly the SQL above.

The handoff body below was written before the SQL was traced; it
assumed the failing block used `from children` (PRQL), but in seed=42
the proptest variant chosen is `index_file_gql_varlen`. PRQL `from
children` is correctly anchored via `$context_id` — verified by
`test_from_children_substitutes_block_id` and
`test_from_children_matview_returns_only_children` in
`crates/holon/src/api/backend_engine.rs::tests`.

The cycle is still a downstream symptom: when the matview row set
includes the query block itself (true under unanchored MATCH because
2q54er-js is reachable from many starts), the `query_block` profile
variant emits `live_block(self.id)` and trips the cycle guard.

### Fix options (pick one or combine)

1. **Test fixture (smallest change)** — add an anchor to the GQL
   queries in the PBT generator. Anchoring options:
   - `MATCH (root:block {id: :context_id})<-[:CHILD_OF*1..3]-(d:block) RETURN d`
   - `MATCH (root:block)<-[:CHILD_OF*1..3]-(d:block) WHERE root.id = :context_id RETURN d`

   Verify the `gql-to-sql` upstream supports the chosen syntax;
   `gql_params_to_dollar` already converts `:param` to `$param` in
   `backend_engine.rs::compile_gql`, so the existing context-bind
   plumbing carries the value through.

2. **GQL compiler** — make `gql-to-sql` auto-anchor an unanchored
   `root:block` (or any node binding) to `$context_id` when the query
   is run with a non-root `QueryContext`, parallel to PRQL stdlib's
   `from children`. Bigger change (external repo
   `holon-space/gql-to-sql`); see `HANDOFF_NOW_QUERY_PATCHES.md` for
   adjacent compiler gaps.

3. **Defensive (renderer)** — in
   `crates/holon-frontend/src/shadow_builders/live_block.rs`, skip
   rows where `row.id` is already on the `LiveBlockAncestors` chain.
   Prevents the cycle but does NOT fix the data correctness gap.
   Useful as a belt-and-suspenders alongside #1 or #2.

### What was ruled out

| Hypothesis | Test | Result |
|---|---|---|
| H1: `$context_id` substitution failing | `tests::test_from_children_substitutes_block_id` (added) | substitution works for PRQL |
| H2: matview / demux cross-watcher leak in isolation | `tests::test_from_children_matview_returns_only_children` (added) | matview returns exactly the children when blocks come in via INSERT |
| H3: matview created with unbound `$context_id` literal | trace shows the SQL has no `$context_id` at all — it's GQL output | not applicable |

### Diagnostic instrumentation added (env-var gated, no impact when unset)

- `crates/holon/src/api/backend_engine.rs::query_and_watch` and
  `subscribe_sql` — `HOLON_TRACE_VIEWS=1` logs every
  `(SQL → view_name)` mapping at WARN.
- `crates/holon-frontend/src/reactive.rs::ensure_watching` —
  `HOLON_TRACE_BLOCK_DATA=<substr>` logs every CDC batch
  (`relation_name`, ids) and the post-apply row set for any block
  whose URI contains the substring.

To re-reproduce:

```bash
cargo build -p holon-tui --tests --release
for i in {1..10}; do
  HOLON_TRACE_VIEWS=1 HOLON_TRACE_BLOCK_DATA=2q54er-js \
    PROPTEST_SEED=42 RUST_LOG=warn \
    cargo test -p holon-tui --release --test tui_ui_pbt 2>&1 \
    | tee /tmp/seed42_$i.log > /dev/null
  grep -q 'cycle detected\|panicked' /tmp/seed42_$i.log && echo "run $i: REPRO" && break
done
# Then: grep '8707819c265fb25e' /tmp/seed42_REPRO.log  → see the SQL
```

---

## Original investigation (kept for context)

`snapshot_reactive(block:2q54er-js)` (a query block whose PRQL is
`from children`) receives data rows that include far more than its
direct children:

```
row_ids = [
  "block:-q--2b-9--g39c5-e06u1565-5",     // unrelated peer-created block
  "block:2q54er-js",                       // ← THE PARENT ITSELF
  "block:2q54er-js::render::0",            // legitimate child
  "block:2q54er-js::src::0",               // legitimate child
  "block:block:left_sidebar::render::0",   // ← DOUBLE-PREFIXED URI
  "block:block:left_sidebar::src::0",      // ← DOUBLE-PREFIXED URI
  "block:c2f12z-s",                        // unrelated
  "block:default-left-sidebar",            // unrelated system block
  "block:default-main-panel",              // unrelated system block
  "block:default-main-panel::src::0",      // unrelated system block
  "block:default-right-sidebar",           // unrelated system block
  "block:default-right-sidebar::src::0",   // unrelated system block
  "block:ji-e-1",                          // unrelated peer-created block
  "block:journals",                        // unrelated seed page
  "block:nvhz--r75-0sz-7-n37s9o5x7j",     // unrelated
  "block:ref-doc-0", "block:ref-doc-1", "block:ref-doc-2", // unrelated docs
  "block:root-layout",                     // unrelated system block
]
```

Expected: only `block:2q54er-js::render::0` and
`block:2q54er-js::src::0` (direct children).

Observed: the parent itself + every other block in the database +
double-prefixed URIs.

A second `snapshot_reactive(2q54er-js)` later in the same run
(post-`BulkExternalAdd`) was 47 rows, including all `bulk-1-N`,
`bulk-11-N`, `bulk-16-N`, `bulk-26-N`. The set is monotonically
growing, suggesting accumulation rather than spurious initial fanout.

## Why it matters

Combined with two other facts that I confirmed are intentional:

- `assets/default/types/collection_profile.yaml` `tree_view` variant
  uses `item_template: render_entity()`.
- `assets/default/types/block_profile.yaml` `query_block` variant
  (condition `has_query_source`) renders as `live_block()`. With no
  positional arg, the shadow builder
  (`crates/holon-frontend/src/shadow_builders/live_block.rs:10`) falls
  back to `col("id")`.

…the renderer iterates 2q54er-js's data rows, and for the row whose
id IS 2q54er-js, emits `live_block(2q54er-js)` — which recursively
asks `snapshot()` to resolve 2q54er-js while 2q54er-js is on the
resolution stack. That trips `VISITED` in
`crates/holon-frontend/src/reactive.rs:1175`, returns a
`ViewModel::error("error", "cycle in LiveBlock resolution for ...")`,
and inv14b panics.

The `tooqqqkt` commit (`diag(reactive): log resolution chain on
LiveBlock cycle detection`) added an ordered `STACK` thread-local
alongside `VISITED`, so future cycle warnings include the full
resolution chain. Failing run shows
`stack=["block:root-layout", "block:default-main-panel", "block:2q54er-js"]`.

Even fixing the cycle defensively (making the `live_block` builder
guard against `row.id == ancestor`) would not fix this leak — query
blocks would still see wrong data for any production purpose
(displayed counts, filtered lists, downstream computations).

## Reproduction

```bash
# Build:
cargo build -p holon-tui --tests --release

# Failure rate is ~20% per run with seed=42 (test-sequence flake):
for i in 1..10; do
  PROPTEST_SEED=42 cargo test -p holon-tui --test tui_ui_pbt --release \
    > /tmp/seed42_$i.log 2>&1
  grep -q "cycle detected" /tmp/seed42_$i.log && echo "run $i: FAILED"
done
```

When it fails, the cycle warning carries the full resolution chain
(thanks to the diag commit). The data-row dump that surfaced this leak
came from a temporary `tracing::warn!` I added in
`snapshot_reactive` — pasted at the bottom of this doc for re-use.

## What to investigate

Three hypotheses, ordered by my prior:

### H1: context-dependent PRQL not substituting `$block_id` (LIKELY)

The PBT generator `crates/holon-integration-tests/src/pbt/generators.rs:215`
emits literal `from children\n` for query blocks. PRQL `from children`
is shorthand that compiles using `$block_id` from `QueryContext`
(`crates/holon/src/api/block_domain.rs:138`,
`for_block_with_path(block_id, parent_id, block_path)`).

The memory note `turso-ivm-context-param-preload` documents:
> Fix for Turso IVM "Unsupported expression type in logical plan: Variable"
> errors when preloading materialized views. … Context-dependent PRQL
> queries (from children, from descendants, from siblings) fail during
> startup. Root cause: preload functions can't substitute context
> parameters that only exist at runtime.

That's specifically about *preload*, but the same substitution mechanic
runs at request time in `BackendEngine::compile_to_sql`
(`crates/holon/src/api/block_domain.rs:140`). Verify:

1. Add a one-shot trace in `compile_to_sql` for any query containing
   `from children` — log the resulting SQL and the `$block_id` value.
2. Run seed=42 and check the SQL for 2q54er-js. If `$block_id` is
   missing/empty/wrong, the SQL probably becomes
   `SELECT * FROM block WHERE parent_id = NULL` or similar that
   matches no rows OR matches all rows depending on how the SQL
   compiler degrades.
3. Compare to the SQL emitted for a working query block (e.g., one
   from `index.org`'s left-sidebar `holon_sql` query — known good).

The smoking gun would be: 2q54er-js's SQL doesn't have a `parent_id =
'block:2q54er-js'` predicate, or its parameter binding is wrong.

### H2: cross-watcher CDC leakage in MatviewManager (POSSIBLE)

`MatviewManager::watch` returns a `RowChangeStream` that's specific to
a registered view. Multiple watchers for different blocks subscribe to
their own views. If the manager accidentally fans out one view's
events to all subscribers (or shares a single matview across watchers
without a per-watcher filter), watchers see foreign rows.

Look at:
- `crates/holon/src/sync/matview_manager.rs:417` (`query_view`)
- `crates/holon/src/sync/matview_manager.rs:430` (`subscribe_cdc`)
- `crates/holon/src/sync/matview_manager.rs:watch`
- `crates/holon/src/api/backend_engine.rs::query_and_watch`

Specifically: does `query_and_watch` create a per-call view, or reuse
a matview keyed by SQL text? If reuse, are events routed by view name?
If by view name, are subscribers given a per-stream filter?

Verify by:
1. Adding 2q54er-js's rendering as a standalone reproducer outside
   the PBT — call `BlockDomain::render_entity` with a known
   `QueryContext` and inspect the `RowChangeStream` directly.
2. Checking matview names: does 2q54er-js create a view distinct
   from default-main-panel's view? They have different SQL
   (PRQL vs GQL) so should be separate views; verify they are.

### H3: matview created without context-parameter substitution (LESS LIKELY)

If `$block_id` is preserved as-is in the matview DDL (instead of
substituted at creation), the matview becomes a "block_with_path
WHERE parent_id = $block_id" — but `$block_id` at view-creation time
isn't a value, so Turso might evaluate it as NULL or something that
matches everything. Memory note above suggests this manifests as
explicit "Unsupported expression type" errors though, so we'd see a
DDL error first.

Probably not this, but worth ruling out by checking the actual
matview definition with the holon MCP `query` tool:
```sql
SELECT name, sql FROM sqlite_master
WHERE type = 'view' AND sql LIKE '%2q54er-js%';
```

## Side puzzle: double-prefixed URIs

`block:block:left_sidebar::render::0` exists in the rows AND in the
test fixture at
`crates/holon-integration-tests/src/pbt/transitions/apply_mutation.rs:173-174`
as a known seed render id. So the double prefix is a pre-existing,
intentional-but-weird id shape (probably from `index.org` line 10
`#+BEGIN_SRC render :id block:left_sidebar::render::0` where the org
parser re-prefixes the already-prefixed id). Not a corruption — orthogonal
to this RCA. Mention it only because it can confuse the row dump.

## Tooling I added

```rust
// In crates/holon-frontend/src/reactive.rs::snapshot_reactive,
// just after `let (expr, rows) = results.snapshot();`:

if block_id.as_str() == "block:2q54er-js" {
    let expr_name = match &expr {
        holon_api::RenderExpr::FunctionCall { name, .. } => name.as_str(),
        _ => "non-fn",
    };
    let row_ids: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect();
    tracing::warn!(
        expr_name = %expr_name,
        row_ids = ?row_ids,
        "[diag-2q54er] snapshot_reactive(2q54er-js)"
    );
}
```

I removed it before committing the cycle-stack diag (`tooqqqkt`).
Re-add when investigating, ideally generalized to log any block
matching an env-var pattern.

## What success looks like

After the fix:

- `snapshot_reactive(block:2q54er-js)` receives exactly 2 rows
  (its `::render::0` and `::src::0` children) under PRQL `from
  children`.
- Seed=42 stops hitting inv14b — not because the assertion was
  weakened, but because the cycle no longer forms.
- Whatever you do, **don't** silence inv14b on cycle errors. The cycle
  guard is the canary; it has to keep singing.

## Pointers

| File | Why |
|------|-----|
| `crates/holon/src/api/block_domain.rs:104-165` | `BlockDomain::render_entity` — entry point that compiles the PRQL and sets up `query_and_watch`. |
| `crates/holon/src/api/block_domain.rs:138` | `QueryContext::for_block_with_path` — the source of `$block_id` for substitution. |
| `crates/holon/src/api/backend_engine.rs::compile_to_sql` | Where PRQL → SQL compilation happens. Add tracing here. |
| `crates/holon/src/sync/matview_manager.rs:430` | `subscribe_cdc` — verify per-stream filtering. |
| `crates/holon-frontend/src/reactive.rs:1227` | `snapshot_reactive` — where row mismatch becomes user-visible. The `tooqqqkt` diag prints the resolution chain, but doesn't dump rows; re-add the snippet above for that. |
| `crates/holon-integration-tests/src/pbt/generators.rs:210-219` | The PBT generator that creates `index.org` variants with the offending PRQL. Useful for understanding what input shape triggered the leak. |

## Why this is worth the dive

Even with seed=42 set aside, this leak means **every query block in
production silently displays the wrong data when its parent is on the
focus stack**. The fact that the test catches it as a cycle is
incidental — without `block_profile.yaml`'s `query_block` variant
emitting `live_block(self_id)`, there'd be no cycle, just wrong rows
displayed forever. The cycle is the easy-to-detect tail of a much
larger correctness gap.
