# Handoff — item-3 follow-up: migrate `origin=Other("sql")` block-create paths

Date: 2026-05-11

## Handoff prompt (paste into a new session)

> The Phase 3.3 step 2 inbound runtime gate is live in production
> (`handle.disable_inbound_runtime()` runs on every controller startup at
> `crates/holon/src/sync/loro_module.rs:238`). Its integration test at
> `crates/holon-integration-tests/tests/inbound_runtime_gate.rs` surfaced
> two production paths that emit block.created events with
> `EventOrigin::Other("sql")` instead of a whitelisted origin
> (`EventOrigin::Org` or `EventOrigin::Loro`). The gate's `warn!` log
> catches each one in real time; the integration test sees them via
> `LoroSyncController::inbound_runtime_drop_count()`.
>
> **Concrete first step**: add `execute_operation_with_origin(entity, op,
> params, origin)` to `OperationProvider` (default delegates to
> `execute_operation`, ignoring origin), override in
> `SqlOperationProvider` to thread `origin` into the `publish_event` call
> in each dispatch arm. Then migrate `LiveDocumentManager::create`
> (`crates/holon-orgmode/src/di.rs:418`) to pass `EventOrigin::Org`. Run
> the gate integration test — `drop_count` should stay at 0 across the
> Org-write phase.
>
> Read this file, then `devlog/2026-05-11-gate-flip-landed.md` for the
> gate context. Memory: index entry "Phase 3.3 step 2" in
> `~/.claude/projects/.../memory/MEMORY.md`.

## What's already done (do not redo)

- Inbound runtime gate flipped on at
  `crates/holon/src/sync/loro_module.rs:238` —
  `handle.disable_inbound_runtime()` runs every startup.
- Gate integration test at
  `crates/holon-integration-tests/tests/inbound_runtime_gate.rs` (3 tests:
  gate-disabled-at-boot, Org→Apply, Ui→Drop). All 3 pass.
- `TestEnvironment::loro_sync_drop_count()` /
  `loro_sync_applied_count()` / `loro_sync_inbound_runtime_enabled()`
  wrappers in `crates/holon-integration-tests/src/test_environment.rs`.

## The two surfaced paths

### 1. `LiveDocumentManager::create` (the obvious one)

**Site**: `crates/holon-orgmode/src/di.rs:418`

```rust
let result = self
    .command_bus
    .execute_operation(&EntityName::new("block"), "create", params)
    .await
    .map_err(|e| anyhow::anyhow!("{}", e))?;
```

Called from `OrgSyncController::on_file_changed` (via
`crates/holon-orgmode/src/org_sync_controller.rs:266` and
`traits.rs:148::get_or_create_by_name_chain`) when a new file's document
block doesn't yet exist. So every "create a new page document"
flow today fires a block.created with `origin=Other("sql")` — the gate
drops it.

The CDC ladder still works because:
- `LoroDocumentStore`'s seed-on-first-load reads the SQL row directly.
- Subsequent CRUDs on the same block go through `OrgSyncController`'s
  `execute_batch_with_origin(EventOrigin::Org)` path → applied.

So the symptom is "first block.created for a new doc is dropped from
the runtime SQL→Loro reflection path; the seed-load eventually catches
up." Not a correctness bug today, but a real `drop_count` tick the gate
flags as suspicious.

### 2. The single-op `SqlOperationProvider::execute_operation` "create" arm (the structural one)

**Site**: `crates/holon/src/core/sql_operation_provider.rs:1137` (the
"create" arm of `execute_operation`).

The dispatcher builds an event via `publish_event` (line 183-208) which
hardcodes `EventOrigin::Other("sql")`. The batch path
(`execute_batch_with_origin`) was retrofitted to override the origin on
all collected events before publish (line 1437-1440), but the single-op
path wasn't.

Any caller that creates blocks via `BackendEngine::execute_operation`
(rather than `execute_batch_with_origin`) inherits this. Audit the
remaining `execute_operation(EntityName::new("block"), "create", ...)`
call sites with:

```
grep -rnE 'execute_operation\(.*"block".*"create"' crates/ frontends/
```

Known callers beyond `LiveDocumentManager::create`:
- `crates/holon/src/api/action_watcher.rs:204` — the DSL-driven block
  creation path. Less hot but real.
- Possibly the MCP tool entry points (`create_table` etc.) — verify.

