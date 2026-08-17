---
id: 2026-08-17-set-field-block-not-found-stale-doc-resolution
date: 2026-08-17
gap: COVERAGE
status: FIXED
summary: >-
  The focus-leave commit funnel re-dispatched text the keystroke sink had
  already persisted, landing on the block join_block had just consumed.
---

## Bug

Found by log analysis of `/private/tmp/holon-cold.log` (2026-08-17, real
vault session, 09:08-10:04). Two occurrences, different blocks: line 8919
`Operation block.set_field failed: ... capture prior state: Block not
found: block:c76e5c74-a47a-4653-a0b3-327d5b96018d` and line 8997 (id
`block:d9a6ee67-...`).

## Root cause

Not a race, and nothing to do with doc resolution: `find_doc_for_block`
(`crates/holon-loro/src/loro_block_operations.rs:106`) ignores its id
argument and returns the global backend, so its "success" carries no claim
that the block exists. The only real signal is `get_block` — the block was
already gone.

The log shows why. Both failures follow the same deterministic sequence
~15ms apart:

1. `join_block(id)` — Backspace at caret 0 — merges the block into its
   previous sibling and DELETES it. Succeeds.
2. Focus moves to the survivor, so the departing editor runs GPUI's
   focus-leave commit funnel (`spawn_focus_binding` →
   `EditorViewModel::pending_commit_intent`, `editor_view.rs`).
3. That funnel dispatches `set_field(id, content=…)` against the deleted id.

The funnel fires at all because the SqlOnly keystroke sink
(`EditorViewModel::apply_local_edit`) wrote the typed text and advanced its
own `buffer`, but never re-baselined `ViewEventHandler::original_value` —
the baseline the funnel diffs against. So every focused SqlOnly editor had a
permanently-dirty commit funnel, and every focus leave re-dispatched text
that was already stored, as a second `set_field` carrying no `write_seq`.
The observed failures are that duplicate write arriving after the block that
owned it was joined away. The signature in the log is exactly this: the
successful keystroke write at 10:00:01.352 carries `write_seq: Integer(39)`,
the failing one at 10:00:02.122 carries none.

## Missing piece

COVERAGE, as filed, but one layer up from where the entry guessed. The
composed keystone drives keystrokes through the same `EditorViewModel`
(`HeadlessEditorMirror`), but the mirror modelled only the keystroke sink and
the data-sync echo — it had no focus-leave commit funnel at all, so the
second writer prod dispatches on every focus move was structurally
ungeneratable.

## Remedy

Two changes, both in `crates/holon-frontend`:

* `EditorViewModel::apply_local_edit` re-baselines the commit funnel to the
  text it just persisted. Text the sink has written is no longer pending, so
  the focus-leave funnel emits nothing for it. Pinned red-first by
  `the_focus_leave_funnel_does_not_recommit_what_the_keystroke_sink_wrote`
  (`editor_view_model.rs`); without the fix it fails on
  `pending_commit_intent` returning an intent.
* `HeadlessEditorMirror::note_focus_settled` / `commit_departing_editor` give
  the headless keystone the focus-leave funnel, driven from
  `ReactiveEngineDriver::converge_editors`. Under correct behaviour it
  dispatches nothing, so it is silent across all 62 hand-authored cases; it
  exists so a regression of this class reaches the keystone instead of only
  the unit test.

Authoring a hand-authored case for the full gesture (split, type, backspace
across the boundary) surfaced a SEPARATE pre-existing divergence — the
tracked caret does not follow the join when the block was typed into first —
which reds independently of this fix. Filed as
`2026-08-17-join-after-typing-loses-the-merge-boundary-caret`; the case is
held there rather than in `keystone.jsonl`.
