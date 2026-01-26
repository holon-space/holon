# Phase C — Validation flake diagnosis (2026-05-19)

Worktree: `.claude/worktrees/phase-c-validation`
Both `general_e2e_pbt` (Full) and `general_e2e_pbt_sql_only` failed at the
same shrunk seed during the validation sweep — both panicked on
`inv-org-render-fixed-point` for file
`__wmqh__gg__685470.org` belonging to `block:ref-doc-1`.

## Observed divergence

```
--- disk (169 bytes) ---
#+ID: ref-doc-1
#+TODO: DOING | CLOSED CANCELLED
* CLOSED EdcblAe3o xoV Fgic
...

--- rendered from SQL (136 bytes) ---
#+ID: ref-doc-1
* CLOSED EdcblAe3o xoV Fgic
...
```

The `#+TODO:` directive is dropped on the SQL → render round trip. Other
content (block ids, headlines, task state on individual blocks) survives.
`#+ID:` survives. Only the doc-level `todo_keywords` property is missing
when re-rendered from `block_raw`.

## Trace

1. `OrgRenderer::render_document` (`crates/holon-org-format/src/org_renderer.rs:23`)
   calls `render_document_header(doc_block)`.
2. `render_document_header` (`models.rs:380`) emits `#+TODO:` iff
   `doc_block.todo_keywords()` is `Some(...)`.
3. `todo_keywords()` (`models.rs:285`) reads
   `properties[org_props::TODO_KEYWORDS]` — i.e. `block_raw.properties.todo_keywords`.

So `block_raw.properties` for the doc row is missing the `todo_keywords`
key. The write path was supposed to put it there:

4. `OrgSyncController::on_file_changed` (`org_sync_controller.rs:401-415`)
   parses disk, sees `parsed_kws != existing_kws`, calls
   `doc_manager.update_metadata(doc)`.
5. `LiveDocumentManager::update_metadata` (`di.rs:592`) →
   `build_block_params` (`block_params.rs:20`) → command_bus
   `execute_operation("block", "update", params)`.
6. `build_block_params` does **not** hard-code a `todo_keywords` key but
   does iterate `block.drawer_properties()` (`block_params.rs:139`),
   which forwards any non-internal-keyed `properties` entry —
   `todo_keywords` qualifies, so the param should be present.
7. `SqlOperationProvider::prepare_update`
   (`sql_operation_provider.rs:720`) lifts any unknown column into the
   `properties` JSON via merge-with-existing.

In theory steps 5–7 should land `todo_keywords` in
`block_raw.properties`. Three unverified hypotheses for why it doesn't:

- **H1 — update_metadata path never runs for the failing file.** The
  failing file is `__wmqh__gg__685470.org` (a temp-named bulk-add file),
  not the seed `__0.org`. `BulkExternalAdd::apply_to_sut` waits for
  block-count sync but **not** for doc-level metadata sync; if
  on_file_changed runs the metadata update asynchronously, the
  invariant check could see SQL before update_metadata commits.
  Probability: high. Easiest to test by adding `tokio::time::sleep`
  before the invariant, or by extending `wait_for_blocks_synced` to
  also assert `properties.todo_keywords` present on doc rows.
- **H2 — update_metadata runs but the doc block in memory has
  no todo_keywords yet.** The order in on_file_changed is parse →
  compare → call update_metadata with the (existing) doc, then
  `doc.set_todo_keywords(parsed_kws)`. Re-reading
  `org_sync_controller.rs:411-414` shows the mutation does happen on
  `doc` before `update_metadata(&doc)` is called — so this looks fine.
  Lower probability.
- **H3 — the matview `block` is read instead of `block_raw` somewhere
  in the chain, and the matview hasn't refreshed.** `snapshot_org_render_pairs`
  explicitly queries `block_raw`, so this would have to be at write
  time, not read time. Unlikely.

## What was checked but not the culprit

- `block_raw` does carry `b.properties` and the snapshot SQL pulls
  it in the column list (`test_environment.rs:1595-1600`).
- `drawer_properties()` includes flat `properties` entries that don't
  start with `_` and aren't in `INTERNAL_KEYS` — `todo_keywords` is
  neither (`models.rs:654-731`).
- `Block::try_from(HashMap)` is the hand-rolled deserializer (per
  MEMORY.md "Block has two deserializers"), so a `serde(skip, default)`
  drop is not the explanation here.
- `BLOCK_WRITE_TABLE = "block_raw"` (`block_table_names.rs:13`) so the
  update writes to the right table.

## Recommended next step

Reproduce the exact shrunk seed standalone:

```rust
// Pseudo:
WriteOrgFile("__0.org", "#+TODO: TODO | CLOSED\n* Aa7PjgGh\n:PROPERTIES:\n:ID: -y51--f-h--\n:END:\n");
StartApp { enable_loro: false };
// After startup, dump block_raw.properties for every page block.
```

If H1 is correct, the bulk-add path needs a barrier that waits for
`block_raw.properties.todo_keywords` to be populated on the doc row.

## Other handoff TODOs — status

- 🔴 **Generator tuning (5 migrated transitions, 0 executions):** not
  attempted in this session. Key blocker for `FocusEditableText` is
  `PBT_ATOMIC_EDITOR=1` env var + `enable_loro` precondition — explains
  0 executions in SqlOnly entirely. For Full variant, the env was not
  set in the run either. Set `PBT_ATOMIC_EDITOR=1` to even consider
  these transitions; then re-measure rejection causes.
- 🔴 **SqlOnly 3-panic flake from prior handoff:** in this run both
  Full and SqlOnly hit `inv-org-render-fixed-point` first; the
  previously-reported `inv-editable-text-has-draggable`/`nav focus
  mismatch` panics may now be downstream of this earlier failure.
  Re-test after fixing `inv-org-render-fixed-point`.
