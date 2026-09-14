---
id: 2026-09-13-undo-precondition-reader-finds-no-content
date: 2026-09-13
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An undo of a block write addressed by the block's unschemed id was always
  stale-dropped, because the precondition reader spliced that id into a query
  against a table keyed on the scheme-qualified one.
---

## Bug

Boot the shared production wiring over a temp vault
(`holon_app::new_from_config_with_di`), dispatch one user `block/set_field` on
`content` addressed by the block's UNSCHEMED id — the form
`docs/Reference/ORG_SYNTAX.md` says vault files carry — then undo it. The write
lands on the right block. The undo does not run: it returns

```
StaleDropped { reason: "state changed under undo: undo-scheme-probe-child.content
expected String(\"set by the user\") but found None" }
```

`found None` is the point. The precondition did not fail because the value
changed; it failed because the live-state read matched no row at all.

Found by the `cell-undo` lane while pinning the one-stack invariant across both
undo mechanisms (D115.A increment 2), confirmed by a verifier on the production
leg, and root-caused in the `undo-precondition` lane.

## Root cause

`SqlUndoStateReader::field_value` (`crates/holon/src/api/undo_persistence.rs`)
took `entity_id: &str` and built `SELECT {field} FROM block_raw WHERE id =
'{entity_id}'` from it verbatim. `block_raw` rows are keyed on the
scheme-qualified id (`block:<id>`, rendered from `EntityUri`), so an unschemed
id matched nothing. Measured directly:

```
READER-SHAPE id="shutdown-probe-child"       -> 0 row(s) content=None
READER-SHAPE id="block:shutdown-probe-child" -> 1 row(s) content=Some("set by the user")
```
(`.claude/worktrees/verify-inc1/lane-logs/r3b-exposure-1789294296.log:43-45`)

The asymmetry that made this reachable: the block-write leg NORMALIZES an
unschemed id (`LoroBackend::resolve_to_tree_id_sync` →
`EntityUri::from_raw`, `crates/holon-loro/src/loro_backend.rs:3985`), so the
write succeeds, while the precondition read did not. The `&str` parameter is
what let the two forms drift apart.

The same untyped parameter served SEVEN further call sites in
`crates/holon/src/api/operation_engine.rs` (eight in total, at lines 1009,
1270, 1274, 1278, 1331, 2320, 2385 and 2436), each carrying the same defect:
`read_task_keyword_prior_state` (three reads, which turn a valid write into
`block {id} does not exist`), `stored_task_keyword` (silently reports no task
state), the `block_to_page` recognition read, and three trust-gate proposal
reads.

One neighbouring finding from the same investigation has its own entry:
[2026-09-13-readonly-gate-parses-block-id-only-when-a-readonly-doc-exists](2026-09-13-readonly-gate-parses-block-id-only-when-a-readonly-doc-exists.md).

## Exposure

**A production surface DOES pass unschemed block ids: the generic MCP
`execute_operation` tool.** An earlier revision of this entry claimed no
production caller did. That was wrong, and a verifier refuted it.

`HolonMcpServer::execute_operation` (`frontends/mcp/src/tools.rs:1140-1160`)
takes the agent's `params` map (`ExecuteOperationParams`,
`frontends/mcp/src/types.rs:136`) and hands it to the dispatcher through
`json_map_to_storage_entity` (`frontends/mcp/src/tools.rs:586-595`), which
copies keys and values through unchanged. There is no `ensure_block_prefix` on
this path — that helper guards only the task-shaped tools — and no `EntityUri`
anywhere in it. Whatever the agent types as `id` is what the operation gets;
the tool's own unit test uses a bare `"block-1"`.

The purpose-built tools are still safe, so the surface list is:

| Surface | Id source | file:line | Form |
|---|---|---|---|
| MCP generic `execute_operation` | the agent's `params` map, verbatim | `frontends/mcp/src/tools.rs:1140-1160`, `:586-595` | **whatever the agent sends** |
| MCP task tools | `ensure_block_prefix` | `frontends/mcp/src/tools.rs:325-331` | qualified |
| GPUI editor | `caret_block_for_edit()` → `EntityUri::as_str()` | `frontends/gpui/src/views/editor_view.rs:1435-1437` | qualified |
| Rule watcher | `PageId::as_str()` / `deterministic_block_id()` | `crates/holon/src/api/holon_rule_watcher.rs:393-407` | qualified |

