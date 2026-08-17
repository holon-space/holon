---
id: 2026-07-09-panics-whenever-caller-omits-two-live
date: 2026-07-09
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  `block` `create` op panics `create: missing 'id'`
  (`sql_operation_provider.rs:1119` `.expect`) whenever a caller omits `id`.
  TWO live callers hit it, both swallowed on a `tokio-rt-worker`: (1) the
  journal auto-create Rhai action `block.create(#{parent_id, name})` at boot →
  today's journal never created; (2) the GPUI creation-slot commit (type into
  the "add a new block" `__virtual:` slot + Enter →
  `view_event_handler.rs:203` builds `create{parent_id, content}` with no id)
  → the typed block silently vanishes. `split_block` mints `block:<uuid>`
  fine, so only the `create` op was affected. Reproduced live over MCP:
  creation-slot commit + the boot action both PANIC. (Supersedes the
  2026-07-07 "creation-slot FIXED" row — `resolve_creation_parent` fixed the
  parent but not the missing id.)
source_line: 873
---

## Bug

`block` `create` op panics `create: missing 'id'`
(`sql_operation_provider.rs:1119` `.expect`) whenever a caller omits `id`.
TWO live callers hit it, both swallowed on a `tokio-rt-worker`: (1) the
journal auto-create Rhai action `block.create(#{parent_id, name})` at boot →
today's journal never created; (2) the GPUI creation-slot commit (type into
the "add a new block" `__virtual:` slot + Enter →
`view_event_handler.rs:203` builds `create{parent_id, content}` with no id)
→ the typed block silently vanishes. `split_block` mints `block:<uuid>`
fine, so only the `create` op was affected. Reproduced live over MCP:
creation-slot commit + the boot action both PANIC. (Supersedes the
2026-07-07 "creation-slot FIXED" row — `resolve_creation_parent` fixed the
parent but not the missing id.)

## Missing piece

CORRECTION (re-investigated 2026-07-09): the keystone's
`create_block_under_focus` transition does NOT supply an id — its headless
`ReactiveEngineDriver::commit_creation_slot → handle_text_sync →
block.create` path already OMITS the id (exactly the prod creation-slot
path). So the escape was NOT "no rung drives the id-less path"; it was that
the transition fired non-deterministically (precondition needs a single
*rendered* Main focus root under the default layout — not a user
`index.org`, which the wide generator often uses) and any RED could be
masked by known keystone flakes. The composed per-tick reconcile
(`harness.rs`, `assert synthetic.len()==real_new.len()`) is ALREADY strict —
it would flag a create no-op/panic. The Rhai `action_watcher.rs` id-less
create path is genuinely un-driven headless.

## Remedy

FIXED (this session): create branch mints `{entity}:{uuid}` when `id` absent
(mirrors `split_block`); id-less-create is the normal "mint a new block"
case, callers may still override. Regression test
`create_without_id_mints_a_block_scoped_id` (green). VERIFIED LIVE on a
fresh sandbox boot of the rebuilt binary: ZERO `create: missing 'id'`
panics; the journal auto-create action fires without panicking. ESCAPE
CLOSED in the keystone: `keystone-create-id-rung` adds `id:
Option<EntityUri>` to `CreateBlockUnderFocus` + a `DirectUserDriver` (no-UI
op-floor) `SutBlockCreate` impl, so the id-less create fires
DETERMINISTICALLY under the no-UI pin and the strict reconcile catches any
no-op/panic (empirically: 0 reconcile panics across mint/explicit runs).
`inv-no-observed-errors` (swallowed-panic guard) confirmed live in the
pinned wide slice.
