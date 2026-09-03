---
id: 2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded
date: 2026-09-03
gap: ENVIRONMENT
secondary: null
status: OPEN
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

Open. Two parts, both needed: the affordance set must derive from the format's
write tier so a read-only block renders non-editable, and any refusal that
discards a user edit must surface in the window, not only the log. Test parity
work: seed the composed fixture with a read-only-format document so the keystone
can generate an edit against one, and add an invariant that no accepted edit is
ever discarded without a user-visible disclosure.