What that surface actually reaches splits in two, and only one half is latent:

- **Undo: still latent.** Journaling is gated on `origin.is_user()`
  (`crates/holon/src/api/operation_engine.rs:2761`), and the MCP service
  dispatches under `OpOrigin::Agent`. An agent's bare-id write therefore never
  produces an undo entry, so its precondition is never read. Reaching the undo
  defect still needs a user-origin caller passing a bare id, and none was
  found.
- **The task-keyword reads: NOT latent, and repaired by this fix.**
  `read_task_keyword_prior_state` and `stored_task_keyword` are not
  origin-gated. Measured on the same gesture through Agent origin: base returns
  `Err("cycle_task_state: block … does not exist")` for a bare id; this
  commit returns `Ok`. So part of what this change repairs is agent-facing and
  live today.

**The outcome was disclosed, not silent.** A `StaleDropped` undo already
reaches the user on all three frontends — GPUI maps it to a dedicated
`DegradedKind::UndoStepDropped` toast with `warn_only: false`
(`frontends/gpui/src/share_ui.rs:1217-1221`), and the MCP and worker frontends
report it too.

Classification: still a COVERAGE escape, and the corrected exposure sharpens
rather than softens it. The fleet has a production surface that accepts an
arbitrary id string, and no test drives a block operation through it with the
unschemed form — which is exactly why both halves above went unnoticed.

## Missing piece

COVERAGE. No test undoes a block write through a full production-wiring boot
with an id in the form the boundary accepts but the projection does not key on.
The keystone's `undo_last_mutation` does drive the production reader and does
assert `applied()`, so it WOULD have caught this — but every keystone
transition addresses blocks by an `EntityUri`, so it never produces the id form
that triggers it. The journal's own suites (`crates/holon/tests/undo_*.rs`) use
fake readers and never see the SQL keying at all.

Secondary ENVIRONMENT: the wide keystone seed contains a read-only document, so
the write-tier gate refuses an unschemed id before the journal is reached (see
the companion entry). A keystone rung for this needs a seed without one.

## Remedy

Fixed by typing the boundary, so the two id forms can no longer drift:

- `UndoStateReader::field_value` now takes `&EntityUri`
  (`crates/holon-core/src/undo.rs`), and `FieldFingerprint.entity_id` is an
  `EntityUri` parsed once where the fingerprint is taken from a `FieldDelta`.
  A persisted snapshot is healed on load by a `try_from_raw` deserializer.
- `SqlUndoStateReader` renders the id from `EntityUri::as_str()` at the single
  place it becomes SQL text.
- All eight engine call sites parse at their boundary; the compiler enumerated
  them.
- `UndoEntry::coalescible_edit` keys word-boundary coalescing on the parsed
  `EntityUri` too, so two spellings of one block cannot open two typing groups.
  The journalled ops keep their raw id strings — out of scope here.

Pinned by `crates/holon-app/tests/undo_precondition_id_scheme.rs`: the
unschemed-id gesture must be undoable (red before, green after) with a
qualified-id control that isolates the id scheme as the discriminator.

RULED (D125.a, 2026-09-14): the operation boundary does not accept an unschemed
block id at all. `OperationDispatcher::parse_entity_references` parses every
param the descriptor types `TypeHint::EntityId` — the subject `id` included,
which the `#[operations_trait]` macro now declares — into an `EntityUri` once,
and refuses an unschemed value with a typed
`holon_api::UnschemedEntityReference`. The two spellings can no longer reach two
legs.

The covering rung is now a keystone transition, `DispatchUnschemedBlockId`,
replayed deterministically as the hand-authored case
`operation-boundary-refuses-unschemed-block-id`. It names a block that does not
exist, so the refusal is a property of the id form alone and the rung draws on
every seed — including the read-only-document seeds that made the earlier
`EditContentByStoredId` attempt panic. `crates/holon-app/tests/undo_precondition_id_scheme.rs`
keeps its qualified-id control and now pins the refusal in place of the
bare-id undo.