## Suggested fix shape

### Trait extension

In `crates/holon/src/core/datasource.rs`, alongside
`execute_batch_with_origin`:

```rust
async fn execute_operation_with_origin(
    &self,
    entity_name: &EntityName,
    op_name: &str,
    params: StorageEntity,
    _: crate::sync::event_bus::EventOrigin,
) -> Result<OperationResult> {
    // Default: drop origin and delegate.
    self.execute_operation(entity_name, op_name, params).await
}
```

### `SqlOperationProvider` override

In `crates/holon/src/core/sql_operation_provider.rs`: override
`execute_operation_with_origin` so it threads `origin` into the
`publish_event` calls. The cleanest path: refactor `publish_event` to
take `origin: EventOrigin` instead of hardcoding (it's only called from
inside `SqlOperationProvider`), then `execute_operation` itself takes
an internal origin parameter that defaults to `Other("sql")`.

### Migrate `LiveDocumentManager::create`

```rust
// Before:
.execute_operation(&EntityName::new("block"), "create", params)

// After:
.execute_operation_with_origin(
    &EntityName::new("block"),
    "create",
    params,
    EventOrigin::Org,
)
```

`OrgSyncController`'s file-content batch is already tagged Org via
`execute_batch_with_origin`; this aligns the page-create path with the
same origin.

### Audit + migrate other callers

Per the grep above. Each caller picks the appropriate
`EventOrigin`:
- `LiveDocumentManager::create` → `Org` (it's a file-driven page).
- `action_watcher.rs` → probably `Other("action")` or a new
  `EventOrigin::Action` variant, then whitelist it in
  `inbound_event_decision` if its block-creates should flow into Loro.

## Verification

After each migration:

```bash
# The gate integration test should now show drop_count == 0 across
# the Org-write phase. Today it shows drop_count++ from the
# LiveDocumentManager call.
cargo test -p holon-integration-tests --test inbound_runtime_gate

# Tighten the test: in `org_origin_events_pass_the_gate_as_apply`, the
# assertion is currently `drop_count` baselined post-startup. Once
# LiveDocumentManager is migrated, the test could write the org file
# from a fresh boot and assert drop_count stays at 0 across the entire
# startup + first-write phase.

# Full PBT
RUST_LOG=error PROPTEST_CASES=1 cargo test \
    -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt \
    -- --nocapture
```

## Why this matters

Strictly, the gate works correctly today — it drops the events the way
it's designed to, and `LiveDocumentManager`'s `block.created` for a new
page is eventually reconciled via Loro's seed path. The win from
migrating is:

1. **Diagnostic clarity** — `drop_count > 0` becomes a real signal of a
   bug instead of "the normal background rate." Future regressions
   surface immediately.
2. **Symmetry** — the batch path is origin-aware; the single-op path
   isn't. Closing that gap removes a class of "why is this dropped"
   confusion.
3. **Prepares item-4 read-side migration** — when the inbound CDC
   `apply_sort_key_hint` path can finally retire (gated on all upstream
   origins being typed positional intents), this is the last piece.

## Verification baseline (must stay green)

```
cargo check --workspace --tests                                                    GREEN
cargo test -p holon-integration-tests --test inbound_runtime_gate                   3/3
cargo test -p holon --lib sync::loro_sync_controller                               16/16
RUST_LOG=error PROPTEST_CASES=1 cargo test -p holon-integration-tests
    --test general_e2e_pbt general_e2e_pbt -- --nocapture                          2/2 ~8-9min
```

## Files to read

- `crates/holon-orgmode/src/di.rs:418` — site 1 (LiveDocumentManager).
- `crates/holon/src/core/sql_operation_provider.rs:1137-1185` — single-op
  create arm.
- `crates/holon/src/core/sql_operation_provider.rs:1437-1440` — how the
  batch path overrides origin (the pattern to mirror).
- `crates/holon/src/core/datasource.rs:172-179` — the existing
  `execute_batch_with_origin` default impl (where to add
  `execute_operation_with_origin`).
- `crates/holon/src/sync/loro_sync_controller.rs:621` —
  `inbound_event_decision` (the decision matrix; whitelist a new origin
  here if `action_watcher` etc. need one).
- `crates/holon-integration-tests/tests/inbound_runtime_gate.rs` — the
  test that surfaces the drops.
