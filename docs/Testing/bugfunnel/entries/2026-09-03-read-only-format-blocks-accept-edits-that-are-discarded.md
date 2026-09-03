---
id: 2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded
date: 2026-09-03
gap: ENVIRONMENT
secondary: null
status: PARTIAL
summary: >-
  A cooklang recipe renders with the full editing affordance set; typing into it
  updates the store and the screen, the disk file never changes, and the only
  disclosure is an ERROR line in the log.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`) against a copy of
Martin's real vault, which carries three `.cook` recipes under
`Resources/Rezepte/`.

`.cook` is a read-only format: the write-back leg refuses it by design
(`crates/holon-filesystem/src/file_sync_controller.rs`, refusal text "is a
read-only format (authoritative input only)"). The UI does not know that. A
recipe step renders as a normal editable row carrying the full operation set —
`insert_text`, `delete_text`, `split_block`, `delete`, `cycle_task_state` and
the rest.

Driven live: click a recipe step, type three characters. Result —

  - the store accepted the edit (`SELECT content FROM block WHERE
    id = 'block:Resources/Rezepte/Linsensuppe.cook::b::3'` shows the mutated text),
  - the screen shows the mutated text,
  - the file on disk is byte-identical (md5 `e4df9c1e…` before and after),
  - four new `WRITE-BACK REFUSED` ERROR lines appear in the app log,
  - and the window shows no banner, no toast, no marker of any kind.

The refusal message itself says the quiet part: "The store holds changes for
block:… that will NOT reach this file, and no other file is written in its
place." The user is never told. This is exactly the failure mode the project's
error philosophy ranks last — silently degrading to look fine.

A second symptom of the same wiring: ten of these ERROR refusals are emitted at
BOOT, from `site="materialize_missing_page_files"` during the initial scan, for
recipe files the user has never touched. Every launch on a vault containing
recipes writes ERROR lines claiming pending changes that do not exist, which
will mask the refusals that do matter.

## Root cause

The read-only property of a format is enforced only at the write-back boundary,
not at the point where the UI decides which operations a block offers. The
renderer builds the editing affordance set from the block's type without
consulting the format registry's write tier, so a block whose backing file can
never be written is presented as fully editable.

## Missing piece

The keystone drives an org-only vault, so no block in the test environment has a
read-only backing format. The interaction (type into a rendered block) is
generated constantly; the wiring under it — a second format whose write tier
refuses — does not exist in the test environment at all. That is what makes this
ENVIRONMENT rather than COVERAGE.

## Remedy

The store no longer takes the edit. The **operation dispatcher** — the one
writer (Model.md invariant 4) — gained a write-tier gate beside its boundary,
declared-guard and net gates
(`crates/holon/src/api/operation_dispatcher.rs`, `enforce_write_tier`): a block
write whose owning document is homed in a `WriteTier::ReadOnly` file is refused
with the typed `EditRefused::ReadOnlyFormat { format, path }`
(`crates/holon-core/src/write_tier_gate.rs`). The rule is per write tier, with
no mention of cooklang anywhere in it.

Only writes that ORIGINATE in the store are judged: `OpOrigin::Ingest` is the
file telling the store what it says, and `OpOrigin::Sync` is a peer's merged
history — refusing either would break the replica it came from.

`FileSyncController` is the only component that knows both a document and the
file homing it, so it publishes the read-only slice of its `doc_home` map into
a shared `ReadOnlyDocuments` registry as it records each home; an org-only
vault leaves that registry empty and the gate returns on one atomic read.

The refusal reaches the window: `ReadOnlyFormatGate`
(`crates/holon-app/src/read_only_format_gate.rs`) raises
`ShareDegradedReason::EditRefusedReadOnlyFormat` on the degraded bus, which the
GPUI banner renders ("Edit refused — read-only file").

The boot ERRORs are gone. `materialize_missing_page_files` skips a
read-only-homed page silently — the sweep only ever mints a file for a page
that has none, and this page has one, so nothing was refused. The remaining
write-back-leg gates (`on_block_changed`, `write_back`, `write_back_target`)
note the skip at `debug!`: with the dispatcher refusing store-origin edits,
what reaches them is the store echoing back what the file just told it.

Tests (`crates/holon-integration-tests/tests/cook_vault_ingest.rs`, a REAL boot
of a vault holding `.cook` and `.org` files):
`an_edit_to_a_recipe_block_is_refused_at_the_dispatcher`,
`a_refused_recipe_edit_raises_a_degraded_condition`,
`booting_a_vault_of_recipes_logs_no_write_back_refusal`. All three were red
against the unfixed code for the stated reason.

The dispatcher is not the only writer, and the first fix missed the other one.
The editor's keystrokes never reach `execute_operation`: `apply_local_edit` has
a cell-mode early return that applies each `TextOp` straight onto the block's
`LoroText` through the `BlockCellRegistry` cell. Ungated, one keystroke forked
the vault's own CRDT doc with no refusal and no banner. The cell now takes its
write authority FROM the dispatcher's: `EntityCellRegistry::editable_field_any`
— the seam `editable_text` hands the editor, distinct from the `live_field_any`
the store's own ingest and sync legs take — wraps the content cell in a
`ReadOnlyTextCellBacking` carrying the same `EditRefused` from the same
`WriteTierAuthority`. Pinned by
`a_keystroke_on_a_recipe_block_is_refused_at_the_cell` (real boot) and
`a_keystroke_on_a_read_only_cell_reaches_no_crdt_op`
(`crates/holon-frontend/src/editor_view_model.rs`, the keystroke path GPUI and
the TUI share).

PARTIAL, two pieces open:

1. **Read-only rendering.** A recipe step still renders with the full editing
   affordance set; typing now reaches no writer at all and raises a banner,
   but the caret still enters the row. The
   affordance set is built from the widget spec, which carries no write tier,
   so making it derive from the tier is its own change through the render
   pipeline.
2. **Keystone parity.** The composed fixture is still org-only
   (`crates/holon-integration-tests/src/pbt/composed/builder.rs`), so the
   keystone cannot generate an edit against a read-only-format block. Seeding a
   `.cook` document there exposes every existing invariant to a second format
   at once, which is a change of its own size.
